use std::{
    collections::{BinaryHeap, HashMap, HashSet},
    fmt::Debug,
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
    schedule,
    task::{Task, TaskState},
    trigger::{_ScheduleTrigger, ScheduleTrigger},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule<Key> {
    id: usize,
    last_run: Option<Instant>,
    pub(super) trigger: _ScheduleTrigger,
    key: Key,
    pub(super) trace_span: Span,
}

#[derive(Clone)]
pub struct Scheduler<K: KeyContract> {
    schedules: Arc<RwLock<Vec<Arc<Schedule<K>>>>>,
    task_queue: Arc<RwLock<HashMap<K, BinaryHeap<Task<K>>>>>,
    schedules_notify: (
        broadcast::Sender<(QueueType, Arc<Schedule<K>>)>,
        Arc<broadcast::Receiver<(QueueType, Arc<Schedule<K>>)>>,
    ),
}

#[cfg(all(feature = "daemon", target_os = "linux"))]
const DEFAULT_LOCKFILE_PATH: &str = "/tmp/not-yet.lock";

#[cfg(all(feature = "daemon", target_os = "macos"))]
const DEFAULT_LOCKFILE_PATH: &str = "/tmp/not-yet.pid";

#[derive(Debug, Clone, Eq, PartialEq)]
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

        let (tx, rx) = broadcast::channel(16);
        Self {
            task_queue: Arc::new(RwLock::new(task_queue)),
            schedules: Arc::new(RwLock::new(schedules)),
            schedules_notify: (tx, Arc::new(rx)),
        }
    }
}

impl<K: KeyContract> Scheduler<K> {
    pub fn new() -> Self {
        let (tx, rx) = broadcast::channel(16);
        Self {
            task_queue: Arc::new(RwLock::new(HashMap::new())),
            schedules: Arc::new(RwLock::new(Vec::new())),
            schedules_notify: (tx, Arc::new(rx)),
        }
    }

    pub async fn add_schedule(
        &self,
        trigger: ScheduleTrigger,
        key: K,
    ) -> Result<Schedule<K>, cron::error::Error> {
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
            if let Some(t) = Task::for_next_schedule_run(s.clone()) {
                self.add_to_task_queue(t).await;
            }
            Ok(s.as_ref().clone())
        }
        .instrument(span)
        .await
    }

    async fn add_to_task_queue(&self, task: Task<K>) {
        let schedule = task.schedule.clone();
        async {
            let mut tq = self.task_queue.write().await;
            let queue_type = match tq.get_mut(schedule.key()) {
                Some(bt) => {
                    event!(Level::DEBUG, "pushing task {task:?} to queue");
                    bt.push(task);
                    QueueType::Exising
                }
                None => {
                    event!(Level::DEBUG, "created new task queue with task {task:?}");
                    let mut bt = BinaryHeap::new();
                    bt.push(task);
                    tq.insert(schedule.key().clone(), bt);
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
                let Some(current_task) = (if let Some(key) = key.as_ref() {
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
                let span = current_task.schedule().trace_span.clone();
                let due_time = current_task.get_due_instant();
                loop {
                    span.in_scope(|| event!(Level::DEBUG, "waiting for next run in {}s", (due_time - Instant::now()).as_secs()));
                    #[cfg(feature = "daemon")]
                    match Self::lock().await {
                        Ok(_) => {}
                        Err(err) => {
                            event!(Level::ERROR, "failed to lock daemon: {}", err);
                            yield Err(TaskCancellationError);
                        }
                    }
                    tokio::select! {
                        Some(_) =
                                async {
                                    reschedule_rx.recv().await.ok().and_then(|(_, schedule)| {
                                        if key.is_none_or(|k| schedule.key() == k)
                                            && schedule
                                                .get_next_run_time()
                                                .is_some_and(|t| t < current_task.due_time())
                                        {
                                            Some(schedule)
                                        } else {
                                            None
                                        }
                                    })
                                }
                            => {
                            span.in_scope(|| event!(Level::DEBUG, "signaled to cancel in favor of a run time ahead"));
                            yield Err(TaskCancellationError)
                        }
                        _ = tokio::time::sleep_until(due_time.into()) => {
                            span.in_scope(|| event!(Level::DEBUG, "signaled to run"));
                            *current_task.state.write().await = TaskState::Running;
                            if let Some(next) = Task::for_next_schedule_run(current_task.schedule.clone()) {
                                self.add_to_task_queue(next).await;
                            } else {
                                span.in_scope(|| event!(Level::DEBUG, "ran out of tasks to run"))
                            }
                            let state = current_task.state.clone();
                            yield Ok(current_task);
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

        async {
            loop {
                match rx.recv().await {
                    Ok((t, s)) => {
                        event!(Level::TRACE, "received {t:?} from {s:?}");
                        return Ok(t);
                    }
                    Err(RecvError::Lagged(n)) => {
                        event!(Level::TRACE, "lagged {n}, ignorning");
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        .instrument(debug_span!("until_next_reschedule"))
        .await
    }

    #[cfg(feature = "daemon")]
    pub async fn lock() -> Result<(), std::io::Error> {
        use std::{io::ErrorKind, path::PathBuf, process};

        async fn handle_io_result<R, Fut>(
            path: &str,
            f: impl Fn() -> Fut,
        ) -> Result<R, std::io::Error>
        where
            Fut: Future<Output = Result<R, std::io::Error>>,
        {
            loop {
                match f().await {
                    Ok(r) => return Ok(r),
                    Err(err) => match err.kind() {
                        ErrorKind::Interrupted
                        | ErrorKind::AlreadyExists
                        | ErrorKind::IsADirectory
                        | ErrorKind::ResourceBusy
                        | ErrorKind::PermissionDenied => {
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

                            event!(Level::DEBUG, "unable to lock {path}: {err}");
                        }
                        _ => return Err(err),
                    },
                }
            }
        }

        let path = std::env::var_os("NOT_YET_LOCK_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_LOCKFILE_PATH));
        let path_display = path.display().to_string();
        event!(Level::DEBUG, "locking {path_display}");
        let pid = process::id();
        let pid_str = pid.to_string();
        let pid_buf = pid_str.as_bytes();
        while path.exists() {
            use tokio::io::AsyncReadExt;

            if handle_io_result(&path_display, async || {
                use sysinfo::{Pid, System};
                use tokio::io::AsyncSeekExt;

                let mut lockfile =
                    handle_io_result(&path_display, async || tokio::fs::File::open(&path).await)
                        .await?;
                let told_len = lockfile.seek(std::io::SeekFrom::End(0)).await?;
                if told_len > 10 {
                    return Ok(true);
                }
                // look at the pid file, if it's not alive then ignore it
                let mut buf = String::new();
                lockfile.seek(std::io::SeekFrom::Start(0)).await?;
                lockfile.read_to_string(&mut buf).await?;
                let Ok(told_pid) = buf.parse::<u32>() else {
                    return Ok(true);
                };
                if told_pid == pid {
                    return Ok(true);
                }
                let mut sys = System::new();
                let told_pid = Pid::from_u32(told_pid);
                sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[told_pid]), true);
                let Some(_) = sys.process(told_pid) else {
                    return Ok(true);
                };

                Ok(false)
            })
            .await?
            {
                return Ok(());
            }

            // spin lock
            event!(Level::DEBUG, "unable to lock {}...", path.display());
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        handle_io_result(&path_display, async || {
            use std::{fs::Permissions, os::unix::fs::PermissionsExt};

            tokio::fs::write(&path, pid_buf).await?;
            tokio::fs::set_permissions(&path, Permissions::from_mode(0o777)).await?;
            Ok(())
        })
        .await
    }

    pub async fn run_now(&self, schedule: &Schedule<K>) -> Option<Task<K>> {
        if let Some(schedule) = self
            .schedules()
            .await
            .iter()
            .find(|s| s.id() == schedule.id())
        {
            let task = Task::for_immediate_run(schedule.clone());
            self.add_to_task_queue(task.clone()).await;
            Some(task)
        } else {
            None
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
