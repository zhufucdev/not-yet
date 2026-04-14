use std::{cell::RefCell, sync::Arc};

use llama_runner::{
    mcp::model::Tool,
    sample::{LlguidanceSamplingParams, SimpleSamplingParams},
};
use tokio::sync::RwLock;

pub mod gemma4;

pub struct MultiTurnDialog<Turn, History> {
    turns: Vec<Turn>,
    history: Arc<RefCell<History>>,
}

#[derive(Debug, Clone)]
pub struct DialogRequest<M> {
    pub message: M,
    pub sampling: SimpleSamplingParams,
    pub llguidance: Option<LlguidanceSamplingParams>,
    pub max_seq: usize,
    pub prefill: Option<String>,
    pub tools: Vec<Tool>,
}

pub trait MultiTurnDialogEnabled<'d, Tmpl> {
    type Error;
    type Turn;
    type Response;
    type History;

    async fn get_dialog_continued(
        self: Arc<Self>,
        req: &'d DialogRequest<Self::Turn>,
        dialog: &'d mut MultiTurnDialog<Self::Turn, Self::History>,
    ) -> Result<Self::Response, Self::Error>;
}

impl<Turn, History: Default> MultiTurnDialog<Turn, History> {
    pub fn new() -> Self {
        Self {
            turns: vec![],
            history: Default::default(),
        }
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn history(&self) -> Arc<RefCell<History>> {
        self.history.clone()
    }
}

impl<Turn, History: Default> Default for MultiTurnDialog<Turn, History> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Default> Default for DialogRequest<M> {
    fn default() -> Self {
        Self {
            message: Default::default(),
            sampling: Default::default(),
            llguidance: None,
            max_seq: usize::MAX,
            prefill: None,
            tools: vec![],
        }
    }
}
