use std::{
    collections::{BinaryHeap, HashSet},
    sync::Arc,
    time::Instant,
};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;
use tracing::{Level, event, warn_span};

use crate::polling::{
    DataContract, error::TaskCancellationError, task::Task, trigger::{_ScheduleTrigger, ScheduleTrigger},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule<D: DataContract> {
    pub(super) id: usize,
    pub(super) last_run: Option<Instant>,
    pub(super) trigger: _ScheduleTrigger,
    pub(super) data: D,
}

pub struct Scheduler<D: DataContract> {
    schedules: Vec<Arc<Schedule<D>>>,
    task_queue: BinaryHeap<Task<D>>,
    schedules_notify: (
        broadcast::Sender<Arc<Schedule<D>>>,
        broadcast::Receiver<Arc<Schedule<D>>>,
    ),
}

impl<D: DataContract> Scheduler<D> {
    pub fn new(schedules: impl IntoIterator<Item = Schedule<D>>) -> Self {
        let mut id_checker = HashSet::new();
        let schedules = schedules.into_iter().map(Arc::new).collect::<Vec<_>>();
        for schedule in &schedules {
            if id_checker.contains(&schedule.id) {
                panic!("Duplicate schedule id: {}", schedule.id);
            }
            id_checker.insert(schedule.id);
        }

        Self {
            task_queue: BinaryHeap::from_iter(
                schedules
                    .iter()
                    .filter_map(|s| Task::for_next_schedule_run(s.clone())),
            ),
            schedules,
            schedules_notify: broadcast::channel(1),
        }
    }

    pub fn add_schedule(
        &mut self,
        trigger: ScheduleTrigger,
        data: D,
    ) -> Result<Arc<Schedule<D>>, cron::error::Error> {
        let span = warn_span!("scheduler.add_schedule");
        span.in_scope(|| {
            let s = Arc::new(Schedule {
                id: self
                    .schedules
                    .iter()
                    .max_by_key(|s| s.id)
                    .map(|s| s.id)
                    .unwrap_or_default()
                    + 1,
                last_run: None,
                trigger: trigger.try_into()?,
                data,
            });
            self.schedules.push(s.clone());
            event!(
                Level::DEBUG,
                "Added schedule id = {}, trigger = {:?}",
                s.id,
                s.trigger
            );
            if let Some(t) = Task::for_next_schedule_run(s.clone()) {
                self.task_queue.push(t);
                let send_result = self.schedules_notify.0.send(s.clone());
                if let Err(err) = send_result {
                    event!(
                        Level::WARN,
                        "Failed to send schedule to task queue: {}",
                        err.to_string()
                    );
                }
            }
            Ok(s)
        })
    }

    /// Get the next schedule and spawn a corresponding task,
    /// returning. If [Self::add_schedule] was called *AND* the new schedule
    /// fits in front, this polling is canceled, returning [TaskCancellationError].
    /// In this case, caller is supposed to call [Self::poll] again.
    async fn poll(&mut self) -> Result<Option<Task<D>>, TaskCancellationError> {
        let mut rx = self.schedules_notify.0.subscribe();
        let Some(expected_next) = self.task_queue.pop() else {
            return Ok(None);
        };
        let mut result = None;
        while result.is_none() {
            result = tokio::select! {
                schedule = rx.recv() => {
                    if let Ok(schedule) = schedule
                        && schedule.get_next_run_time().is_some_and(|t| t < expected_next.get_due_time())
                    {
                        Some(Err(TaskCancellationError))
                    } else {
                        None
                    }
                }
                _ = tokio::time::sleep_until(expected_next.get_due_instant().unwrap().into()) => {
                    None
                }
            }
        }
        result.unwrap()
    }
}

impl<D: DataContract> Default for Scheduler<D> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<D: DataContract> Schedule<D> {
    pub(super) fn get_next_run_time(&self) -> Option<DateTime<Utc>> {
        match &self.trigger {
            _ScheduleTrigger::Cron(schedule) => schedule.upcoming(Utc).next(),
            _ScheduleTrigger::Interval(duration) => Some(Utc::now() + *duration),
        }
    }
}
