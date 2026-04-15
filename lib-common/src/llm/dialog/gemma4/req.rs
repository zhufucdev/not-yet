use rmcp::model::Tool;

use crate::llm::dialog::{self, SimpleDialogHyperParams, gemma4::DialogTurn};

#[derive(Clone, Debug, Default)]
pub struct DialogRequest {
    message: DialogTurn,
    simple: SimpleDialogHyperParams,
    tools: Vec<Tool>,
    enable_thinking: bool,
}

impl dialog::DialogRequest<DialogTurn> for DialogRequest {
    fn new(msg: DialogTurn) -> Self {
        Self {
            message: msg,
            ..Default::default()
        }
    }

    fn get_message(&self) -> &DialogTurn {
        &self.message
    }

    fn set_message(&mut self, msg: DialogTurn) {
        self.message = msg;
    }
}

impl dialog::WithSimpleHyperParams for DialogRequest {
    fn shp_mut(&mut self) -> &mut SimpleDialogHyperParams {
        &mut self.simple
    }

    fn shp(&self) -> &SimpleDialogHyperParams {
        &self.simple
    }
}

impl DialogRequest {
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = Tool>) -> Self {
        self.tools = tools.into_iter().collect();
        self
    }

    pub fn get_tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn enable_thinking(mut self) -> Self {
        self.enable_thinking = true;
        self
    }

    pub fn disable_thinking(mut self) -> Self {
        self.enable_thinking = false;
        self
    }

    pub fn is_thinking(&self) -> bool {
        self.enable_thinking
    }
}
