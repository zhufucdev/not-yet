use std::{
    cell::RefCell,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures::{FutureExt, Stream, StreamExt, future::BoxFuture};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::polling::error::TaskCancellationError;

pub mod accept;
pub mod sqlite;

#[trait_variant::make(Send)]
pub trait UpdatePersistence: Unpin + Send + Sync + 'static {
    type Item: Unpin + Send + Sync;
    type Error: Send;
    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error>;
    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error>;
}

#[trait_variant::make(Send)]
pub trait Updatable {
    type Item: Unpin + Send + Sync;
    type Error;
    async fn get_items(&self) -> Result<Vec<Self::Item>, Self::Error>;
}

pub trait UpdatableExt<W, D> {
    fn persistent_distinct<'s, P>(&'s self, storage: P, waker: W) -> Update<'s, Self, P, W, D>
    where
        Self: Updatable + Sized,
        P: UpdatePersistence<Item = Self::Item>;
}

pub trait UpdateWakerExt<D> {
    fn wake_update<'s, I, P>(self, source: &'s I, persistence: P) -> Update<'s, I, P, Self, D>
    where
        I: Updatable,
        P: UpdatePersistence<Item = I::Item>,
        <I as Updatable>::Item: Unpin + Send + Sync,
        Self: Sized;
}

pin_project_lite::pin_project! {
    pub struct Update<'f, I, P, W, D>
    where
        I: Updatable,
        P: UpdatePersistence<Item = I::Item>,
    {
        source: &'f I,
        persistence: Arc<P>,
        state: RefCell<UpdateState<'f, I::Item, I::Error, P::Error, D>>,
        #[pin]
        waker: W,
    }
}

impl<'f, I, P, W, D> Stream for Update<'f, I, P, W, D>
where
    I: Updatable,
    P: UpdatePersistence<Item = I::Item>,
    I::Item: Unpin + Send + Sync,
    W: Stream<Item = Result<D, TaskCancellationError>>,
{
    type Item = Result<(Option<I::Item>, D), Error<I::Error, P::Error>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match (*this.state.get_mut()).clone() {
                UpdateState::Idle => {
                    match this.waker.as_mut().poll_next_unpin(cx) {
                        Poll::Ready(None) => {
                            // finite stream ends
                            return Poll::Ready(None);
                        }
                        Poll::Ready(Some(Ok(data))) => {
                            let fut = this.source.get_items();
                            *this.state.borrow_mut() = UpdateState::Fetching {
                                fut: Arc::new(RefCell::new(Box::pin(fut))),
                                data: Arc::new(RefCell::new(Some(data))),
                            };
                        }
                        _ => {
                            return Poll::Pending;
                        }
                    }
                }

                UpdateState::Fetching { fut, data } => {
                    let items = match fut.borrow_mut().poll_unpin(cx) {
                        Poll::Pending => return Poll::Pending, // waker registered by fut
                        Poll::Ready(Err(e)) => {
                            *this.state.borrow_mut() = UpdateState::Idle;
                            return Poll::Ready(Some(Err(Error::Fetch(e))));
                        }
                        Poll::Ready(Ok(items)) => items,
                    };
                    let items = Arc::new(RwLock::new(items));
                    let items_cp = items.clone();
                    let persistence = this.persistence.clone();
                    let fut: BoxFuture<_> =
                        Box::pin(
                            async move { persistence.cmp(items_cp.read().await.last()).await },
                        );
                    *this.state.borrow_mut() = UpdateState::Comparing {
                        fut: Arc::new(RefCell::new(fut)),
                        items,
                        data,
                    };
                }

                UpdateState::Comparing { fut, items, data } => {
                    match fut.borrow_mut().poll_unpin(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => {
                            *this.state.borrow_mut() = UpdateState::Idle;
                            return Poll::Ready(Some(Err(Error::Persistence(e))));
                        }
                        Poll::Ready(Ok(true)) => {
                            *this.state.borrow_mut() = UpdateState::Idle;
                        }
                        Poll::Ready(Ok(false)) => {
                            let items_cp = items.clone();
                            let persistence = this.persistence.clone();
                            let fut: Arc<RefCell<BoxFuture<_>>> =
                                Arc::new(RefCell::new(Box::pin(async move {
                                    persistence.update(items_cp.read().await.last()).await
                                })));
                            *this.state.borrow_mut() = UpdateState::Updating { items, fut, data };
                        }
                    }
                }

                UpdateState::Updating { items, fut, data } => {
                    match fut.borrow_mut().poll_unpin(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => {
                            return Poll::Ready(Some(Err(Error::Persistence(e))));
                        }
                        Poll::Ready(Ok(())) => {
                            let fut: Arc<RefCell<BoxFuture<_>>> =
                                Arc::new(RefCell::new(Box::pin(async move {
                                    items.write().await.pop()
                                })));
                            *this.state.borrow_mut() = UpdateState::Updated { fut, data };
                        }
                    }
                }

                UpdateState::Updated { fut, data } => match fut.borrow_mut().poll_unpin(cx) {
                    Poll::Ready(it) => {
                        *this.state.borrow_mut() = UpdateState::Idle;
                        return Poll::Ready(Some(Ok((it, data.borrow_mut().take().unwrap()))));
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

enum UpdateState<'f, Item, FetchErr, UpdateErr, Data> {
    Idle,
    Fetching {
        fut: Arc<RefCell<BoxFuture<'f, Result<Vec<Item>, FetchErr>>>>,
        data: Arc<RefCell<Option<Data>>>,
    },
    Comparing {
        fut: Arc<RefCell<BoxFuture<'f, Result<bool, UpdateErr>>>>,
        items: Arc<RwLock<Vec<Item>>>,
        data: Arc<RefCell<Option<Data>>>,
    },
    Updating {
        items: Arc<RwLock<Vec<Item>>>,
        fut: Arc<RefCell<BoxFuture<'f, Result<(), UpdateErr>>>>,
        data: Arc<RefCell<Option<Data>>>,
    },
    Updated {
        fut: Arc<RefCell<BoxFuture<'f, Option<Item>>>>,
        data: Arc<RefCell<Option<Data>>>,
    },
}

impl<'a, I, F, U, D> Clone for UpdateState<'a, I, F, U, D> {
    fn clone(&self) -> Self {
        match self {
            Self::Idle => Self::Idle,
            Self::Fetching { fut, data } => Self::Fetching {
                fut: fut.clone(),
                data: data.clone(),
            },
            Self::Comparing { fut, items, data } => Self::Comparing {
                fut: fut.clone(),
                items: items.clone(),
                data: data.clone(),
            },
            Self::Updating { items, fut, data } => Self::Updating {
                items: items.clone(),
                fut: fut.clone(),
                data: data.clone(),
            },
            Self::Updated { fut, data } => Self::Updated {
                fut: fut.clone(),
                data: data.clone(),
            },
        }
    }
}

macro_rules! delegate_access_inner {
    ($field:ident, $inner:ty, ($($ind:tt)*)) => {
        /// Acquires a reference to the underlying sink or stream that this combinator is
        /// pulling from.
        pub fn get_ref(&self) -> &$inner {
            (&self.$field) $($ind get_ref())*
        }

        /// Acquires a mutable reference to the underlying sink or stream that this
        /// combinator is pulling from.
        ///
        /// Note that care must be taken to avoid tampering with the state of the
        /// sink or stream which may otherwise confuse this combinator.
        pub fn get_mut(&mut self) -> &mut $inner {
            (&mut self.$field) $($ind get_mut())*
        }

        /// Acquires a pinned mutable reference to the underlying sink or stream that this
        /// combinator is pulling from.
        ///
        /// Note that care must be taken to avoid tampering with the state of the
        /// sink or stream which may otherwise confuse this combinator.
        pub fn get_pin_mut(self: core::pin::Pin<&mut Self>) -> core::pin::Pin<&mut $inner> {
            self.project().$field $($ind get_pin_mut())*
        }

        /// Consumes this combinator, returning the underlying sink or stream.
        ///
        /// Note that this may discard intermediate state of this combinator, so
        /// care should be taken to avoid losing resources when this is called.
        pub fn into_inner(self) -> $inner {
            self.$field $($ind into_inner())*
        }
    }
}

impl<'f, I, P, W, D> Update<'f, I, P, W, D>
where
    I: Updatable,
    P: UpdatePersistence<Item = I::Item>,
    I::Item: Unpin + Send + Sync,
    W: Stream<Item = Result<D, TaskCancellationError>>,
{
    fn new_ref(source: &'f I, persistence: Arc<P>, waker: W) -> Self {
        assert_stream(Update {
            source,
            persistence,
            state: RefCell::new(UpdateState::Idle),
            waker,
        })
    }

    fn new(source: &'f I, persistence: P, waker: W) -> Self {
        Self::new_ref(source, Arc::new(persistence), waker)
    }

    delegate_access_inner!(waker, W, ());
}

impl<S, W, D> UpdatableExt<W, D> for S
where
    S: Updatable + Sized,
    S::Item: Unpin + Send + Sync,
    W: Stream<Item = Result<D, TaskCancellationError>>,
{
    fn persistent_distinct<'s, P>(&'s self, storage: P, waker: W) -> Update<'s, Self, P, W, D>
    where
        Self: Updatable + Sized,
        P: UpdatePersistence<Item = S::Item>,
    {
        Update::new(self, storage, waker)
    }
}

impl<S, D> UpdateWakerExt<D> for S
where
    S: Stream<Item = Result<D, TaskCancellationError>>,
{
    fn wake_update<'s, I, P>(self, source: &'s I, persistence: P) -> Update<'s, I, P, Self, D>
    where
        I: Updatable,
        P: UpdatePersistence<Item = I::Item>,
        <I as Updatable>::Item: Unpin + Send + Sync,
        Self: Sized,
    {
        Update::new(source, persistence, self)
    }
}

pub(crate) fn assert_stream<T, S>(stream: S) -> S
where
    S: Stream<Item = T>,
{
    stream
}

#[derive(Debug, Error)]
pub enum Error<Fetch, Persistence> {
    #[error("fetch: {0}")]
    Fetch(Fetch),
    #[error("save: {0}")]
    Persistence(Persistence),
}
