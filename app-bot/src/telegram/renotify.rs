use std::{borrow::Cow, fmt::Display};

use itertools::Itertools;
use lib_common::agent::optimize::{
    ApproveOrDeny,
    llm::{SystemResult, ToToolResult, Tool, ToolHandlerError, ToolResult},
};
use ollama_rs::re_exports::schemars::JsonSchema;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait, IntoActiveModel};
use serde::{Deserialize, Serialize};
use teloxide::{
    Bot,
    payloads::SendMessageSetters,
    prelude::Requester,
    types::{ChatFullInfo, ChatId, Recipient, User, UserId},
};
use tokio::sync::mpsc;

use crate::{
    db::{
        notify,
        subscription::{self, SubscriptionId},
    },
    telegram::{
        MasterDialog, repmark,
        state::{LlmAssignment, OptimizationTask, StateFeedback},
    },
};

pub struct SetReceipientTool {
    pub chat_id: ChatId,
    pub sub_id: SubscriptionId,
    pub db: DatabaseConnection,
    pub bot: Bot,
    pub dialog: MasterDialog,
}

impl Tool for SetReceipientTool {
    const IS_ACTION: bool = true;

    const RETRIEVER_TOOL_NAME: Option<&'static str> = None;

    type Params = SetRecipientParams;

    fn name() -> &'static str {
        "set_receipient"
    }

    fn description() -> &'static str {
        "change where to push update notifications to"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<ToolResult> {
        let sane_recipient = {
            let r: SetRecipient = parameters.into();
            r.santized()
        };
        let recipient = sane_recipient
            .as_representation(self.chat_id, &self.bot)
            .await?;
        let prompt = self
            .bot
            .send_message(
                self.chat_id,
                format!("I want to redirect notifications to {recipient}"),
            )
            .reply_markup(repmark::button_repmark([vec![
                ("Approve", "y"),
                ("Deny", "n"),
            ]]))
            .await?;
        let (res_tx, mut res_rx) = mpsc::channel(1);
        self.dialog
            .update(
                self.dialog
                    .get_or_default()
                    .await?
                    .with_task_queued([OptimizationTask {
                        prompt,
                        assignment: LlmAssignment::Review { approve: res_tx },
                    }])
                    .await,
            )
            .await?;
        let Some(action) = res_rx.recv().await else {
            return Err(Box::new(ToolHandlerError::ChannelClosed));
        };

        if matches!(action, ApproveOrDeny::Approve) {
            let Some(mut sub) = subscription::Entity::find_by_id(self.sub_id)
                .one(&self.db)
                .await?
                .map(subscription::Model::into_active_model)
            else {
                return Err(format!("subscription {} not found", self.sub_id).into());
            };
            let destination = match sane_recipient {
                SetRecipient::User(id) => {
                    let notice = self.bot.send_message(UserId(id), "Hi!").await?;
                    Some(
                        notify::ActiveModel {
                            kind: ActiveValue::Set(notify::Kind::Chat),
                            chat_id: ActiveValue::Set(Some(notice.chat.id.0)),
                            ..Default::default()
                        }
                        .insert(&self.db)
                        .await?
                        .id,
                    )
                }
                SetRecipient::Channel(id) => Some(
                    notify::ActiveModel {
                        kind: ActiveValue::Set(notify::Kind::Channel),
                        channel_id: ActiveValue::Set(Some(id)),
                        ..Default::default()
                    }
                    .insert(&self.db)
                    .await?
                    .id,
                ),
                SetRecipient::Clear => None,
            };
            sub.notify_id.set_if_not_equals(destination);
            sub.save(&self.db).await?;
        }

        Ok(action
            .map_ok(|_| format!("upcoming notifications will be pushed to {recipient}").into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename_all = "snake_case")]
/// Choose ONLY one of the fields. Omitting all defaults to `clear`
pub struct SetRecipientParams {
    /// Target a user, specifying their numeric user ID
    pub user: Option<u64>,
    /// Target a channel, specifying its ID, usually prefixed with @
    pub channel: Option<String>,
    /// Restore to default settings, targeting the current user
    pub clear: Option<()>,
}

pub enum SetRecipient {
    User(u64),
    Channel(String),
    Clear,
}

#[derive(Debug, Clone)]
pub enum SetReceipientParamsRepresentation {
    User(User),
    Channel(String),
    Clear(ChatFullInfo),
}

impl Into<SetRecipient> for SetRecipientParams {
    fn into(self) -> SetRecipient {
        if let Some(user) = self.user {
            return SetRecipient::User(user);
        }
        if let Some(channel) = self.channel {
            return SetRecipient::Channel(channel);
        }
        return SetRecipient::Clear;
    }
}

impl SetRecipient {
    pub fn santized(self) -> Self {
        match self {
            SetRecipient::User(id) => SetRecipient::User(id),
            SetRecipient::Channel(id) => {
                let new_id = id.strip_prefix("@").unwrap_or(&id).to_string();
                SetRecipient::Channel(new_id)
            }
            SetRecipient::Clear => SetRecipient::Clear,
        }
    }

    pub async fn as_representation<C: Into<Recipient>>(
        &self,
        chat_id: C,
        bot: &Bot,
    ) -> Result<SetReceipientParamsRepresentation, teloxide::RequestError> {
        Ok(match self {
            SetRecipient::User(user_id) => SetReceipientParamsRepresentation::User(
                bot.get_chat_member(chat_id, UserId(user_id.clone()))
                    .await?
                    .user,
            ),
            SetRecipient::Channel(id) => SetReceipientParamsRepresentation::Channel(id.clone()),
            SetRecipient::Clear => {
                SetReceipientParamsRepresentation::Clear(bot.get_chat(chat_id).await?)
            }
        })
    }
}

impl Display for SetReceipientParamsRepresentation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetReceipientParamsRepresentation::User(user) => {
                write!(f, "{}", DisplayUser(Cow::Borrowed(user)))
            }
            SetReceipientParamsRepresentation::Channel(id) => write!(f, "@{id}"),
            SetReceipientParamsRepresentation::Clear(chat) => write!(
                f,
                "{}",
                chat.title()
                    .map(|t| t.to_string())
                    .or_else(|| {
                        let mut memebers = chat.mentioned_users();
                        if let Some(first) = memebers.next()
                            && let None = memebers.next()
                        {
                            Some(format!("{}", DisplayUser(Cow::Borrowed(first))))
                        } else {
                            None
                        }
                    })
                    .or_else(|| chat.first_name().map(|first_name| format!(
                        "{}{}",
                        first_name,
                        chat.last_name()
                            .map(|n| format!(" {n}"))
                            .unwrap_or_default()
                    )))
                    .unwrap_or_else(|| chat
                        .mentioned_users()
                        .map(|u| DisplayUser(Cow::Borrowed(u)).to_string())
                        .join(", "))
            ),
        }
    }
}

struct DisplayUser<'a>(Cow<'a, User>);

impl<'a> Display for DisplayUser<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .username
                .as_ref()
                .map(|name| format!("@{name}"))
                .unwrap_or_else(|| format!(
                    "{}{} ({})",
                    self.0.first_name,
                    self.0
                        .last_name
                        .as_ref()
                        .map(|name| format!(" {name}"))
                        .unwrap_or_default(),
                    self.0.preferably_tme_url()
                ))
        )
    }
}
