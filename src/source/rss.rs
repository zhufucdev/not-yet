use async_trait::async_trait;
use image::DynamicImage;
use llama_runner::ImageOrText;
use rss::Channel;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use smol_str::SmolStr;
use std::str::FromStr;
use thiserror::Error;
use tracing::{Instrument, Level, event, info_span};

use reqwest::header::{HeaderMap, HeaderName};

use crate::{
    agent::memory::sqlite::material,
    serde_utils::DynImageConverter,
    source::{DefaultMetadata, Feed, LlmComprehendable, get_url_as_llm_context},
    update::Updatable,
};

pub struct RssFeed {
    url: String,
    client: reqwest::Client,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmRssItem {
    pub(crate) json: String,
    #[serde_as(as = "Option<DynImageConverter>")]
    extra_image: Option<DynamicImage>,
    extra_text: Option<String>,
}

impl std::hash::Hash for LlmRssItem {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.json.hash(state);
    }
}

impl RssFeed {
    pub fn new(
        url: impl ToString,
        extra_headers: &Option<HeaderMap>,
    ) -> Result<Self, reqwest::Error> {
        let mut headers = extra_headers.clone().unwrap_or_else(|| HeaderMap::new());
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
        })
    }

    async fn get_rss_channel(&self) -> Result<Channel, Error> {
        let span = info_span!("rss_feed.get_rss_channel");
        async move {
            event!(Level::INFO, "fetching URL {}", self.url);
            let resposne = self.client.get(&self.url).send().await?;
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
            let response = resposne.error_for_status_ref()?;
            Ok(Channel::from_str(resposne.text().await?.as_ref())?)
        }
        .instrument(span)
        .await
    }
}

impl Feed for RssFeed {
    type Metadata = DefaultMetadata;

    async fn get_metadata(&self) -> Result<Self::Metadata, Self::Error> {
        let channel = self.get_rss_channel().await?;
        Ok(DefaultMetadata::new(channel.title, Some("RSS feed".into())))
    }
}

#[async_trait]
impl Updatable for RssFeed {
    type Item = LlmRssItem;
    type Error = Error;

    async fn get_items(
        &self,
    ) -> Result<Vec<<Self as Updatable>::Item>, <Self as Updatable>::Error> {
        let channel = self.get_rss_channel().await?;

        futures::future::try_join_all(channel.items.into_iter().map(
            async |item| -> Result<Self::Item, Self::Error> {
                LlmRssItem::from_item(item, &self.client).await
            },
        ))
        .await
    }
}

impl LlmRssItem {
    async fn from_item(item: rss::Item, client: &reqwest::Client) -> Result<Self, Error> {
        let span = info_span!("llm_rss_item.from_item");
        let json = serde_json::to_string(&item)?;

        async move {
            let (extra_image, extra_text) = if let Some(content) = item.content() {
                if content.starts_with("http://") || content.starts_with("https://") {
                    event!(Level::DEBUG, "Getting content from url {}", content);
                    get_url_as_llm_context::<Error>(content, client).await?
                } else {
                    (None, Some(content.to_string()))
                }
            } else {
                (None, None)
            };

            Ok(LlmRssItem {
                json,
                extra_image,
                extra_text,
            })
        }
        .instrument(span)
        .await
    }
}

impl LlmComprehendable for LlmRssItem {
    const KIND: Option<material::Kind> = Some(material::Kind::RssItem);

    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>> {
        let mut chunks = Vec::new();
        chunks.push(ImageOrText::Text(self.json.as_ref()));
        if self.extra_text.is_some() || self.extra_image.is_some() {
            chunks.push(ImageOrText::Text("Fetched content:\n"));
        }
        if let Some(text) = self.extra_text.as_ref() {
            chunks.push(ImageOrText::Text(text));
        }
        if let Some(image) = self.extra_image.as_ref() {
            chunks.push(ImageOrText::Image(image));
        }
        chunks
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
        let feed = RssFeed::new(
            "https://feeds.megaphone.fm/GLT1412515089".to_string(),
            &None,
        )
        .unwrap();
        let items = feed.get_items().await.unwrap();
        assert!(!items.is_empty());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_rss_feed_fetch_reddit() {
        let headers = HeaderMap::from_iter(vec![(
            "user-agent".parse().unwrap(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.4 Safari/605.1.15".parse().unwrap(),
        )]);
        let feed = RssFeed::new("https://www.reddit.com/r/rust.rss", &Some(headers)).unwrap();
        let items = feed.get_items().await.unwrap();
        assert!(!items.is_empty());
    }
}
