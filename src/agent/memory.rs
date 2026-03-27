#[cfg(test)]
use std::collections::BinaryHeap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::source::LlmComprehendable;

pub trait DecisionMemory {
    type Update: LlmComprehendable;
    fn push(&mut self, decision: Decision<Self::Update>);
    fn iter_newest_first<'s>(&'s self) -> impl Iterator<Item = &'s Decision<Self::Update>>;
    fn clear(&mut self);
}

pub struct Decision<U: LlmComprehendable> {
    pub material: U,
    pub is_truthy: bool,
    pub time: DateTime<Utc>,
}

struct TimeOrderedDecisionMemory<U: LlmComprehendable>(Uuid, Decision<U>);

impl<U: LlmComprehendable> TimeOrderedDecisionMemory<U> {
    pub fn unique(decision: Decision<U>) -> Self {
        Self(Uuid::new_v4(), decision)
    }
}
impl<U: LlmComprehendable> Into<Decision<U>> for TimeOrderedDecisionMemory<U> {
    fn into(self) -> Decision<U> {
        self.1
    }
}
impl<U: LlmComprehendable> AsRef<Decision<U>> for TimeOrderedDecisionMemory<U> {
    fn as_ref(&self) -> &Decision<U> {
        &self.1
    }
}

impl<U: LlmComprehendable> PartialEq for TimeOrderedDecisionMemory<U> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<U: LlmComprehendable> Eq for TimeOrderedDecisionMemory<U> {}

impl<U: LlmComprehendable> PartialOrd for TimeOrderedDecisionMemory<U> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.1.time.partial_cmp(&other.1.time)
    }
}

impl<U: LlmComprehendable> Ord for TimeOrderedDecisionMemory<U> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.1.time.cmp(&other.1.time)
    }
}

#[cfg(test)]
pub struct DebugMemory<U: LlmComprehendable> {
    decisions: BinaryHeap<TimeOrderedDecisionMemory<U>>,
}

#[cfg(test)]
impl<U: LlmComprehendable> DebugMemory<U> {
    pub fn new() -> Self {
        Self {
            decisions: BinaryHeap::new(),
        }
    }
}

#[cfg(test)]
impl<U: LlmComprehendable> DecisionMemory for DebugMemory<U> {
    type Update = U;

    fn push(&mut self, decision: Decision<Self::Update>) {
        self.decisions
            .push(TimeOrderedDecisionMemory::unique(decision));
    }

    fn iter_newest_first<'s>(&'s self) -> impl Iterator<Item = &'s Decision<Self::Update>> {
        self.decisions.iter().rev().map(|d| d.as_ref())
    }

    fn clear(&mut self) {
        self.decisions.clear();
    }
}
