mod authenticator;
mod config;
mod db;
mod rss;
mod token;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "telegram")]
pub use telegram as flavor;

pub use flavor::UserId;

pub async fn main() -> anyhow::Result<()> {
    #[cfg(feature = "telegram")]
    flavor::main().await
}
