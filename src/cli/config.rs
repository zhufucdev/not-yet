use serde::Deserialize;
use std::collections::HashMap;

use crate::polling::trigger::ScheduleTrigger;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub mode: RunMode,
    #[serde(rename = "subscription")]
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Deserialize)]
pub enum RunMode {
    #[serde(rename = "oneshot")]
    Oneshot,
    #[serde(rename = "daemon")]
    Daemon { schedules: Vec<Schedule> },
}

#[derive(Clone, Debug, Deserialize)]
pub struct Schedule {
    pub trigger: ScheduleTrigger,
    #[serde(rename = "for")]
    pub for_: usize,
}

#[derive(Debug, Deserialize)]
pub struct Subscription {
    pub feed: Feed,
    pub condition: String,
}

#[derive(Debug, Deserialize)]
pub enum Feed {
    #[serde(rename = "rss")]
    Rss {
        url: String,
        headers: Option<HashMap<String, String>>,
    },
}
