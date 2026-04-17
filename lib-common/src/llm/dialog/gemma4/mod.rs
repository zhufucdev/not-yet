mod assistant;
mod error;
mod req;
mod template;
#[cfg(test)]
mod test;
mod tool;
mod turn;

pub use assistant::AssistantResponse;
pub use error::Error;
pub use req::DialogRequest;
pub use template::DialogTemplate;
pub use tool::{ToolCall, ToolHandler, ToolResponse, ToolResult};
pub use turn::DialogTurn;

pub(self) const ROLE_TOOL: &'static str = "tool";
pub type Dialog = super::MultiTurnDialog<DialogTurn, Vec<minijinja::Value>>;
