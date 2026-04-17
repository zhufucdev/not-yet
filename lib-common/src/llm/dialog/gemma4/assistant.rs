use llama_runner::MessageRole;
use serde::{Deserialize, Serialize};

use crate::llm::dialog::{gemma4::tool::ToolCall, parse};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantResponse {
    pub reasoning: Option<String>,
    pub content: String,
    pub tool_calls: Vec<parse::gemmma4::Result<ToolCall>>,
}

impl Into<minijinja::Value> for &AssistantResponse {
    fn into(self) -> minijinja::Value {
        [
            (
                "tool_cals",
                self.tool_calls
                    .iter()
                    .filter_map(|call| call.as_ref().ok())
                    .map(minijinja::Value::from_serialize)
                    .collect::<minijinja::Value>(),
            ),
            ("content", self.content.clone().into()),
            ("reasoning", self.reasoning.clone().into()),
            ("role", MessageRole::Assistant.to_string().into()),
        ]
        .into_iter()
        .collect()
    }
}
