use std::str::FromStr;

use lib_common::source::RssFeed;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sea_orm::prelude::*;

use crate::db::error::ParseHeaderError;

use super::subscription;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
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

impl TryInto<RssFeed> for Model {
    type Error = ParseHeaderError;

    fn try_into(self) -> Result<RssFeed, Self::Error> {
        let mut extra_headers = HeaderMap::new();
        if self.browser_ua {
            extra_headers.insert(HeaderName::from_static("user-agent"), HeaderValue::from_static("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.4 Safari/605.1.15"));
        }
        if let Some(headers) = self
            .headers
            .map(|h| {
                h.split(';')
                    .map(|pair| {
                        pair.trim()
                            .split_once('=')
                            .ok_or(ParseHeaderError::FormatError)
                            .and_then(|(k, v)| -> Result<_, ParseHeaderError> {
                                Ok((
                                    HeaderName::from_str(k).map_err(|_| {
                                        ParseHeaderError::InvalidPair(k.to_string())
                                    })?,
                                    HeaderValue::from_str(v).map_err(|_| {
                                        ParseHeaderError::InvalidPair(k.to_string())
                                    })?,
                                ))
                            })
                    })
                    .collect::<Result<Vec<(HeaderName, HeaderValue)>, ParseHeaderError>>()
            })
            .transpose()?
        {
            extra_headers.extend(headers);
        }
        Ok(RssFeed::new(self.url, &Some(extra_headers))?)
    }
}
