use std::{marker::PhantomData, path::Path};

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Database, DatabaseConnection, EntityOrSelect,
    EntityTrait, QueryFilter, QueryOrder, Related,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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

pub struct SqliteDecisionMemory<U: LlmComprehendable> {
    db: DatabaseConnection,
    _marker: PhantomData<U>,
}

impl<U: LlmComprehendable> SqliteDecisionMemory<U> {
    pub async fn new(working_dir: impl AsRef<Path>) -> Result<Self, CreateDecisionMemoryError> {
        if U::KIND.is_none() {
            return Err(CreateDecisionMemoryError::UnsupportedMaterialType);
        }
        let fp = working_dir.as_ref().join("dmem.db");
        let Some(fps) = fp.to_str() else {
            return Err(CreateDecisionMemoryError::InvalidWorkingDir(
                working_dir.as_ref().to_path_buf(),
            ));
        };
        Ok(Self {
            db: Database::connect(format!("sqlite://{}?mode=rwc", fps)).await?,
            _marker: PhantomData,
        })
    }
}

impl<U> super::DecisionMemory for SqliteDecisionMemory<U>
where
    U: LlmComprehendable + Serialize + DeserializeOwned,
{
    type Material = U;
    type Error = DecisionMemoryError;

    async fn push(&mut self, decision: Decision<Self::Material>) -> Result<(), Self::Error> {
        let material_binary = rmp_serde::to_vec(&decision.material)?;
        let shasum = format!("{:x?}", Sha256::digest(material_binary));

        decision::ActiveModel::builder()
            .set_time(decision.time)
            .set_is_truthy(decision.is_truthy)
            .set_material(
                material::ActiveModel::builder()
                    .set_kind(Self::Material::KIND.unwrap())
                    .set_shasum(shasum),
            )
            .save(&self.db)
            .await?;
        Ok(())
    }

    async fn iter_newest_first<'s>(
        &'s self,
    ) -> Result<impl Iterator<Item = impl AsRef<Decision<Self::Material>>>, Self::Error> {
        Ok(decision::Entity::find()
            .find_also_related(material::Entity)
            .filter(material::Column::Kind.eq(Self::Material::KIND.unwrap()))
            .order_by_desc(decision::Column::Time)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|item| item.into_decision()))
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

trait IntoDecision<U: LlmComprehendable> {
    fn into_decision(self) -> Decision<U>;
}

impl<U> IntoDecision<U> for (decision::Model, Option<material::Model>)
where
    U: LlmComprehendable + DeserializeOwned,
{
    fn into_decision(self) -> Decision<U> {
        todo!()
    }
}
