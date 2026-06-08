use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
pub enum Broadcast {
    Table,
    Id,
    SubscriptionId,
    Kind,
    RssKey,
    RssTitle,
    RssDescription,
}
