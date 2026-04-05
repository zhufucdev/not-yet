use std::{hash::Hash, sync::Arc};

use image::DynamicImage;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use tracing::{Level, event};

use crate::{llm::SharedImageOrText, serde_utils::DynImageConverter};

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UrlContent {
    Image {
        url: String,
        #[serde_as(as = "Arc<DynImageConverter>")]
        image: Arc<DynamicImage>,
    },
    Text {
        url: String,
        text: String,
    },
}

pub(crate) async fn get_url_content<E>(
    url: impl AsRef<str>,
    client: &reqwest::Client,
) -> Result<Option<UrlContent>, E>
where
    E: From<reqwest::Error> + From<image::ImageError>,
{
    let response = client.get(url.as_ref()).send().await?.error_for_status()?;
    if response.status().is_success()
        && let Some(content_type) = response.headers().get("content-type")
        && let Ok(content_type) = content_type.to_str()
    {
        event!(Level::INFO, "Got extra content, attaching to struct");
        event!(Level::DEBUG, "Content type {}", content_type);
        if content_type.starts_with("text/") {
            return Ok(Some(UrlContent::Text {
                url: url.as_ref().to_string(),
                text: response.text().await?,
            }));
        } else if content_type.starts_with("image/") {
            return Ok(Some(UrlContent::Image {
                url: url.as_ref().to_string(),
                image: Arc::new(image::load_from_memory(response.bytes().await?.as_ref())?),
            }));
        } else {
            event!(Level::WARN, "Unsupported content type, ignoring");
        }
    }

    return Ok(None);
}

pub(crate) fn extract_url_from_feed_item<E>(
    xml: impl AsRef<str>,
    max_depth: Option<i32>,
) -> Result<Vec<String>, E>
where
    E: From<quick_xml::Error>,
{
    let mut reader = quick_xml::reader::Reader::from_str(xml.as_ref());
    let encoding = reader.decoder().encoding();
    let mut depth = 0;
    let mut links = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                if max_depth.is_some_and(|it| depth > it) {
                    continue;
                }
                match e.name().as_ref() {
                    b"img" => {
                        if let Some(src) = e
                            .attributes()
                            .filter_map(|r| r.ok())
                            .filter(|attr| attr.key.as_ref() == b"src")
                            .next()
                        {
                            links.push(encoding.decode(src.value.as_ref()).0.to_string());
                        }
                    }
                    b"a" | b"link" => {
                        if let Some(href) = e
                            .attributes()
                            .filter_map(|r| r.ok())
                            .filter(|attr| attr.key.as_ref() == b"href")
                            .next()
                        {
                            links.push(encoding.decode(href.value.as_ref()).0.to_string());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => depth -= 1,
            Ok(Event::Eof) => break,
            Err(err) => return Err(err.into()),
            _ => {}
        }
    }
    Ok(links)
}

impl UrlContent {
    pub fn url(&self) -> &str {
        match self {
            UrlContent::Image { url, image: _ } => url,
            UrlContent::Text { url, text: _ } => url,
        }
    }
}

impl Into<SharedImageOrText> for UrlContent {
    fn into(self) -> SharedImageOrText {
        match self {
            UrlContent::Image { url: _, image } => image.into(),
            UrlContent::Text { url: _, text } => text.into(),
        }
    }
}

impl Into<SharedImageOrText> for &UrlContent {
    fn into(self) -> SharedImageOrText {
        match self {
            UrlContent::Image { url: _, image } => image.clone().into(),
            UrlContent::Text { url: _, text } => text.into(),
        }
    }
}

impl Hash for UrlContent {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            UrlContent::Image { url: _, image: _ } => state.write_u8(0),
            UrlContent::Text { url: _, text: _ } => state.write_u8(1),
        }
        self.url().hash(state);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_extract_url_from_feed_item() {
        let item = r#"&#32; submitted by &#32; <a href="https://www.reddit.com/user/orhunp"> /u/orhunp </a> <br/> <span><a href="https://blog.orhun.dev/800-rust-projects/">[link]</a></span> &#32; <span><a href="https://www.reddit.com/r/rust/comments/1sb859y/800_rust_terminal_projects_in_3_years/">[comments]</a></span>"#;
        let urls = extract_url_from_feed_item::<anyhow::Error>(item, None).unwrap();
        assert_eq!(urls[0], "https://www.reddit.com/user/orhunp");
        assert_eq!(urls[1], "https://blog.orhun.dev/800-rust-projects/");
        assert_eq!(
            urls[2],
            "https://www.reddit.com/r/rust/comments/1sb859y/800_rust_terminal_projects_in_3_years/"
        );
    }
}
