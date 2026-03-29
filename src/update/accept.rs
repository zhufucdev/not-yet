use std::marker::PhantomData;

use async_trait::async_trait;
use thiserror::Error;

use crate::update::UpdatePersistence;

pub struct AcceptUpdatePersistence<I> {
    _marker: PhantomData<I>,
}

impl<I> AcceptUpdatePersistence<I> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

#[async_trait]
impl<I> UpdatePersistence for AcceptUpdatePersistence<I>
where
    I: Unpin + Send + Sync + 'static,
{
    type Item = I;

    type Error = Non;

    async fn update(&self, _item: Option<&Self::Item>) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn cmp(&self, _current: Option<&Self::Item>) -> Result<bool, Self::Error> {
        Ok(false)
    }
}

#[derive(Debug, Error)]
#[error("this should never happen, logic is flawed")]
pub struct Non;
