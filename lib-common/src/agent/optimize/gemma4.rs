use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    ops::Add,
    sync::Arc,
};

use futures::future;
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
    schedule: Arc<RwLock<Schedule>>,
}

#[derive(Debug, Default)]
struct State {
    checked: HashSet<ToolcallKind>,
    retrival_rejected: HashSet<ToolcallKind>,
    actions_count: u32,
}

#[trait_variant::make(Send)]
pub trait ScheduleParamterAccessor {
    type Error: std::error::Error;

    async fn get_interval_mins(&self) -> u32;
    async fn set_interval_mins(&mut self, new_value: u32) -> Result<(), Self::Error>;

    async fn get_buffer_size(&self) -> usize;
    async fn set_buffer_size(&mut self, new_value: usize) -> Result<(), Self::Error>;
}

#[trait_variant::make(Send)]
pub trait ClarificationReqHandler {
    type Error: std::error::Error;
    async fn on_request(&self, prompt: &str) -> Result<Option<String>, Self::Error>;
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
            schedule: Arc::new(RwLock::new(schedule.into())),
        }
    }
}

impl<Model, DiaMem, CriMem, ClarHandler, Schedule>
    Gemma4Optimizer<Model, DiaMem, CriMem, ClarHandler, Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Send + Sync + 'static,
    ClarHandler::Error: Send + Sync + 'static,
    DiaMem: Send + Sync + 'static,
    CriMem: CriteriaMemory + Send + Sync + 'static,
    CriMem::Error: Display,
    Model: Send + Sync + 'static,
{
    fn get_tools_and_handlers<'a>(
        &'a self,
        state: &'a RwLock<State>,
        action: &'a mpsc::Sender<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>,
    ) -> Vec<(Tool, gemma4::ToolHandler<'a, ToolHandlerError>)> {
        vec![
            (
                Tool::new(
                    "set_polling_interval",
                    "if the user complain about update frequencies, you can change the polling intervals and / or max buffer size. must retrive the current value first",
                    schema_for_type::<SetPollingInterInputParams>(),
                ),
                gemma4::ToolHandler::new(move |args| {
                    let action = action.clone();
                    let schedule = self.schedule.clone();
                    async move {
                        if !state
                            .read()
                            .await
                            .checked
                            .contains(&ToolcallKind::PollingInterval)
                        {
                            state
                                .write()
                                .await
                                .retrival_rejected
                                .insert(ToolcallKind::PollingInterval);
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the polling interval before changing it. please use the `get_polling_interval` tool first",
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
                                state.write().await.actions_count += 1;
                                Ok(gemma4::ToolResult::from(
                                    schedule
                                        .write()
                                        .await
                                        .set_interval_mins(new_value)
                                        .await
                                        .map(|_| {
                                            format!("the new interval is {new_value} minutes.")
                                        }),
                                )
                                .into())
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
                gemma4::ToolHandler::new(move |_| async move {
                    state
                        .write()
                        .await
                        .checked
                        .insert(ToolcallKind::PollingInterval);
                    state
                        .write()
                        .await
                        .retrival_rejected
                        .remove(&ToolcallKind::PollingInterval);
                    Ok(self.schedule.read().await.get_interval_mins().await.into())
                }),
            ),
            (
                Tool::new(
                    "set_buffer_size",
                    "a buffer is where polled updates are staged, discarding overflowing ones. A smaller buffer would suit rapid-changing feeds, while a bigger one preserves more details. must retrive the current value first",
                    schema_for_type::<SetBufferSizeInputParams>(),
                ),
                gemma4::ToolHandler::new(move |args| {
                    let action = action.clone();
                    let schedule = self.schedule.clone();
                    async move {
                        if !state
                            .read()
                            .await
                            .checked
                            .contains(&ToolcallKind::BufferSize)
                        {
                            state
                                .write()
                                .await
                                .retrival_rejected
                                .insert(ToolcallKind::BufferSize);
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the buffer size before changing it. please use the `get_buffer_size` tool first",
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
                                state.write().await.actions_count += 1;
                                Ok(gemma4::ToolResult::from(
                                    schedule
                                        .write()
                                        .await
                                        .set_buffer_size(new_value)
                                        .await
                                        .map(|_| format!("the new buffer size is {new_value}")),
                                )
                                .into())
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
                    state.write().await.checked.insert(ToolcallKind::BufferSize);
                    state
                        .write()
                        .await
                        .retrival_rejected
                        .remove(&ToolcallKind::BufferSize);
                    Ok(self.schedule.read().await.get_buffer_size().await.into())
                }),
            ),
            (
                Tool::new(
                    "add_criteria",
                    "add reflections on this interaction, be it clarification on a definition, notes on user's preferences, or tips to improve judgemental accuracy in general; keep it brief and to the point",
                    schema_for_type::<UpdateCriteriaInputParams>(),
                ),
                gemma4::ToolHandler::new(move |args| {
                    let action = action.clone();
                    let criteria_memory = self.criteria_memory.clone();
                    async move {
                        if criteria_memory.read().await.is_empty().await {
                            state.write().await.checked.insert(ToolcallKind::Criteria);
                        } else if !state.read().await.checked.contains(&ToolcallKind::Criteria) {
                            state
                                .write()
                                .await
                                .retrival_rejected
                                .insert(ToolcallKind::Criteria);
                            return Ok(gemma4::ToolResult::Failure(
                                "you must check the criteria list before changing it. please use the `get_criteria` tool first",
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
                                    state.write().await.actions_count += 1;
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
                    "get_criteria",
                    "retrive a list of criteria previously remembered",
                    schema_for_type::<EmptyObject>(),
                ),
                gemma4::ToolHandler::new(async |_| {
                    state.write().await.checked.insert(ToolcallKind::Criteria);
                    state
                        .write()
                        .await
                        .retrival_rejected
                        .remove(&ToolcallKind::Criteria);
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
                        if let Some(clarification) = ch
                            .on_request(question)
                            .await
                            .map_err(anyhow::Error::from)
                            .map_err(ToolHandlerError::ClarificationRequest)?
                        {
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
            event!(
                Level::DEBUG,
                "read {} messages from dialog memory, amounting to {} turns",
                dialog.history().len(),
                dialog.turns().len()
            );
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
    ClarHandler::Error: Send + Sync + 'static,
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
        let state = State {
            checked: dialog
                .turns()
                .into_iter()
                .filter_map(|turn| match turn {
                    gemma4::DialogTurn::ToolResponses(res) => {
                        Some(res.into_iter().map(|tool| tool.name.clone()))
                    }
                    _ => None,
                })
                .flatten()
                .filter_map(|tool_name| match tool_name.as_str() {
                    "get_polling_interval" => Some(ToolcallKind::PollingInterval),
                    "get_buffer_size" => Some(ToolcallKind::BufferSize),
                    "get_criteria" => Some(ToolcallKind::Criteria),
                    _ => None,
                })
                .collect(),
            ..Default::default()
        };
        const DEFAULT_PROMPT: &str = "I don't like this post.";
        let initial_prompt = if state.checked.is_empty() {
            let literals = [(
                "user_prompt".into(),
                prompt
                    .map(|p| p.to_string())
                    .unwrap_or(DEFAULT_PROMPT.into()),
            )]
            .into_iter()
            .collect();
            template::expand_prompt(include_str!("./prompt.xml"), &literals, &Default::default())
                .expect("initial prompt failed")
        } else {
            [prompt
                .map(|p| p.to_string())
                .unwrap_or(DEFAULT_PROMPT.into())
                .into()]
            .into()
        };

        let this = self.clone();
        let mut dialog = dialog.clone();
        OptimizationCallback::new(async move |action| {
            let state = RwLock::new(state);
            let (tools, handlers) = this
                .get_tools_and_handlers(&state, &action)
                .into_iter()
                .fold((Vec::new(), HashMap::new()), |(mut t, mut h), curr| {
                    h.insert(curr.0.name.to_string(), curr.1);
                    t.push(curr.0);
                    (t, h)
                });
            let mut req = gemma4::DialogRequest::new(gemma4::DialogTurn::User(initial_prompt))
                .with_tools(tools)
                .enable_thinking();
            let runner = this.model.get_runner().await.map_err(Error::Model)?;
            loop {
                let res = runner.get_dialog_continued(&req, &mut dialog).await?;
                event!(Level::DEBUG, "model: {:#?}", res);
                if !res.tool_calls.is_empty() {
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
                    _ = this
                        .dialog_memory
                        .write()
                        .await
                        .update(&dialog)
                        .await
                        .inspect_err(|err| {
                            event!(Level::WARN, "failed to update dialog memory: {err}")
                        });
                    req.set_message(gemma4::DialogTurn::ToolResponses(tool_responses));
                } else if let Some(gemma4::DialogTurn::Assistant(res)) = dialog.turns().last()
                    && res.tool_calls.is_empty()
                {
                    let gave_up = res.content.to_lowercase().contains("give up");
                    let state = state.read().await;
                    if state.retrival_rejected.is_empty() && state.actions_count > 0 || gave_up {
                        event!(Level::DEBUG, "optimization finished");
                        event!(Level::TRACE, "history: {:#?}", dialog.history());
                        break;
                    } else if !state.retrival_rejected.is_empty() {
                        let msg = format!(
                            concat!(
                                "System: there {} {} error{} to be resolved. Presumably you did not finish your tasks well. ",
                                "Please try again. However, if this is intentional, respond with `give up`"
                            ),
                            if state.retrival_rejected.len() == 1 {
                                "is"
                            } else {
                                "are"
                            },
                            state.retrival_rejected.len(),
                            if state.retrival_rejected.len() == 1 {
                                ""
                            } else {
                                "s"
                            },
                        );
                        req.set_message(gemma4::DialogTurn::User(vec![msg.into()]));
                    } else {
                        req.set_message(gemma4::DialogTurn::User(vec!["System: though there're no errors, you have effectively taken no actions (no settings changed). Please try again. If intentional, respond with `give up`. If confused, use `request_clarification` for human intervension".into()]));
                    }
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

#[derive(Debug, Error)]
pub enum ToolHandlerError {
    #[error("channel closed")]
    ChannelClosed,
    #[error("clarification request: {0}")]
    ClarificationRequest(anyhow::Error),
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
        format!("user rejected, reason: {reason}")
    } else {
        "the user actively rejected your request".into()
    }
}

impl<T> From<mpsc::error::SendError<T>> for ToolHandlerError {
    fn from(_value: mpsc::error::SendError<T>) -> Self {
        Self::ChannelClosed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ToolcallKind {
    Criteria,
    PollingInterval,
    BufferSize,
}
