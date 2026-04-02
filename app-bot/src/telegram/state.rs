use smol_str::SmolStr;

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    Authenticating,
    ChoosingSubscriptionKind,
    ChoseRss,
    GotRssUrl {
        url: SmolStr,
    },
    GotRssCondition {
        condition: SmolStr,
        url: SmolStr,
    },
    GotRssMockBrowserUa {
        mock: bool,
        condition: SmolStr,
        url: SmolStr,
    },
}
