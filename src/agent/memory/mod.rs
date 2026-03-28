use chrono::{DateTime, Utc};
use futures::Stream;
use uuid::Uuid;

use crate::source::LlmComprehendable;

#[cfg(test)]
pub mod debug;
pub mod sqlite;

pub trait DecisionMemory {
    type Material: LlmComprehendable;
    type Error;

    async fn push(&mut self, decision: Decision<Self::Material>) -> Result<(), Self::Error>;
    fn iter_newest_first<'s>(
        &'s self,
    ) -> impl Stream<Item = Result<impl AsRef<Decision<Self::Material>>, Self::Error>>;
    async fn clear(&mut self) -> Result<(), Self::Error>;
}

pub struct Decision<M: LlmComprehendable> {
    pub material: M,
    pub is_truthy: bool,
    pub time: DateTime<Utc>,
}

struct TimeOrderedDecisionMemory<U: LlmComprehendable>(Uuid, Decision<U>);

impl<U: LlmComprehendable> AsRef<Decision<U>> for Decision<U> {
    fn as_ref(&self) -> &Decision<U> {
        self
    }
}

impl<U: LlmComprehendable> AsRef<Decision<U>> for TimeOrderedDecisionMemory<U> {
    fn as_ref(&self) -> &Decision<U> {
        &self.1
    }
}

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
