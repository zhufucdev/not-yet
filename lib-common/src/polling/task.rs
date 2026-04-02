use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{Level, event, warn_span};

use crate::polling::{KeyContract, Schedule};

#[derive(Debug, Clone)]
pub struct Task<K: KeyContract> {
    pub(super) schedule: Arc<Schedule<K>>,
    pub(super) state: Arc<RwLock<TaskState>>,
    due_time: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskState {
    #[default]
    Pending,
    Running,
    Finished,
}

impl<K: KeyContract> Task<K> {
    pub fn for_next_schedule_run(value: Arc<Schedule<K>>) -> Option<Self> {
        value.trace_span.clone().in_scope(|| {
            event!(Level::DEBUG, "schedule trigger = {:?}", value.trigger);
            let t = value.get_next_run_time().map(|due_time| {
                event!(Level::DEBUG, "next run time is {due_time:?}");
                Self {
                    schedule: value,
                    state: Arc::new(RwLock::new(TaskState::Pending)),
                    due_time,
                }
            });
            if t.is_none() {
                event!(Level::WARN, "no next run time for task");
            }
            t
        })
    }

    pub fn for_immediate_run(schedule: Arc<Schedule<K>>) -> Self {
        Self {
            schedule,
            state: Arc::new(RwLock::new(TaskState::Pending)),
            due_time: Utc::now(),
        }
    }

    pub fn due_time(&self) -> DateTime<Utc> {
        self.due_time
    }

    pub fn get_due_instant(&self) -> Instant {
        Instant::now()
            + (self.due_time - Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO)
    }

    pub fn schedule(&self) -> &Schedule<K> {
        &self.schedule
    }
}

impl<K: KeyContract> PartialEq for Task<K> {
    fn eq(&self, other: &Self) -> bool {
        self.schedule.id() == other.schedule.id() && self.due_time == other.due_time
    }
}

impl<K: KeyContract> Eq for Task<K> {}

impl<K: KeyContract> PartialOrd for Task<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.due_time.partial_cmp(&other.due_time)
    }
}

impl<K: KeyContract> Ord for Task<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.due_time.cmp(&other.due_time)
    }
}
