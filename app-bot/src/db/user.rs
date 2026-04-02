use sea_orm::prelude::*;

use crate::UserId;

use super::subscription;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: UserId,
    pub access_level: AccessLevel,
    #[sea_orm(has_many)]
    pub subscriptions: HasMany<subscription::Entity>,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum AccessLevel {
    #[sea_orm(num_value = 0)]
    ConfiguredWhitelist,
    #[sea_orm(num_value = 1)]
    OnetimeToken,
}

impl ActiveModelBehavior for ActiveModel {}
