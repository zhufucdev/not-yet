use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::{fmt::Display, hash::Hash, path::Path, sync::Arc, time::Duration};

use ::futures::future;
use anyhow::anyhow;
use app_common::config::ParseConfigPath;
use app_common::poller::{UpdateContext, Updater};
use futures::{FutureExt, StreamExt, TryStreamExt};
use itertools::Itertools;
use lib_common::agent::decision::Decider;
use lib_common::agent::error::GetTruthValueError;
use lib_common::agent::memory::criteria::sqlite::SqliteCriteriaMemory;
use lib_common::agent::memory::decision::DecisionMemory;
use lib_common::agent::memory::dialog::DialogMemory;
use lib_common::agent::memory::dialog::fs::FsDialogMemory;
use lib_common::agent::optimize::llm::LlmOptimizer;
use lib_common::agent::optimize::{
    ApproveOrDeny, BasicOptimizerAction, OptimizationCallback, Optimizer, OptimizerAction,
};
use lib_common::runner::OllamaRunner;
use lib_common::secure;
use lib_common::update::Source;
use lib_common::{
    agent::{LlmConditionMatcher, memory::decision::SqliteDecisionMemory},
    polling::{Schedule, Scheduler, schedule::QueueType},
    source::{DefaultMetadata, Feed, LlmComprehendable, RssFeed, atom::AtomFeed},
    update::sqlite::SqliteUpdatePersistence,
};
use ollama_rs::generation::chat::ChatMessage;
use sea_orm::{
    ActiveEnum, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, Iterable, ModelTrait,
    QueryFilter,
};
use serde::Deserialize;
use serde::{Serialize, de::DeserializeOwned};
use smol_str::SmolStr;
use teloxide::sugar::request::RequestReplyExt;
use teloxide::types::{MessageId, Recipient};
use teloxide::utils::render::RenderMessageTextHelper;
use teloxide::{
    dispatching::{
        UpdateHandler,
        dialogue::{GetChatId, InMemStorage},
    },
    payloads::SendMessageSetters,
    prelude::*,
};
use tokio::sync::RwLock;
use tracing::{Instrument, Level, event, info_span};

use crate::authenticator::whitelist;
use crate::db::notify;
use crate::init::InitResult;
use crate::telegram::optimize::TgOptimizerAction;
use crate::telegram::optimize::renotify::SetReceipientTool;
use crate::telegram::state::{LlmAssignment, OptimizationTask, StateFeedback};
use crate::telegram::update_echo::{UpdateEcho, UpdateEchoHistory};
use crate::{
    authenticator::{
        Access, Authenticator as _, priority::PriorityAuthenticator, sqlite::SqliteAuthenticator,
        whitelist::WhitelistAuthenticator,
    },
    config::Config,
    db::{
        self, atom,
        subscription::{self, SubscriptionId},
        user::AccessLevel,
    },
    rss::add_feed_subscription_for,
    telegram::{command::Command, repmark::button_repmark, state::State},
    token::OnetimeToken,
};

mod args;
mod clarify;
mod command;
mod optimize;
mod repmark;
mod state;
mod update_echo;

type MasterDialog = Dialogue<State, InMemStorage<State>>;
type Authenticator = PriorityAuthenticator<(
    WhitelistAuthenticator<UserId, AccessLevel>,
    SqliteAuthenticator,
)>;

pub type UserId = i64;
pub use args::Args;

pub(super) async fn init(args: &Args, config: &Config) -> anyhow::Result<TgInitResult> {
    let bot = Bot::with_client(
        std::env::var("BOT_TOKEN")
            .ok()
            .or(args.bot_token.clone())
            .or(config.bot_token.clone())
            .ok_or(anyhow!("invalid BOT_TOKEN environment variable, either set the BOT_TOKEN environment variable, specify in the configuration or use the --bot-token command line argument"))?,
        teloxide::net::client_from_env(),
    );

    let data_path = args.parse_config_path()?;
    let db = app_common::config::setup_db(&data_path).await?;
    let runner = OllamaRunner::default();
    let echos = UpdateEchoHistory::default();
    let whitelist = WhitelistAuthenticator::new(
        config.whitelist.clone().unwrap_or_default(),
        AccessLevel::ConfiguredWhitelist,
    );

    Ok(TgInitResult {
        bot,
        db,
        data_path,
        runner,
        echos,
        whitelist,
    })
}

pub(super) struct TgInitResult {
    bot: Bot,
    db: DatabaseConnection,
    data_path: PathBuf,
    runner: OllamaRunner,
    echos: UpdateEchoHistory,
    whitelist: WhitelistAuthenticator<UserId, AccessLevel>,
}

impl InitResult for TgInitResult {
    type ScheduleKey = SubscriptionId;

    async fn main(self, scheduler: Arc<Scheduler<Self::ScheduleKey>>) -> Result<(), anyhow::Error> {
        let token = OnetimeToken::new();
        println!("access token: {}", token.value().await);

        Dispatcher::builder(self.bot.clone(), bot_state_machine())
            .dependencies(dptree::deps![
                InMemStorage::<State>::new(),
                self.db.clone(),
                self.data_path.clone(),
                self.runner.clone(),
                Arc::new(Authenticator::from((
                    self.whitelist,
                    SqliteAuthenticator::new(self.db.clone())
                ))),
                self.echos.clone(),
                token,
                scheduler
            ])
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;
        Ok(())
    }

    async fn attach_to_poller<'a>(
        &self,
        mut poller: app_common::poller::PollerTransaction<'a, Self::ScheduleKey>,
        key: Self::ScheduleKey,
    ) -> anyhow::Result<()> {
        let Some(sub) = subscription::Entity::find_by_id(key).one(&self.db).await? else {
            event!(Level::ERROR, "failed to find subscription from id");
            return Ok(());
        };
        let (rss, atom) = future::try_join(
            sub.find_related(db::rss::Entity).one(&self.db),
            sub.find_related(atom::Entity).one(&self.db),
        )
        .await?;

        let buffer_size = sub.buffer_size as usize;
        match sub.kind {
            subscription::Kind::Rss => {
                let Some(rss) = rss else {
                    return Err(anyhow!("RSS subscription has no associated feed"));
                };
                let feed: RssFeed = rss.try_into()?;
                let updater = LlmDeciderUpdater {
                    sub,
                    runner: self.runner.clone(),
                    bot: self.bot.clone(),
                    echos: self.echos.clone(),
                    db: self.db.clone(),
                    working_dir: self.data_path.clone(),
                    feed_url: feed.url().to_string(),
                    _marker: PhantomData,
                };
                poller.add_updater(
                    key,
                    updater,
                    feed,
                    SqliteUpdatePersistence::new(self.db.clone(), key)?,
                    buffer_size,
                );
            }
            subscription::Kind::Atom => {
                let Some(atom) = atom else {
                    return Err(anyhow!("Atom subscription has no associated feed"));
                };
                let feed: AtomFeed = atom.try_into()?;
                let updater = LlmDeciderUpdater {
                    sub,
                    runner: self.runner.clone(),
                    bot: self.bot.clone(),
                    echos: self.echos.clone(),
                    db: self.db.clone(),
                    working_dir: self.data_path.clone(),
                    feed_url: feed.url().to_string(),
                    _marker: PhantomData,
                };
                poller.add_updater(
                    key,
                    updater,
                    feed,
                    SqliteUpdatePersistence::new(self.db.clone(), key)?,
                    buffer_size,
                );
            }
        }

        Ok(())
    }

    async fn get_schedules(
        &self,
    ) -> anyhow::Result<impl IntoIterator<Item = Schedule<Self::ScheduleKey>>> {
        Ok(subscription::Entity::find()
            .all(&self.db)
            .await?
            .into_iter()
            .enumerate()
            .map(|(id, s)| Schedule::new(id, s.id, s.schedule_trigger()).unwrap()))
    }

    #[cfg(feature = "serve-rss")]
    async fn get_rss_broadcasts(
        &self,
    ) -> anyhow::Result<impl IntoIterator<Item = app_common::rss::Broadcast>> {
        async fn get_rss_items<M>(
            db: DatabaseConnection,
            working_dir: PathBuf,
            sub_id: SubscriptionId,
        ) -> anyhow::Result<Vec<rss::Item>>
        where
            M: LlmComprehendable
                + Clone
                + Into<rss::Item>
                + Serialize
                + DeserializeOwned
                + Send
                + Sync,
        {
            let items: Vec<rss::Item> =
                SqliteDecisionMemory::<M>::new(db, working_dir, Some(sub_id))?
                    .iter_newest_first()
                    .try_filter(|item| future::ready(item.as_ref().is_truthy))
                    .map_ok(|item| item.as_ref().material.clone().into())
                    .try_collect::<Vec<_>>()
                    .await?;
            Ok(items)
        }

        let broadcasts = future::try_join_all(
            db::broadcast::Entity::find()
                .find_also_related(db::subscription::Entity)
                .filter(db::broadcast::Column::Kind.eq(db::broadcast::Kind::Rss))
                .all(&self.db)
                .await?
                .into_iter()
                .map(|(config, sub)| (config, sub.unwrap()))
                .chunk_by(|(config, _)| config.rss_key.as_ref().unwrap().clone())
                .into_iter()
                .map(|(_, group)| {
                    group.map(
                        async |(config, sub)| -> anyhow::Result<(db::broadcast::Model, Vec<rss::Item>)> {
                            Ok((
                                config,
                                (match sub.kind {
                                    subscription::Kind::Rss => {
                                        use lib_common::source::LlmRssItem;

                                        get_rss_items::<LlmRssItem>(
                                            self.db.clone(),
                                            self.data_path.clone(),
                                            sub.id,
                                        )
                                        .await
                                    }
                                    subscription::Kind::Atom => {
                                        use lib_common::source::atom::AtomFeedItem;

                                        get_rss_items::<AtomFeedItem>(
                                            self.db.clone(),
                                            self.data_path.clone(),
                                            sub.id,
                                        )
                                        .await
                                    }
                                })?
                                .into_iter()
                                .sorted_by_key(|item| item.pub_date().map(|date_str| date_str.to_string()).unwrap_or_default())
                                .rev()
                                .collect_vec()
                            ))
                        },
                    )
                })
                .flatten(),
        )
        .await?
        .into_iter()
        .map(|(config, items)| {
            app_common::rss::Broadcast::new(
                config.rss_key.unwrap(),
                config.rss_title.unwrap(),
                config.rss_description.unwrap(),
                items
            )
        });

        Ok(broadcasts)
    }
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
                .endpoint(receive_generic_msg),
        )
}

struct LlmDeciderUpdater<F> {
    sub: subscription::Model,
    runner: OllamaRunner,
    bot: Bot,
    echos: UpdateEchoHistory,
    db: DatabaseConnection,
    working_dir: PathBuf,
    feed_url: String,
    _marker: PhantomData<F>,
}

impl<F> Updater for LlmDeciderUpdater<F>
where
    F: Feed<Metadata = DefaultMetadata> + Send + Sync,
    F::Item: LlmComprehendable
        + Clone
        + Send
        + Sync
        + Serialize
        + DeserializeOwned
        + Display
        + Into<rss::Item>
        + 'static,
    F::Error: Display,
{
    type Key = SubscriptionId;

    type Source = F;

    async fn on_update(
        &self,
        material: Option<Box<<Self::Source as Source>::Item>>,
        source: &Self::Source,
        ctx: UpdateContext,
    ) -> Result<bool, anyhow::Error> {
        let Some(item) = material else {
            return Ok(false);
        };
        let dialog_id = secure::generate_random_id(32);
        let mut decision_mem =
            SqliteDecisionMemory::new(self.db.clone(), &self.working_dir, Some(self.sub.id))?;
        let decider = LlmConditionMatcher::new(
            self.runner.clone(),
            self.sub.condition.to_string(),
            decision_mem.clone(),
            FsDialogMemory::new(&self.working_dir, &dialog_id),
            SqliteCriteriaMemory::new(self.db.clone(), Some(self.sub.id)),
        );
        match decider.get_truth_value(item.as_ref()).await {
            Ok(false) => {
                return Ok(false);
            }
            Ok(true) => {}
            Err(GetTruthValueError::DecisionMemory(err)) => {
                event!(Level::ERROR, "decision memory error: {err}");
                event!(Level::WARN, "will clear decision memory");
                decision_mem.clear().await?;
            }
            Err(err) => {
                event!(Level::ERROR, "decider error: {err}");
            }
        }

        let msg = match source.get_metadata().await {
            Ok(meta) => format!(
                "Your subscription to \"{}\" ({}) has an update! Check it out!\n{}",
                meta.name,
                self.feed_url,
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
                    self.feed_url,
                    item.to_string(),
                )
                .trim_end()
                .to_string()
            }
        };
        let dest = self.sub.find_related(notify::Entity).one(&self.db).await?;
        let recipient: Recipient = dest
            .map(|d| d.into())
            .unwrap_or_else(|| UserId(self.sub.user_id as u64).into());
        let echo = UpdateEcho {
            msg: msg.clone(),
            dialog_id: dialog_id.clone(),
            sub_id: self.sub.id,
            recipient: recipient.clone(),
        };
        let tg_msg = self.bot.send_message(recipient.clone(), msg).await?;
        echo.as_active_model()
            .set_msg_id(tg_msg.id.0)
            .set_chat_id(tg_msg.chat.id.0)
            .insert(&self.db)
            .await
            .inspect_err(|err| event!(Level::WARN, "failed to save dialog: {err}"))?;
        if matches!(recipient, Recipient::ChannelUsername(_)) {
            self.echos.write().await.push(echo);
        }
        #[cfg(feature = "serve-rss")]
        async {
            future::join_all(
                self.sub
                    .find_related(db::broadcast::Entity)
                    .filter(db::broadcast::Column::Kind.eq(db::broadcast::Kind::Rss))
                    .all(&self.db)
                    .await?
                    .into_iter()
                    .map(async |config| {
                        ctx.rss_server
                            .broadcast(config.rss_key.unwrap())
                            .await
                            .unwrap()
                            .push_item((*item.clone()).into())
                            .await;
                    }),
            )
            .await;

            Result::<_, anyhow::Error>::Ok(())
        }
        .await?;
        Ok(true)
    }
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
                    .chain([vec![("Cancel".to_string(), "cancel".to_string())]])
                    .collect::<Vec<_>>(),
            ))
            .await?;
        }
        Ok(Access::Denied) => {
            dialog.update(State::Authenticating).await?;
            bot.send_message(msg.chat_id().unwrap(), "Good day! Nice to see you! I can ping you update messages as instructed. To get started, paste here the one-time access token. You can get it from the console").await?;
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
            current_token.rotate().await;
            println!("rotated token: {}", current_token.value().await);
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
    if query.data.as_ref().is_some_and(|it| it == "cancel") {
        dialog.reset().await?;
        repmark::remove(&query, &bot).await;
        if let Some(message) = query.regular_message() {
            bot.edit_message_text(
                query.chat_id().unwrap(),
                message.id,
                "I have canceled your request. Feel free to try again anytime!",
            )
            .await?;
        }
        return Ok(());
    }
    let Some(kind_hex) = &query.data else {
        bot.send_message(query.chat_id().unwrap(), UNKNOWN_ACTION_RESPONSE)
            .await?;
        bot.answer_callback_query(query.id).await?;
        return Ok(());
    };
    const INVALID_DATA_RESPONSE: &str = "I don't believe the data attached to the button you just clicked is valid. How did this happen?";
    let Ok(kind_id) = i32::from_str_radix(kind_hex, 16) else {
        bot.send_message(query.chat_id().unwrap(), INVALID_DATA_RESPONSE)
            .await?;
        bot.answer_callback_query(query.id).await?;
        return Ok(());
    };
    let Ok(kind) = subscription::Kind::try_from_value(&kind_id) else {
        bot.send_message(query.chat_id().unwrap(), INVALID_DATA_RESPONSE)
            .await?;
        bot.answer_callback_query(query.id).await?;
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
    bot.answer_callback_query(query.id.clone()).await?;
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
                .await?;
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
            bot.answer_callback_query(query.id).await?;
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
    bot.answer_callback_query(query.id.clone()).await?;
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
        }
        _ => {
            bot.send_message(chat_id, UNKNOWN_ACTION_RESPONSE).await?;
        }
    }
    bot.answer_callback_query(query.id.clone()).await?;
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

async fn receive_generic_msg(
    bot: Bot,
    echos: UpdateEchoHistory,
    msg: Message,
    db: DatabaseConnection,
    runner: OllamaRunner,
    dialog: MasterDialog,
    working_dir: PathBuf,
    authenticator: Arc<Authenticator>,
) -> anyhow::Result<()> {
    if let Some(echo) = echos.pop_similar(&msg).await {
        event!(Level::DEBUG, "received a self message from public chat");
        echo.as_active_model()
            .set_chat_id(msg.chat.id.0)
            .set_msg_id(msg.id.0)
            .insert(&db)
            .await?;
        return Ok(());
    }

    let Some(sender) = &msg.from else {
        return Ok(());
    };
    if sender.is_bot || sender.is_telegram() || sender.is_channel() || sender.is_anonymous() {
        return Ok(());
    }
    if !matches!(
        authenticator.get_access(&(sender.id.0 as UserId)).await?,
        Access::Granted(_)
    ) {
        return Ok(());
    }
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
                .find_position(|t| t.prompt.id == reply.id)
            {
                match &task.assignment {
                    LlmAssignment::Review { approve } => {
                        approve
                            .send(ApproveOrDeny::Deny {
                                reason: Some(msg_text),
                            })
                            .await?;
                    }
                    LlmAssignment::Clarify { send } => send.send(Some(msg_text)).await?,
                }
                tasks.write().await.remove(idx);
                task.reset_user_prompt(&bot).await;
            } else {
                let send_back = if let Some(quote) = reply.markdown_text() {
                    format!("> {}\n{}", quote.replace("\n", "\n> "), msg_text)
                } else {
                    msg_text
                };
                let task = tasks.write().await.pop().unwrap();
                match &task.assignment {
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
                task.reset_user_prompt(&bot).await;
            }
        } else {
            let task = tasks.write().await.pop().unwrap();
            match &task.assignment {
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
            task.reset_user_prompt(&bot).await;
        };
    } else {
        event!(Level::TRACE, "state = start");
        if let Some(reply) = msg.reply_to_message() {
            let Some((model, Some(sub))) = db::dialog::Entity::find()
                .filter(
                    db::dialog::Column::MsgId.eq(reply.id.0).and(
                        db::dialog::Column::ChatId
                            .eq(reply.chat.id.0)
                            .or(db::dialog::Column::ChatId.is_null()),
                    ),
                )
                .find_also_related(db::subscription::Entity)
                .one(&db)
                .await?
            else {
                event!(
                    Level::TRACE,
                    "no dialog found, reply.id = {}, reply.chat.id = {}",
                    reply.id,
                    reply.chat.id
                );
                bot.send_message(
                    msg.chat_id().unwrap(),
                    "There's nothing I can do with that message. Sorry!",
                )
                .await?;
                return Ok(());
            };
            let dialog_mem = FsDialogMemory::<Vec<ChatMessage>>::new(working_dir, model.dialog_id);
            let optimizer = Arc::new(
                LlmOptimizer::new(
                    runner,
                    dialog_mem.clone(),
                    SqliteCriteriaMemory::new(db.clone(), Some(model.subscription_id)),
                    clarify::TgClarReqHandler::new(bot.clone(), chat_id, dialog.clone()),
                    subscription::ModelParamterAccessor::new(db.clone(), sub),
                )
                .add_tool(SetReceipientTool {
                    sub_id: model.subscription_id,
                    db: db.clone(),
                }),
            );
            if let Some(decision_dialog) = dialog_mem.get().await? {
                let optimization = optimizer.optimize(msg.text(), decision_dialog);
                dialog
                    .update(State::Feedingback {
                        tasks: Default::default(),
                    })
                    .await?;
                tokio::spawn(handle_optimization(
                    optimization,
                    bot,
                    chat_id,
                    msg.id,
                    dialog,
                ));
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
            bot.answer_callback_query(query.id.clone()).await?;
            return Ok(());
        };
        let Some((idx, task)) = (async || {
            let guard = tasks.read().await;
            guard
                .iter()
                .find_position(|t| t.prompt.id == msg.id)
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
                    .map(|t| t.prompt.id)
                    .collect::<Vec<_>>()
            );
            bot.answer_callback_query(query.id.clone()).await?;
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
                    bot.answer_callback_query(query.id.clone()).await?;
                    return Ok(());
                }
            },
            LlmAssignment::Clarify { send } => {
                if data.as_str() == "n" {
                    send.send(None).await?;
                } else {
                    bot.send_message(query.chat_id().unwrap(), UNKNOWN_ACTION_RESPONSE)
                        .await?;
                }
            }
        }
        task.reset_user_prompt(&bot).await;
        bot.answer_callback_query(query.id.clone()).await?;
        tasks.write().await.remove(idx);
        Ok(())
    }
    .instrument(info_span!("receive_feedback_query", data = ?query.data, msg_id = ?msg.id))
    .await
}

async fn handle_optimization<Error>(
    mut optimization: OptimizationCallback<Error, TgOptimizerAction>,
    bot: Bot,
    chat_id: ChatId,
    reply_to: impl Into<MessageId> + Clone,
    dialog: MasterDialog,
) -> anyhow::Result<()>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    bot.send_message(chat_id, "Working on it...").await?;
    let mut actions_required = 0;
    while let Ok(Some((action, approve))) = optimization.accept().await {
        let prompt = match action {
            OptimizerAction::Basic(BasicOptimizerAction::ContextPrefill(context)) => bot
                .send_message(
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
            OptimizerAction::Basic(BasicOptimizerAction::Schedule(schedule)) => {
                bot.send_message(chat_id, format!("Better to reschedule this as {schedule}"))
            }
            OptimizerAction::Extra(TgOptimizerAction::SetReceipient(recipient)) => {
                let represntation = recipient.as_representation(chat_id, &bot).await?;
                bot.send_message(
                    chat_id,
                    format!("I want to redirect notifications to {represntation}"),
                )
            }
        }
        .reply_markup(repmark::button_repmark([vec![
            ("Approve", "y"),
            ("Deny", "n"),
        ]]))
        .reply_to(reply_to.clone())
        .await?;
        dialog
            .update(
                dialog
                    .get_or_default()
                    .await?
                    .with_task_queued([OptimizationTask {
                        prompt,
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
            "No actions were taken. Thank you for cooperation, and feel free to retry next time!",
        )
    } else {
        bot.send_message(
            chat_id,
            "I have finished my job. If there's anything more, feel free to ask!",
        )
    }
    .await?;
    dialog.update(State::Start).await?;
    event!(Level::DEBUG, "reset dialog after multi-turn LLM");
    Ok(())
}
