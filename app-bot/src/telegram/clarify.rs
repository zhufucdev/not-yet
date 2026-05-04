use lib_common::agent::optimize::gemma4::ClarificationReqHandler;
use teloxide::{
    Bot, dispatching::dialogue::InMemStorageError, payloads::SendMessageSetters,
    prelude::Requester, types::ChatId,
};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{Level, event};

use crate::telegram::{
    MasterDialog, repmark,
    state::{LlmAssignment, OptimizationTask, StateFeedback},
};

#[derive(Clone)]
pub struct TgClarReqHandler {
    bot: Bot,
    chat_id: ChatId,
    dialog: MasterDialog,
}

impl TgClarReqHandler {
    pub fn new(bot: Bot, chat_id: ChatId, dialog: MasterDialog) -> Self {
        Self {
            bot,
            chat_id,
            dialog,
        }
    }
}

impl ClarificationReqHandler for TgClarReqHandler {
    type Error = Error;

    async fn on_request(&self, prompt: &str) -> Result<Option<String>, Self::Error> {
        let prompt_for_user = self
            .bot
            .send_message(self.chat_id, prompt)
            .reply_markup(repmark::button_repmark([[("Figure it out yourself", "n")]]))
            .await
            .inspect_err(|err| event!(Level::ERROR, "error sending clarification prompt: {err}"))?;
        let (tx, mut rx) = mpsc::channel(1);
        self.dialog
            .update(
                self.dialog
                    .get_or_default()
                    .await?
                    .with_task_queued([OptimizationTask {
                        prompt: prompt_for_user.id,
                        assignment: LlmAssignment::Clarify { send: tx },
                    }])
                    .await,
            )
            .await?;
        Ok(rx.recv().await.expect(CLOSED_CHANNEL_MSG))
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("bot: {0}")]
    Bot(#[from] teloxide::RequestError),
    #[error("dialog state: {0}")]
    Dialog(#[from] InMemStorageError),
}

const CLOSED_CHANNEL_MSG: &str = "clarification request handler has a closed channel";
