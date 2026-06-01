use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveIden)]
pub enum Notify {
    Table,
    Id,
    Kind,
    ChatId,
    ChannelId,
}

