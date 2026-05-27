use std::{io::Cursor, sync::Arc};

use base64::{Engine, prelude::BASE64_STANDARD};
use ollama_rs::{
    Ollama,
    coordinator::Coordinator,
    generation::{
        chat::{ChatMessage, MessageRole},
        images::Image,
    },
    history::ChatHistory,
};
use reqwest::Url;
use smol_str::SmolStr;

use crate::source::SharedImageOrText;

#[derive(Debug, Clone)]
pub struct OllamaRunner {
    pub ollama: Arc<Ollama>,
    pub model_name: SmolStr,
}

impl super::Runner for OllamaRunner {}

impl Default for OllamaRunner {
    fn default() -> Self {
        let url = std::env::var("OLLAMA_ENDPOINT")
            .map(|s| Url::parse(s.as_str()).expect("invalid OLLAMA_ENDPOINT environment variable"))
            .unwrap_or(Url::parse("http://localhost:11434").unwrap());
        Self {
            ollama: Arc::new(Ollama::from_url(url)),
            model_name: "gemma4:e4b".into(),
        }
    }
}

impl OllamaRunner {
    pub fn to_coordinator<C: ChatHistory>(&self, history: C) -> Coordinator<C> {
        Coordinator::new(
            self.ollama.as_ref().clone(),
            self.model_name.clone().into(),
            history,
        )
    }
}

pub fn chat_message_from_shared(
    content: impl IntoIterator<Item = SharedImageOrText>,
    role: MessageRole,
) -> ChatMessage {
    let mut texts = String::new();
    let mut images = vec![];
    content.into_iter().for_each(|m| match m {
        SharedImageOrText::Image(im) => {
            let mut buf = Cursor::new(Vec::new());
            im.write_to(&mut buf, image::ImageFormat::Png)
                .expect("failed to encode PNG");
            images.push(Image::from_base64(BASE64_STANDARD.encode(buf.get_ref())));
            texts.push_str(format!(" <image_{}/> ", images.len()).as_str());
        }
        SharedImageOrText::Text(text) => texts.push_str(format!("{text}\n").as_str()),
    });
    ChatMessage::new(role, texts.trim_end().to_string()).with_images(images)
}
