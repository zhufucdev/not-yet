use std::sync::Arc;

pub mod timeout;
pub mod owned;

pub trait Model {
    type Runner;
    type Error;
    async fn get_runner(&self) -> Result<Arc<Self::Runner>, Self::Error>;
}
