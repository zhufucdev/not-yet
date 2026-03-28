use std::collections::BinaryHeap;

use async_stream::try_stream;
use futures::Stream;

use crate::{
    agent::memory::{Decision, DecisionMemory, TimeOrderedDecisionMemory},
    source::LlmComprehendable,
};

pub struct DebugDecisionMemory<U: LlmComprehendable> {
    decisions: BinaryHeap<TimeOrderedDecisionMemory<U>>,
}

impl<U: LlmComprehendable> DebugDecisionMemory<U> {
    pub fn new() -> Self {
        Self {
            decisions: BinaryHeap::new(),
        }
    }
}

impl<U: LlmComprehendable> DecisionMemory for DebugDecisionMemory<U> {
    type Material = U;
    type Error = ();

    async fn push(&mut self, decision: Decision<Self::Material>) -> Result<(), Self::Error> {
        self.decisions
            .push(TimeOrderedDecisionMemory::unique(decision));
        Ok(())
    }

    fn iter_newest_first<'s>(
        &'s self,
    ) -> impl Stream<Item = Result<impl AsRef<Decision<Self::Material>>, Self::Error>> {
        try_stream! {
            for decision in self.decisions.iter().rev() {
                yield decision;
            }
        }
    }

    async fn clear(&mut self) -> Result<(), Self::Error> {
        self.decisions.clear();
        Ok(())
    }
}
