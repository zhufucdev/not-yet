use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Atom::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Atom::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Atom::Url).string().not_null())
                    .col(
                        ColumnDef::new(Atom::BrowserUa)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Atom::Headers).string().null())
                    .col(
                        ColumnDef::new(Atom::SubscriptionId)
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_rss_subscription_id")
                            .from(Atom::Table, Atom::SubscriptionId)
                            .to(Subscription::Table, Subscription::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Atom::Table).to_owned())
            .await
    }
}
///
/// Identifiers for the `rss` table.
#[derive(DeriveIden, Clone)]
enum Atom {
    Table,
    Id,
    Url,
    BrowserUa,
    Headers,
    SubscriptionId,
}

/// Identifiers for the `subscription` table.
#[derive(DeriveIden, Clone)]
enum Subscription {
    Table,
    Id,
}
