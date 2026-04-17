use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use std::{
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use thiserror::Error;

use async_stream::try_stream;
use futures::Stream;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{agent::memory::decision::Decision, source::LlmComprehendable};

#[cfg(test)]
mod test;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "decision_mem")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub agent_id: Option<i32>,
    pub is_truthy: bool,
    pub time: DateTime<Utc>,
    pub material_id: i32,
    #[sea_orm(belongs_to, from = "material_id", to = "id")]
    pub material: HasOne<super::material::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

pub struct SqliteDecisionMemory<U> {
    db: DatabaseConnection,
    agent_id: Option<i32>,
    material_dir: PathBuf,
    _marker: PhantomData<U>,
}

impl<U: LlmComprehendable> SqliteDecisionMemory<U> {
    pub fn new(
        db: DatabaseConnection,
        working_dir: impl AsRef<Path>,
        agent_id: Option<i32>,
    ) -> Result<Self, CreateDecisionMemoryError> {
        if U::KIND.is_none() {
            return Err(CreateDecisionMemoryError::UnsupportedMaterialType);
        }
        let material_dir = working_dir.as_ref().join("material");
        fs::create_dir_all(&material_dir)?;

        Ok(Self {
            db,
            agent_id,
            _marker: PhantomData,
            material_dir,
        })
    }
}

impl<U> super::DecisionMemory for SqliteDecisionMemory<U>
where
    U: LlmComprehendable + Serialize + DeserializeOwned + Send + Sync,
{
    type Material = U;
    type Error = DecisionMemoryError;

    async fn push(&mut self, decision: Decision<Self::Material>) -> Result<(), Self::Error> {
        let material_binary = rmp_serde::to_vec(&decision.material)?;
        let shasum = Sha256::digest(&material_binary)
            .map(|b| format!("{:x}", b))
            .join("");
        let material_fp = self.material_dir.join(&shasum);
        futures::future::try_join(
            async || -> Result<(), Self::Error> {
                tokio::fs::write(material_fp, material_binary).await?;
                Ok(())
            }(),
            async || -> Result<(), Self::Error> {
                ActiveModel::builder()
                    .set_agent_id(self.agent_id)
                    .set_time(decision.time)
                    .set_is_truthy(decision.is_truthy)
                    .set_material(
                        super::material::ActiveModel::builder()
                            .set_kind(Self::Material::KIND.unwrap())
                            .set_shasum(shasum),
                    )
                    .insert(&self.db)
                    .await?;
                Ok(())
            }(),
        )
        .await?;
        Ok(())
    }

    fn iter_newest_first<'s>(
        &'s self,
    ) -> impl Stream<Item = Result<impl AsRef<Decision<Self::Material>>, Self::Error>> {
        let mut query = Entity::find().find_also_related(super::material::Entity);
        if let Some(agent_id) = self.agent_id {
            query = query.filter(Column::AgentId.eq(agent_id));
        }
        query = query
            .filter(super::material::Column::Kind.eq(Self::Material::KIND.unwrap()))
            .order_by_desc(Column::Time);
        try_stream! {
            for (decision, material) in query
                .all(&self.db)
                .await? {
                let fp = self.material_dir.join(&material.unwrap().shasum);
                let bin = tokio::fs::read(fp).await?;
                let material = rmp_serde::from_slice(&bin)?;
                yield Decision {
                    time: decision.time,
                    is_truthy: decision.is_truthy,
                    material
                };
            }
        }
    }

    async fn clear(&mut self) -> Result<(), Self::Error> {
        let ids = super::material::Entity::find_related()
            .filter(super::material::Column::Kind.eq(Self::Material::KIND.unwrap()))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|d| d.id);
        Entity::delete_many()
            .filter_by_ids(ids)
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

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
