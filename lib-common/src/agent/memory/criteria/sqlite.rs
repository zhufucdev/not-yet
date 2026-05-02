use chrono::Utc;
use sea_orm::entity::prelude::*;
use thiserror::Error;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "criteria")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub creation_time: DateTimeUtc,
    pub agent_id: Option<i32>,
    pub content: String,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "Entity")]
struct CriterionWithId {
    id: i32,
}

impl ActiveModelBehavior for ActiveModel {}

pub struct SqliteCriteriaMemory {
    db: DatabaseConnection,
    agent_id: Option<i32>,
}

impl SqliteCriteriaMemory {
    pub fn new(db: DatabaseConnection, agent_id: Option<i32>) -> Self {
        Self { db, agent_id }
    }
}

impl super::CriteriaMemory for SqliteCriteriaMemory {
    type Error = Error;

    async fn get(&self) -> Result<Vec<impl AsRef<str> + Send>, Self::Error> {
        let criteria = Entity::find()
            .filter(Column::AgentId.eq(self.agent_id))
            .order_by_id_asc()
            .all(&self.db)
            .await?;
        Ok(criteria.into_iter().map(|c| c.content).collect())
    }

    async fn add(&mut self, criteria: impl AsRef<str> + Send) -> Result<(), Self::Error> {
        ActiveModel::builder()
            .set_agent_id(self.agent_id.clone())
            .set_content(criteria.as_ref().to_string())
            .set_creation_time(Utc::now())
            .save(&self.db)
            .await?;
        Ok(())
    }

    async fn remove(&mut self, index: usize) -> Result<(), Self::Error> {
        let models = Entity::find()
            .filter(Column::AgentId.eq(self.agent_id))
            .order_by_id_asc()
            .into_partial_model()
            .all(&self.db)
            .await?;
        let Some(CriterionWithId { id }) = models.get(index) else {
            return Err(Error::IndexOutOfBounds);
        };
        Entity::delete_by_id(id.clone()).exec(&self.db).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("index out of bounds")]
    IndexOutOfBounds,
}
