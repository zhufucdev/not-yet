use sea_orm::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "broadcast_rss")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub key: String,
    pub title: String,
    pub description: String,
}

impl ActiveModelBehavior for ActiveModel {}
