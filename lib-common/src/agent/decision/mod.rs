pub mod llm;

pub trait Decider {
    type Material;
    type Error;
    async fn get_truth_value(&self, update: &Self::Material) -> Result<bool, Self::Error>;
}
