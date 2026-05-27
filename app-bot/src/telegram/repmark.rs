use teloxide::{
    Bot,
    dispatching::dialogue::GetChatId,
    payloads::EditMessageReplyMarkupSetters,
    prelude::Requester,
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardButtonKind,
        InlineKeyboardMarkup, Message, MessageId, ReplyMarkup,
    },
};
use tracing::{Level, event};

pub fn button_repmark<R, C, K, V>(rows: R) -> impl Into<ReplyMarkup>
where
    R: IntoIterator<Item = C>,
    C: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    InlineKeyboardMarkup::new(rows.into_iter().map(|cols| {
        cols.into_iter().map(|(k, v)| {
            InlineKeyboardButton::new(k, InlineKeyboardButtonKind::CallbackData(v.into()))
        })
    }))
}

pub async fn remove(from: &CallbackQuery, bot: &Bot) {
    if let Some(msg) = from.regular_message() {
        remove_from_msg(&msg, bot).await;
    }
}

pub async fn remove_from_msg(msg: &Message, bot: &Bot) {
    remove_from_msg_and_chat_id(msg.id, msg.chat_id().unwrap(), bot).await;
}

pub async fn remove_from_msg_and_chat_id(msg_id: MessageId, chat_id: ChatId, bot: &Bot) {
    _ = bot
        .edit_message_reply_markup(chat_id, msg_id)
        .reply_markup(InlineKeyboardMarkup::default())
        .await
        .inspect_err(|err| event!(Level::WARN, "error removing reply markup: {err}"));
}
