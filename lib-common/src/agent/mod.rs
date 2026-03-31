use std::sync::Arc;

use crate::{
    agent::{
        error::GetTruthValueError,
        memory::{Decision, DecisionMemory},
        template::{AsBorrowedMessages, PromptMacros},
    },
    llm::{self, SharedImageOrText},
    secure,
    source::LlmComprehendable,
};
use chrono::Utc;
use futures::{StreamExt, TryStreamExt, future};
use llama_runner::{VisionLmRequest, VisionLmRunner, VisionLmRunnerExt};
use smol_str::ToSmolStr;
use tokio::{pin, sync::RwLock};
use tracing::{Instrument, Level, debug_span, event, info_span};

pub mod error;
pub mod memory;
mod template;

pub trait Decider {
    type Material;
    type Error;
    async fn get_truth_value(&self, update: Self::Material) -> Result<bool, Self::Error>;
}

pub struct LlmConditionMatcher<Model, Memory> {
    model: Model,
    condition: String,
    memory: Arc<RwLock<Memory>>,
}

impl<Model, Memory> LlmConditionMatcher<Model, Memory> {
    pub fn new(model: Model, condition: impl ToString, memory: Memory) -> Self {
        Self {
            model,
            condition: condition.to_string(),
            memory: Arc::new(RwLock::new(memory)),
        }
    }
}

impl<Model, Runner, Update, Memory> Decider for LlmConditionMatcher<Model, Memory>
where
    Model: llm::Model<Runner = Runner> + Sync + Send,
    for<'r, 'req> Runner: VisionLmRunner<'r, 'req> + 'static,
    Update: LlmComprehendable + Send + Sync,
    Memory: DecisionMemory<Material = Update> + Send + Sync,
    Memory::Error: Send,
    Memory::Material: Send + Sync,
{
    type Material = Update;
    type Error = GetTruthValueError<Model::Error, Memory::Error>;

    async fn get_truth_value(&self, update: Update) -> Result<bool, Self::Error> {
        let inference_span = info_span!("condition_matcher.get_truth_value.inference");
        let response: Result<String, Self::Error> = async {
            let messages = {
                let mem = self.memory.read().await;
                let stream = mem
                    .iter_newest_first()
                    .filter(|r| {
                        future::ready(match r {
                            Ok(d) => d.as_ref().is_truthy,
                            Err(_) => true,
                        })
                    })
                    .map_err(|e| GetTruthValueError::Memory(e));
                pin!(stream);
                let newest_truthy_mem = stream.try_next().await?;

                let literals = [("condition".into(), self.condition.clone())].into();
                let mut macros = PromptMacros::new();
                macros.insert(
                    "update".into(),
                    Box::new(|| {
                        let content_boundary = secure::generate_content_boundary().to_smolstr();
                        std::iter::once(content_boundary.clone().into())
                            .chain(update.get_message().into_iter().map(|m| m.into()))
                            .chain(std::iter::once(content_boundary.into()))
                            .collect::<Vec<_>>()
                    }),
                );
                if let Some(truthy_mem) = newest_truthy_mem.as_ref() {
                    macros.insert(
                        "previous_acknowledgement".into(),
                        Box::new(move || {
                            let content_boundary = secure::generate_content_boundary().to_smolstr();
                            let r = truthy_mem.as_ref();
                            let message = r.material.get_message();
                            std::iter::once(content_boundary.clone().into())
                                .chain(message.into_iter())
                                .chain(std::iter::once(content_boundary.into()))
                                .collect::<Vec<SharedImageOrText>>()
                        }),
                    );
                    template::expand_prompt(
                        include_str!("../../prompt/judge_with_history.xml"),
                        &literals,
                        &macros,
                    )
                    .unwrap()
                } else {
                    template::expand_prompt(
                        include_str!("../../prompt/judge_without_history.xml"),
                        &literals,
                        &macros,
                    )
                    .unwrap()
                }
            };
            let runner = self
                .model
                .get_runner()
                .await
                .map_err(GetTruthValueError::Model)?;
            let res = runner.get_vlm_response(VisionLmRequest {
                messages: messages.as_ref_msg(),
                prefill: Some("<think>\n".into()),
                ..Default::default()
            })?;
            event!(Level::DEBUG, "full response from LLM: \n{}", res);
            Ok(res)
        }
        .instrument(inference_span)
        .await;
        let mut response = response?;

        let post_process_span = debug_span!("condition_matcher.get_truth_value.post_process");
        async {
            loop {
                if let Some(think_start) = response.find("<think>")
                    && let Some(think_end) = response.find("</think>")
                {
                    response =
                        format!("{}{}", &response[..think_start], &response[think_end + 8..]);
                } else {
                    break;
                }
            }
            event!(
                Level::DEBUG,
                "response after stripping out ttc: \n{}",
                response
            );
            let truthy = response.contains("Yes") || response.contains("yes");
            event!(Level::INFO, "remember this update as {}", truthy);
            self.memory
                .write()
                .await
                .push(Decision {
                    time: Utc::now(),
                    material: update,
                    is_truthy: truthy,
                })
                .await
                .map_err(GetTruthValueError::Memory)?;
            Ok(truthy)
        }
        .instrument(post_process_span)
        .await
    }
}

#[cfg(test)]
mod test {
    use llama_runner::{Gemma3VisionRunner, ImageOrText};
    use serde_json::json;
    use tracing_test::traced_test;

    use crate::{
        agent::memory::debug::DebugDecisionMemory, llm::owned::OwnedModel, source::DefaultUpdate,
    };

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_condition_matcher() {
        let matcher = LlmConditionMatcher::new(
            OwnedModel::new(Gemma3VisionRunner::default().await.unwrap()),
            "there has been at least 2 chapters since last time or ever",
            DebugDecisionMemory::<DefaultUpdate>::new(),
        );
        let ch1 = json!({
            "title": "Ch. 1: The New Girl",
            "pubDate": "28th Nov 2010, 1:00 PM",
            "link": "https://rain.thecomicseries.com/comics/2",
            "guid": "https://rain.thecomicseries.com/comics/2"
        })
        .to_string();
        let ch1_applicable = matcher
            .get_truth_value(DefaultUpdate::new(
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
            .get_truth_value(DefaultUpdate::new(
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
            .get_truth_value(DefaultUpdate::new(
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
            .get_truth_value(DefaultUpdate::new(
                "Chapter 4 - Not the Same",
                [ch4.into()],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(ch4_applicable);
    }
}
