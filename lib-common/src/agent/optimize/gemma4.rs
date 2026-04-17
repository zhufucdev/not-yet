use std::{collections::HashMap, fmt::Display, sync::Arc};

use futures::{future, lock::Mutex};
use rmcp::{
    handler::server::tool::schema_for_type,
    model::{EmptyObject, Tool},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};

use llama_runner::VisionLmRunner;
use tracing::{Level, event};

use crate::{
    agent::{
        memory::dialog::DialogMemory,
        optimize::{
            ApproveOrDeny, OptimizationCallback, Optimizer, OptimizerAction, ScheduleParamters,
        },
        template,
    },
    llm::{
        self,
        dialog::{DialogRequest, MultiTurnDialog, MultiTurnDialogEnabled, gemma4, toolcall},
    },
};

pub struct Gemma4Optimizer<Model, DiaMem, ClarHandler, Schedule> {
    model: Model,
    dialog_memory: RwLock<DiaMem>,
    clarification_handler: Arc<ClarHandler>,
    schedule: Schedule,
    checked_interval: Arc<RwLock<bool>>,
    checked_buffer_size: Arc<RwLock<bool>>,
}

#[trait_variant::make(Send)]
pub trait ScheduleParamterAccessor {
    async fn get_interval_mins(&self) -> u32;
    async fn get_buffer_size(&self) -> usize;
}

#[trait_variant::make(Send)]
pub trait ClarificationReqHandler {
    async fn on_request(&self, prompt: &str) -> Option<String>;
}

impl<Model, DiaMem, ClarHandler, Schedule> Gemma4Optimizer<Model, DiaMem, ClarHandler, Schedule>
where
    ClarHandler: ClarificationReqHandler,
    Schedule: ScheduleParamterAccessor,
    DiaMem: DialogMemory,
{
    pub fn new(
        model: Model,
        dialog_memory: impl Into<DiaMem>,
        clarification_handler: impl Into<ClarHandler>,
        schedule: impl Into<Schedule>,
    ) -> Self {
        Self {
            model,
            dialog_memory: RwLock::new(dialog_memory.into()),
            clarification_handler: Arc::new(clarification_handler.into()),
            schedule: schedule.into(),
            checked_interval: Arc::new(RwLock::new(false)),
            checked_buffer_size: Arc::new(RwLock::new(false)),
        }
    }
}

impl<Model, DiaMem, ClarHandler, Schedule> Gemma4Optimizer<Model, DiaMem, ClarHandler, Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Send + Sync + 'static,
    DiaMem: Send + Sync + 'static,
    Model: Send + Sync + 'static,
{
    fn get_tools_and_handlers<'a>(
        &'a self,
        action: &'a mpsc::Sender<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>,
    ) -> Vec<(Tool, gemma4::ToolHandler<'a, ToolHandlerError>)> {
        vec![
            (
                Tool::new(
                    "set_polling_interval",
                    "if the user complain about update frequencies, you can change the polling intervals and / or max buffer size",
                    schema_for_type::<SetPollingInterInputParams>(),
                ),
                gemma4::ToolHandler::new(|args| {
                    let action = action.clone();
                    let checked_interval = self.checked_interval.clone();
                    async move {
                        if !checked_interval.read().await.clone() {
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the polling interval before changing it",
                            )
                            .into());
                        }
                        let Some(new_value) = args["new_value"].as_u64().map(|v| v as u32) else {
                            return Ok(
                                gemma4::ToolResult::Failure("invalid type for `new_value`").into()
                            );
                        };
                        let (tx, mut rx) = mpsc::channel(1);
                        action
                            .send((
                                OptimizerAction::Schedule(ScheduleParamters {
                                    interval_mins: Some(new_value),
                                    buffer_size: None,
                                }),
                                tx,
                            ))
                            .await
                            .map_err(|_| ToolHandlerError::ChannelClosed)?;
                        match rx.recv().await {
                            Some(ApproveOrDeny::Approve) => {
                                Ok(format!("the new interval is {new_value} minutes.").into())
                            }
                            Some(ApproveOrDeny::Deny { reason }) => {
                                Ok(reject_message(reason).into())
                            }
                            None => Err(ToolHandlerError::ChannelClosed),
                        }
                    }
                }),
            ),
            (
                Tool::new(
                    "get_polling_interval",
                    "current polling interval in minutes",
                    schema_for_type::<EmptyObject>(),
                ),
                gemma4::ToolHandler::new(async |_| {
                    *self.checked_interval.write().await = true;
                    Ok(self.schedule.get_interval_mins().await.into())
                }),
            ),
            (
                Tool::new(
                    "set_buffer_size",
                    "a buffer is where polled updates are staged, discarding overflowing ones. A smaller buffer would suit rapid-changing feeds, while a bigger one preserves more details",
                    schema_for_type::<SetBufferSizeInputParams>(),
                ),
                gemma4::ToolHandler::new(move |args| {
                    let action = action.clone();
                    let checked_buffer_size = self.checked_buffer_size.clone();
                    async move {
                        if !checked_buffer_size.read().await.clone() {
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the buffer size before changing it",
                            )
                            .into());
                        }
                        let new_value = args["new_value"].as_u64().unwrap() as usize;
                        let (tx, mut rx) = mpsc::channel(1);
                        action
                            .send((
                                OptimizerAction::Schedule(ScheduleParamters {
                                    interval_mins: None,
                                    buffer_size: Some(new_value),
                                }),
                                tx,
                            ))
                            .await
                            .unwrap();
                        match rx.recv().await {
                            Some(ApproveOrDeny::Approve) => {
                                Ok(format!("the new buffer size is {new_value}").into())
                            }
                            Some(ApproveOrDeny::Deny { reason }) => {
                                Ok(reject_message(reason).into())
                            }
                            None => Err(ToolHandlerError::ChannelClosed),
                        }
                    }
                }),
            ),
            (
                Tool::new(
                    "get_buffer_size",
                    "get the current buffer size",
                    schema_for_type::<EmptyObject>(),
                ),
                gemma4::ToolHandler::new(async |args| {
                    *self.checked_buffer_size.write().await = true;
                    Ok(self.schedule.get_buffer_size().await.into())
                }),
            ),
            (
                Tool::new(
                    "update_criteria",
                    "reflections on this interaction, be it clarification on a definition, notes on user's preferences, or tips to improve judgemental accuracy in general; keep it brief and to the point",
                    schema_for_type::<UpdateCriteriaInputParams>(),
                ),
                gemma4::ToolHandler::new(async |args| "".into()),
            ),
            (
                Tool::new(
                    "recall_criterias",
                    "retrive a list of criteria previously remembered",
                    schema_for_type::<EmptyObject>(),
                ),
                gemma4::ToolHandler::new(async |args| "".into()),
            ),
            (
                Tool::new(
                    "request_clarification",
                    "ask a question for clarification on a definition or preference",
                    schema_for_type::<RequestClarificationInputParams>(),
                ),
                gemma4::ToolHandler::new(|args| {
                    let ch = self.clarification_handler.clone();
                    async move {
                        let Some(question) = args["question"].as_str() else {
                            return Ok(
                                gemma4::ToolResult::Failure("invalid type for `question`").into()
                            );
                        };
                        if let Some(clarification) = ch.on_request(question).await {
                            Ok(clarification.into())
                        } else {
                            Ok("the user actively rejected your request".into())
                        }
                    }
                }),
            ),
        ]
    }
}

impl<Model, Runner, DiaMem, ClarHandler, Schedule> Optimizer<gemma4::Dialog>
    for Gemma4Optimizer<Model, DiaMem, ClarHandler, Schedule>
where
    for<'se, 'req> Runner:
        VisionLmRunner<'se, 'req, gemma4::DialogTemplate> + Send + Sync + 'static,
    Model: llm::Model<Runner = Runner>,
    Model: Send + Sync + 'static,
    Model::Error: Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Sync + 'static,
    Schedule: ScheduleParamterAccessor + Sync + 'static,
    DiaMem: DialogMemory<Dialog = gemma4::Dialog> + Send + Sync + 'static,
    DiaMem::Error: Display,
{
    type Error = Error<Model::Error>;

    async fn optimize(
        self: &Arc<Self>,
        prompt: Option<String>,
        dialog: &gemma4::Dialog,
    ) -> OptimizationCallback<Self::Error> {
        let initial_prompt = {
            let literals = [(
                "user_prompt".into(),
                prompt.unwrap_or("I don't like this post.".into()),
            )]
            .into_iter()
            .collect();
            template::expand_prompt(include_str!("./prompt.xml"), &literals, &Default::default())
                .expect("initial prompt failed")
        };

        let this = self.clone();
        let mut dialog = dialog.clone();
        OptimizationCallback::new(async move |action| {
            let (tools, handlers) = this.get_tools_and_handlers(&action).into_iter().fold(
                (Vec::new(), HashMap::new()),
                |(mut t, mut h), curr| {
                    h.insert(curr.0.name.to_string(), curr.1);
                    t.push(curr.0);
                    (t, h)
                },
            );
            let mut req = gemma4::DialogRequest::new(gemma4::DialogTurn::User(initial_prompt))
                .with_tools(tools);
            let runner = this.model.get_runner().await.map_err(Error::Model)?;
            while !dialog.turns().last().is_some_and(|turn| match turn {
                gemma4::DialogTurn::Assistant(res) => res.tool_calls.is_empty(),
                _ => false,
            }) {
                let res = runner.get_dialog_continued(&req, &mut dialog).await?;
                event!(Level::DEBUG, "model: {:#?}", res);
                let tool_responses =
                    future::try_join_all(res.tool_calls.into_iter().enumerate().map(
                        async |(idx, tool_call)| {
                            let Ok(tool_call) = tool_call else {
                                let err = tool_call.unwrap_err();
                                return Ok(gemma4::ToolResponse::new(
                                    format!("unparsed function call {}", idx + 1),
                                    err.to_string(), // renders the error *message* to model
                                ));
                            };
                            toolcall::handle_tool_call(tool_call.into(), &handlers).await
                        },
                    ))
                    .await?;
                event!(Level::DEBUG, "tool responses: {:#?}", tool_responses);
                this.dialog_memory
                    .write()
                    .await
                    .update(&dialog)
                    .await
                    .inspect_err(|err| {
                        event!(Level::WARN, "failed to update dialog memory: {err}")
                    });
                req.set_message(gemma4::DialogTurn::ToolResponses(tool_responses));
            }
            Ok(())
        })
    }
}

#[derive(Debug, Error)]
pub enum Error<Model> {
    #[error("model: {0}")]
    Model(Model),
    #[error("dialog: {0}")]
    Dialog(#[from] gemma4::Error),
    #[error("tool handler")]
    ToolHandler(#[from] ToolHandlerError),
}

#[derive(Debug, Clone, Error)]
pub enum ToolHandlerError {
    #[error("channel closed")]
    ChannelClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SetPollingInterInputParams {
    /// pause between polling in minutes (minimum 1)
    new_value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SetPollingIntervalInputParams {
    /// pause between polling in minutes (minimum 1)
    new_value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SetBufferSizeInputParams {
    /// buffer size, no unit; 0 to discard all (effectively disabling this subscription);
    /// 1 to only look at the top update even though there may be several, et cetera
    new_value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct UpdateCriteriaInputParams {
    /// no more than 500 characters
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct RequestClarificationInputParams {
    /// question to ask
    question: String,
}

fn reject_message(reason: Option<String>) -> String {
    if let Some(reason) = reason {
        format!("rejected: {reason}")
    } else {
        "the user actively rejected your request".into()
    }
}
