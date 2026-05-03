use serde::{Deserialize, Serialize};

use crate::llm::dialog::toolcall::{self, FromKeyAndResult, ToolNotFound};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResponse {
    pub name: String,
    pub response: minijinja::Value,
}

pub type ToolHandler<'a, Error> =
    toolcall::ToolHandler<'a, serde_json::Map<String, serde_json::Value>, minijinja::Value, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolResult<'s> {
    #[serde(rename = "success")]
    Success(&'s str),
    #[serde(rename = "failure")]
    Failure(&'s str),
}

impl ToolResponse {
    pub fn new<R>(name: impl ToString, response: R) -> Self
    where
        R: Serialize,
    {
        Self {
            name: name.to_string(),
            response: minijinja::Value::from_serialize(response),
        }
    }
}

impl Into<toolcall::ToolCall<String, serde_json::Map<String, serde_json::Value>>> for ToolCall {
    fn into(self) -> toolcall::ToolCall<String, serde_json::Map<String, serde_json::Value>> {
        toolcall::ToolCall {
            tool: self.name,
            args: self.arguments,
        }
    }
}

impl ToolNotFound<String> for ToolResponse {
    fn not_found(tool: String) -> Self {
        Self::new(tool, ToolResult::Failure("not found"))
    }
}

impl<S> FromKeyAndResult<String, S> for ToolResponse
where
    S: Serialize,
{
    fn from(key: String, res: S) -> Self {
        Self::new(key, res)
    }
}

impl Into<minijinja::Value> for ToolResult<'_> {
    fn into(self) -> minijinja::Value {
        minijinja::Value::from_serialize(self)
    }
}
