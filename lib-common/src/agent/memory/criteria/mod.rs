pub mod debug;
pub mod sqlite;

#[trait_variant::make(Send)]
pub trait CriteriaMemory {
    type Error;

    async fn get(&self) -> Result<Vec<impl AsRef<str> + Send>, Self::Error>;
    async fn add(&mut self, criteria: impl AsRef<str> + Send) -> Result<(), Self::Error>;
    async fn remove(&mut self, index: usize) -> Result<(), Self::Error>;
    async fn is_empty(&self) -> bool;
}
