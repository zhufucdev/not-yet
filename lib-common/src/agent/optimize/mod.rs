use std::fmt::Display;

use tokio::{select, sync::mpsc};
use tracing::{Level, event};

pub mod llm;

#[cfg(test)]
mod test;

/// Analyze agents' actions to get
/// what to prefill the context window with
/// or scheduler parameters to tune
#[trait_variant::make(Send)]
pub trait Optimizer<Dialog> {
    type Error;
    type ExtraAction;
    fn optimize(
        &self,
        prompt: Option<impl ToString + Send>,
        dialog: Dialog,
    ) -> OptimizationCallback<Self::Error, Self::ExtraAction>;
}

#[derive(Debug, Clone)]
pub enum OptimizerAction<Extra> {
    Basic(BasicOptimizerAction),
    Extra(Extra),
}

#[derive(Debug, Clone)]
pub enum BasicOptimizerAction {
    ContextPrefill(Vec<String>),
    Schedule(ScheduleParamters),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveOrDeny {
    Approve,
    Deny { reason: Option<String> },
}

pub struct OptimizationCallback<Error, ExtraAction> {
    action_rx: mpsc::Receiver<(OptimizerAction<ExtraAction>, mpsc::Sender<ApproveOrDeny>)>,
    task_completion: mpsc::Receiver<()>,
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

pub type Actor<T> = mpsc::Sender<(T, mpsc::Sender<ApproveOrDeny>)>;

impl<Error, ExtraAction> OptimizationCallback<Error, ExtraAction>
where
    Error: Display,
    ExtraAction: Send + 'static,
{
    pub fn new<F, Fut>(task: F) -> Self
    where
        F: FnOnce(mpsc::Sender<(OptimizerAction<ExtraAction>, mpsc::Sender<ApproveOrDeny>)>) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
        Error: Send + 'static,
    {
        let (action_tx, action_rx) = mpsc::channel(1);
        let (tc_tx, tc_rx) = mpsc::channel(1);
        Self {
            action_rx,
            task_completion: tc_rx,
            task_handle: Some(tokio::spawn(async move {
                let r = match task(action_tx).await {
                    Ok(_) => Ok(()),
                    Err(err) => {
                        event!(Level::ERROR, "optimization task failed: {err}");
                        Err(err)
                    }
                };
                tc_tx.send(()).await.unwrap();
                r
            })),
        }
    }

    pub async fn accept(
        &mut self,
    ) -> Result<Option<(OptimizerAction<ExtraAction>, mpsc::Sender<ApproveOrDeny>)>, Error> {
        if self.task_handle.is_none() {
            return Ok(None);
        }
        select! {
            _ = self.task_completion.recv() => {
                let task_handle = self.task_handle.take().unwrap();
                if let Err(err) = task_handle.await.unwrap() {
                    return Err(err);
                }
                return Ok(None);
            }
            option = self.action_rx.recv() => {
                if let Some(res) = option {
                    return Ok(Some(res));
                }
                let task_handle = self.task_handle.take().unwrap();
                if task_handle.is_finished() {
                    if let Err(err) = task_handle.await.unwrap() {
                        return Err(err);
                    }
                } else {
                    panic!("receiver dropped without dropping task handle");
                }
                return Ok(None);
            }
        }
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
