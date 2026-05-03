use lib_common::agent::optimize::ApproveOrDeny;
use smol_str::SmolStr;
use teloxide::types::MessageId;
use tokio::sync::mpsc;

use crate::{db::subscription, telegram::clarify};

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
        clareq: clarify::TgClarReqHandler,
    },
    ReviewingOptimization {
        clareq: clarify::TgClarReqHandler,
        approve: mpsc::Sender<ApproveOrDeny>,
        prompt: MessageId,
    },
}
