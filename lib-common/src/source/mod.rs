use std::{cmp::max, sync::Arc};

use escaping::Escape;
use serde::Serialize;
use smol_str::SmolStr;

pub mod atom;
pub mod rss;
pub mod utils;

pub use rss::{LlmRssItem, RssFeed};

use crate::{agent::memory::decision::material, llm::SharedImageOrText, update::Updatable};

pub trait LlmComprehendable {
    const KIND: Option<material::Kind> = None;
    fn get_message(&self) -> Vec<SharedImageOrText>;
}

#[derive(Debug, Clone)]
pub struct DefaultUpdate {
    pub title: String,
    images: Vec<Arc<image::DynamicImage>>,
    msg_json: String,
    pub type_: Option<SmolStr>,
}

#[derive(Debug, Clone)]
pub struct DefaultMetadata {
    pub name: String,
    pub type_: Option<SmolStr>,
}

#[trait_variant::make(Send)]
pub trait Feed: Updatable {
    type Metadata: LlmComprehendable;

    async fn get_metadata(&self) -> Result<Self::Metadata, <Self as Updatable>::Error>;
}

impl DefaultUpdate {
    pub fn new(
        title: impl AsRef<str>,
        content: impl AsRef<[SharedImageOrText]>,
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
                SharedImageOrText::Text(text) => escape_im.escape(text).to_string(),
                SharedImageOrText::Image(_) => "&".to_string(),
            })
            .collect::<Vec<_>>()
            .join("");
        let images = content
            .as_ref()
            .iter()
            .filter_map(|piece| {
                if let SharedImageOrText::Image(im) = piece {
                    Some(im.clone())
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

impl LlmComprehendable for DefaultUpdate {
    fn get_message(&self) -> Vec<SharedImageOrText> {
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
                chunks.push(
                    self.msg_json
                        .get(left_delim..i + left_delim)
                        .unwrap()
                        .into(),
                );
                chunks.push(self.images[im_idx].clone().into());
                left_delim += i + 1;
            } else {
                panic!(
                    "Failed to convert to LLM format: json is missing image components. This software is so broken"
                )
            }
        }
        chunks.push(
            self.msg_json
                .get(left_delim..self.msg_json.len())
                .unwrap()
                .into(),
        );
        return chunks;
    }
}

impl DefaultMetadata {
    pub fn new(name: String, type_: Option<SmolStr>) -> Self {
        Self { name, type_ }
    }
}

impl LlmComprehendable for DefaultMetadata {
    fn get_message(&self) -> Vec<SharedImageOrText> {
        let msg = if let Some(typ) = self.type_.clone() {
            format!("{} named \"{}\"", typ, self.name)
        } else {
            self.name.clone()
        };
        vec![msg.into()]
    }
}
