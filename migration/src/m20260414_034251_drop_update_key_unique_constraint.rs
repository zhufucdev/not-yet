use sea_orm_migration::{
    prelude::{extension::postgres::Type, *},
    schema::*,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .rename_table(
                TableRenameStatement::new()
                    .table(Update::Table, "_update")
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Update::Table)
                    .if_not_exists()
                    .col(pk_auto(Update::Id))
                    .col(string(Update::Key).not_null())
                    .col(big_unsigned(Update::Hash).not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("INSERT INTO `update` SELECT * FROM `_update`")
            .await?;
        manager
            .drop_table(TableDropStatement::new().table("_update").to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .rename_table(
                TableRenameStatement::new()
                    .table(Update::Table, "_update")
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Update::Table)
                    .if_not_exists()
                    .col(pk_auto(Update::Id))
                    .col(string(Update::Key).not_null().unique_key())
                    .col(big_unsigned(Update::Hash).not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("INSERT INTO `update` SELECT * FROM `_update`")
            .await?;
        manager
            .drop_table(TableDropStatement::new().table("_update").to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Update {
    Table,
    Id,
    Key,
    Hash,
}
