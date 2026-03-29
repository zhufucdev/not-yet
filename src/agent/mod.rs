use std::{cell::RefCell, rc::Rc};

use crate::{
    agent::{
        error::GetTruthValueError,
        memory::{Decision, DecisionMemory},
        template::{AsBorrowedMessages, BorrowImageOwnText, PromptMacros},
    },
    secure,
    source::LlmComprehendable,
};
use chrono::Utc;
use futures::{StreamExt, TryStreamExt, future};
use llama_runner::{VisionLmRequest, VisionLmRunner, VisionLmRunnerExt};
use tokio::pin;
use tracing::{Instrument, Level, debug_span, event, info_span};

pub mod error;
pub mod memory;
mod template;

pub trait Decider {
    type Material;
    type Error;
    async fn get_truth_value(&self, update: Self::Material) -> Result<bool, Self::Error>;
}

pub struct LlmConditionMatcher<'r, Runner, Memory> {
    runner: &'r Runner,
    condition: String,
    memory: Rc<RefCell<Memory>>,
}

impl<'r, Runner, Memory> LlmConditionMatcher<'r, Runner, Memory> {
    pub fn new(runner: &'r Runner, condition: impl ToString, memory: Memory) -> Self {
        Self {
            runner,
            condition: condition.to_string(),
            memory: Rc::new(RefCell::new(memory)),
        }
    }
}

impl<'r, Runner, Update, Memory> Decider for LlmConditionMatcher<'r, Runner, Memory>
where
    for<'req> Runner: VisionLmRunner<'r, 'req>,
    Update: LlmComprehendable,
    Memory: DecisionMemory<Material = Update>,
{
    type Material = Update;
    type Error = GetTruthValueError<Memory::Error>;

    async fn get_truth_value(
        &self,
        update: Update,
    ) -> Result<bool, GetTruthValueError<Memory::Error>> {
        let inference_span = info_span!("condition_matcher.get_truth_value.inference");
        let response: Result<String, GetTruthValueError<Memory::Error>> = async {
            let mem = self.memory.borrow();
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
                    let content_boundary = secure::generate_content_boundary();
                    std::iter::once(BorrowImageOwnText::Text(content_boundary.clone()))
                        .chain(update.get_message().into_iter().map(|m| m.into()))
                        .chain(std::iter::once(BorrowImageOwnText::Text(content_boundary)))
                        .collect::<Vec<_>>()
                }),
            );
            let messages = if let Some(truthy_mem) = newest_truthy_mem.as_ref() {
                macros.insert(
                    "previous_acknowledgement".into(),
                    Box::new(|| {
                        let content_boundary = secure::generate_content_boundary();
                        let message = truthy_mem.as_ref().material.get_message();
                        std::iter::once(BorrowImageOwnText::Text(content_boundary.clone()))
                            .chain(message.into_iter().map(|m| m.into()))
                            .chain(std::iter::once(BorrowImageOwnText::Text(content_boundary)))
                            .collect::<Vec<_>>()
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
            };
            let res = self.runner.get_vlm_response(VisionLmRequest {
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
                .borrow_mut()
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

    use crate::{agent::memory::debug::DebugDecisionMemory, source::DefaultUpdate};

    use super::*;

    #[tokio::test]
    #[traced_test]
    async fn test_condition_matcher() {
        let runner = Gemma3VisionRunner::default().await.unwrap();
        let matcher = LlmConditionMatcher::new(
            &runner,
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
                [ImageOrText::Text(&ch1)],
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
                [ImageOrText::Text(&ch2)],
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
                [ImageOrText::Text(&ch3)],
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
                [ImageOrText::Text(&ch4)],
                Some("RSS item".into()),
            ))
            .await
            .unwrap();
        assert!(ch4_applicable);
    }
}
