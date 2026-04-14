use chrono::{DateTime, FixedOffset, Utc};
use futures::future;
use rss::Channel;
use serde::{Deserialize, Serialize};
use smol_str::{SmolStr, ToSmolStr};
use std::{fmt::Display, str::FromStr};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{Instrument, Level, event, info_span};

use reqwest::header::{HeaderMap, HeaderName};

use crate::{
    agent::memory::sqlite::material,
    llm::SharedImageOrText,
    source::{
        DefaultMetadata, Feed, LlmComprehendable,
        utils::{self, UrlContent},
    },
    update::Updatable,
};

pub struct RssFeed {
    url: String,
    client: reqwest::Client,
    cache: RwLock<Option<(Channel, Vec<LlmRssItem>)>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRssItem {
    title: String,
    item: rss::Item,
    extra: Vec<UrlContent>,
}

impl RssFeed {
    pub fn new(
        url: impl ToString,
        extra_headers: Option<&HeaderMap>,
    ) -> Result<Self, reqwest::Error> {
        let mut headers = extra_headers.cloned().unwrap_or_else(|| HeaderMap::new());
        headers.append(
            reqwest::header::ACCEPT,
            "application/rss+xml".parse().unwrap(),
        );
        let client = reqwest::ClientBuilder::new()
            .default_headers(headers)
            .build()?;
        Ok(Self {
            url: url.to_string(),
            client,
            cache: RwLock::new(None),
        })
    }

    async fn get_rss_channel(&self) -> Result<Channel, Error> {
        let span = info_span!("rss_feed.get_rss_channel");
        async move {
            event!(Level::INFO, "fetching URL {}", self.url);
            let resposne = self
                .client
                .get(&self.url)
                .send()
                .await
                .inspect(|v| event!(Level::DEBUG, "HTTP response: {v:?}"))
                .inspect_err(|err| event!(Level::ERROR, "IO failed: {err}"))?
                .error_for_status()
                .inspect_err(|err| event!(Level::ERROR, "status error: {err}"))?;
            event!(
                Level::INFO,
                "got status {}, content type {}",
                resposne.status(),
                resposne
                    .headers()
                    .get(HeaderName::from_static("content-type"))
                    .map(|h| h.to_str().unwrap_or_default())
                    .unwrap_or_default()
            );
            let channel = Channel::from_str(resposne.text().await?.as_ref())?;
            Ok(channel)
        }
        .instrument(span)
        .await
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    pub(crate) async fn update_from_channel(&self, channel: Channel) -> Result<(), Error> {
        let mut rss_items = channel.items().iter().collect::<Vec<_>>();
        rss_items.sort_by_key(|item| {
            item.pub_date()
                .and_then(|pd| DateTime::<FixedOffset>::parse_from_rfc2822(pd).ok())
                .unwrap_or(Utc::now().fixed_offset())
        });
        let items = futures::future::try_join_all(rss_items.into_iter().map(
            async |item| -> Result<_, Error> {
                LlmRssItem::from_item(item.clone(), &self.client).await
            },
        ))
        .await?;
        *self.cache.write().await = Some((channel, items));
        Ok(())
    }
}

impl Feed for RssFeed {
    type Metadata = DefaultMetadata;

    async fn get_metadata(&self) -> Result<Self::Metadata, Self::Error> {
        let title = if let Some((channel, _)) = self.cache.read().await.as_ref() {
            channel.title().to_string()
        } else {
            self.get_rss_channel().await?.title().to_string()
        };
        Ok(DefaultMetadata::new(title, Some("RSS feed".into())))
    }
}

impl Updatable for RssFeed {
    type Item = LlmRssItem;
    type Error = Error;

    async fn get_items(
        &self,
    ) -> Result<Vec<<Self as Updatable>::Item>, <Self as Updatable>::Error> {
        let (_, items) = self.cache.read().await.clone().expect("call update first");
        return Ok(items);
    }

    async fn update(&self) -> Result<(), Self::Error> {
        let channel = self.get_rss_channel().await?;
        self.update_from_channel(channel).await
    }
}

impl LlmRssItem {
    async fn from_item(item: rss::Item, client: &reqwest::Client) -> Result<Self, Error> {
        let span = info_span!("llm_rss_item.from_item");

        async move {
            let extra =
                if let Some(content) = item.content()
                    && let Ok(urls) =
                        utils::extract_url_from_feed_item::<anyhow::Error>(content, Some(1))
                {
                    future::join_all(urls.into_iter().map(async |url| {
                        utils::get_url_content::<anyhow::Error>(url, client).await
                    }))
                    .await
                    .into_iter()
                    .filter_map(|r| r.ok().flatten())
                    .collect()
                } else {
                    Vec::new()
                };

            Ok(LlmRssItem {
                title: format!(
                    "{} {}",
                    item.title()
                        .or(item.description())
                        .unwrap_or_default()
                        .to_string(),
                    item.link().unwrap_or_default()
                )
                .trim()
                .to_string(),
                item,
                extra,
            })
        }
        .instrument(span)
        .await
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn guid(&self) -> Option<&str> {
        self.item.guid().map(|g| g.value())
    }
}

impl LlmComprehendable for LlmRssItem {
    const KIND: Option<material::Kind> = Some(material::Kind::RssItem);

    fn get_message(&self) -> Vec<SharedImageOrText> {
        let mut chunks = Vec::new();
        let content = serde_json::to_string(&self.item)
            .map(SmolStr::from)
            .unwrap_or_else(|_| self.title().to_smolstr());
        chunks.push(content.into());
        chunks.extend(
            self.extra
                .iter()
                .map(|content| {
                    vec![
                        SharedImageOrText::Text(
                            format!("Fetched content for \"{}\"", content.url()).into(),
                        ),
                        content.into(),
                    ]
                })
                .flatten(),
        );
        chunks
    }
}

impl Display for LlmRssItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title())
    }
}

impl std::hash::Hash for LlmRssItem {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(guid) = &self.guid() {
            guid.hash(state);
        } else {
            // effectively hashing name and url which is not guid but good enough
            self.title().hash(state);
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("parsing response: {0}")]
    Parsing(#[from] rss::Error),
    #[error("invalid channel item: missing {missing}")]
    InvalidItem { missing: SmolStr },
    #[error("serializing: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("loading image: {0}")]
    LoadImage(#[from] image::ImageError),
}

#[cfg(test)]
mod test {
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_rss_feed_fetch_megaphone() {
        let feed =
            RssFeed::new("https://feeds.megaphone.fm/GLT1412515089".to_string(), None).unwrap();
        let items = feed.get_items().await.unwrap();
        assert!(!items.is_empty());
    }
}
