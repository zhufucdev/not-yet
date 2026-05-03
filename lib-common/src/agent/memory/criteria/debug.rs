use crate::error::NaE;

pub struct DebugCriteriaMemory {
    criteria: Vec<String>,
}

impl DebugCriteriaMemory {
    pub fn new() -> Self {
        Self {
            criteria: Vec::new(),
        }
    }
}

impl Default for DebugCriteriaMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl super::CriteriaMemory for DebugCriteriaMemory {
    type Error = NaE;

    async fn get(&self) -> Result<Vec<impl AsRef<str> + Send>, Self::Error> {
        Ok(self.criteria.iter().map(|c| c.as_str()).collect())
    }

    async fn add(&mut self, criteria: impl AsRef<str> + Send) -> Result<(), Self::Error> {
        self.criteria.push(criteria.as_ref().to_string());
        Ok(())
    }

    async fn remove(&mut self, index: usize) -> Result<(), Self::Error> {
        self.criteria.remove(index);
        Ok(())
    }
}
