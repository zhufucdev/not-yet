use std::sync::Arc;

use crate::llm::SharedImageOrText;
use tokio::sync::{broadcast, mpsc};

pub mod gemma4;

/// Analyze agents' actions to get
/// what to prefill the context window with
/// or scheduler parameters to tune
#[trait_variant::make(Send)]
pub trait Optimizer<Dialog> {
    type Error;
    async fn optimize(
        self: &Arc<Self>,
        prompt: Option<String>,
        dialog: &Dialog,
    ) -> OptimizationCallback<Self::Error>;
}

#[derive(Debug, Clone)]
pub enum OptimizerAction {
    ContextPrefill(Vec<SharedImageOrText>),
    Schedule(ScheduleParamters),
}

#[derive(Debug, Clone)]
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
