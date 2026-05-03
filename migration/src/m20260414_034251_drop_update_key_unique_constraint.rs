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
            .alter_table(
                Table::alter()
                    .table("update")
                    .rename_column("key", "key_")
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("update")
                    .add_column(ColumnDef::new("key").string().not_null().default(""))
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE update SET key = key_")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("update")
                    .drop_column("_key")
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table("update")
                    .rename_column("key", "key_")
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("update")
                    .add_column(
                        ColumnDef::new("key")
                            .string()
                            .not_null()
                            .default("")
                            .unique_key(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE update SET key = key_")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("update")
                    .drop_column("_key")
                    .to_owned(),
            )
            .await
    }
}
