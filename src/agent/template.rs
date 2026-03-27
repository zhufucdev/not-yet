use std::collections::HashMap;

use llama_runner::{ImageOrText, MessageRole};
use quick_xml::events::Event;

use crate::agent::error::TemplateExpansionError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Paragraph,
    Expand,
    Prompt,
}

pub enum BorrowImageOwnText<'i> {
    Image(&'i image::DynamicImage),
    Text(String),
}

pub type PromptMacros<'u> = HashMap<String, Box<dyn Fn() -> Vec<BorrowImageOwnText<'u>> + 'u>>;

pub fn expand_prompt<'u>(
    template: impl AsRef<str>,
    literals: &HashMap<String, String>,
    macros: &PromptMacros<'u>,
) -> Result<Vec<(MessageRole, BorrowImageOwnText<'u>)>, TemplateExpansionError> {
    let macro_expand = move |name: &str| {
        if !name.starts_with("{") || !name.ends_with("}") {
            return Err(TemplateExpansionError::InvalidMacro(name.to_string()));
        }
        let name = name.get(1..name.len() - 1).unwrap();
        let Some(expander) = macros.get(name) else {
            return Err(TemplateExpansionError::InvalidMacro(name.to_string()));
        };
        Ok(expander())
    };
    let literal_replace = move |content: &str| {
        let mut result = content.to_string();
        for (k, v) in literals {
            let pattern = format!("{{{}}}", k);
            result = result.replace(pattern.as_str(), v.as_str())
        }
        result
    };

    let mut messages = Vec::new();
    let mut reader = quick_xml::Reader::from_str(template.as_ref());
    let encoding = reader.decoder().encoding();

    let mut tag_hierarchy = Vec::new();
    let role = MessageRole::User;
    loop {
        match reader.read_event()? {
            Event::Start(_) if tag_hierarchy.first().is_some_and(|t| *t != TagKind::Prompt) => {
                return Err(TemplateExpansionError::InvalidHirarchy);
            }
            Event::Start(e) if e.name().as_ref() == b"prompt" => {
                if tag_hierarchy.len() != 0 {
                    return Err(TemplateExpansionError::InvalidHirarchy);
                }
                tag_hierarchy.push(TagKind::Prompt)
            }
            Event::Start(e) if e.name().as_ref() == b"p" => tag_hierarchy.push(TagKind::Paragraph),
            Event::Start(e) if e.name().as_ref() == b"expand" => {
                tag_hierarchy.push(TagKind::Expand)
            }
            Event::Start(e) => {
                return Err(TemplateExpansionError::InvalidTag(
                    encoding.decode(e.name().as_ref()).0.to_string(),
                ));
            }
            Event::End(_) => {
                if tag_hierarchy.pop() == None {
                    break;
                }
            }
            Event::Text(byte_text) if tag_hierarchy.last() == Some(&TagKind::Paragraph) => {
                let text = encoding.decode(&byte_text).0;
                messages.push((
                    role.clone(),
                    BorrowImageOwnText::Text(literal_replace(text.trim())),
                ));
            }
            Event::Text(byte_text) if tag_hierarchy.last() == Some(&TagKind::Expand) => {
                let text = encoding.decode(&byte_text).0;
                let expansions = macro_expand(text.as_ref())?;
                for expansion in expansions {
                    messages.push((role.clone(), expansion));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(messages)
}

impl<'i> Into<ImageOrText<'i>> for &'i BorrowImageOwnText<'i> {
    fn into(self) -> ImageOrText<'i> {
        match self {
            BorrowImageOwnText::Image(dynamic_image) => ImageOrText::Image(dynamic_image),
            BorrowImageOwnText::Text(text) => ImageOrText::Text(&text),
        }
    }
}

pub trait AsBorrowedMessages {
    fn as_ref_msg<'s>(&'s self) -> Vec<(MessageRole, ImageOrText<'s>)>;
}

impl<'i> AsBorrowedMessages for [(MessageRole, BorrowImageOwnText<'i>)] {
    fn as_ref_msg<'s>(&'s self) -> Vec<(MessageRole, ImageOrText<'s>)> {
        self.iter()
            .map(|m| (m.0.clone(), (&m.1).into()))
            .collect::<Vec<_>>()
    }
}

impl<'i> From<ImageOrText<'i>> for BorrowImageOwnText<'i> {
    fn from(value: ImageOrText<'i>) -> Self {
        match value {
            ImageOrText::Text(text) => Self::Text(text.to_string()),
            ImageOrText::Image(dynamic_image) => Self::Image(dynamic_image),
        }
    }
}

impl<'i> From<&'i ImageOrText<'i>> for BorrowImageOwnText<'i> {
    fn from(value: &'i ImageOrText<'i>) -> Self {
        match value {
            ImageOrText::Text(text) => Self::Text(text.to_string()),
            ImageOrText::Image(dynamic_image) => Self::Image(dynamic_image),
        }
    }
}
