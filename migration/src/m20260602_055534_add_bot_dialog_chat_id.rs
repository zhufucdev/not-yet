use sea_orm_migration::{prelude::*, schema::*};

use crate::id_msg_id_by_dialog_id::MsgIdByDialogId;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MsgIdByDialogId::Table)
                    .add_column(ColumnDef::new(MsgIdByDialogId::ChatId).integer())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MsgIdByDialogId::Table)
                    .drop_column(MsgIdByDialogId::ChatId)
                    .to_owned(),
            )
            .await
    }
}
