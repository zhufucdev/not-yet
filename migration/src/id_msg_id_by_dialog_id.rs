use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveIden)]
pub enum MsgIdByDialogId {
    Table,
    Id,
    DialogId,
    MsgId,
    ChatId,
    SubscriptionId,
}

