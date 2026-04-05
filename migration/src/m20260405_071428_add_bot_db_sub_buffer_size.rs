use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Subscription::Table)
                    .add_column(
                        ColumnDef::new(Subscription::BufferSize)
                            .integer()
                            .default(i32::MAX),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Subscription::Table)
                    .drop_column(Subscription::Table)
                    .to_owned(),
            )
            .await
    }
}

/// Identifiers for the `subscription` table.
#[derive(DeriveIden, Clone)]
enum Subscription {
    Table,
    BufferSize,
}
