use std::{any::Any, collections::HashMap, fmt::Display, pin::Pin, sync::Arc, time::Duration};

use futures::{FutureExt, StreamExt, TryStreamExt, future};
use lib_common::{
    polling::{KeyContract, Scheduler, schedule::QueueType},
    update::{AnyUpdatable, AnyUpdatePersistence, Source, Updatable, UpdatePersistence},
};
use tokio::{
    sync::{RwLock, RwLockWriteGuard},
    task::JoinHandle,
};
use tracing::{Instrument, Level, event, info_span};

use crate::feed;

pub struct Poller<K> {
    items: RwLock<HashMap<K, PollerItem>>,
    tasks: RwLock<HashMap<K, JoinHandle<()>>>,
    ctx: UpdateContext,
}

pub struct PollerTransaction<'a, K> {
    items: RwLockWriteGuard<'a, HashMap<K, PollerItem>>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateContext {
    pub rss_server: Arc<crate::rss::ServerState>,
}

pub trait Updater: Send {
    type Key: KeyContract;
    type Source: Source;

    #[allow(async_fn_in_trait)]
    async fn on_update(
        &self,
        material: Option<Box<<Self::Source as Source>::Item>>,
        source: &Self::Source,
        ctx: UpdateContext,
    ) -> Result<bool, anyhow::Error>;
}

impl<K: KeyContract> Poller<K> {
    pub fn new(ctx: UpdateContext) -> Self {
        Self {
            items: RwLock::new(HashMap::new()),
            tasks: RwLock::new(HashMap::new()),
            ctx,
        }
    }

    pub async fn transaction<'s>(&'s self) -> PollerTransaction<'s, K> {
        PollerTransaction {
            items: self.items.write().await,
        }
    }

    pub fn context(&self) -> &UpdateContext {
        &self.ctx
    }
}

impl<K: KeyContract> PollerTransaction<'_, K> {
    pub fn add_updater<U, S, P>(
        &mut self,
        key: K,
        updater: U,
        source: S,
        persistence: P,
        buffer_size: usize,
    ) where
        U: Updater<Key = K, Source = S> + Send + Sync + 'static,
        S: Updatable + Sync + 'static,
        S::Error: Into<anyhow::Error>,
        P: UpdatePersistence<Item = S::Item>,
        P::Error: Into<anyhow::Error>,
    {
        self.items.insert(
            key,
            PollerItem {
                updater: Box::new(updater),
                source: AnyUpdatable::from(source),
                persistence: AnyUpdatePersistence::from(persistence),
                buffer_size,
            },
        );
    }
}

impl<K> Poller<K>
where
    K: KeyContract + Display + 'static,
{
    pub async fn poll_all(&self, scheduler: Arc<Scheduler<K>>) -> Result<(), anyhow::Error> {
        event!(Level::TRACE, "poll_all");
        /// This is safe because the outside does not know about any internal state of `poll_one`
        struct UnsafeCell<T>(T);
        unsafe impl<T> Send for UnsafeCell<T> {}
        unsafe impl<T> Sync for UnsafeCell<T> {}
        impl<T: Future> Future for UnsafeCell<T> {
            type Output = T::Output;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let inner: Pin<&mut T> = unsafe { self.map_unchecked_mut(|s| &mut s.0) };
                inner.poll(cx)
            }
        }

        let tasks = {
            let mut updaters_guard = self.items.write().await;
            scheduler
                .schedules()
                .await
                .into_iter()
                .filter_map(|schedule| {
                    updaters_guard
                        .remove(schedule.key())
                        .map(|u| (u, schedule.key().clone()))
                })
                .map(|(updater, key)| {
                    let scheduler = Arc::clone(&scheduler);
                    let ctx = self.ctx.clone();
                    (key.clone(), async move {
                        poll_one_guarded(updater, key.clone(), scheduler, ctx)
                            .instrument(info_span!("polling_task", key = ?key))
                            .await
                    })
                })
                .map(|(k, f)| (k, tokio::spawn(UnsafeCell(f))))
                .collect::<HashMap<_, _>>()
        };
        event!(Level::DEBUG, "created {} tasks", tasks.len());
        *self.tasks.write().await = tasks;
        loop {
            event!(Level::TRACE, "waiting for next reschedule");
            let Ok((queue_type, key)) = scheduler.until_next_reschedule().await else {
                event!(Level::WARN, "reschedule received an error, ignoring");
                continue;
            };
            match queue_type {
                QueueType::Exising => {
                    // This case it is handled within `Update`
                    event!(Level::TRACE, "reschedule triggered for key {key:?}");
                }
                QueueType::New => {
                    event!(Level::TRACE, "new schedule triggered for key {key:?}");
                    let Some(updater) = self.items.write().await.remove(&key) else {
                        event!(Level::WARN, "missing updater for key {key:?}");
                        continue;
                    };
                    let scheduler = Arc::clone(&scheduler);
                    let ctx = self.ctx.clone();
                    self.tasks.write().await.insert(
                        key.clone(),
                        tokio::spawn(UnsafeCell(async move {
                            poll_one_guarded(updater, key.clone(), scheduler, ctx)
                                .instrument(info_span!("polling_task", key = ?key))
                                .await
                        })),
                    );
                }
            }
        }
    }
}

trait UpdaterHolder: Send {
    fn on_update<'s>(
        &'s self,
        material: Option<Box<dyn Any + Send + Sync>>,
        source: &'s AnyUpdatable,
        ctx: UpdateContext,
    ) -> Pin<Box<dyn Future<Output = Result<bool, anyhow::Error>> + 's>>;
}

impl<T> UpdaterHolder for T
where
    T: Updater + Sync + 'static,
{
    fn on_update<'s>(
        &'s self,
        material: Option<Box<dyn Any + Send + Sync>>,
        source: &'s AnyUpdatable,
        ctx: UpdateContext,
    ) -> Pin<Box<dyn Future<Output = Result<bool, anyhow::Error>> + 's>> {
        Box::pin(async move {
            let material = material.map(|m| m.downcast::<<T::Source as Source>::Item>().unwrap());
            let source = source.inner().downcast_ref::<T::Source>().unwrap();
            T::on_update(&self, material, source, ctx).await
        })
    }
}

struct PollerItem {
    updater: Box<dyn UpdaterHolder>,
    source: AnyUpdatable,
    persistence: AnyUpdatePersistence,
    buffer_size: usize,
}

async fn poll_one_guarded<K>(
    mut updater: PollerItem,
    key: K,
    scheduler: Arc<Scheduler<K>>,
    ctx: UpdateContext,
) where
    K: KeyContract + Display,
{
    let mut error_count = 0;
    while error_count < 10 {
        // This is safe because outside has not access to the internal mutables, making
        // observations of invariants impossible
        match std::panic::AssertUnwindSafe(poll_one(&mut updater, key.clone(), &scheduler, &ctx))
            .catch_unwind()
            .await
        {
            Ok(Err(err)) => {
                event!(Level::ERROR, "{err}");
                error_count += 1;
            }
            Ok(Ok(())) => {
                event!(Level::WARN, "polling task ended prematurely");
            }
            Err(info) => {
                let msg: &str = if let Some(msg) = info.downcast_ref::<String>() {
                    msg
                } else if let Some(msg) = info.downcast_ref::<&'static str>() {
                    msg
                } else {
                    "unknown"
                };
                event!(Level::ERROR, "polling task panicked: {msg}");
                error_count += 1;
            }
        }
        event!(Level::INFO, "sleep for 10s before retrying");
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    event!(Level::WARN, "too many errors, ending this task");
}

async fn poll_one<K>(
    data: &mut PollerItem,
    key: K,
    scheduler: &Scheduler<K>,
    ctx: &UpdateContext,
) -> Result<(), anyhow::Error>
where
    K: KeyContract + Display,
{
    let updater = &data.updater;
    feed::check(
        &key,
        &data.source,
        scheduler,
        &data.persistence,
        data.buffer_size,
    )
    .inspect_err(|e| event!(Level::WARN, "check feed error: {e}"))
    .filter_map(|r| future::ready(r.ok()))
    .then(async |(item, _)| updater.on_update(item, &data.source, ctx.clone()).await)
    .try_for_each(async |handled| Ok(()))
    .await?;

    Ok(())
}
