use sea_orm::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "broadcast")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub subscription_id: i32,
    pub kind: Kind,
    #[cfg(feature = "serve-rss")]
    pub rss_key: Option<String>,
    #[cfg(feature = "serve-rss")]

    #[sea_orm(belongs_to, from = "subscription_id", to = "id")]
    pub subscription: HasOne<super::subscription::Entity>,

    #[cfg(feature = "serve-rss")]
    #[sea_orm(belongs_to, from = "rss_key", to = "key")]
    pub rss: Option<super::broadcast_rss::Entity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Kind {
    Rss = 0,
}

impl ActiveModelBehavior for ActiveModel {}
