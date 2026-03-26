use escaping::Escape;
use llama_runner::ImageOrText;
use serde::Serialize;
use smol_str::SmolStr;

mod rss;

pub use rss::RssFeed;

pub trait LlmComprehendable {
    fn get_message(&self) -> Vec<ImageOrText>;
}

#[derive(Debug, Clone)]
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
        content: Vec<ImageOrText<'m>>,
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
            .iter()
            .map(|piece| match piece {
                ImageOrText::Text(text) => escape_im.escape(text).to_string(),
                ImageOrText::Image(_) => "&".to_string(),
            })
            .collect::<Vec<_>>()
            .join("");
        let images = content
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
    fn get_message(&self) -> Vec<ImageOrText> {
        let mut chunks = Vec::new();
        let mut left_delim = 0;
        for im_idx in 0..self.images.len() - 1 {
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
    fn get_message(&self) -> Vec<ImageOrText> {
        vec![ImageOrText::Text(self.msg.as_str())]
    }
}
