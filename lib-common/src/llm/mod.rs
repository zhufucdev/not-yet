use std::sync::Arc;

use smol_str::{SmolStr, ToSmolStr};

pub mod owned;
pub mod timeout;

#[derive(Clone)]
pub enum SharedImageOrText {
    Image(Arc<image::DynamicImage>),
    Text(SmolStr),
}

#[trait_variant::make(Send)]
pub trait Model {
    type Runner;
    type Error;
    async fn get_runner(&self) -> Result<Arc<Self::Runner>, Self::Error>;
}

impl From<SmolStr> for SharedImageOrText {
    fn from(value: SmolStr) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for SharedImageOrText {
    fn from(value: &str) -> Self {
        Self::Text(value.to_smolstr())
    }
}

impl From<String> for SharedImageOrText {
    fn from(value: String) -> Self {
        Self::Text(value.to_smolstr())
    }
}

impl From<&String> for SharedImageOrText {
    fn from(value: &String) -> Self {
        Self::Text(value.to_smolstr())
    }
}

impl From<image::DynamicImage> for SharedImageOrText {
    fn from(value: image::DynamicImage) -> Self {
        Self::Image(Arc::new(value))
    }
}

impl From<Arc<image::DynamicImage>> for SharedImageOrText {
    fn from(value: Arc<image::DynamicImage>) -> Self {
        Self::Image(value)
    }
}
