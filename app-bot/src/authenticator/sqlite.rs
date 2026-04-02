use sea_orm::{DatabaseConnection, DbErr, EntityTrait};

use crate::{
    authenticator::{Access, Authenticator},
    db::user::{self, AccessLevel},
};

pub struct SqliteAuthenticator {
    db: DatabaseConnection,
}

impl SqliteAuthenticator {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

impl Authenticator for SqliteAuthenticator {
    type UserId = crate::UserId;
    type Level = AccessLevel;
    type Error = DbErr;

    async fn get_access(&self, user_id: &Self::UserId) -> Result<Access<Self::Level>, Self::Error> {
        if let Some(user) = user::Entity::find_by_id(user_id.clone())
            .one(&self.db)
            .await?
        {
            return Ok(Access::Granted(user.access_level));
        }
        return Ok(Access::Denied);
    }

    async fn grant(&self, user_id: Self::UserId, access: Self::Level) -> Result<(), Self::Error> {
        user::ActiveModel::builder()
            .set_id(user_id)
            .set_access_level(access)
            .insert(&self.db)
            .await?;
        Ok(())
    }
}
