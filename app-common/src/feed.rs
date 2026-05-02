use std::fmt::Display;

use anyhow::anyhow;
use futures::{Stream, StreamExt, TryStreamExt};
use tracing::{Level, event};

use lib_common::{
    polling::{KeyContract, Scheduler, task::Task},
    source::Feed,
    update::{UpdatePersistence, UpdateWakerExt},
};

/// Generate an infinite stream (unless `key` was not present in `scheduler`)
/// where each item's truth value is determined by the `decider`
pub fn check<'f, Key, Item, FeedError, Feed_, PersistenceError, Persistence>(
    key: &'f Key,
    feed: &'f Feed_,
    scheduler: &Scheduler<Key>,
    persistence: Persistence,
    buffer_size: usize,
) -> impl Stream<Item = anyhow::Result<(Option<Item>, Task<Key>)>>
where
    Key: KeyContract + Display,
    Item: Send + Sync + Unpin + 'static,
    FeedError: Display,
    Feed_: Feed<Item = Item, Error = FeedError>,
    PersistenceError: Display,
    Persistence: UpdatePersistence<Item = Item, Error = PersistenceError>,
{
    scheduler
        .start_polling(Some(key))
        .wake_update(feed, persistence, buffer_size)
        .map_ok(move |(update, task)| {
            event!(
                Level::DEBUG,
                "woke for update, schedule id = {}, key = {}",
                task.schedule().id(),
                task.schedule().key()
            );
            (update, task)
        })
        .map_err(|err| anyhow!("update error: {err}"))
}
