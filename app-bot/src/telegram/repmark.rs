use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup, ReplyMarkup,
};

pub fn button_repmark<R, C, K, V>(rows: R) -> impl Into<ReplyMarkup>
where
    R: IntoIterator<Item = C>,
    C: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    InlineKeyboardMarkup::new(rows.into_iter().map(|cols| {
        cols.into_iter()
            .map(|(k, v)| InlineKeyboardButton::new(k, InlineKeyboardButtonKind::CallbackData(v.into())))
    }))
}
