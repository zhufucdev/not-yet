use std::sync::Arc;

use crate::error::NaE;

pub struct OwnedModel<Runner> {
    runner: Arc<Runner>,
}

impl<Runner> OwnedModel<Runner> {
    pub fn new(runner: Runner) -> Self {
        Self { runner: Arc::new(runner) }
    }
}

impl<Runner> super::Model for OwnedModel<Runner> {
    type Runner = Runner;

    type Error = NaE;

    async fn get_runner(&self) -> Result<Arc<Self::Runner>, Self::Error> {
        Ok(self.runner.clone())
    }
}
