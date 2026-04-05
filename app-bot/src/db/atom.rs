use lib_common::source::atom::AtomFeed;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sea_orm::prelude::*;
use tracing::{Level, event};

use crate::db::{error::ParseHeaderError, header};

use super::subscription;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "atom")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub url: String,
    #[sea_orm(default_value = "true")]
    pub browser_ua: bool,
    pub headers: Option<String>,
    #[sea_orm(unique)]
    pub subscription_id: i32,
    #[sea_orm(belongs_to, from = "subscription_id", to = "id")]
    pub subscription: HasOne<subscription::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

impl TryInto<AtomFeed> for Model {
    type Error = ParseHeaderError;

    fn try_into(self) -> Result<AtomFeed, Self::Error> {
        let mut extra_headers = HeaderMap::new();
        if self.browser_ua {
            extra_headers.insert(
                HeaderName::from_static("user-agent"),
                HeaderValue::from_static(header::SAFARI_UA),
            );
        }
        if let Some(headers) = self.headers.map(header::parse_headers).transpose()? {
            extra_headers.extend(headers);
        }
        event!(
            Level::DEBUG,
            "created Atom feed {:?} for subscription {}",
            self.url,
            self.subscription_id
        );
        Ok(AtomFeed::new(self.url, Some(&extra_headers))?)
    }
}
