use std::sync::Arc;

use lib_common::agent::optimize::ApproveOrDeny;
use smol_str::SmolStr;
use teloxide::types::MessageId;
use tokio::sync::{RwLock, mpsc};

use crate::db::subscription;

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    Authenticating,
    ChoosingSubscriptionKind,
    ChoseFeed {
        kind: subscription::Kind,
    },
    GotFeedUrl {
        url: SmolStr,
        kind: subscription::Kind,
    },
    GotFeedCondition {
        condition: SmolStr,
        url: SmolStr,
        kind: subscription::Kind,
    },
    GotFeedMockBrowserUa {
        mock: bool,
        condition: SmolStr,
        url: SmolStr,
        kind: subscription::Kind,
    },
    Feedingback {
        tasks: Arc<RwLock<Vec<OptimizationTask>>>,
    },
}

#[derive(Clone)]
pub struct OptimizationTask {
    pub prompt: MessageId,
    pub assignment: LlmAssignment,
}

#[derive(Clone)]
pub enum LlmAssignment {
    Review {
        approve: mpsc::Sender<ApproveOrDeny>,
    },
    Clarify {
        send: mpsc::Sender<Option<String>>,
    },
}

pub trait StateFeedback {
    async fn with_task_queued(self, queue: impl IntoIterator<Item = OptimizationTask>) -> Self;
}

impl StateFeedback for State {
    async fn with_task_queued(self, queue: impl IntoIterator<Item = OptimizationTask>) -> Self {
        match &self {
            State::Feedingback { tasks } => {
                tasks.write().await.extend(queue);
                self
            }
            _ => State::Feedingback {
                tasks: Default::default(),
            },
        }
    }
}
