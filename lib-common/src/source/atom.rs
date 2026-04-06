use std::{fmt::Display, hash::Hash, str::FromStr};

use futures::future;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use smol_str::ToSmolStr;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{Instrument, Level, debug_span, event};

use crate::{
    agent::memory::sqlite::material,
    llm::SharedImageOrText,
    source::{
        DefaultMetadata, Feed, LlmComprehendable,
        utils::{self, UrlContent},
    },
    update::Updatable,
};

pub struct AtomFeed {
    client: reqwest::Client,
    url: String,
    cache: RwLock<Option<(atom_syndication::Feed, Vec<AtomFeedItem>)>>,
    span: tracing::Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomFeedItem {
    title: String,
    json: String,
    extra: Vec<UrlContent>,
}

impl AtomFeed {
    pub fn new(
        url: impl Into<String>,
        extra_headers: Option<&HeaderMap>,
    ) -> Result<Self, reqwest::Error> {
        let mut headers = extra_headers.cloned().unwrap_or_else(|| HeaderMap::new());
        headers.append(
            reqwest::header::ACCEPT,
            "application/atom+xml".parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        let url = url.into();
        Ok(Self {
            url: url.clone(),
            client,
            cache: RwLock::new(None),
            span: debug_span!("atom_feed", url = ?url).clone(),
        })
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    async fn get_feed(&self) -> Result<atom_syndication::Feed, Error> {
        async {
            event!(Level::INFO, "fetching Atom feed");
            let resposne = self
                .client
                .get(&self.url)
                .send()
                .await
                .inspect_err(|err| event!(Level::ERROR, "fetch failed with {err}"))?
                .error_for_status()
                .inspect_err(|err| event!(Level::ERROR, "responded with failure status: {err}"))?;

            event!(
                Level::INFO,
                "got status {}, content type {}",
                resposne.status(),
                resposne
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .map(|h| h.to_str().unwrap_or_default())
                    .unwrap_or_default()
            );
            Ok(atom_syndication::Feed::from_str(
                resposne.text().await?.as_str(),
            )?)
        }
        .instrument(self.span.clone())
        .await
    }
}

impl Updatable for AtomFeed {
    type Item = AtomFeedItem;

    type Error = Error;

    async fn get_items(&self) -> Result<Vec<Self::Item>, Self::Error> {
        let (_, items) = self.cache.read().await.clone().expect("call update first");
        Ok(items)
    }

    async fn update(&self) -> Result<(), Self::Error> {
        let feed = self.get_feed().await?;
        let mut entries = feed.entries().iter().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.updated());
        let items = future::join_all(entries.iter().map(|entry| {
            async { AtomFeedItem::from_entry(entry, &self.client).await }
                .instrument(debug_span!("item_from_entry", entry = ?entry.id()))
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        *self.cache.write().await = Some((feed, items));
        Ok(())
    }
}

impl Feed for AtomFeed {
    type Metadata = DefaultMetadata;

    async fn get_metadata(&self) -> Result<Self::Metadata, <Self as Updatable>::Error> {
        let title = if let Some((cache, _)) = self.cache.read().await.as_ref() {
            cache.title().to_string()
        } else {
            self.get_feed().await?.title().to_string()
        };
        Ok(DefaultMetadata::new(title, Some("atom feed".into())))
    }
}

impl LlmComprehendable for AtomFeedItem {
    const KIND: Option<material::Kind> = Some(material::Kind::AtomItem);

    fn get_message(&self) -> Vec<SharedImageOrText> {
        let mut chunks = Vec::new();
        chunks.push(self.json.to_smolstr().into());
        chunks.extend(
            self.extra
                .iter()
                .map(|c| {
                    vec![
                        format!("Fetched content for \"{}\"", c.url()).into(),
                        c.into(),
                    ]
                })
                .flatten(),
        );
        chunks
    }
}

impl AtomFeedItem {
    pub async fn from_entry(
        entry: &atom_syndication::Entry,
        client: &reqwest::Client,
    ) -> Result<Self, Error> {
        let json = serde_json::to_string(&entry)?;
        let extra = if let Some(content_xml) = entry.content().and_then(|content| content.value())
            && let Ok(urls) =
                utils::extract_url_from_feed_item::<anyhow::Error>(content_xml, Some(1))
        {
            future::join_all(
                urls.into_iter()
                    .map(|url| utils::get_url_content::<anyhow::Error>(url, client)),
            )
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|o| o)
            .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            title: format!(
                "{} {}",
                entry.title().to_string(),
                entry
                    .links()
                    .iter()
                    .map(|link| link.href())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            json,
            extra,
        })
    }
}

impl Display for AtomFeedItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title)
    }
}

impl Hash for AtomFeedItem {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.title.hash(state); // effectively hashing name and url which is guid
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("parse feed: {0}")]
    Parse(#[from] atom_syndication::Error),
    #[error("serialization: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[cfg(test)]
mod test {
    use tracing_test::traced_test;

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_rss_feed_fetch_reddit() {
        let headers = HeaderMap::from_iter(vec![(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.4 Safari/605.1.15".parse().unwrap(),
        )]);
        let feed = AtomFeed::new("https://www.reddit.com/r/rust.rss", Some(&headers)).unwrap();
        let items = feed.get_items().await.unwrap();
        assert!(!items.is_empty());
        println!("{}", serde_json::to_string(items.last().unwrap()).unwrap());
        panic!();
    }
}
