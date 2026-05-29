use std::collections::HashMap;

use futures::future::BoxFuture;
use quick_xml::events::Event;

use crate::{agent::error::TemplateExpansionError, source::SharedImageOrText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Paragraph,
    Expand,
    Prompt,
}

pub type PromptMacros<'a, Err> = HashMap<String, PromptMacro<'a, Err>>;

pub struct PromptMacro<'a, Err>(
    Box<dyn Fn() -> BoxFuture<'a, Result<Vec<SharedImageOrText>, Err>> + Send + Sync + 'a>,
);

impl<'a, Err> PromptMacro<'a, Err> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'a,
        Fut: Future<Output = Result<Vec<SharedImageOrText>, Err>> + Send + 'a,
    {
        Self(Box::new(move || Box::pin(f())))
    }
}

pub async fn expand_prompt<'a, MacroErr>(
    template: impl AsRef<str>,
    literals: &HashMap<String, String>,
    macros: &PromptMacros<'a, MacroErr>,
) -> Result<Vec<SharedImageOrText>, TemplateExpansionError<MacroErr>> {
    let macro_expand = async move |name: &str| {
        if !name.starts_with("{") || !name.ends_with("}") {
            return Err(TemplateExpansionError::InvalidMacro(name.to_string()));
        }
        let name = name.get(1..name.len() - 1).unwrap();
        let Some(expander) = macros.get(name) else {
            return Err(TemplateExpansionError::InvalidMacro(name.to_string()));
        };
        Ok(expander.0()
            .await
            .map_err(TemplateExpansionError::MacroInternal)?)
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
                messages.push(SharedImageOrText::Text(literal_replace(text.trim()).into()));
            }
            Event::Text(byte_text) if tag_hierarchy.last() == Some(&TagKind::Expand) => {
                let text = encoding.decode(&byte_text).0;
                let expansions = macro_expand(text.as_ref()).await?;
                for expansion in expansions {
                    messages.push(expansion);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(messages)
}
