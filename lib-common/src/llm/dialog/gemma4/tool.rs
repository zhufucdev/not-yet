use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolResponse {
    name: String,
    response: minijinja::Value,
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

