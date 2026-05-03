use std::sync::Arc;

use lib_common::agent::optimize::gemma4::ClarificationReqHandler;
use teloxide::{Bot, payloads::SendMessageSetters, prelude::Requester, types::ChatId};
use tokio::sync::{RwLock, mpsc};
use tracing::{Level, event};

use crate::telegram::repmark;

#[derive(Clone)]
pub struct TgClarReqHandler {
    bot: Bot,
    chat_id: ChatId,
    tx: mpsc::Sender<Option<String>>,
    rx: Arc<RwLock<mpsc::Receiver<Option<String>>>>,
    pending: Arc<RwLock<usize>>,
}

impl TgClarReqHandler {
    pub fn new(bot: Bot, chat_id: ChatId) -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self {
            bot,
            chat_id,
            tx,
            rx: Arc::new(RwLock::new(rx)),
            pending: Arc::new(RwLock::new(0)),
        }
    }

    pub async fn send(&self, ans: impl ToString) {
        self.tx
            .send(Some(ans.to_string()))
            .await
            .expect(CLOSED_CHANNEL_MSG);
    }

    pub async fn reject(&self) {
        self.tx.send(None).await.expect(CLOSED_CHANNEL_MSG)
    }

    pub async fn empty(&self) -> bool {
        Arc::strong_count(&self.rx) <= 0
    }
}

impl ClarificationReqHandler for TgClarReqHandler {
    async fn on_request(&self, prompt: &str) -> Option<String> {
        _ = self
            .bot
            .send_message(self.chat_id, prompt)
            .reply_markup(repmark::button_repmark([[("Figure it out yourself", "n")]]))
            .await
            .inspect_err(|err| event!(Level::ERROR, "error sending clarification prompt: {err}"));
        self.rx
            .write()
            .await
            .recv()
            .await
            .expect(CLOSED_CHANNEL_MSG)
    }
}

const CLOSED_CHANNEL_MSG: &str = "clarification request handler has a closed channel";
