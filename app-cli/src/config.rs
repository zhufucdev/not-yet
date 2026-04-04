use lib_common::source::{RssFeed, atom::AtomFeed};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{collections::BTreeMap, fmt::Display, time::Duration};

use crate::polling::trigger::ScheduleTrigger;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub mode: RunMode,
    #[serde(rename = "subscription")]
    pub subscriptions: Vec<Subscription>,
    #[serde(default = "default_drop_model_timeout")]
    pub drop_model_in: Duration,
}

fn default_drop_model_timeout() -> Duration {
    Duration::from_mins(5)
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
    Rss(RssConfig),
    #[serde(rename = "atom")]
    Atom(AtomConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RssConfig {
    pub url: SmolStr,
    pub headers: Option<BTreeMap<SmolStr, SmolStr>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AtomConfig {
    pub url: SmolStr,
    pub headers: Option<BTreeMap<SmolStr, SmolStr>>,
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

pub trait ToFeed<F> {
    fn to_feed(&self) -> Result<F, anyhow::Error>;
}

impl ToFeed<RssFeed> for RssConfig {
    fn to_feed(&self) -> anyhow::Result<RssFeed> {
        Ok(RssFeed::new(
            self.url.clone(),
            self.headers
                .as_ref()
                .map(|map| -> anyhow::Result<_> {
                    Ok(HeaderMap::from_iter(
                        map.iter()
                            .map(|(k, v)| Ok((k.parse()?, v.parse()?)))
                            .collect::<Result<Vec<_>, anyhow::Error>>()?,
                    ))
                })
                .transpose()?
                .as_ref(),
        )?)
    }
}

impl ToFeed<AtomFeed> for AtomConfig {
    fn to_feed(&self) -> Result<AtomFeed, anyhow::Error> {
        Ok(AtomFeed::new(
            self.url.clone(),
            self.headers
                .as_ref()
                .map(|map| -> anyhow::Result<_> {
                    Ok(HeaderMap::from_iter(
                        map.iter()
                            .map(|(k, v)| Ok((k.parse()?, v.parse()?)))
                            .collect::<Result<Vec<_>, anyhow::Error>>()?,
                    ))
                })
                .transpose()?
                .as_ref(),
        )?)
    }
}
