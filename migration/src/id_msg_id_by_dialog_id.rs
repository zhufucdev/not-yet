use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveIden)]
pub enum MsgIdByDialogId {
    Table,
    DialogId,
    MsgId,
    ChatId,
    SubscriptionId,
}

