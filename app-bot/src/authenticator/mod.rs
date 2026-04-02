pub mod sqlite;
pub mod whitelist;
pub mod priority;

pub trait Authenticator {
    type UserId;
    type Level;
    type Error;

    async fn get_access(
        &self,
        user_id: &Self::UserId,
    ) -> Result<Access<Self::Level>, Self::Error>;
    async fn grant(&self, user_id: Self::UserId, level: Self::Level) -> Result<(), Self::Error>;
}

pub enum Access<Data> {
    Granted(Data),
    Denied,
}
