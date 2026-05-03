use std::time::Duration;

use lib_common::agent::optimize::gemma4::ScheduleParamterAccessor;
use lib_common::polling::trigger::ScheduleTrigger;
use sea_orm::{prelude::*, strum};

use crate::UserId;
use crate::db::{atom, rss};

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
    #[sea_orm(default_value = "0")]
    pub kind: Kind,
    #[sea_orm(belongs_to, from = "user_id", to = "id")]
    pub user: HasOne<user::Entity>,
    #[sea_orm(default_value = "i32::MAX")]
    pub buffer_size: i32,

    #[sea_orm(has_one)]
    pub rss: HasOne<rss::Entity>,
    #[sea_orm(has_one)]
    pub atom: HasOne<atom::Entity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, strum::Display)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Kind {
    #[strum(to_string = "RSS")]
    Rss = 0,
    #[strum(to_string = "Atom")]
    Atom = 1,
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

impl Model {
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

pub struct ModelParamterAccessor {
    inner: Model,
    db: DatabaseConnection,
}

impl ModelParamterAccessor {
    pub fn new(db: DatabaseConnection, inner: Model) -> Self {
        Self { inner, db }
    }
}

impl ScheduleParamterAccessor for ModelParamterAccessor {
    type Error = DbErr;

    async fn get_interval_mins(&self) -> u32 {
        self.inner.interval_mins.unwrap_or(60) as u32
    }

    async fn set_interval_mins(&mut self, new_value: u32) -> Result<(), Self::Error> {
        self.inner.interval_mins = Some(new_value as i32);
        Entity::update(ActiveModel {
            id: sea_orm::Unchanged(self.inner.id),
            interval_mins: sea_orm::Set(Some(new_value as i32)),
            ..Default::default()
        })
        .exec(&self.db)
        .await?;
        Ok(())
    }

    async fn get_buffer_size(&self) -> usize {
        self.inner.buffer_size as usize
    }

    async fn set_buffer_size(&mut self, new_value: usize) -> Result<(), Self::Error> {
        self.inner.buffer_size = new_value as i32;
        Entity::update(ActiveModel {
            id: sea_orm::Unchanged(self.inner.id),
            buffer_size: sea_orm::Set(new_value as i32),
            ..Default::default()
        })
        .exec(&self.db)
        .await?;
        Ok(())
    }
}
