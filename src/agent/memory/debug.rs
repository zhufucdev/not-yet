use std::collections::BinaryHeap;

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

    async fn iter_newest_first<'s>(
        &'s self,
    ) -> Result<impl Iterator<Item = impl AsRef<Decision<Self::Material>>>, Self::Error> {
        Ok(self.decisions.iter().rev())
    }

    async fn clear(&mut self) -> Result<(), Self::Error> {
        self.decisions.clear();
        Ok(())
    }
}
