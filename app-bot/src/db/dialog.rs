use super::subscription;
use sea_orm::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "msg_id_by_dialog_id")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub dialog_id: String,
    pub msg_id: i32,
    pub subscription_id: i32,
    #[sea_orm(belongs_to, from = "subscription_id", to = "id")]
    pub subscription: HasOne<subscription::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
