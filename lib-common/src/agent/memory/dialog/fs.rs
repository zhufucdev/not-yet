use std::path::PathBuf;

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::fs;

pub struct FsDialogMemory<D> {
    file_path: PathBuf,
    _marker: std::marker::PhantomData<D>,
}

impl<D> FsDialogMemory<D> {
    pub fn new(working_dir: impl Into<PathBuf>, mem_id: impl AsRef<str>) -> Self {
        Self {
            file_path: working_dir.into().join("dialog").join(mem_id.as_ref()),
            _marker: Default::default(),
        }
    }
}

impl<D> super::DialogMemory for FsDialogMemory<D>
where
    D: Serialize + DeserializeOwned + Send + Sync,
{
    type Error = Error;

    type Dialog = D;

    async fn update(&mut self, dialog: &Self::Dialog) -> Result<(), Self::Error> {
        if let Some(parent) = self.file_path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).await?;
        }
        let binary = rmp_serde::to_vec(dialog)?;
        fs::write(&self.file_path, binary).await?;
        Ok(())
    }

    async fn get(&self) -> Result<Option<Self::Dialog>, Self::Error> {
        if !self.file_path.exists() {
            return Ok(None);
        }
        Ok(rmp_serde::from_slice(
            fs::read(&self.file_path).await?.as_slice(),
        )?)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO: {0}")]
    IO(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("deserialization: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
}
