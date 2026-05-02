use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    sync::Arc,
};

use futures::future;
use rmcp::{
    handler::server::tool::schema_for_type,
    model::{EmptyObject, Tool},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::ToSmolStr;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};

use llama_runner::VisionLmRunner;
use tracing::{Level, event};

use crate::{
    agent::{
        memory::{criteria::CriteriaMemory, dialog::DialogMemory},
        optimize::{
            ApproveOrDeny, OptimizationCallback, Optimizer, OptimizerAction, ScheduleParamters,
        },
        template,
    },
    llm::{
        self,
        dialog::{DialogRequest, MultiTurnDialogEnabled, gemma4, toolcall},
    },
};

pub struct Gemma4Optimizer<Model, DiaMem, CriMem, ClarHandler, Schedule> {
    model: Model,
    dialog_memory: RwLock<DiaMem>,
    criteria_memory: Arc<RwLock<CriMem>>,
    clarification_handler: Arc<ClarHandler>,
    schedule: Schedule,
    checked_interval: Arc<RwLock<bool>>,
    checked_buffer_size: Arc<RwLock<bool>>,
    checked_criteria: Arc<RwLock<bool>>,
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

impl<Model, DiaMem, CriMem, ClarHandler, Schedule>
    Gemma4Optimizer<Model, DiaMem, CriMem, ClarHandler, Schedule>
where
    ClarHandler: ClarificationReqHandler,
    Schedule: ScheduleParamterAccessor,
    DiaMem: DialogMemory,
{
    pub fn new(
        model: Model,
        dialog_memory: DiaMem,
        criteria_memory: CriMem,
        clarification_handler: ClarHandler,
        schedule: Schedule,
    ) -> Self {
        Self {
            model,
            dialog_memory: RwLock::new(dialog_memory.into()),
            criteria_memory: Arc::new(RwLock::new(criteria_memory.into())),
            clarification_handler: Arc::new(clarification_handler.into()),
            schedule: schedule.into(),
            checked_interval: Arc::new(RwLock::new(false)),
            checked_buffer_size: Arc::new(RwLock::new(false)),
            checked_criteria: Arc::new(RwLock::new(false)),
        }
    }
}

impl<Model, DiaMem, CriMem, ClarHandler, Schedule>
    Gemma4Optimizer<Model, DiaMem, CriMem, ClarHandler, Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Send + Sync + 'static,
    DiaMem: Send + Sync + 'static,
    CriMem: CriteriaMemory + Send + Sync + 'static,
    CriMem::Error: Display,
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
                            return Ok(gemma4::ToolResult::Failure(
                                "missing or invalid parameter: new_value",
                            )
                            .into());
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
                gemma4::ToolHandler::new(|args| {
                    let checked_buffer_size = self.checked_buffer_size.clone();
                    let action = action.clone();
                    async move {
                        if !checked_buffer_size.read().await.clone() {
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the buffer size before changing it",
                            )
                            .into());
                        }
                        let Some(new_value) = args["new_value"].as_u64().map(|it| it as usize)
                        else {
                            return Ok(gemma4::ToolResult::Failure(
                                "missing or invalid parameter: new_value",
                            )
                            .into());
                        };
                        let (tx, mut rx) = mpsc::channel(1);
                        action
                            .send((
                                OptimizerAction::Schedule(ScheduleParamters {
                                    interval_mins: None,
                                    buffer_size: Some(new_value),
                                }),
                                tx,
                            ))
                            .await?;
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
                gemma4::ToolHandler::new(async |_| {
                    *self.checked_buffer_size.write().await = true;
                    Ok(self.schedule.get_buffer_size().await.into())
                }),
            ),
            (
                Tool::new(
                    "update_criteria",
                    "add reflections on this interaction, be it clarification on a definition, notes on user's preferences, or tips to improve judgemental accuracy in general; keep it brief and to the point",
                    schema_for_type::<UpdateCriteriaInputParams>(),
                ),
                gemma4::ToolHandler::new(|args| {
                    let checked_criterias = self.checked_criteria.clone();
                    let action = action.clone();
                    let criteria_memory = self.criteria_memory.clone();
                    async move {
                        if !checked_criterias.read().await.clone() {
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the criteria list before adding one",
                            )
                            .into());
                        }
                        let Some(content) = args["content"].as_str() else {
                            return Ok(gemma4::ToolResult::Failure(
                                "missing or invalid parameter: content",
                            )
                            .into());
                        };
                        let (tx, mut rx) = mpsc::channel(1);
                        action
                            .send((OptimizerAction::ContextPrefill(vec![content.into()]), tx))
                            .await?;
                        match rx.recv().await {
                            Some(ApproveOrDeny::Approve) => {
                                if let Err(err) = criteria_memory.write().await.add(content).await {
                                    Ok(gemma4::ToolResult::Failure(err.to_string().as_str()).into())
                                } else {
                                    Ok("criterion added".into())
                                }
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
                    "recall_criteria",
                    "retrive a list of criteria previously remembered",
                    schema_for_type::<EmptyObject>(),
                ),
                gemma4::ToolHandler::new(async |_| {
                    *self.checked_criteria.write().await = true;
                    let criteria = self.criteria_memory.read().await;
                    let criteria = match criteria.get().await {
                        Err(err) => {
                            return Ok(gemma4::ToolResult::Failure(err.to_string().as_str()).into());
                        }
                        Ok(criteria) => criteria,
                    };
                    Ok(format!(
                        "[{}]",
                        criteria
                            .into_iter()
                            .map(|c| c.as_ref().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .into())
                }),
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
                            return Ok(gemma4::ToolResult::Failure(
                                "missing or invalid parameter: question",
                            )
                            .into());
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

    pub async fn optimize_inplace(
        self: &Arc<Self>,
        prompt: Option<impl ToString + Send>,
    ) -> Result<
        Option<OptimizationCallback<<Self as Optimizer<gemma4::Dialog>>::Error>>,
        DiaMem::Error,
    >
    where
        Self: Optimizer<gemma4::Dialog>,
        DiaMem: DialogMemory<Dialog = gemma4::Dialog>,
        DiaMem::Error: Display + Debug,
    {
        if let Some(dialog) = self.dialog_memory.read().await.get().await? {
            Ok(Some(self.optimize(prompt, &dialog)))
        } else {
            Ok(None)
        }
    }
}

impl<Model, Runner, DiaMem, CriMem, ClarHandler, Schedule> Optimizer<gemma4::Dialog>
    for Gemma4Optimizer<Model, DiaMem, CriMem, ClarHandler, Schedule>
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
    CriMem: CriteriaMemory + Send + Sync + 'static,
    CriMem::Error: Display,
{
    type Error = Error<Model::Error>;

    fn optimize(
        self: &Arc<Self>,
        prompt: Option<impl ToString + Send>,
        dialog: &gemma4::Dialog,
    ) -> OptimizationCallback<Self::Error> {
        let initial_prompt = {
            let literals = [(
                "user_prompt".into(),
                prompt
                    .map(|p| p.to_string())
                    .unwrap_or("I don't like this post.".into()),
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
            loop {
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

                if !dialog.turns().last().is_some_and(|turn| match turn {
                    gemma4::DialogTurn::Assistant(res) => res.tool_calls.is_empty(),
                    _ => false,
                }) {
                    event!(Level::DEBUG, "optimization finished");
                    break;
                }
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

impl<T> From<mpsc::error::SendError<T>> for ToolHandlerError {
    fn from(_value: mpsc::error::SendError<T>) -> Self {
        Self::ChannelClosed
    }
}
