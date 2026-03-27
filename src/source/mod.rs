use std::cmp::max;

use escaping::Escape;
use image::DynamicImage;
use llama_runner::ImageOrText;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub mod rss;

pub use rss::RssFeed;
use tracing::{Level, event};

use crate::agent::memory::sqlite::material;

pub trait LlmComprehendable {
    const KIND: Option<material::Kind> = None;
    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>>;
}

pub struct DefaultUpdate<'m> {
    pub title: String,
    images: Vec<&'m image::DynamicImage>,
    msg_json: String,
    pub type_: Option<SmolStr>,
}

#[derive(Debug, Clone)]
pub struct DefaultMetadata {
    pub name: String,
    pub type_: Option<SmolStr>,
    msg: String,
}

pub trait Feed<'s> {
    type Item: LlmComprehendable;
    type Metadata: LlmComprehendable;
    type Error: std::error::Error;

    async fn get_metadata(&'s self) -> Result<Self::Metadata, Self::Error>;
    async fn get_items(&'s self) -> Result<Vec<Self::Item>, Self::Error>;
}

impl<'m> DefaultUpdate<'m> {
    pub fn new(
        title: impl AsRef<str>,
        content: impl AsRef<[ImageOrText<'m>]>,
        type_: Option<SmolStr>,
    ) -> Self {
        #[derive(Serialize)]
        struct Body<'a> {
            title: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            content: Option<&'a str>,
            #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
            type_: Option<&'a str>,
        }
        let escape_im: Escape = Escape::new('&', &['&'], &[], None).unwrap();
        let text_content = content
            .as_ref()
            .iter()
            .map(|piece| match piece {
                ImageOrText::Text(text) => escape_im.escape(text).to_string(),
                ImageOrText::Image(_) => "&".to_string(),
            })
            .collect::<Vec<_>>()
            .join("");
        let images = content
            .as_ref()
            .iter()
            .filter_map(|piece| {
                if let ImageOrText::Image(im) = piece {
                    Some(*im)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let body = Body {
            title: title.as_ref(),
            content: if !text_content.is_empty() {
                Some(&text_content)
            } else {
                None
            },
            type_: type_.as_ref().map(|t| t.as_str()),
        };
        let msg_json = serde_json::to_string(&body).unwrap();

        Self {
            title: title.as_ref().to_string(),
            images,
            msg_json,
            type_,
        }
    }
}

impl<'a> LlmComprehendable for DefaultUpdate<'a> {
    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>> {
        let mut chunks = Vec::new();
        let mut left_delim = 0;
        for im_idx in 0..max(self.images.len(), 1) - 1 {
            if let Some(i) = self
                .msg_json
                .get(left_delim..self.msg_json.len())
                .unwrap()
                .find("&")
                && self
                    .msg_json
                    .bytes()
                    .nth(i + 1)
                    .map_or(false, |c| c != b'&')
            {
                chunks.push(ImageOrText::Text(
                    self.msg_json.get(left_delim..i + left_delim).unwrap(),
                ));
                chunks.push(ImageOrText::Image(self.images[im_idx]));
                left_delim += i + 1;
            } else {
                panic!(
                    "Failed to convert to LLM format: json is missing image components. This software is so broken"
                )
            }
        }
        chunks.push(ImageOrText::Text(
            self.msg_json.get(left_delim..self.msg_json.len()).unwrap(),
        ));
        return chunks;
    }
}

impl DefaultMetadata {
    pub fn new(name: String, type_: Option<SmolStr>) -> Self {
        let msg = if let Some(typ) = type_.clone() {
            format!("{} named \"{}\"", typ, name)
        } else {
            name.clone()
        };
        Self { name, type_, msg }
    }
}

impl LlmComprehendable for DefaultMetadata {
    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>> {
        vec![ImageOrText::Text(self.msg.as_str())]
    }
}

async fn get_url_as_llm_context<E>(
    url: &str,
    client: &reqwest::Client,
) -> Result<(Option<DynamicImage>, Option<String>), E>
where
    E: From<reqwest::Error> + From<image::ImageError>,
{
    let mut extra_image = None;
    let mut extra_text = None;

    let response = client.get(url).send().await?;
    if response.status().is_success()
        && let Some(content_type) = response.headers().get("content-type")
        && let Ok(content_type) = content_type.to_str()
    {
        event!(Level::INFO, "Got extra content, attaching to struct");
        event!(Level::DEBUG, "Content type {}", content_type);
        if content_type.starts_with("text/") {
            extra_text = Some(response.text().await?);
        } else if content_type.starts_with("image/") {
            extra_image = Some(image::load_from_memory(response.bytes().await?.as_ref())?);
        } else {
            event!(Level::WARN, "Unsupported content type, ignoring");
        }
    }
    Ok((extra_image, extra_text))
}
