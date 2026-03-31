use std::sync::Arc;

use ::futures::future;
use anyhow::anyhow;
use clap::Parser;
use futures::{TryStreamExt, pin_mut};
use itertools::Itertools;
use lib_common::{
    config::ParseConfigPath,
    polling::{Scheduler, task::Task},
};
use migration::FromValueTuple;
use sea_orm::{ActiveEnum, DatabaseConnection, Iterable, TryFromU64};
use teloxide::{
    dispatching::{
        UpdateHandler,
        dialogue::{GetChatId, InMemStorage},
    },
    payloads::SendMessageSetters,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup},
};
use tokio::sync::RwLock;
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
    telegram::{args::Args, command::Command, state::State},
    token::OnetimeToken,
};

mod args;
mod command;
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
    let data_path = args.config.parse()?;
    let config = lib_common::config::parse_config::<Config>(
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
    let db = lib_common::config::setup_db(&data_path).await?;
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
                            config.whitelist,
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
                let tasks = scheduler.start_polling(None);
                pin_mut!(tasks);
                while let Ok(Some(task)) = tasks.try_next().await {
                    async {
                        run_task(&task, &db, &bot).await;
                    }
                    .instrument(info_span!(
                        "run_task",
                        subscription_id = task.schedule().key()
                    ))
                    .await
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
        .branch(Update::filter_callback_query().branch(
            dptree::case![State::ChoosingSubscriptionKind].endpoint(chose_subscription_type),
        ))
        .branch(
            Update::filter_message()
                .enter_dialogue::<Message, InMemStorage<State>, State>()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .branch(dptree::case![Command::Start].endpoint(start)),
                )
                .branch(dptree::case![State::Authenticating].endpoint(authenticate))
                .branch(dptree::case![State::ChoseRss].endpoint(received_rss_url)),
        )
}

async fn run_task(
    task: &Task<SubscriptionId>,
    db: &DatabaseConnection,
    bot: &Bot,
) -> anyhow::Result<()> {
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
            .reply_markup(InlineKeyboardMarkup::new(
                db::subscription::Kind::iter()
                    .map(|kind| {
                        InlineKeyboardButton::new(
                            kind.to_string(),
                            InlineKeyboardButtonKind::CallbackData(format!(
                                "{:x}",
                                kind.to_value()
                            )),
                        )
                    })
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
        bot.send_message(
            query.chat_id().unwrap(),
            "I don't recogize this action, and please try something else",
        )
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

async fn received_rss_url(
    bot: Bot,
    msg: Message,
    dialog: MasterDialog,
    scheduler: Arc<Scheduler<SubscriptionId>>,
    db: DatabaseConnection,
) -> anyhow::Result<()> {
    let Some(user_id) = msg.from.as_ref().map(|user| user.id.0 as UserId) else {
        return Ok(());
    };
    let Some(url) = msg.text() else {
        bot.send_message(msg.chat_id().unwrap(), EMPTY_MESSAGE_RESPONSE)
            .await?;
        return Ok(());
    };
    subscription::ActiveModel::builder()
        .set_user_id(user_id)
        .set_rss(rss::ActiveModel::builder().set_url(url))
        .save(&db)
        .await?;
    Ok(())
}
