use std::{
    sync::{Arc, RwLock},
    time::Instant,
};

use chrono::{DateTime, Utc};
use tracing::{Level, event, warn_span};

use crate::polling::{DataContract, Schedule};

#[derive(Debug, Clone)]
pub struct Task<D: DataContract> {
    schedule: Arc<Schedule<D>>,
    state: Arc<RwLock<TaskState>>,
    due_time: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskState {
    #[default]
    Pending,
    Running,
}

impl<D: DataContract> Task<D> {
    pub fn for_next_schedule_run(value: Arc<Schedule<D>>) -> Option<Self> {
        let span = warn_span!("task.from_next_schedule_run", schedule_id = value.id);
        span.in_scope(|| {
            event!(Level::DEBUG, "Schedule trigger = {:?}", value.trigger);
            let t = value.get_next_run_time().map(|due_time| Self {
                schedule: value,
                state: Arc::new(RwLock::new(TaskState::Pending)),
                due_time,
            });
            if t.is_none() {
                event!(Level::WARN, "No next run time for task");
            }
            t
        })
    }

    pub fn get_due_time(&self) -> DateTime<Utc> {
        self.due_time
    }

    pub fn get_due_instant(&self) -> Result<Instant, chrono::OutOfRangeError> {
        Ok(Instant::now() + (self.due_time - Utc::now()).to_std()?)
    }
}

impl<D: DataContract> PartialEq for Task<D> {
    fn eq(&self, other: &Self) -> bool {
        self.schedule.id == other.schedule.id
            && *self.state.read().unwrap() == *other.state.read().unwrap()
            && self.due_time == other.due_time
    }
}

impl<D: DataContract> Eq for Task<D> {}

impl<D: DataContract> PartialOrd for Task<D> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.due_time.partial_cmp(&other.due_time)
    }
}

impl<D: DataContract> Ord for Task<D> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.due_time.cmp(&other.due_time)
    }
}
