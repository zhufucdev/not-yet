use std::{fmt::Display, sync::Arc};

use tokio::sync::mpsc;

pub mod llm;

#[cfg(test)]
mod test;

/// Analyze agents' actions to get
/// what to prefill the context window with
/// or scheduler parameters to tune
#[trait_variant::make(Send)]
pub trait Optimizer<Dialog> {
    type Error;
    fn optimize(
        &self,
        prompt: Option<impl ToString + Send>,
        dialog: Dialog,
    ) -> OptimizationCallback<Self::Error>;
}

#[derive(Debug, Clone)]
pub enum OptimizerAction {
    ContextPrefill(Vec<String>),
    Schedule(ScheduleParamters),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveOrDeny {
    Approve,
    Deny { reason: Option<String> },
}

pub struct OptimizationCallback<Error> {
    rx: mpsc::Receiver<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>,
    task_handle: Option<tokio::task::JoinHandle<Result<(), Error>>>,
}

#[derive(Debug, Clone)]
pub struct ScheduleParamters {
    pub interval_mins: Option<u32>,
    pub buffer_size: Option<usize>,
}

#[trait_variant::make(Send)]
pub trait ScheduleParamterAccessor {
    type Error: std::error::Error;

    async fn get_interval_mins(&self) -> u32;
    async fn set_interval_mins(&mut self, new_value: u32) -> Result<(), Self::Error>;

    async fn get_buffer_size(&self) -> usize;
    async fn set_buffer_size(&mut self, new_value: usize) -> Result<(), Self::Error>;
}

#[trait_variant::make(Send)]
pub trait ClarificationReqHandler {
    type Error: std::error::Error;
    async fn on_request(&self, prompt: &str) -> Result<Option<String>, Self::Error>;
}

impl<Error> OptimizationCallback<Error> {
    pub fn new<F, Fut>(task: F) -> Self
    where
        F: FnOnce(mpsc::Sender<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>) -> Fut,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
        Error: Send + 'static,
    {
        let (action_tx, action_rx) = mpsc::channel(1);
        Self {
            rx: action_rx,
            task_handle: Some(tokio::spawn(task(action_tx))),
        }
    }

    pub async fn accept(
        &mut self,
    ) -> Result<Option<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>, Error> {
        if self.task_handle.is_none() {
            return Ok(None);
        }
        let Some(res) = self.rx.recv().await else {
            let task_handle = self.task_handle.take().unwrap();
            if task_handle.is_finished() {
                if let Err(err) = task_handle.await.unwrap() {
                    return Err(err);
                }
            } else {
                panic!("receiver dropped without dropping task handle");
            }
            return Ok(None);
        };
        Ok(Some(res))
    }
}

impl Display for ScheduleParamters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut add_comma = false;
        if let Some(interval) = self.interval_mins {
            write!(f, "{} minutes apart", interval)?;
            add_comma = true;
        }
        if let Some(buffer) = self.buffer_size {
            if add_comma {
                write!(f, ", and ")?;
            }
            write!(f, "allow at most {} staged posts", buffer)?;
        }
        Ok(())
    }
}
