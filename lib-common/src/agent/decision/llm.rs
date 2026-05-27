use std::{fmt::Debug, sync::Arc};

use crate::{
    agent::{
        error::GetTruthValueError,
        memory::{
            criteria::CriteriaMemory,
            decision::{Decision, DecisionMemory},
            dialog::DialogMemory,
        },
        template::{self, PromptMacros},
    },
    ollama::OllamaSharedChatHistory,
    runner::{OllamaRunner, ollama},
    secure,
    source::{LlmComprehendable, SharedImageOrText},
};
use chrono::Utc;
use futures::{StreamExt, TryStreamExt, future};
use ollama_rs::{
    error::OllamaError,
    generation::chat::{ChatMessage, MessageRole},
};
use smol_str::ToSmolStr;
use tokio::{pin, sync::RwLock};
use tracing::{Instrument, Level, debug_span, event, info_span};

pub struct LlmConditionMatcher<Runner, DecisionMemory, DialogMemory, Criteria> {
    runner: Runner,
    condition: String,
    decmem: Arc<RwLock<DecisionMemory>>,
    diamem: Arc<RwLock<DialogMemory>>,
    criteria: Arc<Criteria>,
}

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
    DecisionMem: DecisionMemory,
    Criteria: CriteriaMemory,
{
    async fn get_messages<RunnerErr>(
        &self,
        update: &impl LlmComprehendable,
    ) -> Result<
        Vec<SharedImageOrText>,
        GetTruthValueError<DecisionMem::Error, Criteria::Error, RunnerErr>,
    > {
        let mem = self.decmem.read().await;
        let newest_truthy_mem = {
            let stream = mem
                .iter_newest_first()
                .filter(|r| {
                    future::ready(match r {
                        Ok(d) => d.as_ref().is_truthy,
                        Err(_) => true,
                    })
                })
                .map_err(|e| GetTruthValueError::DecisionMemory(e));
            pin!(stream);
            stream.try_next().await?
        };

        let literals = [
            ("condition".into(), self.condition.clone()),
            (
                "criteria".into(),
                self.criteria
                    .get()
                    .await
                    .map_err(GetTruthValueError::CriteriaMemory)?
                    .into_iter()
                    .map(|c| format!("- {}", c.as_ref()))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ]
        .into();

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
        Ok(if let Some(truthy_mem) = newest_truthy_mem.as_ref() {
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
                include_str!("../../../prompt/judge_with_history.xml"),
                &literals,
                &macros,
            )
            .unwrap()
        } else {
            template::expand_prompt(
                include_str!("../../../prompt/judge_without_history.xml"),
                &literals,
                &macros,
            )
            .unwrap()
        })
    }
}

impl<Update, DecMem, DiaMem, Criteria> super::Decider
    for LlmConditionMatcher<OllamaRunner, DecMem, DiaMem, Criteria>
where
    Update: LlmComprehendable + Send + Sync + Clone,
    DecMem: DecisionMemory<Material = Update> + Send + Sync,
    DecMem::Error: Send,
    DecMem::Material: Send + Sync,
    DiaMem: DialogMemory<Dialog = Vec<ChatMessage>>,
    DiaMem::Error: Debug,
    Criteria: CriteriaMemory,
{
    type Material = Update;
    type Error = GetTruthValueError<DecMem::Error, Criteria::Error, OllamaError>;

    async fn get_truth_value(&self, update: &Update) -> Result<bool, Self::Error> {
        let response: Result<String, Self::Error> = async {
            let request_msgs = self.get_messages(update).await?;
            let history = OllamaSharedChatHistory::new(vec![]);
            let mut coordinator = self.runner.to_coordinator(history.clone());
            let res = coordinator
                .chat(vec![ollama::chat_message_from_shared(
                    request_msgs,
                    MessageRole::User,
                )])
                .await
                .map_err(GetTruthValueError::Runner)?;
            event!(Level::TRACE, "full response from LLM: \n{:#?}", res);
            if let Err(err) = self.diamem.write().await.update(history.as_ref()).await {
                event!(Level::ERROR, "failed to update dialog memory: {:?}", err);
            }

            Ok(res.message.content)
        }
        .instrument(info_span!("condition_matcher.get_truth_value.inference"))
        .await;
        let mut response = response?;

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
}
