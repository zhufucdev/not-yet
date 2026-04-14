use std::collections::HashMap;

use llama_runner::{ImageOrText, MessageRole};
use quick_xml::events::Event;

use crate::{agent::error::TemplateExpansionError, llm::SharedImageOrText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Paragraph,
    Expand,
    Prompt,
}

pub type PromptMacros<'a> = HashMap<String, Box<dyn Fn() -> Vec<SharedImageOrText> + 'a>>;

pub fn expand_prompt(
    template: impl AsRef<str>,
    literals: &HashMap<String, String>,
    macros: &PromptMacros,
) -> Result<Vec<(MessageRole, SharedImageOrText)>, TemplateExpansionError> {
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
                    SharedImageOrText::Text(literal_replace(text.trim()).into()),
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

impl<'i> Into<ImageOrText<'i>> for &'i SharedImageOrText {
    fn into(self) -> ImageOrText<'i> {
        match self {
            SharedImageOrText::Image(dynamic_image) => ImageOrText::Image(dynamic_image),
            SharedImageOrText::Text(text) => ImageOrText::Text(&text),
        }
    }
}
