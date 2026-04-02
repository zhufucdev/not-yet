use thiserror::Error;

use crate::authenticator::{Access, Authenticator};

pub struct WhitelistAuthenticator<UserId, Level> {
    accept: Vec<UserId>,
    level: Level,
}

impl<UserId: Clone, Level> WhitelistAuthenticator<UserId, Level> {
    pub fn new(accept: impl AsRef<[UserId]>, level: Level) -> Self {
        Self {
            accept: accept.as_ref().to_vec(),
            level,
        }
    }
}

impl<UserId, Level> Authenticator for WhitelistAuthenticator<UserId, Level>
where
    UserId: Clone + PartialEq + Eq + Send + Sync,
    Level: Clone + PartialEq + Eq + Send + Sync,
{
    type UserId = UserId;

    type Level = Level;

    type Error = DenyError;

    async fn get_access(&self, user_id: &Self::UserId) -> Result<Access<Self::Level>, Self::Error> {
        if self.accept.contains(user_id) {
            return Ok(Access::Granted(self.level.clone()));
        }
        Ok(Access::Denied)
    }

    async fn grant(&self, user_id: Self::UserId, level: Self::Level) -> Result<(), Self::Error> {
        if level == self.level && self.accept.contains(&user_id) {
            return Ok(());
        }
        return Err(DenyError);
    }
}

#[derive(Debug, Clone, Error)]
#[error("access denied")]
pub struct DenyError;
