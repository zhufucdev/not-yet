use std::path::PathBuf;

use thiserror::Error;
use tokio::sync::oneshot::error;

#[derive(Debug, Error)]
pub enum DecisionMemoryError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("serialization: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("deserialization: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    #[error("file IO: {0}")]
    FileIo(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum CreateDecisionMemoryError {
    #[error("unsupported material type")]
    UnsupportedMaterialType,
    #[error("file io: {0}")]
    FileIo(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
}
