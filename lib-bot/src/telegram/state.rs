#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    Authenticating,
    ChoosingSubscriptionKind,
    ChoseRss,
}
