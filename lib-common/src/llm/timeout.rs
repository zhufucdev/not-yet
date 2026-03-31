use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use tokio::{sync::RwLock, task::JoinHandle};

use tracing::{Level, event};

use crate::llm::Model;

pub struct ModelProducer<Model, Error>(
    Box<dyn Fn() -> BoxFuture<'static, Result<Model, Error>> + Send + Sync>,
);

/// Unloads the model when not in use
pub struct TimedModel<Runner, Error> {
    name: String,
    timeout: Duration,
    cache: Arc<RwLock<Option<Arc<Runner>>>>,
    timeout_job: Arc<RwLock<Option<JoinHandle<()>>>>,
    builder: Arc<ModelProducer<Runner, Error>>,
}

impl<Runner, Error> TimedModel<Runner, Error>
where
    Runner: Send + Sync + 'static,
    Error: Send + Sync + 'static,
{
    pub fn new(
        name: impl ToString,
        timeout: Duration,
        builder: ModelProducer<Runner, Error>,
    ) -> Self {
        Self {
            name: name.to_string(),
            timeout,
            cache: Arc::new(RwLock::new(None)),
            timeout_job: Arc::new(RwLock::new(None)),
            builder: Arc::new(builder),
        }
    }

    async fn add_timeout_job(&self) {
        let timeout = self.timeout.clone();
        let cache = self.cache.clone();
        self.timeout_job
            .write()
            .await
            .replace(tokio::task::spawn(async move {
                tokio::time::sleep(timeout).await;
                event!(Level::DEBUG, "dropping model");
                *cache.write().await = None;
            }));
    }
}

impl<Runner, Error> Model for TimedModel<Runner, Error>
where
    Runner: Send + Sync + 'static,
    Error: Send + Sync + 'static,
{
    type Runner = Runner;
    type Error = Error;

    async fn get_runner(&self) -> Result<Arc<Self::Runner>, Self::Error> {
        if let Some(timeout_job) = self.timeout_job.write().await.take() {
            event!(Level::DEBUG, "aborting timeout job for {}", self.name);
            timeout_job.abort();
        }
        self.add_timeout_job().await;
        if let Some(cached) = self.cache.read().await.as_ref() {
            event!(Level::DEBUG, "cache hit for model {}", self.name);
            return Ok(cached.clone());
        }
        event!(Level::DEBUG, "cache missed, building model {}", self.name);
        let model = Arc::new(self.builder.0().await?);
        *self.cache.write().await = Some(model.clone());
        Ok(model.clone())
    }
}

impl<Model, Error> ModelProducer<Model, Error> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Model, Error>> + Send + 'static,
    {
        Self(Box::new(move || Box::pin(f())))
    }
}

impl<Model, Error> Clone for TimedModel<Model, Error> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            timeout: self.timeout.clone(),
            cache: self.cache.clone(),
            timeout_job: self.timeout_job.clone(),
            builder: self.builder.clone(),
        }
    }
}
