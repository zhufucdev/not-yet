use std::{
    cell::RefCell,
    fmt::Debug,
    hash::{DefaultHasher, Hash, Hasher},
    str::FromStr,
    sync::Arc,
};

use futures::{StreamExt, TryStreamExt, stream};
use tokio::sync::RwLock;
use tracing::{Level, event};
use tracing_test::traced_test;

use crate::{
    source::{self, LlmRssItem, RssFeed},
    update::{Updatable, UpdatePersistence, UpdateWakerExt},
};

struct MockUpdateable(RefCell<Vec<u64>>);

#[derive(Clone)]
struct DebugPersistence<T> {
    current: Arc<RwLock<Vec<u64>>>,
    _phantom: std::marker::PhantomData<T>,
}

struct MockRssFeed {
    inner: RssFeed,
}

#[tokio::test]
#[traced_test]
async fn test_update_simple() {
    let source = MockUpdateable(RefCell::new(vec![1, 2, 3]));
    let persistence = DebugPersistence::default();
    let series = stream::once(async { Ok(()) })
        .wake_update(&source, persistence.clone(), usize::MAX)
        .map_ok(|(num, _)| num.unwrap())
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap(), vec![1, 2, 3]);

    let series = stream::once(async { Ok(()) })
        .wake_update(&source, persistence.clone(), usize::MAX)
        .map_ok(|(num, _)| num)
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap()[0], None);

    source.0.borrow_mut().push(4);
    let series = stream::once(async { Ok(()) })
        .wake_update(&source, persistence.clone(), usize::MAX)
        .map_ok(|(num, _)| num.unwrap())
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap(), vec![4]);

    source.0.borrow_mut().clear();
    let series = stream::once(async { Ok(()) })
        .wake_update(&source, persistence, usize::MAX)
        .map_ok(|(num, _)| num)
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap(), vec![None]);
}

#[tokio::test]
#[traced_test]
async fn test_hacker_news() {
    let feed = MockRssFeed::new("https://hnrss.org/newest?points=100").unwrap();
    let channels = [
        include_str!("hn-newest-18-57.xml"),
        include_str!("hn-newest-08-18.xml"),
        include_str!("hn-newest-09-21.xml"),
        include_str!("hn-newest-11-30.xml"),
    ]
    .into_iter()
    .map(|s| rss::Channel::from_str(s).unwrap())
    .collect::<Vec<_>>();
    let updates = stream::iter(channels.into_iter())
        .enumerate()
        .then(async |(idx, channel)| {
            event!(Level::DEBUG, "updated to channel {}", idx);
            feed.update_from_channel(channel).await.unwrap();
            Ok(())
        })
        .wake_update(&feed, DebugPersistence::default(), usize::MAX)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(updates.len(), 42);
    assert!(updates[40].0.is_none());
    assert_eq!(
        updates[41].0.as_ref().map(|i| i.title()),
        Some("Lean proved this program correct; then I found a bug https://kirancodes.me/posts/log-who-watches-the-watchers.html".into())
    )
}

impl<T> Default for DebugPersistence<T> {
    fn default() -> Self {
        Self {
            current: Default::default(),
            _phantom: Default::default(),
        }
    }
}

impl<T> UpdatePersistence for DebugPersistence<T>
where
    T: Send + Sync + Clone + Unpin + Debug + Hash + 'static,
{
    type Item = T;

    type Error = anyhow::Error;

    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error> {
        event!(Level::DEBUG, "update(item = {:#?})", item);
        let mut current_guard = self.current.write().await;
        if let Some(item) = item {
            let mut hasher = DefaultHasher::new();
            item.hash(&mut hasher);
            current_guard.push(hasher.finish());
        } else {
            current_guard.clear();
        };
        Ok(())
    }

    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error> {
        if let Some(item) = current {
            let mut hasher = DefaultHasher::new();
            item.hash(&mut hasher);
            let hash = hasher.finish();
            Ok(self.current.read().await.contains(&hash))
        } else {
            Ok(self.current.read().await.is_empty())
        }
    }
}

unsafe impl Send for MockUpdateable {}
unsafe impl Sync for MockUpdateable {}

impl Updatable for MockUpdateable {
    type Item = u64;

    type Error = anyhow::Error;

    async fn get_items(&self) -> Result<Vec<Self::Item>, Self::Error> {
        Ok(self.0.borrow().clone())
    }

    async fn update(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl MockRssFeed {
    fn new(url: impl ToString) -> Result<Self, reqwest::Error> {
        Ok(Self {
            inner: RssFeed::new(url, None)?,
        })
    }

    pub(crate) async fn update_from_channel(
        &self,
        channel: rss::Channel,
    ) -> Result<(), source::rss::Error> {
        self.inner.update_from_channel(channel).await
    }
}

impl Updatable for MockRssFeed {
    type Item = LlmRssItem;

    type Error = source::rss::Error;

    async fn get_items(&self) -> Result<Vec<Self::Item>, Self::Error> {
        self.inner.get_items().await
    }

    async fn update(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
