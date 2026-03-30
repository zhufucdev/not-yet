use std::{str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleTrigger {
    #[serde(rename = "cron")]
    Cron(String),
    #[serde(rename = "interval", with = "humantime_serde")]
    Interval(Duration),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum _ScheduleTrigger {
    Cron(cron::Schedule),
    Interval(Duration),
}

impl TryFrom<ScheduleTrigger> for _ScheduleTrigger {
    type Error = cron::error::Error;

    fn try_from(value: ScheduleTrigger) -> Result<Self, Self::Error> {
        Ok(match value {
            ScheduleTrigger::Cron(expr) => _ScheduleTrigger::Cron(cron::Schedule::from_str(&expr)?),
            ScheduleTrigger::Interval(duration) => _ScheduleTrigger::Interval(duration),
        })
    }
}
