use std::{path::Path, sync::Arc, time::Duration};

use ::futures::future;
use anyhow::{Context, anyhow};
use app_common::config::ParseConfigPath;
use clap::Parser;
use futures::{TryStreamExt, pin_mut};
use itertools::Itertools;
use lib_common::{
    agent::{LlmConditionMatcher, memory::sqlite::SqliteDecisionMemory},
    llm::{
        Model,
        timeout::{ModelProducer, TimedModel},
    },
    polling::{Schedule, Scheduler, task::Task},
    source::{Feed, RssFeed},
    update::sqlite::SqliteUpdatePersistence,
};
use llama_runner::{
    Gemma3VisionRunner, RunnerWithRecommendedSampling, error::CreateLlamaCppRunnerError,
};
use migration::FromValueTuple;
use sea_orm::{ActiveEnum, DatabaseConnection, EntityTrait, Iterable, ModelTrait, TryFromU64};
use smol_str::SmolStr;
use teloxide::{
    dispatching::{
        UpdateHandler,
        dialogue::{GetChatId, InMemStorage},
    },
    payloads::SendMessageSetters,
    prelude::*,
};
use tokio::{select, sync::RwLock};
use tracing::{Instrument, Level, event, info_span};

use crate::{
    authenticator::{
        Access, Authenticator as _, priority::PriorityAuthenticator, sqlite::SqliteAuthenticator,
        whitelist::WhitelistAuthenticator,
    },
    config::Config,
    db::{
        self, rss,
        subscription::{self, SubscriptionId},
        user::AccessLevel,
    },
    rss::add_rss_subscription_for,
    telegram::{args::Args, command::Command, repmark::button_repmark, state::State},
    token::OnetimeToken,
};

mod args;
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
        .with_max_level(args.verbosity)
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
    let scheduler = Arc::new(Scheduler::<SubscriptionId>::new());

    future::select(
        Box::pin(
            Dispatcher::builder(bot.clone(), bot_state_machine())
                .dependencies(dptree::deps![
                    InMemStorage::<State>::new(),
                    token,
                    db.clone(),
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
                let model = TimedModel::new(
                    "decider_vlm",
                    Duration::from_mins(5),
                    ModelProducer::new(async || Gemma3VisionRunner::default().await),
                );
                loop {
                    select! {
                        _ = start_polling_all(scheduler.clone(), model.clone(), &db, &data_path, &bot) => {
                            event!(
                                Level::INFO,
                                "empty schedule queue, waiting 30s before retrying"
                            );
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        },
                        _ = scheduler.until_next_reschedule() => {},
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
                    dptree::case![State::GotRssCondition { condition, url }]
                        .endpoint(choose_rss_mock_browser),
                )
                .branch(
                    dptree::case![State::GotRssMockBrowserUa {
                        mock,
                        condition,
                        url
                    }]
                    .endpoint(choose_rss_custom_headers),
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
                .branch(dptree::case![State::ChoseRss].endpoint(receive_rss_url))
                .branch(dptree::case![State::GotRssUrl { url }].endpoint(receive_rss_condition))
                .branch(
                    dptree::case![State::GotRssMockBrowserUa {
                        mock,
                        condition,
                        url
                    }]
                    .endpoint(receive_rss_custom_headers),
                ),
        )
}

async fn start_polling_all(
    scheduler: Arc<Scheduler<SubscriptionId>>,
    model: TimedModel<RunnerWithRecommendedSampling<Gemma3VisionRunner>, CreateLlamaCppRunnerError>,
    db: &DatabaseConnection,
    working_dir: &Path,
    bot: &Bot,
) -> anyhow::Result<()> {
    let tasks = scheduler.schedules().await.into_iter().map(|schedule| {
        let model = model.clone();
        let scheduler = scheduler.clone();
        let sub_id = *schedule.key();
        async move {
            let Some((sub, rss)) = subscription::Entity::find_by_id(sub_id)
                .find_also_related(rss::Entity)
                .one(db)
                .await?
            else {
                event!(Level::ERROR, "failed to find subscription from id");
                return Ok(());
            };

            if let Some(rss) = rss {
                let feed: RssFeed = rss.try_into()?;
                let decider = LlmConditionMatcher::new(
                    model,
                    sub.condition,
                    SqliteDecisionMemory::new(db.clone(), working_dir, Some(sub_id))?,
                );
                let updates = app_common::feed::check(
                    &sub_id,
                    &feed,
                    &decider,
                    &scheduler,
                    SqliteUpdatePersistence::new(db.clone(), sub_id)?,
                );
                pin_mut!(updates);
                while let Some((_, is_truthy)) = updates.try_next().await? {
                    if is_truthy {
                        let msg = match feed.get_metadata().await {
                            Ok(meta) => format!(
                                "Your subscription to \"{}\" ({}) has an update! Check it out",
                                meta.name,
                                feed.url()
                            ),
                            Err(err) => {
                                event!(Level::WARN, "failed to fetch RSS feed metadata, falling back to URL only: {err}");
                                format!(
                                    "Your subscription to {} has an update! Check it out",
                                    feed.url()
                                )
                            }
                        };
                        bot.send_message(teloxide::types::UserId(sub.user_id as u64), msg)
                            .await?;
                    }
                }
                return Ok(());
            }

            Err(anyhow!("subscription has no associated feed"))
        }
        .instrument(info_span!("run_task", subscription_id = sub_id))
    });
    future::try_join_all(tasks).await?;
    Ok(())
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
            dialog.update(State::ChoseRss).await?;
            bot.send_message(
                query.chat_id().unwrap(),
                "Perfect! Let's fill in the details. Tell me, what's the URL to the RSS feed?",
            )
            .await?;
        }
    }
    Ok(())
}

async fn receive_rss_url(bot: Bot, msg: Message, dialog: MasterDialog) -> anyhow::Result<()> {
    let Some(url) = msg.text() else {
        bot.send_message(msg.chat_id().unwrap(), EMPTY_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };
    dialog.update(State::GotRssUrl { url: url.into() }).await?;
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
    url: SmolStr,
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
        .update(State::GotRssCondition {
            condition: text.into(),
            url,
        })
        .await?;
    bot.send_message(msg.chat_id().unwrap(), "Great! But many sites may block access, unless they recoginze me as a web broswer. Do you want to add extra user agent headers to solve this issue?")
        .reply_markup(button_repmark([vec![("Yes", "y"), ("No", "n")], vec![("Custom", "c"), ("Skip", "s")]]))
        .await?;
    Ok(())
}

async fn choose_rss_mock_browser(
    bot: Bot,
    (condition, url): (SmolStr, SmolStr), // State::GotRssUrl
    query: CallbackQuery,
    dialog: MasterDialog,
    db: DatabaseConnection,
    scheduler: Arc<Scheduler<SubscriptionId>>,
) -> anyhow::Result<()> {
    let chat_id = query.chat_id().unwrap();
    let user_id = query.from.id.0 as UserId;
    let Some(action_id) = query.data else {
        bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        return Ok(());
    };
    match action_id.as_str() {
        "y" => {
            dialog
                .update(State::GotRssMockBrowserUa {
                    mock: true,
                    condition,
                    url,
                })
                .await?
        }
        "n" => {
            dialog
                .update(State::GotRssMockBrowserUa {
                    mock: false,
                    condition,
                    url,
                })
                .await?;
        }
        "c" => {
            dialog
                .update(State::GotRssMockBrowserUa {
                    mock: false,
                    condition,
                    url,
                })
                .await?;
            bot.send_message(chat_id, CUSTOM_HEADERS_PROMPT).await?;
            return Ok(());
        }
        "s" => {
            add_rss_subscription_for(user_id, url, condition, false, None, &db, &scheduler).await?;
            dialog.reset().await?;
            bot.send_message(chat_id, "I see. You are ready to go")
                .await?;
            return Ok(());
        }
        _ => {
            bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
            return Ok(());
        }
    }
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
    (mock, condition, url): (bool, SmolStr, SmolStr), // State::GotRssMockBrowserUa
    db: DatabaseConnection,
    scheduler: Arc<Scheduler<SubscriptionId>>,
) -> anyhow::Result<()> {
    let chat_id = query.chat_id().unwrap();
    let user_id = query.from.id.0 as UserId;
    let Some(action_id) = query.data else {
        bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        return Ok(());
    };
    match action_id.as_str() {
        "y" => {
            bot.send_message(chat_id, CUSTOM_HEADERS_PROMPT).await?;
        }
        "n" => {
            add_rss_subscription_for(user_id, url, condition, mock, None, &db, &scheduler).await?;
            dialog.reset().await?;
            bot.send_message(chat_id, "Perfect! You are ready to go")
                .await?;
            return Ok(());
        }
        _ => {
            bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        }
    }
    Ok(())
}

async fn receive_rss_custom_headers(
    bot: Bot,
    msg: Message,
    (mock, condition, url): (bool, SmolStr, SmolStr), // State::GotRssMockBrowserUa
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
    match add_rss_subscription_for(
        user.id.0 as UserId,
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
