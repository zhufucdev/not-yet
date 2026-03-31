use thiserror::Error;

#[derive(Debug, Error)]
#[error("this should have never happened, logic is flawed")]
pub struct NaE;
