use sea_orm_migration::{prelude::*, schema::*};

use crate::{
    id_broadcast::{Broadcast, BroadcastRss},
    id_subscription::Subscription,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BroadcastRss::Table)
                    .col(string(BroadcastRss::Key).primary_key())
                    .col(string(BroadcastRss::Title))
                    .col(string(BroadcastRss::Description))
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .if_not_exists()
                    .table(Broadcast::Table)
                    .col(integer(Broadcast::Id).primary_key().auto_increment())
                    .col(integer(Broadcast::SubscriptionId).not_null())
                    .col(integer(Broadcast::Kind).not_null().default(0))
                    .col(string(Broadcast::RssKey))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_broadcast_sub_id")
                            .from(Broadcast::Table, Broadcast::SubscriptionId)
                            .to(Subscription::Table, Subscription::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_broadcast_rss_key")
                            .from(Broadcast::Table, Broadcast::RssKey)
                            .to(BroadcastRss::Table, BroadcastRss::Key)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Broadcast::Table).to_owned())
            .await
    }
}
