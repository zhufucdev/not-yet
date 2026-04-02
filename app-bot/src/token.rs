use std::{fmt::Display, sync::Arc};

use smol_str::SmolStr;
use tokio::sync::RwLock;

use lib_common::secure;

#[derive(Debug, Clone)]
pub struct OnetimeToken {
    value: Arc<RwLock<SmolStr>>,
}

impl OnetimeToken {
    pub fn new() -> Self {
        Self {
            value: Arc::new(RwLock::new(secure::generate_random_id(32).into())),
        }
    }

    pub async fn test(&self, token: impl AsRef<str>) -> bool {
        token.as_ref() == self.value.read().await.as_str()
    }

    pub async fn rotate(&self) {
        *self.value.write().await = secure::generate_random_id(32).into();
    }

    pub async fn value(&self) -> SmolStr {
        self.value.read().await.clone()
    }
}
