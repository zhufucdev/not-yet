use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};

use chrono::Utc;
use sea_orm::entity::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::fs;
use tracing::{Instrument, Level, event, info_span};

use crate::agent::memory::dialog::DialogMemory;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "dialog")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub creation_time: DateTimeUtc,
    pub agent_id: Option<i32>,
}

impl ActiveModelBehavior for ActiveModel {}

pub struct SqliteDialogMemory<D> {
    db: DatabaseConnection,
    mem_id: Option<i32>,
    agent_id: Option<i32>,
    data_dir: PathBuf,
    _marker: PhantomData<D>,
}

impl<D> SqliteDialogMemory<D> {
    pub fn new(
        db: DatabaseConnection,
        agent_id: Option<i32>,
        working_dir: impl AsRef<Path>,
    ) -> Self {
        Self {
            db,
            mem_id: None,
            agent_id,
            data_dir: working_dir.as_ref().join("dialog"),
            _marker: PhantomData,
        }
    }

    pub fn existing(db: DatabaseConnection, mem_id: i32, working_dir: impl AsRef<Path>) -> Self {
        Self {
            db,
            mem_id: Some(mem_id),
            agent_id: None,
            data_dir: working_dir.as_ref().join("dialog"),
            _marker: PhantomData,
        }
    }
}

impl<D> DialogMemory for SqliteDialogMemory<D>
where
    D: Serialize + DeserializeOwned + Send + Sync,
{
    type Error = Error;
    type Dialog = D;

    async fn update(&mut self, dialog: &Self::Dialog) -> Result<(), Self::Error> {
        let old_mem_id = self.mem_id.clone();
        async {
            if !fs::try_exists(&self.data_dir).await? {
                event!(Level::INFO, "creating data dir at {:?}", self.data_dir);
                fs::create_dir_all(&self.data_dir)
                    .await
                    .inspect_err(|err| event!(Level::ERROR, "unable to create data dir: {err}"))?;
            }

            if let Some(mem_id) = self.mem_id {
                Entity::find_by_id(mem_id)
                    .one(&self.db)
                    .await?
                    .ok_or(Error::NotFound(mem_id))?;
                event!(Level::DEBUG, "found db record");
                let binary = rmp_serde::to_vec(dialog)?;
                fs::write(self.data_dir.join(mem_id.to_string()), &binary).await?;
                event!(Level::INFO, "updated file at {:?}", self.data_dir);
            } else {
                let id = ActiveModel::builder()
                    .set_agent_id(self.agent_id.clone())
                    .set_creation_time(Utc::now())
                    .save(&self.db)
                    .await?
                    .id
                    .unwrap();
                event!(Level::DEBUG, "new record id: {id}");
                let binary = rmp_serde::to_vec(dialog)?;
                fs::write(self.data_dir.join(id.to_string()), &binary).await?;
                event!(Level::INFO, "updated file at {:?}", self.data_dir);
                self.mem_id = Some(id);
            }
            Ok(())
        }
        .instrument(
            info_span!("sqlite_dialog_mem.update", mem_id = ?old_mem_id, agent_id = ?self.agent_id),
        )
        .await
    }

    async fn get(&self) -> Result<Option<Self::Dialog>, Self::Error> {
        async {
            let Some(mem_id) = self.mem_id else {
                return Ok(None);
            };

            {
                let mut query = Entity::find_by_id(mem_id);
                if let Some(agent_id) = self.agent_id {
                    query = query.filter(Column::AgentId.eq(agent_id));
                }
                let count = query.count(&self.db).await?;
                if count <= 0 {
                    return Err(Error::NotFound(mem_id));
                }
            };

            let binary = tokio::fs::read(self.data_dir.join(mem_id.to_string())).await?;
            Ok(Some(rmp_serde::from_slice(&binary)?))
        }
        .instrument(
            info_span!("sqlite_dialog_mem.get", mem_id = ?self.mem_id, agent_id = ?self.agent_id),
        )
        .await
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("db: {0}")]
    DbErr(#[from] sea_orm::DbErr),
    #[error("serialization: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("deserialization: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    #[error("file not found for mem id {0}")]
    NotFound(i32),
    #[error("file IO: {0}")]
    Io(#[from] std::io::Error),
}
