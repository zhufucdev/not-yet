use thiserror::Error;

#[derive(Debug, Error)]
#[error("Task cancelled")]
pub struct TaskCancellationError;
