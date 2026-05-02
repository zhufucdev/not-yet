use sea_orm::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "msgIdByDialogId")]
struct Model {
    #[sea_orm(primary_key)]
    pub dialog_id: String,
    pub msg_id: i32,
}

impl ActiveModelBehavior for ActiveModel {}
