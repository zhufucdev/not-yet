use sea_orm_migration::{
    prelude::*,
    schema::*,
    sea_orm::{DbBackend, Statement},
};

use crate::id_msg_id_by_dialog_id::MsgIdByDialogId;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_keys = OFF;",
            ))
            .await?;
        manager
            .create_table(
                Table::create()
                    .table("msg_id_by_dialog_id_")
                    .if_not_exists()
                    .col(string(MsgIdByDialogId::DialogId).not_null())
                    .col(integer(MsgIdByDialogId::MsgId).not_null())
                    .col(integer(MsgIdByDialogId::SubscriptionId).not_null())
                    .col(integer(MsgIdByDialogId::ChatId))
                    .primary_key(
                        Index::create()
                            .col(MsgIdByDialogId::DialogId)
                            .col(MsgIdByDialogId::MsgId)
                            .col(MsgIdByDialogId::ChatId),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                manager.get_database_backend(),
                "INSERT INTO `msg_id_by_dialog_id_` SELECT * FROM `msg_id_by_dialog_id`;",
            ))
            .await?;
        manager
            .drop_table(Table::drop().table(MsgIdByDialogId::Table).to_owned())
            .await?;
        manager
            .rename_table(
                Table::rename()
                    .table("msg_id_by_dialog_id_", MsgIdByDialogId::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_keys = ON;",
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_keys = OFF;",
            ))
            .await?;
        manager
            .create_table(
                Table::create()
                    .table("msg_id_by_dialog_id_")
                    .if_not_exists()
                    .col(string(MsgIdByDialogId::DialogId).primary_key())
                    .col(integer(MsgIdByDialogId::MsgId))
                    .col(integer(MsgIdByDialogId::SubscriptionId))
                    .col(integer(MsgIdByDialogId::ChatId))
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                manager.get_database_backend(),
                "INSERT INTO `msg_id_by_dialog_id_` SELECT * FROM `msg_id_by_dialog_id`;",
            ))
            .await?;
        manager
            .drop_table(Table::drop().table(MsgIdByDialogId::Table).to_owned())
            .await?;
        manager
            .rename_table(
                Table::rename()
                    .table("msg_id_by_dialog_id_", MsgIdByDialogId::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                DbBackend::Sqlite,
                "PRAGMA foreign_keys = ON;",
            ))
            .await?;
        Ok(())
    }
}
