use std::{any::Any, pin::Pin};

use anyhow::anyhow;

use crate::update::Source;

#[trait_variant::make(Send)]
pub trait Updatable: Source {
    async fn update(&self) -> Result<(), Self::Error>;
}

pub struct AnyUpdatable(Box<dyn UpdatableHolder>);

impl AnyUpdatable {
    pub fn from<U>(source: U) -> Self
    where
        U: Updatable + Sync + 'static,
        U::Error: Into<anyhow::Error>,
    {
        Self(Box::new(source))
    }

    pub fn inner<'s>(&'s self) -> &'s dyn Any {
        self.0.as_ref()
    }
}

trait UpdatableHolder: Any + Send + Sync {
    fn update<'s>(&'s self)
    -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 's>>;
    fn get_items<'s>(
        &'s self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<Box<dyn Any + Send + Sync>>, anyhow::Error>> + Send + 's,
        >,
    >;
}

impl<U> UpdatableHolder for U
where
    U: Updatable + Sync + 'static,
    U::Item: 'static,
    U::Error: Into<anyhow::Error>,
{
    fn update<'s>(
        &'s self,
    ) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 's>> {
        Box::pin(async move { U::update(&self).await.map_err(|e| anyhow!(e)) })
    }

    fn get_items<'s>(
        &'s self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<Box<dyn Any + Send + Sync>>, anyhow::Error>> + Send + 's,
        >,
    > {
        Box::pin(async move {
            U::get_items(&self)
                .await
                .map_err(|e| anyhow!(e))
                .map(|items| {
                    items
                        .into_iter()
                        .map(|item| Box::new(item) as Box<dyn Any + Send + Sync>)
                        .collect()
                })
        })
    }
}

impl Source for AnyUpdatable {
    type Item = Box<dyn Any + Send + Sync>;

    type Error = anyhow::Error;

    async fn get_items(&self) -> Result<Vec<Self::Item>, Self::Error> {
        self.0.get_items().await
    }
}

impl Updatable for AnyUpdatable {
    async fn update(&self) -> Result<(), Self::Error> {
        self.0.update().await
    }
}
