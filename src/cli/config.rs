use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{collections::BTreeMap, fmt::Display};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Subscription {
    pub feed: Feed,
    pub condition: SmolStr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Feed {
    #[serde(rename = "rss")]
    Rss {
        url: SmolStr,
        headers: Option<BTreeMap<SmolStr, SmolStr>>,
    },
}

impl AsRef<Subscription> for &Subscription {
    fn as_ref(&self) -> &Subscription {
        self
    }
}

impl Display for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
