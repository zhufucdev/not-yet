use std::{cell::RefCell, sync::Arc};

use futures::{StreamExt, TryStreamExt, pin_mut, stream};
use tokio::sync::RwLock;
use tracing::{Level, event};
use tracing_test::traced_test;

use crate::update::{Updatable, UpdatePersistence, UpdateWakerExt};

struct MockUpdateable(RefCell<Vec<u64>>);

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

#[derive(Default, Clone)]
struct DebugPersistence {
    current: Arc<RwLock<Option<u64>>>,
}

impl UpdatePersistence for DebugPersistence {
    type Item = u64;

    type Error = anyhow::Error;

    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error> {
        event!(Level::DEBUG, "update({:?})", item);
        *self.current.write().await = item.cloned();
        Ok(())
    }

    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error> {
        event!(
            Level::DEBUG,
            "cmp({:?}, {:?})",
            self.current.read().await,
            current
        );
        Ok(self.current.read().await.as_ref() == current)
    }
}

#[tokio::test]
#[traced_test]
async fn test_update() {
    let source = MockUpdateable(RefCell::new(vec![1, 2, 3]));
    let persistence = DebugPersistence::default();
    let stream =
        stream::once(async { Ok(()) }).wake_update(&source, persistence.clone(), usize::MAX);
    pin_mut!(stream);
    let series = stream
        .map_ok(|(num, _)| num.unwrap())
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap(), vec![1, 2, 3]);

    let stream =
        stream::once(async { Ok(()) }).wake_update(&source, persistence.clone(), usize::MAX);
    pin_mut!(stream);
    let series = stream.map_ok(|(num, _)| num).try_collect::<Vec<_>>().await;
    assert_eq!(series.unwrap()[0], None);

    source.0.borrow_mut().push(4);
    let stream = stream::once(async { Ok(()) }).wake_update(&source, persistence.clone(), usize::MAX);
    pin_mut!(stream);
    let series = stream
        .map_ok(|(num, _)| num.unwrap())
        .try_collect::<Vec<_>>()
        .await;
    assert_eq!(series.unwrap(), vec![4]);

    source.0.borrow_mut().clear();
    let stream = stream::once(async { Ok(()) }).wake_update(&source, persistence, usize::MAX);
    pin_mut!(stream);
    let series = stream.map_ok(|(num, _)| num).try_collect::<Vec<_>>().await;
    assert_eq!(series.unwrap(), vec![None]);
}
