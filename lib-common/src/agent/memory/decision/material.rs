use sea_orm::entity::prelude::*;


#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "material")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub kind: Kind,
    pub shasum: String,
    #[sea_orm(has_many)]
    pub decisions: HasMany<super::sqlite::Entity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "u32", db_type = "Integer")]
pub enum Kind {
    #[sea_orm(num_value = 0)]
    RssItem,
    #[sea_orm(num_value = 1)]
    AtomItem,
}

impl ActiveModelBehavior for ActiveModel {}
