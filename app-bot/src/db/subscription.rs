use std::time::Duration;

use lib_common::polling::trigger::ScheduleTrigger;
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
    pub cron: Option<String>,
    #[sea_orm(default_value = "60")]
    pub interval_mins: Option<i32>,
    pub condition: String,
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

impl ModelEx {
    pub fn schedule_trigger(&self) -> ScheduleTrigger {
        if let Some(interval_mins) = self.interval_mins {
            ScheduleTrigger::Interval(Duration::from_mins(interval_mins as u64))
        } else if let Some(cron) = self.cron.as_ref() {
            ScheduleTrigger::Cron(cron.to_string())
        } else {
            ScheduleTrigger::Interval(Duration::from_mins(60))
        }
    }
}
