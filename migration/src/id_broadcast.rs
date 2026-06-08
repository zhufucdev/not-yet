use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
pub enum Broadcast {
    Table,
    Id,
    SubscriptionId,
    Kind,
    RssKey,
}

#[derive(DeriveIden)]
pub enum BroadcastRss {
    Table,
    Key,
    Title,
    Description,
}
