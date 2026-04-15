mod assistant;
mod error;
mod req;
mod template;
#[cfg(test)]
mod test;
mod tool;
mod turn;

pub use assistant::AssistantResponse;
pub use req::DialogRequest;
pub use template::DialogTemplate;
pub use tool::{ToolCall, ToolResponse};
pub use turn::DialogTurn;

pub(self) const ROLE_TOOL: &'static str = "tool";
