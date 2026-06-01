use sea_orm_migration::{prelude::*, schema::*, sea_orm::Statement};

use crate::{id_notify::Notify, id_subscription::Subscription, id_user::User};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Notify::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Notify::Id)
                            .integer()
                            .primary_key()
                            .auto_increment(),
                    )
                    .col(ColumnDef::new(Notify::Kind).integer().not_null().default(0))
                    .col(ColumnDef::new(Notify::ChatId).integer())
                    .col(ColumnDef::new(Notify::ChannelId).string())
                    .to_owned(),
            )
            .await?;
        manager
            .rename_table(
                Table::rename()
                    .table(Subscription::Table, "subscription_")
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "PRAGMA foreign_keys = OFF;",
            ))
            .await?;
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
                    .col(
                        ColumnDef::new(Subscription::Kind)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Subscription::BufferSize)
                            .integer()
                            .default(i32::MAX),
                    )
                    .col(ColumnDef::new(Subscription::NotifyId).integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_subscription_user_id")
                            .from(Subscription::Table, Subscription::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_subscription_notify_id")
                            .from(Subscription::Table, Subscription::NotifyId)
                            .to(Notify::Table, Notify::Id)
                            .on_update(ForeignKeyAction::Cascade)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                r#"INSERT INTO `subscription` (id, cron, interval_mins, condition, user_id, kind, buffer_size) SELECT * FROM `subscription_`;"#,
            ))
            .await?;
        manager
            .drop_table(Table::drop().table("subscription_").to_owned())
            .await?;
        manager
            .get_connection()
            .execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "PRAGMA foreign_keys = ON;",
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Subscription::Table)
                    .drop_column(Subscription::NotifyId)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Notify::Table).to_owned())
            .await
    }
}
