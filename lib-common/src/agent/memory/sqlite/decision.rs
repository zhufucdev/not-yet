use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

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
