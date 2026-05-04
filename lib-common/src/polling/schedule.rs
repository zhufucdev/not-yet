use std::{
    collections::{BinaryHeap, HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use async_stream::stream;
use chrono::{DateTime, Utc};
use futures::Stream;
use tokio::sync::{
    RwLock,
    broadcast::{self, error::RecvError},
};
use tracing::{Instrument, Level, Span, debug_span, event, info_span};

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
    schedules: Arc<RwLock<Vec<Arc<Schedule<K>>>>>,
    task_queue: Arc<RwLock<HashMap<K, BinaryHeap<Task<K>>>>>,
    schedules_notify: (
        broadcast::Sender<(QueueType, Arc<Schedule<K>>)>,
        broadcast::Receiver<(QueueType, Arc<Schedule<K>>)>,
    ),
}

#[derive(Debug, Clone)]
pub enum QueueType {
    Exising,
    New,
}

impl<K: KeyContract> FromIterator<Schedule<K>> for Scheduler<K> {
    fn from_iter<T: IntoIterator<Item = Schedule<K>>>(iter: T) -> Self {
        let mut id_checker = HashSet::new();
        let schedules = iter.into_iter().map(Arc::new).collect::<Vec<_>>();
        for schedule in &schedules {
            if id_checker.contains(&schedule.id) {
                panic!("duplicate schedule id: {}", schedule.id);
            }
            id_checker.insert(schedule.id);
        }
        let mut task_queue = HashMap::<K, BinaryHeap<Task<K>>>::new();
        for schedule in schedules.iter() {
            if let Some(ref mut bt) = task_queue.get_mut(schedule.key()) {
                let task = Task::for_immediate_run(schedule.clone());
                bt.push(task);
            } else {
                let mut bt = BinaryHeap::new();
                let task = Task::for_immediate_run(schedule.clone());
                bt.push(task);
                task_queue.insert(schedule.key().clone(), bt);
            };
        }
        event!(Level::DEBUG, "loaded {} schedules", schedules.len());

        Self {
            task_queue: Arc::new(RwLock::new(task_queue)),
            schedules: Arc::new(RwLock::new(schedules)),
            schedules_notify: broadcast::channel(1),
        }
    }
}

impl<K: KeyContract> Scheduler<K> {
    pub fn new() -> Self {
        Self {
            task_queue: Arc::new(RwLock::new(HashMap::new())),
            schedules: Arc::new(RwLock::new(Vec::new())),
            schedules_notify: broadcast::channel(1),
        }
    }

    pub async fn add_schedule(
        &self,
        trigger: ScheduleTrigger,
        key: K,
    ) -> Result<Arc<Schedule<K>>, cron::error::Error> {
        let span = info_span!("scheduler.add_schedule");
        async {
            let id = self
                .schedules
                .read()
                .await
                .iter()
                .max_by_key(|s| s.id)
                .map(|s| s.id)
                .unwrap_or_default()
                + 1;
            let s = Arc::new(Schedule::new(id, key, trigger)?);
            self.schedules.write().await.push(s.clone());
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
                let queue_type = match tq.get_mut(schedule.key()) {
                    Some(bt) => {
                        bt.push(t);
                        event!(Level::DEBUG, "pushing task to queue");
                        QueueType::Exising
                    }
                    None => {
                        let mut bt = BinaryHeap::new();
                        bt.push(t);
                        tq.insert(schedule.key().clone(), bt);
                        event!(Level::DEBUG, "created new task queue");
                        QueueType::New
                    }
                };
                let send_result = self.schedules_notify.0.send((queue_type, schedule.clone()));
                if let Err(err) = send_result {
                    event!(
                        Level::WARN,
                        "failed to send reschedule notifiction to task queue: {}",
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
        key: Option<&K>,
    ) -> impl Stream<Item = Result<Task<K>, TaskCancellationError>> {
        stream! {
            loop {
                event!(Level::TRACE, "polling for next task");
                let mut reschedule_rx = self.schedules_notify.0.subscribe();
                let Some(expected_next) = (if let Some(key) = key.as_ref() {
                    let mut guard = self.task_queue.write().await;
                    guard.get_mut(key).map(|bt| bt.pop()).flatten()
                } else {
                    let key = {
                        let guard = self.task_queue.read().await;
                        let Some((k, _)) = guard.iter().min_by_key(|(_, bt)| bt.peek()) else {
                            return;
                        };
                        k.clone()
                    };
                    self.task_queue.write().await.get_mut(&key).unwrap().pop()
                }) else {
                    event!(Level::TRACE, "no future tasks expected, end polling now");
                    return;
                };
                let span = expected_next.schedule().trace_span.clone();
                let due_time = expected_next.get_due_instant();
                loop {
                    span.in_scope(|| event!(Level::DEBUG, "waiting for next run"));
                    tokio::select! {
                        schedule = reschedule_rx.recv() => {
                            if let Ok((_, schedule)) = schedule
                                && key.is_none_or(|k| schedule.key() == k)
                                && schedule.get_next_run_time().is_some_and(|t| t < expected_next.due_time())
                            {
                                span.in_scope(|| event!(Level::DEBUG, "signaled to cancel"));
                                yield Err(TaskCancellationError)
                            } else {
                                span.in_scope(|| event!(Level::DEBUG, "signaled to retry"));
                                continue
                            }
                        }
                        _ = tokio::time::sleep_until(due_time.into()) => {
                            span.in_scope(|| event!(Level::DEBUG, "signaled to run"));
                            *expected_next.state.write().await = TaskState::Running;
                            self.add_to_task_queue(expected_next.schedule.clone()).await;
                            let state = expected_next.state.clone();
                            yield Ok(expected_next);
                            *state.write().await = TaskState::Finished;
                            break;
                        }
                    };
                }
            }
        }
    }

    pub async fn schedules(&self) -> Vec<Arc<Schedule<K>>> {
        self.schedules.read().await.to_vec()
    }

    pub async fn until_next_reschedule(&self) -> Result<QueueType, RecvError> {
        let mut rx = self.schedules_notify.0.subscribe();

        while rx.try_recv().is_ok() {}
        Ok(rx.recv().await?.0)
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

    pub fn new(id: usize, key: K, trigger: ScheduleTrigger) -> Result<Self, cron::error::Error> {
        Ok(Self {
            id,
            last_run: None,
            trigger: trigger.try_into()?,
            key: key.clone(),
            trace_span: debug_span!("schedule", id = id, key = ?key),
        })
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
