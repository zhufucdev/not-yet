use serde::Deserialize;

use crate::UserId;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub bot_token: Option<String>,

    /// Whitelisted user ids, who skip token authentication
    pub whitelist: Option<Vec<UserId>>,

    #[cfg(feature = "serve-rss")]
    pub serve_rss: Option<ServeRssConfig>,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ServeRssConfig {
    pub bind: String,
    pub host: Option<String>,
}
