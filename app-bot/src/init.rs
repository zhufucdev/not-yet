use std::{fmt::Display, sync::Arc};

use app_common::poller::AttachToPoller;
#[cfg(feature = "serve-rss")]
use app_common::rss;
use lib_common::polling::{KeyContract, Schedule, Scheduler};

pub trait InitResult: AttachToPoller<Self::ScheduleKey> {
    type ScheduleKey: KeyContract + Display;
    async fn main(&self, scheduler: Arc<Scheduler<Self::ScheduleKey>>) -> anyhow::Result<()>;
    async fn get_schedules(
        &self,
    ) -> anyhow::Result<impl IntoIterator<Item = Schedule<Self::ScheduleKey>>>;
    #[cfg(feature = "serve-rss")]
    async fn get_rss_broadcasts(&self) -> anyhow::Result<impl IntoIterator<Item = rss::Broadcast>>;
}
