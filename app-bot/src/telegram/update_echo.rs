use std::sync::Arc;

use sea_orm::DatabaseConnection;
use teloxide::{
    prelude::*,
    types::{ChatKind, ChatPublic, PublicChatKind, Recipient},
};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::db::{self, subscription::SubscriptionId};

#[derive(Debug, Clone)]
pub struct UpdateEcho {
    pub msg: String,
    pub dialog_id: String,
    pub sub_id: SubscriptionId,
    pub recipient: Recipient,
}

#[derive(Default, Clone)]
pub struct UpdateEchoHistory(Arc<RwLock<Vec<UpdateEcho>>>);

impl UpdateEchoHistory {
    pub async fn read<'s>(&'s self) -> RwLockReadGuard<'s, Vec<UpdateEcho>> {
        self.0.read().await
    }

    pub async fn write<'s>(&'s self) -> RwLockWriteGuard<'s, Vec<UpdateEcho>> {
        self.0.write().await
    }

    pub async fn pop_similar(&self, reference: &Message) -> Option<UpdateEcho> {
        let Some(ref_text) = reference.text() else {
            return None;
        };
        if let Some(sender_chat) = &reference.sender_chat
            && let ChatKind::Public(ChatPublic {
                kind: PublicChatKind::Channel(channel),
                ..
            }) = &sender_chat.kind
        {
            let Some(channel_name) = channel.username.as_ref() else {
                return None;
            };
            let channel_id = format!("@{}", channel_name);
            let mut echos = self.write().await;
            let mut echo_filter =
                echos
                    .iter()
                    .enumerate()
                    .filter(|(_, UpdateEcho { recipient, msg, .. })| {
                        msg == ref_text
                            && match recipient {
                                Recipient::Id(_) => false,
                                Recipient::ChannelUsername(id) => &channel_id == id,
                            }
                    });
            let Some((idx, echo)) = echo_filter.next().map(|(idx, echo)| (idx, echo.clone()))
            else {
                return None;
            };
            echos.remove(idx);
            Some(echo)
        } else {
            // TODO: support other chat types
            None
        }
    }
}

impl UpdateEcho {
    pub fn as_active_model(&self) -> db::dialog::ActiveModelEx {
        let mut r = db::dialog::ActiveModel::builder()
            .set_dialog_id(&self.dialog_id)
            .set_subscription_id(self.sub_id);
        if let Recipient::Id(id) = &self.recipient {
            r = r.set_chat_id(id.0);
        }
        r
    }
}
