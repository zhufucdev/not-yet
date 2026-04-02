use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. Create `user` table
        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(User::Id)
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(User::AccessLevel).integer().not_null())
                    .to_owned(),
            )
            .await?;

        // 2. Create `subscription` table (depends on `user`)
        manager
            .create_table(
                Table::create()
                    .table(Subscription::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Subscription::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Subscription::Cron).string().null())
                    .col(
                        ColumnDef::new(Subscription::IntervalMins)
                            .integer()
                            .null()
                            .default(60),
                    )
                    .col(ColumnDef::new(Subscription::Condition).string().not_null())
                    .col(
                        ColumnDef::new(Subscription::UserId)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_subscription_user_id")
                            .from(Subscription::Table, Subscription::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // 3. Create `rss` table (depends on `subscription`)
        manager
            .create_table(
                Table::create()
                    .table(Rss::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Rss::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Rss::Url).string().not_null())
                    .col(
                        ColumnDef::new(Rss::BrowserUa)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Rss::Headers).string().null())
                    .col(
                        ColumnDef::new(Rss::SubscriptionId)
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_rss_subscription_id")
                            .from(Rss::Table, Rss::SubscriptionId)
                            .to(Subscription::Table, Subscription::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in reverse dependency order
        manager
            .drop_table(Table::drop().table(Rss::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Subscription::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(User::Table).to_owned())
            .await?;

        Ok(())
    }
}

/// Identifiers for the `user` table.
/// Note: SeaORM's `rename_all = "camelCase"` affects column naming at the ORM
/// layer; the underlying DB columns use the field names as-is unless explicitly
/// renamed. Column names here match what the derive macro emits for a camelCase
/// table — adjust if your DB driver applies the rename to column identifiers.
#[derive(DeriveIden, Clone)]
enum User {
    Table,
    Id,
    AccessLevel,
}

/// Identifiers for the `subscription` table.
#[derive(DeriveIden, Clone)]
enum Subscription {
    Table,
    Id,
    Cron,
    IntervalMins,
    Condition,
    UserId,
}

/// Identifiers for the `rss` table.
#[derive(DeriveIden, Clone)]
enum Rss {
    Table,
    Id,
    Url,
    BrowserUa,
    Headers,
    SubscriptionId,
}
