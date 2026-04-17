pub mod debug;
pub mod sqlite;

#[trait_variant::make(Send)]
pub trait DialogMemory {
    type Error;
    type Dialog;

    async fn update(&mut self, dialog: &Self::Dialog) -> Result<(), Self::Error>;
    async fn get(&self) -> Result<Option<Self::Dialog>, Self::Error>;
}
