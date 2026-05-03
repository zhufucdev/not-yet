use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveIden)]
enum MsgIdByDialogId {
    Table,
    DialogId,
    MsgId,
    SubscriptionId,
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MsgIdByDialogId::Table)
                    .if_not_exists()
                    .col(string(MsgIdByDialogId::DialogId).primary_key())
                    .col(integer(MsgIdByDialogId::MsgId))
                    .col(integer(MsgIdByDialogId::SubscriptionId))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MsgIdByDialogId::Table).to_owned())
            .await
    }
}
