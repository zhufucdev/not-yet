use sea_orm::prelude::*;
use sea_orm::strum::Display;

use crate::UserId;
use crate::db::rss;

use super::user;

pub type SubscriptionId = i32;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "subscription")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: SubscriptionId,
    pub user_id: UserId,
    #[sea_orm(belongs_to, from = "user_id", to = "id")]
    pub user: HasOne<user::Entity>,
    #[sea_orm(has_one)]
    pub rss: HasOne<rss::Entity>,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, Display)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Kind {
    #[strum(to_string = "RSS")]
    Rss = 0,
}

impl ActiveModelBehavior for ActiveModel {}
