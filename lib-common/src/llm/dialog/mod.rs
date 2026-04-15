use std::{cell::RefCell, sync::Arc};

use llama_runner::sample::{LlguidanceSamplingParams, SimpleSamplingParams};

pub mod gemma4;
mod parse;

pub struct MultiTurnDialog<Turn, History> {
    turns: Vec<Turn>,
    history: Arc<RefCell<History>>,
}

pub trait DialogRequest<Msg> {
    fn new(msg: Msg) -> Self;
    fn get_message(&self) -> &Msg;
    fn set_message(&mut self, msg: Msg);
}

pub trait WithSampling
where
    Self: Sized,
{
    #[allow(unused_mut, unused_variables)]
    fn with_sampling(mut self, sampling: SimpleSamplingParams) -> Self {
        todo!()
    }
    fn get_sampling(&self) -> &SimpleSamplingParams;
}

pub trait WithLlguidance
where
    Self: Sized,
{
    #[allow(unused_mut, unused_variables)]
    fn with_llguidance(mut self, llguidance: LlguidanceSamplingParams) -> Self {
        todo!()
    }
    fn get_llguidance(&self) -> Option<&LlguidanceSamplingParams>;
}

pub trait WithMaxSeq
where
    Self: Sized,
{
    #[allow(unused_mut, unused_variables)]
    fn with_max_seq(mut self, max_seq: usize) -> Self {
        todo!()
    }
    fn get_max_seq(&self) -> usize;
}

pub trait WithPrefill
where
    Self: Sized,
{
    #[allow(unused_mut, unused_variables)]
    fn with_prefill(mut self, prefill: String) -> Self {
        todo!()
    }
    fn get_prefill(&self) -> Option<&String>;
}

pub trait WithSimpleHyperParams {
    fn shp_mut(&mut self) -> &mut SimpleDialogHyperParams;
    fn shp(&self) -> &SimpleDialogHyperParams;
}

#[derive(Debug, Clone, Default)]
pub struct SimpleDialogHyperParams {
    pub sampling: SimpleSamplingParams,
    pub llguidance: Option<LlguidanceSamplingParams>,
    pub max_seq: usize,
    pub prefill: Option<String>,
}

pub trait MultiTurnDialogEnabled<'d, Tmpl> {
    type Error;
    type Turn;
    type Response;
    type History;
    type Request;

    #[allow(async_fn_in_trait)]
    async fn get_dialog_continued(
        self: &Arc<Self>,
        req: &'d Self::Request,
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

impl<S: WithSimpleHyperParams + Sized> WithLlguidance for S {
    fn with_llguidance(mut self, llguidance: LlguidanceSamplingParams) -> Self {
        self.shp_mut().llguidance = Some(llguidance);
        self
    }

    fn get_llguidance(&self) -> Option<&LlguidanceSamplingParams> {
        self.shp().llguidance.as_ref()
    }
}

impl<S: WithSimpleHyperParams + Sized> WithMaxSeq for S {
    fn with_max_seq(mut self, max_seq: usize) -> Self {
        self.shp_mut().max_seq = max_seq;
        self
    }

    fn get_max_seq(&self) -> usize {
        self.shp().max_seq
    }
}

impl<S: WithSimpleHyperParams + Sized> WithPrefill for S {
    fn with_prefill(mut self, prefill: String) -> Self {
        self.shp_mut().prefill = Some(prefill);
        self
    }

    fn get_prefill(&self) -> Option<&String> {
        self.shp().prefill.as_ref()
    }
}

impl<S: WithSimpleHyperParams> WithSampling for S {
    fn with_sampling(mut self, sampling: SimpleSamplingParams) -> Self {
        self.shp_mut().sampling = sampling;
        self
    }

    fn get_sampling(&self) -> &SimpleSamplingParams {
        &self.shp().sampling
    }
}
