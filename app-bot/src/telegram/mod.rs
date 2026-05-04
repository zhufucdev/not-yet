use std::cell::LazyCell;
use std::path::PathBuf;
use std::{fmt::Display, hash::Hash, path::Path, sync::Arc, time::Duration};

use ::futures::future;
use anyhow::{Context, anyhow};
use app_common::config::ParseConfigPath;
use clap::Parser;
use futures::Stream;
use futures::future::Lazy;
use futures::{TryStreamExt, pin_mut};
use itertools::Itertools;
use lib_common::agent::decision::Decider;
use lib_common::agent::error::GetTruthValueError;
use lib_common::agent::memory::criteria::sqlite::SqliteCriteriaMemory;
use lib_common::agent::memory::decision::DecisionMemory;
use lib_common::agent::memory::dialog::fs::FsDialogMemory;
use lib_common::agent::optimize::gemma4::Gemma4Optimizer;
use lib_common::agent::optimize::{ApproveOrDeny, OptimizationCallback, OptimizerAction};
use lib_common::llm::dialog::gemma4;
use lib_common::{agent, llm, secure};
use lib_common::{
    agent::{LlmConditionMatcher, memory::decision::SqliteDecisionMemory},
    llm::timeout::{ModelProducer, TimedModel},
    polling::{Schedule, Scheduler, schedule::QueueType},
    source::{DefaultMetadata, Feed, LlmComprehendable, RssFeed, atom::AtomFeed},
    update::sqlite::SqliteUpdatePersistence,
};
use llama_runner::Gemma4VisionRunner;
use llama_runner::sample::SimpleSamplingParams;
use llama_runner::{RunnerWithRecommendedSampling, error::CreateLlamaCppRunnerError};
use sea_orm::{ActiveEnum, ColumnTrait, DatabaseConnection, EntityTrait, Iterable, QueryFilter};
use serde::{Serialize, de::DeserializeOwned};
use smol_str::SmolStr;
use teloxide::types::{InlineKeyboardMarkup, MessageId};
use teloxide::utils::render::RenderMessageTextHelper;
use teloxide::{
    dispatching::{
        UpdateHandler,
        dialogue::{GetChatId, InMemStorage},
    },
    payloads::SendMessageSetters,
    prelude::*,
};
use tokio::select;
use tokio::sync::{RwLock, mpsc};
use tracing::{Instrument, Level, event, info_span};
use tracing_subscriber::EnvFilter;

use crate::telegram::state::{LlmAssignment, OptimizationTask, StateFeedback};
use crate::{
    authenticator::{
        Access, Authenticator as _, priority::PriorityAuthenticator, sqlite::SqliteAuthenticator,
        whitelist::WhitelistAuthenticator,
    },
    config::Config,
    db::{
        self, atom, rss,
        subscription::{self, SubscriptionId},
        user::AccessLevel,
    },
    rss::add_feed_subscription_for,
    telegram::{args::Args, command::Command, repmark::button_repmark, state::State},
    token::OnetimeToken,
};

mod args;
mod clarify;
mod command;
mod repmark;
mod state;

type MasterDialog = Dialogue<State, InMemStorage<State>>;
type Authenticator = PriorityAuthenticator<(
    WhitelistAuthenticator<UserId, AccessLevel>,
    SqliteAuthenticator,
)>;

pub type UserId = i64;

pub(super) async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(args.verbosity.tracing_level_filter().into())
                .from_env()
                .unwrap_or_default(),
        )
        .init();
    let data_path = args.config.parse_config()?;
    let config = app_common::config::parse_config::<Config>(
        &data_path,
        include_bytes!("../../asset/default_config.toml"),
    )
    .await?;

    let bot = Bot::with_client(
        std::env::var("BOT_TOKEN")
            .ok()
            .or(config.bot_token)
            .or(args.bot_token)
            .ok_or(anyhow!("invalid BOT_TOKEN environment variable, either set the BOT_TOKEN environment variable, specify in the configuration or use the --bot-token command line argument"))?,
        teloxide::net::client_from_env(),
    );

    let token = OnetimeToken::new();
    println!("access token: {}", token.value().await);
    let db = app_common::config::setup_db(&data_path).await?;
    let scheduler = Arc::new(
        get_schedules(&db)
            .await?
            .into_iter()
            .collect::<Scheduler<SubscriptionId>>(),
    );

    future::select(
        Box::pin(
            Dispatcher::builder(bot.clone(), bot_state_machine())
                .dependencies(dptree::deps![
                    InMemStorage::<State>::new(),
                    token,
                    db.clone(),
                    data_path.clone(),
                    Arc::new(Authenticator::from((
                        WhitelistAuthenticator::new(
                            config.whitelist.unwrap_or_default(),
                            AccessLevel::ConfiguredWhitelist
                        ),
                        SqliteAuthenticator::new(db.clone())
                    ))),
                    scheduler.clone()
                ])
                .enable_ctrlc_handler()
                .build()
                .dispatch(),
        ),
        Box::pin(
            async {
                loop {
                    select! {
                        result = start_polling_all(scheduler.clone(), llm::DEFAULT_MODEL.clone(), &db, &data_path, &bot) => {
                            match result {
                                Ok(_) => {
                                    event!(
                                        Level::WARN,
                                        "empty schedule queue, waiting 30s before retrying"
                                    );
                                    tokio::time::sleep(Duration::from_secs(30)).await;
                                },
                                Err(err) => {
                                    event!(Level::ERROR, "while polling: {err}");
                                },
                            }
                        },
                        Ok(QueueType::New) = scheduler.until_next_reschedule() => {},
                    }
                }
            }
            .instrument(info_span!("task_loop")),
        ),
    )
    .await;

    Ok(())
}

fn bot_state_machine() -> UpdateHandler<anyhow::Error> {
    dptree::entry()
        .branch(
            Update::filter_callback_query()
                .enter_dialogue::<CallbackQuery, InMemStorage<State>, State>()
                .branch(
                    dptree::case![State::ChoosingSubscriptionKind]
                        .endpoint(chose_subscription_type),
                )
                .branch(
                    dptree::case![State::GotFeedCondition {
                        condition,
                        url,
                        kind,
                    }]
                    .endpoint(choose_rss_mock_browser),
                )
                .branch(
                    dptree::case![State::GotFeedMockBrowserUa {
                        mock,
                        condition,
                        url,
                        kind,
                    }]
                    .endpoint(choose_rss_custom_headers),
                )
                .branch(
                    dptree::case![State::Feedingback { tasks }].endpoint(receive_feedback_query),
                ),
        )
        .branch(
            Update::filter_message()
                .enter_dialogue::<Message, InMemStorage<State>, State>()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .branch(dptree::case![Command::Start].endpoint(start)),
                )
                .branch(dptree::case![State::Authenticating].endpoint(authenticate))
                .branch(dptree::case![State::ChoseFeed { kind }].endpoint(receive_rss_url))
                .branch(
                    dptree::case![State::GotFeedUrl { url, kind }].endpoint(receive_rss_condition),
                )
                .branch(
                    dptree::case![State::GotFeedMockBrowserUa {
                        mock,
                        condition,
                        url,
                        kind
                    }]
                    .endpoint(receive_rss_custom_headers),
                )
                .endpoint(receive_feedback_msg),
        )
}

async fn start_polling_all(
    scheduler: Arc<Scheduler<SubscriptionId>>,
    model: TimedModel<RunnerWithRecommendedSampling<Gemma4VisionRunner>, CreateLlamaCppRunnerError>,
    db: &DatabaseConnection,
    working_dir: &Path,
    bot: &Bot,
) -> anyhow::Result<()> {
    let tasks = scheduler.schedules().await.into_iter().map(|schedule| {
        let model = model.clone();
        let scheduler = scheduler.clone();
        let sub_id = *schedule.key();
        async move {
            let Some((sub, rss, atom)) = subscription::Entity::find_by_id(sub_id)
                .find_also_related(rss::Entity)
                .find_also_related(atom::Entity)
                .one(db)
                .await?
            else {
                event!(Level::ERROR, "failed to find subscription from id");
                return Ok(());
            };

            match sub.kind {
                subscription::Kind::Rss => {
                    let feed: RssFeed = rss.unwrap().try_into()?;
                    send_update_messages(
                        &bot,
                        &feed,
                        feed.url(),
                        &sub,
                        model,
                        &scheduler,
                        db,
                        working_dir,
                    )
                    .await?;
                }
                subscription::Kind::Atom => {
                    let feed: AtomFeed = atom.unwrap().try_into()?;
                    send_update_messages(
                        &bot,
                        &feed,
                        feed.url(),
                        &sub,
                        model,
                        &scheduler,
                        db,
                        working_dir,
                    )
                    .await?;
                }
            }

            Err(anyhow!("subscription has no associated feed"))
        }
        .instrument(info_span!("run_task", subscription_id = sub_id))
    });
    future::try_join_all(tasks).await?;
    Ok(())
}

async fn send_update_messages<Feed_>(
    bot: &Bot,
    feed: &Feed_,
    feed_url: &str,
    sub: &subscription::Model,
    model: TimedModel<RunnerWithRecommendedSampling<Gemma4VisionRunner>, CreateLlamaCppRunnerError>,
    scheduler: &Scheduler<SubscriptionId>,
    db: &DatabaseConnection,
    working_dir: &Path,
) -> anyhow::Result<()>
where
    Feed_: Feed<Metadata = DefaultMetadata>,
    Feed_::Item:
        LlmComprehendable + Clone + Hash + Display + Serialize + DeserializeOwned + 'static,
    Feed_::Error: Display,
{
    let sub_id = sub.id;
    app_common::feed::check(
        &sub_id,
        feed,
        &scheduler,
        SqliteUpdatePersistence::new(db.clone(), sub_id)?,
        sub.buffer_size as usize,
    )
    .try_for_each(async |(item, _)| {
        let Some(item) = item else {
            return Ok(());
        };
        let dialog_id = secure::generate_random_id(32);
        event!(Level::INFO, "created dialog_id = {dialog_id}");
        async {
            let mut decision_mem =
                SqliteDecisionMemory::new(db.clone(), working_dir, Some(sub_id))?;
            let decider = LlmConditionMatcher::new(
                model.clone(),
                sub.condition.to_string(),
                decision_mem.clone(),
                FsDialogMemory::new(working_dir, &dialog_id),
                SqliteCriteriaMemory::new(db.clone(), Some(sub_id)),
            );
            match decider.get_truth_value(&item).await {
                Ok(false) => {
                    return Ok(());
                }
                Ok(true) => {}
                Err(GetTruthValueError::DecisionMemory(err)) => {
                    event!(Level::ERROR, "decision memory error: {err}");
                    event!(Level::WARN, "will clear decision memory");
                    decision_mem.clear().await?;
                }
                Err(err) => return Err(err.into()),
            }

            let msg = match feed.get_metadata().await {
                Ok(meta) => format!(
                    "Your subscription to \"{}\" ({}) has an update! Check it out!\n{}",
                    meta.name,
                    feed_url,
                    item.to_string(),
                )
                .trim_end()
                .to_string(),
                Err(err) => {
                    event!(
                        Level::WARN,
                        "failed to fetch RSS feed metadata, falling back to URL only: {err}"
                    );
                    format!(
                        "Your subscription to {} has an update! Check it out!\n{}",
                        feed_url,
                        item.to_string(),
                    )
                    .trim_end()
                    .to_string()
                }
            };
            let tg_msg = bot.send_message(UserId(sub.user_id as u64), msg).await?;
            db::dialog::ActiveModel::builder()
                .set_dialog_id(&dialog_id)
                .set_msg_id(tg_msg.id.0)
                .set_subscription_id(sub.id)
                .insert(db)
                .await
                .inspect_err(|err| event!(Level::WARN, "failed to save dialog: {err}"))?;
            Ok(())
        }
        .instrument(info_span!("send_message", dialog_id = dialog_id))
        .await
    })
    .await?;
    event!(
        Level::WARN,
        "updates is supposed to be infinite, but ended prematurely. sub_id = {sub_id}"
    );
    Ok(())
}

async fn get_schedules(
    db: &DatabaseConnection,
) -> Result<impl IntoIterator<Item = Schedule<SubscriptionId>>, sea_orm::DbErr> {
    Ok(subscription::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .enumerate()
        .map(|(id, s)| Schedule::new(id, s.id, s.schedule_trigger()).unwrap()))
}

async fn start(
    bot: Bot,
    dialog: MasterDialog,
    authenticator: Arc<Authenticator>,
    msg: Message,
) -> anyhow::Result<()> {
    let Some(user) = &msg.from else {
        return Ok(());
    };
    match authenticator.get_access(&(user.id.0 as UserId)).await {
        Ok(Access::Granted(_)) => {
            dialog.update(State::ChoosingSubscriptionKind).await?;
            bot.send_message(
                msg.chat_id().unwrap(),
                "Here is a list of data source types I can subscribe to",
            )
            .reply_markup(button_repmark(
                db::subscription::Kind::iter()
                    .map(|kind| (kind.to_string(), format!("{:x}", kind.to_value())))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .chunks(2)
                    .into_iter()
                    .map(|chunk| chunk.collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            ))
            .await?;
        }
        Ok(Access::Denied) => {
            dialog.update(State::Authenticating).await?;
            bot.send_message(msg.chat_id().unwrap(), "Good days! Nice to see you! I can ping you update messages as instructed. To get started, paste here the one-time access token. You can get it from the console").await?;
        }
        Err(err) => {
            event!(Level::ERROR, "failed to get access: {err}");
            dialog.reset().await?;
            bot.send_message(
                msg.chat_id().unwrap(),
                format!(
                    "Could not get you in. Please refer to the console for detailed information"
                ),
            )
            .await?;
        }
    }

    Ok(())
}

const EMPTY_MESSAGE_RESPONSE: &str = "You sent an empty message. That's techniquely incredible! Feel free to try again however, to proceed";
const UNKNOWN_ACTION_RESPONSE: &str = "I don't know what to do with this action. Please try again";
const CUSTOM_HEADERS_PROMPT: &str =
    "Hint: header kv pairs follow format KEY_1=VALUE_1; KEY_2=VALUE_2; ...";

async fn authenticate(
    bot: Bot,
    dialog: MasterDialog,
    current_token: OnetimeToken,
    msg: Message,
    authenticator: Arc<Authenticator>,
) -> anyhow::Result<()> {
    let Some(token) = msg.text() else {
        bot.send_message(msg.chat_id().unwrap(), EMPTY_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };
    if current_token.test(token).await {
        let Err(err) = async || -> anyhow::Result<()> {
            dialog.reset().await?;
            let Some(user) = &msg.from else {
                return Ok(());
            };
            authenticator
                .grant(user.id.0 as UserId, AccessLevel::OnetimeToken)
                .await?;
            Ok(())
        }()
        .await
        else {
            bot.send_message(
                msg.chat_id().unwrap(),
                "Perfect! You're now authenticated. Use /start to get started~",
            )
            .await?;
            return Ok(());
        };
        bot.send_message(
            msg.chat_id().unwrap(),
            "I could not sign you in, due to some technical issues. Feel free to try again later",
        )
        .await?;
        return Err(err);
    } else {
        bot.send_message(
            msg.chat_id().unwrap(),
            "This token looks invalid to me. Feel free to try again, anytime",
        )
        .await?;
    }
    Ok(())
}

async fn chose_subscription_type(
    bot: Bot,
    query: CallbackQuery,
    dialog: MasterDialog,
) -> anyhow::Result<()> {
    let Some(kind_hex) = &query.data else {
        bot.send_message(query.chat_id().unwrap(), UNKNOWN_ACTION_RESPONSE)
            .await?;
        return Ok(());
    };
    const INVALID_DATA_RESPONSE: &str = "I don't believe the data attached to the button you just clicked is valid. How did this happen?";
    let Ok(kind_id) = i32::from_str_radix(kind_hex, 16) else {
        bot.send_message(query.chat_id().unwrap(), INVALID_DATA_RESPONSE)
            .await?;
        return Ok(());
    };
    let Ok(kind) = subscription::Kind::try_from_value(&kind_id) else {
        bot.send_message(query.chat_id().unwrap(), INVALID_DATA_RESPONSE)
            .await?;
        return Ok(());
    };
    match kind {
        subscription::Kind::Rss => {
            dialog
                .update(State::ChoseFeed {
                    kind: subscription::Kind::Rss,
                })
                .await?;
            bot.send_message(
                query.chat_id().unwrap(),
                "Perfect! Let's fill in the details. Tell me, what's the URL to the RSS feed?",
            )
            .await?;
        }
        subscription::Kind::Atom => {
            dialog
                .update(State::ChoseFeed {
                    kind: subscription::Kind::Atom,
                })
                .await?;
            bot.send_message(
                query.chat_id().unwrap(),
                "Perfect! Let's fill in the details. Tell me, what's the URL to the Atom feed?",
            )
            .await?;
        }
    }
    repmark::remove(&query, &bot).await;
    Ok(())
}

async fn receive_rss_url(
    bot: Bot,
    msg: Message,
    dialog: MasterDialog,
    kind: subscription::Kind, // State::ChoseFeed
) -> anyhow::Result<()> {
    let Some(url) = msg.text() else {
        bot.send_message(msg.chat_id().unwrap(), EMPTY_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };
    dialog
        .update(State::GotFeedUrl {
            url: url.into(),
            kind,
        })
        .await?;
    bot.send_message(
        msg.chat_id().unwrap(),
        "Sure thing! Now, under what circumstances do you want to receive updates from this feed?",
    )
    .await?;
    Ok(())
}

async fn receive_rss_condition(
    bot: Bot,
    msg: Message,
    dialog: MasterDialog,
    (url, kind): (SmolStr, subscription::Kind), // State::GotFeedUrl
) -> anyhow::Result<()> {
    let chat_id = msg.chat_id().unwrap();
    let Some(text) = msg.text() else {
        bot.send_message(chat_id, EMPTY_MESSAGE_RESPONSE).await?;
        return Ok(());
    };
    if text.trim().is_empty() {
        bot.send_message(chat_id, EMPTY_MESSAGE_RESPONSE).await?;
        return Ok(());
    }
    dialog
        .update(State::GotFeedCondition {
            condition: text.into(),
            url,
            kind,
        })
        .await?;
    bot.send_message(msg.chat_id().unwrap(), "Great! But many sites may block access, unless they recoginze me as a web broswer. Do you want to add extra user agent headers to solve this issue?")
        .reply_markup(button_repmark([vec![("Yes", "y"), ("No", "n")], vec![("Custom", "c"), ("Skip", "s")]]))
        .await?;
    Ok(())
}

async fn choose_rss_mock_browser(
    bot: Bot,
    (condition, url, kind): (SmolStr, SmolStr, subscription::Kind), // State::GotFeedUrl
    query: CallbackQuery,
    dialog: MasterDialog,
    db: DatabaseConnection,
    scheduler: Arc<Scheduler<SubscriptionId>>,
) -> anyhow::Result<()> {
    let chat_id = query.chat_id().unwrap();
    let user_id = query.from.id.0 as UserId;
    let Some(action_id) = query.data.as_ref() else {
        bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        return Ok(());
    };
    match action_id.as_str() {
        "y" => {
            dialog
                .update(State::GotFeedMockBrowserUa {
                    mock: true,
                    condition,
                    url,
                    kind,
                })
                .await?
        }
        "n" => {
            dialog
                .update(State::GotFeedMockBrowserUa {
                    mock: false,
                    condition,
                    url,
                    kind,
                })
                .await?;
        }
        "c" => {
            dialog
                .update(State::GotFeedMockBrowserUa {
                    mock: false,
                    condition,
                    url,
                    kind,
                })
                .await?;
            bot.send_message(chat_id, CUSTOM_HEADERS_PROMPT).await?;
            return Ok(());
        }
        "s" => {
            add_feed_subscription_for(user_id, kind, url, condition, false, None, &db, &scheduler)
                .await?;
            dialog.reset().await?;
            bot.send_message(chat_id, "I see. You are ready to go")
                .await?;
            repmark::remove(&query, &bot).await;
            return Ok(());
        }
        _ => {
            bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
            return Ok(());
        }
    }
    repmark::remove(&query, &bot).await;
    bot.send_message(
        chat_id,
        "Perfect! One more question. Any other custom headers?",
    )
    .reply_markup(button_repmark([vec![("Yes", "y"), ("No", "n")]]))
    .await?;
    Ok(())
}

async fn choose_rss_custom_headers(
    bot: Bot,
    query: CallbackQuery,
    dialog: MasterDialog,
    (mock, condition, url, kind): (bool, SmolStr, SmolStr, subscription::Kind), // State::GotRssMockBrowserUa
    db: DatabaseConnection,
    scheduler: Arc<Scheduler<SubscriptionId>>,
) -> anyhow::Result<()> {
    let chat_id = query.chat_id().unwrap();
    let user_id = query.from.id.0 as UserId;
    let Some(action_id) = query.data.as_ref() else {
        bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        return Ok(());
    };
    match action_id.as_str() {
        "y" => {
            bot.send_message(chat_id, CUSTOM_HEADERS_PROMPT).await?;
        }
        "n" => {
            add_feed_subscription_for(user_id, kind, url, condition, mock, None, &db, &scheduler)
                .await?;
            dialog.reset().await?;
            bot.send_message(chat_id, "Perfect! You are ready to go")
                .await?;
            return Ok(());
        }
        _ => {
            bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        }
    }
    repmark::remove(&query, &bot).await;
    Ok(())
}

async fn receive_rss_custom_headers(
    bot: Bot,
    msg: Message,
    (mock, condition, url, kind): (bool, SmolStr, SmolStr, subscription::Kind), // State::GotRssMockBrowserUa
    db: DatabaseConnection,
    scheduler: Arc<Scheduler<SubscriptionId>>,
) -> anyhow::Result<()> {
    let chat_id = msg.chat_id().unwrap();
    let Some(headers) = msg.text() else {
        bot.send_message(chat_id, EMPTY_MESSAGE_RESPONSE).await?;
        return Ok(());
    };
    let Some(user) = &msg.from else {
        return Ok(());
    };
    match add_feed_subscription_for(
        user.id.0 as UserId,
        kind,
        url,
        condition,
        mock,
        Some(headers.into()),
        &db,
        &scheduler,
    )
    .await
    {
        Ok(()) => {
            bot.send_message(
                chat_id,
                "Perfect! You are ready to go. I will check every hour to see",
            )
            .await?;
        }
        Err(_) => todo!(),
    }
    Ok(())
}

async fn receive_feedback_msg(
    bot: Bot,
    msg: Message,
    db: DatabaseConnection,
    dialog: MasterDialog,
    working_dir: PathBuf,
) -> anyhow::Result<()> {
    let chat_id = msg.chat_id().unwrap();
    if let Some(State::Feedingback { tasks }) = dialog.get().await? {
        event!(Level::TRACE, "state = feedback");
        if tasks.read().await.is_empty() {
            event!(Level::WARN, "feedback task queue is empty, ignoring");
            return Ok(());
        }
        let Some(msg_text) = msg.markdown_text() else {
            bot.send_message(chat_id, "I do not understand your input. Sorry! Feel free to try again using a different one.").await?;
            return Ok(());
        };
        if let Some(reply) = msg.reply_to_message() {
            if let Some((idx, task)) = tasks
                .read()
                .await
                .iter()
                .find_position(|t| t.prompt == reply.id)
            {
                match &task.assignment {
                    LlmAssignment::Review { approve } => {
                        approve
                            .send(ApproveOrDeny::Deny {
                                reason: Some(msg_text),
                            })
                            .await?
                    }
                    LlmAssignment::Clarify { send } => send.send(Some(msg_text)).await?,
                }
                tasks.write().await.remove(idx);
            } else {
                let send_back = if let Some(quote) = reply.markdown_text() {
                    format!("> {}\n{}", quote.replace("\n", "\n> "), msg_text)
                } else {
                    msg_text
                };
                match tasks.write().await.pop().unwrap().assignment {
                    LlmAssignment::Review { approve } => {
                        approve
                            .send(ApproveOrDeny::Deny {
                                reason: Some(send_back),
                            })
                            .await?;
                    }
                    LlmAssignment::Clarify { send } => {
                        send.send(Some(send_back)).await?;
                    }
                }
            }
        } else {
            match tasks.write().await.pop().unwrap().assignment {
                LlmAssignment::Review { approve } => {
                    approve
                        .send(ApproveOrDeny::Deny {
                            reason: Some(msg_text),
                        })
                        .await?;
                }
                LlmAssignment::Clarify { send } => {
                    send.send(Some(msg_text)).await?;
                }
            }
        };
        repmark::remove_from_msg(&msg, &bot).await;
    } else {
        event!(Level::TRACE, "state = start");
        if let Some(reply) = msg.reply_to_message() {
            let Some((model, Some(sub))) = db::dialog::Entity::find()
                .filter(db::dialog::Column::MsgId.eq(reply.id.0))
                .find_also_related(db::subscription::Entity)
                .one(&db)
                .await?
            else {
                bot.send_message(
                    msg.chat_id().unwrap(),
                    "There's nothing I can do with that message. Sorry!",
                )
                .await?;
                return Ok(());
            };
            let optimizer = Arc::new(Gemma4Optimizer::new(
                llm::DEFAULT_MODEL.clone(),
                FsDialogMemory::<gemma4::Dialog>::new(working_dir, model.dialog_id),
                SqliteCriteriaMemory::new(db.clone(), Some(model.subscription_id)),
                clarify::TgClarReqHandler::new(bot.clone(), chat_id, dialog.clone()),
                subscription::ModelParamterAccessor::new(db.clone(), sub),
            ));
            if let Some(optimization) = optimizer.optimize_inplace(msg.text()).await? {
                dialog
                    .update(State::Feedingback {
                        tasks: Default::default(),
                    })
                    .await?;
                tokio::spawn(handle_optimization(optimization, bot, chat_id, dialog));
            } else {
                bot.send_message(chat_id, "I don't know how to help with that, cause I forgot about the conversation. Sorry!").await?;
            }
        } else {
            bot.send_message(
                chat_id,
                concat!(
                    "I don't know what to help you with. ",
                    "Please reply to a message of mine so that we can get started."
                ),
            )
            .await?;
        }
    }
    Ok(())
}

async fn receive_feedback_query(
    bot: Bot,
    query: CallbackQuery,
    tasks: Arc<RwLock<Vec<OptimizationTask>>>,
) -> anyhow::Result<()> {
    let Some(msg) = query.regular_message() else {
        event!(Level::WARN, "query message is empty, ignoring");
        return Ok(());
    };
    async {
        let Some(data) = query.data.as_ref() else {
            bot.send_message(query.chat_id().unwrap(), EMPTY_MESSAGE_RESPONSE)
                .await?;
            return Ok(());
        };
        let Some((idx, task)) = (async || {
            let guard = tasks.read().await;
            guard
                .iter()
                .find_position(|t| t.prompt == msg.id)
                .map(|(idx, task)| (idx, task.clone()))
        })()
        .await
        else {
            bot.send_message(query.chat_id().unwrap(), "That's beyond my scope. Sorry!")
                .await?;
            event!(
                Level::WARN,
                "scope error, available: {:?}",
                tasks
                    .read()
                    .await
                    .iter()
                    .map(|t| t.prompt)
                    .collect::<Vec<_>>()
            );
            return Ok(());
        };

        match &task.assignment {
            LlmAssignment::Review { approve } => match data.as_str() {
                "y" => {
                    repmark::remove(&query, &bot).await;
                    approve.send(ApproveOrDeny::Approve).await?
                }
                "n" => {
                    repmark::remove(&query, &bot).await;
                    approve.send(ApproveOrDeny::Deny { reason: None }).await?
                }
                _ => {
                    bot.send_message(query.chat_id().unwrap(), UNKNOWN_ACTION_RESPONSE)
                        .await?;
                    return Ok(());
                }
            },
            LlmAssignment::Clarify { send } => {
                if data.as_str() == "n" {
                    repmark::remove(&query, &bot).await;
                    send.send(None).await?;
                } else {
                    bot.send_message(query.chat_id().unwrap(), UNKNOWN_ACTION_RESPONSE)
                        .await?;
                }
            }
        }
        tasks.write().await.remove(idx);
        Ok(())
    }
    .instrument(info_span!("receive_feedback_query", data = ?query.data, msg_id = ?msg.id))
    .await
}

async fn handle_optimization<Error>(
    mut optimization: OptimizationCallback<Error>,
    bot: Bot,
    chat_id: ChatId,
    dialog: MasterDialog,
) -> anyhow::Result<()>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    bot.send_message(chat_id, "Working on it...").await?;
    let mut actions_required = 0;
    while let Some((action, approve)) = optimization
        .accept()
        .await
        .context("optimization channel closed")?
    {
        let prompt = match action {
            OptimizerAction::ContextPrefill(context) => bot.send_message(
                chat_id,
                if context.len() == 1 {
                    format!(
                        "I would like to add a filtering criterion:\n{}",
                        context.first().unwrap()
                    )
                } else {
                    format!(
                        "I would like to add filtering criteria:\n- {}",
                        context.join("\n- ")
                    )
                },
            ),
            OptimizerAction::Schedule(schedule) => {
                bot.send_message(chat_id, format!("Better to reschedule this as {schedule}"))
            }
        }
        .reply_markup(repmark::button_repmark([vec![
            ("Approve", "y"),
            ("Deny", "n"),
        ]]))
        .await?;
        dialog
            .update(
                dialog
                    .get_or_default()
                    .await?
                    .with_task_queued([OptimizationTask {
                        prompt: prompt.id,
                        assignment: LlmAssignment::Review { approve },
                    }])
                    .await,
            )
            .await?;
        actions_required += 1;
    }
    if actions_required == 0 {
        bot.send_message(
            chat_id,
            "No actions were taken. Thank you anyway, and feel free to retry next time!",
        )
        .await?;
    } else {
        bot.send_message(
            chat_id,
            "I have finished my job. If there's anything more, feel free to ask!",
        )
        .await?;
    }
    dialog.update(State::Start).await?;
    event!(Level::DEBUG, "reset dialog after multi-turn LLM");
    Ok(())
}
