use lib_common::source::RssFeed;
use sea_orm::prelude::*;

use super::subscription;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub url: String,
    pub headers: Option<String>,
    #[sea_orm(unique)]
    pub subscription_id: i32,
    #[sea_orm(belongs_to, from = "subscription_id", to = "id")]
    pub subscription: HasOne<subscription::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

impl TryInto<RssFeed> for Model {
    type Error = reqwest::Error;

    fn try_into(self) -> Result<RssFeed, Self::Error> {
        RssFeed::new(self.url, extra_headers)
    }
}
