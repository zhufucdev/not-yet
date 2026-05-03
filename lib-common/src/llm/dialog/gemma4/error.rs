use llama_runner::{
    error::GenericRunnerError,
    mcp::error::{JinjaTemplateError, ParseToolError},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported role")]
    UnsupportedRole,
    #[error(transparent)]
    Runner(#[from] GenericRunnerError<JinjaTemplateError>),
    #[error("parse tool {0}: {1}")]
    ParseTool(String, #[source] ParseToolError),
}
