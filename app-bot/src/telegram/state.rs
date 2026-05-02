use lib_common::agent::optimize::{Optimizer, gemma4::Gemma4Optimizer};
use smol_str::SmolStr;

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
        optimizer: Gemma4Optimizer<>
    },
}
