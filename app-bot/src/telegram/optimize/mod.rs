use crate::telegram::optimize::renotify::SetRecipient;

pub mod renotify;

#[derive(Debug, Clone)]
pub enum TgOptimizerAction {
    SetReceipient(SetRecipient),
}
