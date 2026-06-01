use sea_orm_migration::prelude::*;

/// Identifiers for the `subscription` table.
#[derive(DeriveIden, Clone)]
pub enum Subscription {
    Table,
    Id,
    Kind,
    Cron,
    IntervalMins,
    Condition,
    BufferSize,
    UserId,
    NotifyId,
}
