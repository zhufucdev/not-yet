use std::{fmt::Debug, sync::Arc};

use crate::{
    agent::{
        error::{GetTruthValueError, TemplateExpansionError},
        memory::{
            criteria::CriteriaMemory,
            decision::{Decision, DecisionMemory},
            dialog::DialogMemory,
        },
        optimize::llm::ToolResult,
        template::{self, PromptMacro, PromptMacros},
    },
    ollama::OllamaSharedChatHistory,
    runner::{OllamaRunner, ollama},
    secure,
    source::{LlmComprehendable, SharedImageOrText, utils::SAFARI_UA},
};
use chrono::Utc;
use futures::{StreamExt, TryStreamExt};
use html_to_markdown_rs::{ConversionOptions, LinkStyle, OutputFormat};
use ollama_rs::{
    error::OllamaError,
    generation::{
        chat::{ChatMessage, MessageRole},
        parameters::ThinkType,
        tools::Tool,
    },
    history::ChatHistory,
};
use rand::distr::uniform::SampleRange;
use reqwest::Url;
use schemars::JsonSchema;
use serde::Deserialize;
use smol_str::ToSmolStr;
use thiserror::Error;
use tokio::{pin, sync::RwLock};
use tracing::{Instrument, Level, debug_span, event, info_span};

pub struct LlmConditionMatcher<Runner, DecisionMemory, DialogMemory, Criteria> {
    runner: Runner,
    condition: String,
    decmem: Arc<RwLock<DecisionMemory>>,
    diamem: Arc<RwLock<DialogMemory>>,
    criteria: Arc<Criteria>,
}

struct GetPreviousDecisions<M> {
    mem: Arc<RwLock<M>>,
}

struct FetchUrl(reqwest::Client);

impl<Runner, DecisionMemory, DialogMemory, Criteria>
    LlmConditionMatcher<Runner, DecisionMemory, DialogMemory, Criteria>
{
    pub fn new(
        runner: Runner,
        condition: impl ToString,
        decision_memory: DecisionMemory,
        dialog_memory: DialogMemory,
        criteria: Criteria,
    ) -> Self {
        Self {
            runner,
            condition: condition.to_string(),
            decmem: Arc::new(RwLock::new(decision_memory)),
            diamem: Arc::new(RwLock::new(dialog_memory)),
            criteria: Arc::new(criteria),
        }
    }
}

impl<Runner, DecisionMem, DialogMemory, Criteria>
    LlmConditionMatcher<Runner, DecisionMem, DialogMemory, Criteria>
where
    DecisionMem: DecisionMemory + Sync,
    Criteria: CriteriaMemory + Sync,
    DecisionMem::Error: std::error::Error + Send + Sync + 'static,
    Runner: Sync,
    DialogMemory: Send + Sync,
{
    async fn get_messages<'a, RunnerErr>(
        &'a self,
        update: &'a (impl LlmComprehendable + Send + Sync),
    ) -> Result<
        Vec<SharedImageOrText>,
        GetTruthValueError<DecisionMem::Error, Criteria::Error, RunnerErr>,
    > {
        let literals = [
            ("condition".into(), self.condition.clone()),
            (
                "time".into(),
                Utc::now().format("%a %B %d %r %Y UTC").to_string(),
            ),
        ]
        .into();

        let mut macros = PromptMacros::<'a>::new();
        macros.insert(
            "notes".into(),
            PromptMacro::new(async || {
                let mut notes = self
                    .criteria
                    .get()
                    .await
                    .map_err(GetTruthValueError::CriteriaMemory)?
                    .into_iter()
                    .map(|criterion| criterion.as_ref().to_string())
                    .map(|c| format!("- {}", c).into())
                    .collect::<Vec<_>>();
                if !self
                    .decmem
                    .read()
                    .await
                    .is_empty()
                    .await
                    .map_err(GetTruthValueError::DecisionMemory)?
                {
                    notes.push("You have seen this series before.".into());
                }
                Ok(notes)
            }),
        );
        macros.insert(
            "update".into(),
            PromptMacro::new(async || {
                let content_boundary = secure::generate_content_boundary().to_smolstr();
                Ok(std::iter::once(content_boundary.clone().into())
                    .chain(update.get_message().into_iter().map(|m| m.into()))
                    .chain(std::iter::once(content_boundary.into()))
                    .collect::<Vec<_>>())
            }),
        );

        match template::expand_prompt(include_str!("./prompt/judge.xml"), &literals, &macros).await
        {
            Ok(e) => Ok(e),
            Err(TemplateExpansionError::MacroInternal(err)) => Err(err),
            _ => panic!(),
        }
    }
}

impl<Update, DecMem, DiaMem, Criteria> super::Decider
    for LlmConditionMatcher<OllamaRunner, DecMem, DiaMem, Criteria>
where
    Update: LlmComprehendable + Send + Sync + Clone,
    DecMem: DecisionMemory<Material = Update> + Send + Sync + 'static,
    DecMem::Error: std::error::Error + Send + Sync,
    DecMem::Material: Send + Sync,
    DiaMem: DialogMemory<Dialog = Vec<ChatMessage>> + Sync,
    DiaMem::Error: Debug,
    Criteria: CriteriaMemory + Sync,
{
    type Material = Update;
    type Error = GetTruthValueError<DecMem::Error, Criteria::Error, OllamaError>;

    async fn get_truth_value(&self, update: &Update) -> Result<bool, Self::Error> {
        let response = async {
            let mut request_msgs = vec![ollama::chat_message_from_shared(
                self.get_messages(update).await?,
                MessageRole::User,
            )];
            let history = OllamaSharedChatHistory::new(vec![]);
            let mut coordinator = self
                .runner
                .to_coordinator(history.clone())
                .think(ThinkType::High)
                .add_tool(GetPreviousDecisions {
                    mem: self.decmem.clone(),
                })
                .add_tool(FetchUrl(
                    reqwest::Client::builder()
                        .default_headers(
                            [(reqwest::header::USER_AGENT, SAFARI_UA.parse().unwrap())]
                                .into_iter()
                                .collect(),
                        )
                        .build()
                        .unwrap(),
                ));

            // This can get expensive, so when model does not require
            // decision memory, we don't query
            let mut dec_mem_cache = None;

            loop {
                match coordinator.chat(request_msgs).await {
                    Ok(res) => {
                        event!(Level::TRACE, "full response from LLM: \n{:#?}", res);
                        if let Err(err) = self
                            .diamem
                            .write()
                            .await
                            .update(history.borrow().as_ref())
                            .await
                        {
                            event!(Level::ERROR, "failed to update dialog memory: {:?}", err);
                        }

                        return Ok(res.message.content);
                    }
                    Err(OllamaError::ToolCallError(
                        ollama_rs::error::ToolCallError::InternalToolError(err),
                    )) => match err.downcast::<GetPreviousDecisionsInterrupt>() {
                        Ok(call) => {
                            event!(Level::DEBUG, "interrupted by get decisions");
                            let params = call.0;
                            let removed = history.borrow_mut().pop(); // remove the tool call request
                            assert!(
                                removed.is_some_and(|it| it.role == MessageRole::Assistant
                                    && !it.tool_calls.is_empty())
                            );
                            let history = history.clone();
                            let dec_mem = if let Some(mem) = dec_mem_cache.as_ref() {
                                mem
                            } else {
                                let guard = self.decmem.read().await;
                                dec_mem_cache = Some(
                                    guard
                                        .iter_newest_first()
                                        .map_ok(|it| it.as_ref().clone())
                                        .try_collect::<Vec<_>>()
                                        .await
                                        .map_err(GetTruthValueError::DecisionMemory)?,
                                );
                                dec_mem_cache.as_ref().unwrap()
                            };
                            let mut inserted_indices = vec![];
                            for (idx, decision) in dec_mem
                                .iter()
                                .enumerate()
                                .filter(|(_, decision)| {
                                    params.filter.map(|f| f.matches(decision)).unwrap_or(true)
                                })
                                .skip(params.offset.unwrap_or(0))
                                .take(params.limit.unwrap_or(3usize))
                            {
                                let decision = decision.as_ref();
                                history.borrow_mut().splice(
                                    0..0,
                                    [
                                        ollama::chat_message_from_shared(
                                            self.get_messages(&decision.material).await?,
                                            MessageRole::User,
                                        ),
                                        ChatMessage::assistant(
                                            if decision.is_truthy { "Yes" } else { "No" }.into(),
                                        ),
                                    ]
                                    .into_iter(),
                                );
                                inserted_indices.push(idx);
                            }
                            event!(
                                Level::DEBUG,
                                "inserted {} historical decisions",
                                inserted_indices.len()
                            );
                            if let Err(err) = self
                                .diamem
                                .write()
                                .await
                                .update(history.borrow().as_ref())
                                .await
                            {
                                event!(Level::ERROR, "failed to update dialog memory: {:?}", err);
                            }

                            event!(Level::TRACE, "history: {:#?}", &history.borrow().messages());

                            let dec_mem = dec_mem_cache.as_mut().unwrap();
                            for inserted in inserted_indices.iter().rev() {
                                // the indices are sorted ascendingly
                                dec_mem.remove(*inserted);
                            }

                            request_msgs = vec![];
                        }
                        Err(err) => {
                            return Err(GetTruthValueError::Runner(OllamaError::ToolCallError(
                                ollama_rs::error::ToolCallError::InternalToolError(err),
                            )));
                        }
                    },
                    Err(err) => return Err(GetTruthValueError::Runner(err)),
                }
            }
        }
        .instrument(info_span!("condition_matcher.get_truth_value.inference"))
        .await?;

        let post_process_span = debug_span!("condition_matcher.get_truth_value.post_process");
        async {
            let truthy = response.contains("Yes") || response.contains("yes");
            event!(Level::INFO, "remember this update as {}", truthy);
            self.decmem
                .write()
                .await
                .push(Decision {
                    time: Utc::now(),
                    material: update.clone(),
                    is_truthy: truthy,
                })
                .await
                .map_err(GetTruthValueError::DecisionMemory)?;
            Ok(truthy)
        }
        .instrument(post_process_span)
        .await
    }
}

#[derive(Debug, Clone, Default, JsonSchema, Deserialize)]
#[allow(dead_code)]
struct GetPreviousDecisionsParams {
    /// Skip the first n items, defaults to 0.
    offset: Option<usize>,
    /// Cap the number of items returned, defaults to 3.
    limit: Option<usize>,
    /// Return matching items only.
    filter: Option<DecisionFilter>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[allow(dead_code)]
struct FetchUrlParams {
    url: String,
    /// Get the raw text response regardless of its content type
    no_sanitize: Option<bool>,
    /// Cap the number of characters returned, defaults to 5k
    limit: Option<usize>,
    /// Skip the first n characters, defaults to 0
    offset: Option<usize>,
}

#[derive(Debug, Clone, Copy, JsonSchema, Deserialize)]
enum DecisionFilter {
    /// You said yes.
    Truthy,
    /// You said no.
    Falsy,
}

impl DecisionFilter {
    pub fn matches<M: LlmComprehendable>(&self, decision: &Decision<M>) -> bool {
        match self {
            DecisionFilter::Truthy => decision.is_truthy,
            DecisionFilter::Falsy => !decision.is_truthy,
        }
    }
}

use ollama_rs::generation::tools::Result as SystemResult;

#[derive(Debug, Clone, Error)]
#[error("get previous decisions interrupted")]
struct GetPreviousDecisionsInterrupt(GetPreviousDecisionsParams);

impl<M> Tool for GetPreviousDecisions<M>
where
    M: DecisionMemory + Send + Sync + 'static,
    M::Error: std::error::Error + Send + Sync,
{
    type Params = GetPreviousDecisionsParams;

    fn name() -> &'static str {
        "get_previous_decisions"
    }

    fn description() -> &'static str {
        "Previous chat history was trimmed. Use this tool when the request involves knowledge of the previous decisions you made. For example, determining whether the material happends to be a new chapter, you first query the previous truthy decision, pay attention to its chapter number, and compare it to the current material."
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        let guard = self.mem.read().await;
        let stream = guard.iter_newest_first();
        pin!(stream);
        let mut skipped = 0usize;
        while let Some(result) = stream.next().await {
            let Ok(decision) = result else {
                return Err(Box::new(result.err().unwrap()));
            };
            let decision = decision.as_ref();
            if parameters
                .filter
                .map(|it| it.matches(decision))
                .unwrap_or(true)
            {
                if let Some(offset) = parameters.offset {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                }
                return Err(Box::new(GetPreviousDecisionsInterrupt(parameters)));
            }
        }
        Ok("[]".into())
    }
}

impl Tool for FetchUrl {
    type Params = FetchUrlParams;

    fn name() -> &'static str {
        "fetch_url"
    }

    fn description() -> &'static str {
        "Fetch content from a URL as Markdown"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        let url = match Url::parse(&parameters.url) {
            Ok(r) => r,
            Err(err) => {
                return SystemResult::Ok(ToolResult::Failure(err.to_string()).into());
            }
        };
        match self
            .0
            .get(url)
            .send()
            .await
            .map(|res| res.error_for_status())
            .flatten()
        {
            Ok(res) => {
                let content_type = res.headers()[reqwest::header::CONTENT_TYPE]
                    .to_str()
                    .unwrap();
                if !content_type.starts_with("text/") {
                    return Ok(format!("expect a text content type, got {content_type:?}").into());
                }
                let Some(text_type) = content_type
                    .split('/')
                    .skip(1)
                    .next()
                    .and_then(|rem| rem.split(';').next())
                    .map(|t| t.trim())
                else {
                    return Ok(format!("unknown content type: {content_type}").into());
                };
                match (text_type, parameters.no_sanitize) {
                    ("plain", _) => Ok(res.text_with_charset("utf-8").await?.into()),
                    ("html" | "xml", _) => {
                        let md = html_to_markdown_rs::convert(
                            res.text_with_charset("utf-8").await?.as_str(),
                            Some(
                                ConversionOptions::builder()
                                    .capture_svg(false)
                                    .output_format(OutputFormat::Markdown)
                                    .link_style(LinkStyle::Reference)
                                    .build(),
                            ),
                        )?;
                        let content = md.content.unwrap();
                        let skip = parameters.offset.unwrap_or(0);
                        let range =
                            skip..skip + content.len().min(parameters.limit.unwrap_or(5_000));
                        Ok(content[range].into())
                    }
                    (_, None | Some(false)) => Ok(format!("unsupported: {content_type}").into()),
                    (_, Some(true)) => Ok(res.text_with_charset("utf-8").await?.into()),
                }
            }
            Err(err) => Ok(format!("HTTP reqest failed: {err}").into()),
        }
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;
    use tracing_test::traced_test;

    use crate::{
        agent::{
            decision::Decider,
            memory::{
                criteria::debug::DebugCriteriaMemory, decision::debug::DebugDecisionMemory,
                dialog::debug::DebugDialogMemory,
            },
        },
        source::DefaultUpdate,
    };

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_condition_matcher() {
        let matcher = LlmConditionMatcher::new(
            Default::default(),
            "there has been at least 2 chapters since last time or ever",
            DebugDecisionMemory::new(),
            DebugDialogMemory::new(),
            DebugCriteriaMemory::new(),
        );
        let ch1 = json!({
            "title": "Ch. 1: The New Girl",
            "pubDate": "28th Nov 2010, 1:00 PM",
            "link": "https://rain.thecomicseries.com/comics/2",
            "guid": "https://rain.thecomicseries.com/comics/2"
        })
        .to_string();
        let ch1_applicable = matcher
            .get_truth_value(&DefaultUpdate::new(
                "Chapter 1 - The New Girl",
                [ch1.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(!ch1_applicable);

        let ch2 = json!({
            "title": "Ch. 2: Secrets and Lies",
            "pubDate": "18th Jan 2011, 6:00 PM",
            "link": "https://rain.thecomicseries.com/comics/26",
            "guid": "https://rain.thecomicseries.com/comics/26",
        })
        .to_string();
        let ch2_applicable = matcher
            .get_truth_value(&DefaultUpdate::new(
                "Chapter 2 - Secrets and Lies",
                [ch2.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(ch2_applicable);

        let ch3 = json!({
            "title": "Ch. 3: Normal People",
            "pubDate": "27th Mar 2011, 8:00 PM",
            "link": "https://rain.thecomicseries.com/comics/56",
            "guid": "https://rain.thecomicseries.com/comics/56",
        })
        .to_string();
        let ch3_applicable = matcher
            .get_truth_value(&DefaultUpdate::new(
                "Chapter 3 - Normal People",
                [ch3.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(!ch3_applicable);

        let ch4 = json!({
            "title": "Ch. 4: Not the Same",
            "pubDate": "27th Mar 2011, 8:00 PM",
            "link": "https://rain.thecomicseries.com/comics/85",
            "guid": "https://rain.thecomicseries.com/comics/85",
        })
        .to_string();
        let ch4_applicable = matcher
            .get_truth_value(&DefaultUpdate::new(
                "Chapter 4 - Not the Same",
                [ch4.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(ch4_applicable);
    }

    #[tokio::test]
    #[traced_test]
    async fn fetch_url_tool() {
        let matcher = LlmConditionMatcher::new(
            Default::default(),
            "there are at least 5 comments",
            DebugDecisionMemory::new(),
            DebugDialogMemory::new(),
            DebugCriteriaMemory::new(),
        );
        let update = json!({
            "title": "The $500K AI Film That ‘Premiered at Cannes’ Didn’t Actually Premiere at Cannes",
            "pubDate": "Thu, 28 May 2026 17:43:36 +0000",
            "link": "https://firethering.com/hell-grind-ai-film-cannes-premiere-higgsfield/",
            "guid": "https://news.ycombinator.com/item?id=48320985",
            "comments": "https://news.ycombinator.com/item?id=48320985"
        }).to_string();
        let applicable = matcher
            .get_truth_value(&DefaultUpdate::new(
                "The K AI Film That ‘Premiered at Cannes’ Didn’t Actually Premiere at Cannes",
                [update.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        dbg!(matcher.diamem.read().await.get().await);
        assert!(applicable);
    }
}
