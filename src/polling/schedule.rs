use std::{
    cell::RefCell,
    collections::{BinaryHeap, HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use async_stream::stream;
use chrono::{DateTime, Utc};
use futures::Stream;
use tokio::sync::{RwLock, broadcast};
use tracing::{Instrument, Level, Span, debug_span, event, info_span, warn_span};

use crate::polling::{
    KeyContract,
    error::TaskCancellationError,
    task::{Task, TaskState},
    trigger::{_ScheduleTrigger, ScheduleTrigger},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule<Key: KeyContract> {
    id: usize,
    last_run: Option<Instant>,
    pub(super) trigger: _ScheduleTrigger,
    key: Key,
    pub(super) trace_span: Span,
}

pub struct Scheduler<K: KeyContract> {
    schedules: Vec<Arc<Schedule<K>>>,
    task_queue: Arc<RwLock<HashMap<K, BinaryHeap<Task<K>>>>>,
    schedules_notify: (
        broadcast::Sender<Arc<Schedule<K>>>,
        broadcast::Receiver<Arc<Schedule<K>>>,
    ),
}

impl<K: KeyContract> FromIterator<Schedule<K>> for Scheduler<K> {
    fn from_iter<T: IntoIterator<Item = Schedule<K>>>(iter: T) -> Self {
        let mut id_checker = HashSet::new();
        let schedules = iter.into_iter().map(Arc::new).collect::<Vec<_>>();
        for schedule in &schedules {
            if id_checker.contains(&schedule.id) {
                panic!("Duplicate schedule id: {}", schedule.id);
            }
            id_checker.insert(schedule.id);
        }
        let mut task_queue = HashMap::<K, BinaryHeap<Task<K>>>::new();
        for schedule in schedules.iter() {
            if let Some(ref mut bt) = task_queue.get_mut(schedule.key()) {
                if let Some(task) = Task::for_next_schedule_run(schedule.clone()) {
                    bt.push(task);
                }
            } else {
                let mut bt = BinaryHeap::new();
                if let Some(task) = Task::for_next_schedule_run(schedule.clone()) {
                    bt.push(task);
                }
                task_queue.insert(schedule.key().clone(), bt);
            };
        }

        Self {
            task_queue: Arc::new(RwLock::new(task_queue)),
            schedules,
            schedules_notify: broadcast::channel(1),
        }
    }
}

impl<K: KeyContract> Scheduler<K> {
    pub fn new() -> Self {
        Self {
            task_queue: Arc::new(RwLock::new(HashMap::new())),
            schedules: Vec::new(),
            schedules_notify: broadcast::channel(1),
        }
    }

    pub async fn add_schedule(
        &mut self,
        trigger: ScheduleTrigger,
        data: K,
    ) -> Result<Arc<Schedule<K>>, cron::error::Error> {
        let span = info_span!("scheduler.add_schedule");
        async {
            let id = self
                .schedules
                .iter()
                .max_by_key(|s| s.id)
                .map(|s| s.id)
                .unwrap_or_default()
                + 1;
            let s = Arc::new(Schedule {
                id,
                last_run: None,
                trigger: trigger.try_into()?,
                key: data.clone(),
                trace_span: debug_span!("schedule", id = id, data = ?data),
            });
            self.schedules.push(s.clone());
            event!(
                Level::DEBUG,
                "added schedule id = {}, trigger = {:?}",
                s.id,
                s.trigger
            );
            self.add_to_task_queue(s.clone()).await;
            Ok(s)
        }
        .instrument(span)
        .await
    }

    async fn add_to_task_queue(&self, schedule: Arc<Schedule<K>>) {
        async {
            if let Some(t) = Task::for_next_schedule_run(schedule.clone()) {
                let mut tq = self.task_queue.write().await;
                if let Some(bt) = tq.get_mut(schedule.key()) {
                    bt.push(t);
                    event!(Level::DEBUG, "pushing task to queue");
                } else {
                    let mut bt = BinaryHeap::new();
                    bt.push(t);
                    tq.insert(schedule.key().clone(), bt);
                    event!(Level::DEBUG, "created new task queue");
                }
                let send_result = self.schedules_notify.0.send(schedule.clone());
                if let Err(err) = send_result {
                    event!(
                        Level::WARN,
                        "failed to send schedule to task queue: {}",
                        err.to_string()
                    );
                }
            }
        }
        .instrument(schedule.trace_span.clone())
        .await
    }

    /// Get the next schedule and spawn a corresponding task,
    /// yielding. If [Self::add_schedule] was called *AND* the new schedule
    /// fits in front, this polling is canceled, yield [TaskCancellationError].
    /// In this case, receiver can choose to keep polling.
    pub fn start_polling(
        &self,
        key: Option<K>,
    ) -> impl Stream<Item = Result<Task<K>, TaskCancellationError>> {
        stream! {
            loop {
                let mut rx = self.schedules_notify.0.subscribe();
                let expected_next = if let Some(key) = key.as_ref() {
                    let mut guard = self.task_queue.write().await;
                    let local_queue = guard.get_mut(key).unwrap();
                    local_queue.pop()
                } else {
                    let guard = self.task_queue.read().await;
                    let Some((k, _)) = guard.iter().min_by_key(|(_, bt)| bt.peek()) else {
                        return;
                    };
                    self.task_queue.write().await.get_mut(k).unwrap().pop()
                };
                let Some(expected_next) = expected_next else {
                    return;
                };
                let span = expected_next.schedule().trace_span.clone();
                span.in_scope(|| event!(Level::DEBUG, "waiting for next run"));
                tokio::select! {
                    schedule = rx.recv() => {
                        if let Ok(schedule) = schedule
                            && key.as_ref().is_none_or(|k| schedule.key() == k)
                            && schedule.get_next_run_time().is_some_and(|t| t < expected_next.due_time())
                        {
                            yield Err(TaskCancellationError)
                        } else {
                            continue
                        }
                    }
                    _ = tokio::time::sleep_until(expected_next.get_due_instant().into()) => {
                        span.in_scope(|| event!(Level::DEBUG, "signaled to run"));
                        *expected_next.state.write().await = TaskState::Running;
                        self.add_to_task_queue(expected_next.schedule.clone()).await;
                        let state = expected_next.state.clone();
                        yield Ok(expected_next);
                        *state.write().await = TaskState::Finished;
                    }
                };
            }
        }
    }
}

impl<D: KeyContract> Default for Scheduler<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: KeyContract> Schedule<K> {
    pub(super) fn get_next_run_time(&self) -> Option<DateTime<Utc>> {
        match &self.trigger {
            _ScheduleTrigger::Cron(schedule) => schedule.upcoming(Utc).next(),
            _ScheduleTrigger::Interval(duration) => Some(Utc::now() + *duration),
        }
    }

    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn last_run(&self) -> Option<&Instant> {
        self.last_run.as_ref()
    }
}
