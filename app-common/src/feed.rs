use std::fmt::Display;
use std::hash::Hash;

use anyhow::anyhow;
use futures::{Stream, StreamExt, TryStreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tracing::{Level, event};

use lib_common::{
    agent::Decider,
    polling::{KeyContract, Scheduler, task::Task},
    source::{Feed, LlmComprehendable},
    update::{UpdatePersistence, UpdateWakerExt},
};

/// Generate an infinite stream (unless `key` was not present in `scheduler`)
/// where each item's truth value is determined by the `decider`
pub fn check<
    'f,
    Key,
    Item,
    FeedError,
    Feed_,
    DeciderError,
    Decider_,
    PersistenceError,
    Persistence,
>(
    key: &'f Key,
    feed: &'f Feed_,
    decider: &'f Decider_,
    scheduler: &Scheduler<Key>,
    persistence: Persistence,
) -> impl Stream<Item = anyhow::Result<(Task<Key>, bool)>>
where
    Key: KeyContract + Display,
    Item: LlmComprehendable + Hash + Serialize + DeserializeOwned + Send + Sync + Unpin + 'static,
    FeedError: Display,
    Feed_: Feed<Item = Item, Error = FeedError>,
    DeciderError: std::error::Error + Send + Sync + 'static,
    Decider_: Decider<Material = Item, Error = DeciderError> + ?Sized,
    PersistenceError: Display,
    Persistence: UpdatePersistence<Item = Item, Error = PersistenceError>,
{
    scheduler
        .start_polling(Some(key))
        .wake_update(feed, persistence)
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
        .then(async move |result| -> anyhow::Result<(Task<Key>, bool)> {
            match result {
                Ok((update, task)) => {
                    let Some(material) = update else {
                        return Ok((task, false));
                    };
                    Ok((task, decider.get_truth_value(material).await?))
                }
                Err(err) => Err(err),
            }
        })
}
