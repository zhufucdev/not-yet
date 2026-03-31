use serde::Deserialize;

use crate::UserId;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub bot_token: Option<String>,

    /// Whitelisted user ids, who skip token authentication
    pub whitelist: Vec<UserId>,
}
