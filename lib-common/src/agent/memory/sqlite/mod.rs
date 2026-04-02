use std::{
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};

use async_stream::try_stream;
use futures::Stream;
use sea_orm::{
    ColumnTrait, Database, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter, QueryOrder,
    Related,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    agent::memory::{
        Decision,
        sqlite::error::{CreateDecisionMemoryError, DecisionMemoryError},
    },
    source::LlmComprehendable,
};

pub mod decision;
pub mod error;
pub mod material;
#[cfg(test)]
mod test;

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
                decision::ActiveModel::builder()
                    .set_agent_id(self.agent_id)
                    .set_time(decision.time)
                    .set_is_truthy(decision.is_truthy)
                    .set_material(
                        material::ActiveModel::builder()
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
        let mut query = decision::Entity::find().find_also_related(material::Entity);
        if let Some(agent_id) = self.agent_id {
            query = query.filter(decision::Column::AgentId.eq(agent_id));
        }
        query = query
            .filter(material::Column::Kind.eq(Self::Material::KIND.unwrap()))
            .order_by_desc(decision::Column::Time);
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
        let ids = material::Entity::find_related()
            .filter(material::Column::Kind.eq(Self::Material::KIND.unwrap()))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|d| d.id);
        decision::Entity::delete_many()
            .filter_by_ids(ids)
            .exec(&self.db)
            .await?;
        Ok(())
    }
}
