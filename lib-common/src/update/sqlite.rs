use std::{
    hash::{DefaultHasher, Hash, Hasher},
    marker::PhantomData,
};

use async_trait::async_trait;
use sea_orm::{prelude::*, sea_query};
use tracing::{Level, event};

use crate::{source::LlmComprehendable, update::UpdatePersistence};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "update")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub key: String,
    pub hash: i64,
}

impl ActiveModelBehavior for ActiveModel {}

#[derive(Debug)]
pub struct SqliteUpdatePersistence<M>
where
    M: LlmComprehendable + Hash + Send + 'static,
{
    db: DatabaseConnection,
    key: String,
    marker: PhantomData<M>,
}

impl<M> SqliteUpdatePersistence<M>
where
    M: LlmComprehendable + Hash + Send + 'static,
{
    pub fn new(db: DatabaseConnection, key: impl ToString) -> Result<Self, sea_orm::DbErr> {
        let key = key.to_string();
        Ok(Self {
            db,
            key,
            marker: PhantomData,
        })
    }
}

impl<M> UpdatePersistence for SqliteUpdatePersistence<M>
where
    M: LlmComprehendable + Hash + Unpin + Send + Sync + 'static,
{
    type Item = M;

    type Error = sea_orm::DbErr;

    async fn update(&self, item: Option<&Self::Item>) -> Result<(), Self::Error> {
        let mut hasher = DefaultHasher::new();
        item.hash(&mut hasher);
        let hash = hasher.finish();

        let key = self.key.clone();
        Entity::insert(ActiveModel::builder().set_hash(hash as i64).set_key(key))
            .on_conflict(
                sea_query::OnConflict::column(Column::Key)
                    .update_column(Column::Hash)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn cmp(&self, current: Option<&Self::Item>) -> Result<bool, Self::Error> {
        let mut hasher = DefaultHasher::new();
        current.hash(&mut hasher);
        let hash = hasher.finish();
        let record = Entity::find_by_key(&self.key).one(&self.db).await?;
        if current.is_none() {
            Ok(record.is_none())
        } else if record.is_none() {
            return Ok(false);
        } else {
            Ok(record.unwrap().hash as u64 == hash)
        }
    }
}

impl<M> Clone for SqliteUpdatePersistence<M>
where
    M: LlmComprehendable + Hash + Send,
{
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            key: self.key.clone(),
            marker: self.marker.clone(),
        }
    }
}
