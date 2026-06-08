use std::{any::Any, pin::Pin};

use anyhow::anyhow;

#[trait_variant::make(Send)]
pub trait UpdatePersistence: Unpin + Send + Sync + 'static {
    type Item: Unpin + Send + Sync;
    type Error: Send;
    /// Mark an item as seen
    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error>;
    /// If the given item is seen, return `true`, otherwise `false`
    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error>;
}

pub struct AnyUpdatePersistence(Box<dyn UpdatePersistenceHolder>);

impl AnyUpdatePersistence {
    pub fn from<P>(persistence: P) -> Self
    where
        P: UpdatePersistence,
        P::Error: Into<anyhow::Error>,
    {
        Self(Box::new(persistence))
    }
}

trait UpdatePersistenceHolder: Send + Sync {
    fn update<'s>(
        &'s self,
        item: Option<&'s Box<dyn Any + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 's>>;

    fn cmp<'s>(
        &'s self,
        current: Option<&'s Box<dyn Any + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, anyhow::Error>> + Send + 's>>;
}

impl<P> UpdatePersistenceHolder for P
where
    P: UpdatePersistence,
    P::Error: Into<anyhow::Error>,
{
    fn update<'s>(
        &'s self,
        item: Option<&'s Box<dyn Any + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 's>> {
        Box::pin(async move {
            let item = item.map(|item| item.downcast_ref::<P::Item>().unwrap());
            P::update(&self, item).await.map_err(|err| anyhow!(err))
        })
    }

    fn cmp<'s>(
        &'s self,
        current: Option<&'s Box<dyn Any + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, anyhow::Error>> + Send + 's>> {
        Box::pin(async move {
            let current = current.map(|item| item.downcast_ref::<P::Item>().unwrap());
            P::cmp(&self, current).await.map_err(|err| anyhow!(err))
        })
    }
}

impl UpdatePersistence for AnyUpdatePersistence {
    type Item = Box<dyn Any + Send + Sync>;

    type Error = anyhow::Error;

    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error> {
        self.0.update(item).await
    }

    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error> {
        self.0.cmp(current).await
    }
}
