use std::{
    collections::HashSet,
    fmt::{Debug, Display},
    ops::Deref,
    sync::Arc,
};

use ollama_rs::{
    error::OllamaError,
    generation::{
        chat::{ChatMessage, MessageRole},
        parameters::ThinkType,
        tools::Tool,
    },
    history::ChatHistory,
};
use rmcp::model::EmptyObject;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};

use tracing::{Level, event};

use ollama_rs::generation::tools::Result as SystemResult;

use crate::{
    agent::{
        memory::{criteria::CriteriaMemory, dialog::DialogMemory},
        optimize::{
            ApproveOrDeny, ClarificationReqHandler, OptimizationCallback, Optimizer,
            OptimizerAction, ScheduleParamterAccessor, ScheduleParamters,
        },
        template,
    },
    ollama::OllamaSharedChatHistory,
    runner::{OllamaRunner, ollama},
};

pub struct LlmOptimizer<Runner, DiaMem, CriMem, ClarHandler, Schedule> {
    runner: Runner,
    dialog_memory: Arc<RwLock<DiaMem>>,
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

impl<Runner, DiaMem, CriMem, ClarHandler, Schedule>
    LlmOptimizer<Runner, DiaMem, CriMem, ClarHandler, Schedule>
where
    ClarHandler: ClarificationReqHandler,
    Schedule: ScheduleParamterAccessor,
    DiaMem: DialogMemory,
{
    pub fn new(
        runner: Runner,
        dialog_memory: DiaMem,
        criteria_memory: CriMem,
        clarification_handler: ClarHandler,
        schedule: Schedule,
    ) -> Self {
        Self {
            runner,
            dialog_memory: Arc::new(RwLock::new(dialog_memory.into())),
            criteria_memory: Arc::new(RwLock::new(criteria_memory.into())),
            clarification_handler: Arc::new(clarification_handler.into()),
            schedule: Arc::new(RwLock::new(schedule.into())),
        }
    }
}

type Actor = mpsc::Sender<(OptimizerAction, mpsc::Sender<ApproveOrDeny>)>;

// --- Tools ---
struct SetPollingInterval<Schedule> {
    state: Arc<RwLock<State>>,
    action: Arc<Actor>,
    schedule: Arc<RwLock<Schedule>>,
}

struct GetPollingInterval<Schedule> {
    state: Arc<RwLock<State>>,
    schedule: Arc<RwLock<Schedule>>,
}

struct SetBufferSize<Schedule> {
    state: Arc<RwLock<State>>,
    action: Arc<Actor>,
    schedule: Arc<RwLock<Schedule>>,
}

struct GetBufferSize<Schedule> {
    state: Arc<RwLock<State>>,
    schedule: Arc<RwLock<Schedule>>,
}

struct AddCriterion<Criteria> {
    state: Arc<RwLock<State>>,
    action: Arc<Actor>,
    criteria: Arc<RwLock<Criteria>>,
}

struct GetCriteria<Criteria> {
    state: Arc<RwLock<State>>,
    criteria: Arc<RwLock<Criteria>>,
}

struct RequireClarification<Ch> {
    handler: Arc<Ch>,
}

impl<History, DiaMem, CriMem, ClarHandler, Schedule> Optimizer<History>
    for LlmOptimizer<OllamaRunner, DiaMem, CriMem, ClarHandler, Schedule>
where
    History: ChatHistory + Default + Clone + Debug + Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Sync + 'static,
    ClarHandler::Error: Send + Sync + 'static,
    Schedule: ScheduleParamterAccessor + Sync + 'static,
    DiaMem: DialogMemory<Dialog = History> + Send + Sync + 'static,
    DiaMem::Error: Display + Send + Sync + 'static,
    CriMem: CriteriaMemory + Send + Sync + 'static,
    CriMem::Error: Display,
{
    type Error = Error<DiaMem::Error>;

    fn optimize(
        &self,
        prompt: Option<impl ToString + Send>,
        dialog: History,
    ) -> OptimizationCallback<Self::Error> {
        const DEFAULT_PROMPT: &str = "I don't like this post.";
        let prompt = prompt.map(|p| p.to_string());
        event!(Level::TRACE, "prompt = {prompt:?}, history = {:#?}", dialog);

        let state = Arc::new(RwLock::new(State {
            checked: dialog
                .messages()
                .iter()
                .map(|turn| &turn.tool_calls)
                .flatten()
                .map(|tool| tool.function.name.as_str())
                .filter_map(|tool_name| match tool_name {
                    "get_polling_interval" => Some(ToolcallKind::PollingInterval),
                    "get_buffer_size" => Some(ToolcallKind::BufferSize),
                    "get_criteria" => Some(ToolcallKind::Criteria),
                    _ => None,
                })
                .collect(),
            ..Default::default()
        }));
        let history = OllamaSharedChatHistory::new(dialog);
        let mut coordinator = self
            .runner
            .to_coordinator(history.clone())
            .think(ThinkType::High);
        let schedule = self.schedule.clone();
        let dialog_mem = self.dialog_memory.clone();
        let criteria_mem = self.criteria_memory.clone();
        let clarification_handler = self.clarification_handler.clone();
        OptimizationCallback::new(async move |action| {
            let action = Arc::new(action);
            coordinator = coordinator
                .add_tool(SetPollingInterval {
                    state: state.clone(),
                    action: action.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(GetPollingInterval {
                    state: state.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(SetBufferSize {
                    state: state.clone(),
                    action: action.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(GetBufferSize {
                    state: state.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(AddCriterion {
                    state: state.clone(),
                    action: action.clone(),
                    criteria: criteria_mem.clone(),
                })
                .add_tool(GetCriteria {
                    state: state.clone(),
                    criteria: criteria_mem.clone(),
                })
                .add_tool(RequireClarification {
                    handler: clarification_handler.clone(),
                });
            let mut res = coordinator
                .chat({
                    let initial_prompt = if state.try_read().unwrap().checked.is_empty() {
                        let literals = [(
                            "user_prompt".into(),
                            prompt
                                .map(|p| p.to_string())
                                .unwrap_or(DEFAULT_PROMPT.into()),
                        )]
                        .into_iter()
                        .collect();
                        template::expand_prompt(
                            include_str!("./prompt.xml"),
                            &literals,
                            &Default::default(),
                        )
                        .expect("initial prompt failed")
                    } else {
                        [prompt
                            .map(|p| p.to_string())
                            .unwrap_or(DEFAULT_PROMPT.into())
                            .into()]
                        .into()
                    };
                    vec![ollama::chat_message_from_shared(
                        initial_prompt,
                        MessageRole::User,
                    )]
                })
                .await?;

            loop {
                dialog_mem
                    .write()
                    .await
                    .update(history.deref())
                    .await
                    .map_err(Error::Dialog)?;
                let gave_up = res.message.content.to_lowercase().contains("give up");
                let state = state.read().await;
                if state.retrival_rejected.is_empty() && state.actions_count > 0 || gave_up {
                    event!(Level::DEBUG, "optimization finished");
                    event!(Level::TRACE, "history: {:#?}", &history);
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
                    event!(Level::TRACE, "state: {:#?}", state);
                    event!(Level::TRACE, "history: {:#?}", &history);
                    res = coordinator.chat(vec![ChatMessage::user(msg)]).await?;
                } else {
                    let prompt = "System: though there're no errors, you have effectively taken no actions (no settings changed). Please try again. If intentional, respond with `give up`. If confused, use `request_clarification` for human intervension";
                    event!(Level::TRACE, "state: {:#?}", state);
                    event!(Level::TRACE, "history: {:#?}", &history);
                    res = coordinator
                        .chat(vec![ChatMessage::user(prompt.into())])
                        .await?;
                }
            }
            Ok(())
        })
    }
}

#[derive(Debug, Error)]
pub enum Error<DialogErr> {
    #[error("dialog: {0}")]
    Dialog(DialogErr),
    #[error("runner: {0}")]
    Runner(#[from] OllamaError),
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

#[derive(Display)]
pub enum ToolResult {
    #[strum(to_string = "success: {0}")]
    Success(String),
    #[strum(to_string = "failure: {0}")]
    Failure(String),
}

impl<Schedule> Tool for SetPollingInterval<Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync,
{
    type Params = SetPollingIntervalInputParams;

    fn name() -> &'static str {
        "set_polling_interval"
    }

    fn description() -> &'static str {
        "if the user complain about update frequencies, you can change the polling intervals and / or max buffer size. must retrive the current value first"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        if !self
            .state
            .read()
            .await
            .checked
            .contains(&ToolcallKind::PollingInterval)
        {
            self.state
                .write()
                .await
                .retrival_rejected
                .insert(ToolcallKind::PollingInterval);
            return Ok(ToolResult::Failure(
                                "you must check the polling interval before changing it. please use the `get_polling_interval` tool first".into(),
                            )
                            .into());
        }
        let (tx, mut rx) = mpsc::channel(1);
        let new_value = parameters.new_value;
        self.action
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
                self.state.write().await.actions_count += 1;
                Ok(ToolResult::from(
                    self.schedule
                        .write()
                        .await
                        .set_interval_mins(new_value)
                        .await
                        .map(|_| format!("the new interval is {new_value} minutes.")),
                )
                .into())
            }
            Some(ApproveOrDeny::Deny { reason }) => Ok(reject_message(reason).into()),
            None => Err(Box::new(ToolHandlerError::ChannelClosed)),
        }
    }
}

impl<Schedule> Tool for GetPollingInterval<Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync,
{
    type Params = EmptyObject;

    fn name() -> &'static str {
        "get_polling_interval"
    }

    fn description() -> &'static str {
        "current polling interval in minutes"
    }

    async fn call(&mut self, _: Self::Params) -> SystemResult<String> {
        let mut state = self.state.write().await;
        state.checked.insert(ToolcallKind::PollingInterval);
        state
            .retrival_rejected
            .remove(&ToolcallKind::PollingInterval);
        Ok(self
            .schedule
            .read()
            .await
            .get_interval_mins()
            .await
            .to_string())
    }
}

impl<Schedule> Tool for SetBufferSize<Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync,
{
    type Params = SetBufferSizeInputParams;

    fn name() -> &'static str {
        "set_buffer_size"
    }

    fn description() -> &'static str {
        "a buffer is where polled updates are staged, discarding overflowing ones. A smaller buffer would suit rapid-changing feeds, while a bigger one preserves more details. Must retrive the current value first"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        if !self
            .state
            .read()
            .await
            .checked
            .contains(&ToolcallKind::BufferSize)
        {
            self.state
                .write()
                .await
                .retrival_rejected
                .insert(ToolcallKind::BufferSize);
            return Ok(
                ToolResult::Failure(
                    "you must check the buffer size before changing it. please use the `get_buffer_size` tool first".into()
                ).into()
            );
        }
        let new_value = parameters.new_value as usize;
        let (tx, mut rx) = mpsc::channel(1);
        self.action
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
                self.state.write().await.actions_count += 1;
                Ok(ToolResult::from(
                    self.schedule
                        .write()
                        .await
                        .set_buffer_size(new_value)
                        .await
                        .map(|_| format!("the new buffer size is {new_value}")),
                )
                .into())
            }
            Some(ApproveOrDeny::Deny { reason }) => Ok(reject_message(reason).into()),
            None => Err(Box::new(ToolHandlerError::ChannelClosed)),
        }
    }
}

impl<Schedule> Tool for GetBufferSize<Schedule>
where
    Schedule: ScheduleParamterAccessor + Send + Sync,
{
    type Params = EmptyObject;

    fn name() -> &'static str {
        "get_buffer_size"
    }

    fn description() -> &'static str {
        "get the current buffer size"
    }

    async fn call(&mut self, _: Self::Params) -> SystemResult<String> {
        let mut state = self.state.write().await;
        state.checked.insert(ToolcallKind::BufferSize);
        state.retrival_rejected.remove(&ToolcallKind::BufferSize);
        Ok(self
            .schedule
            .read()
            .await
            .get_buffer_size()
            .await
            .to_string())
    }
}

impl<Criteria> Tool for AddCriterion<Criteria>
where
    Criteria: CriteriaMemory + Send + Sync,
    Criteria::Error: Display,
{
    type Params = AddCriteriaInputParams;

    fn name() -> &'static str {
        "add_criteria"
    }

    fn description() -> &'static str {
        "add reflections on this interaction, be it clarification on a definition, notes on user's preferences, or tips to improve judgemental accuracy in general; keep it brief and to the point"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        if self.criteria.read().await.is_empty().await {
            self.state
                .write()
                .await
                .checked
                .insert(ToolcallKind::Criteria);
        } else if !self
            .state
            .read()
            .await
            .checked
            .contains(&ToolcallKind::Criteria)
        {
            self.state
                .write()
                .await
                .retrival_rejected
                .insert(ToolcallKind::Criteria);
            return Ok(ToolResult::Failure(
                                "you must check the criteria list before changing it. please use the `get_criteria` tool first".into(),
                            )
                            .into());
        }
        let content = parameters.content;
        let (tx, mut rx) = mpsc::channel(1);
        self.action
            .send((OptimizerAction::ContextPrefill(vec![content.clone()]), tx))
            .await?;
        match rx.recv().await {
            Some(ApproveOrDeny::Approve) => {
                if let Err(err) = self.criteria.write().await.add(content).await {
                    Ok(ToolResult::Failure(err.to_string()).into())
                } else {
                    self.state.write().await.actions_count += 1;
                    Ok("criterion added".into())
                }
            }
            Some(ApproveOrDeny::Deny { reason }) => Ok(reject_message(reason).into()),
            None => Err(Box::new(ToolHandlerError::ChannelClosed)),
        }
    }
}

impl<Criteria> Tool for GetCriteria<Criteria>
where
    Criteria: CriteriaMemory + Send + Sync,
    Criteria::Error: Display,
{
    type Params = EmptyObject;

    fn name() -> &'static str {
        "get_criteria"
    }

    fn description() -> &'static str {
        "retrive a list of criteria previously remembered"
    }

    async fn call(&mut self, _: Self::Params) -> SystemResult<String> {
        let mut state = self.state.write().await;
        state.checked.insert(ToolcallKind::Criteria);
        state.retrival_rejected.remove(&ToolcallKind::Criteria);
        let criteria = self.criteria.read().await;
        let criteria = match criteria.get().await {
            Err(err) => {
                return Ok(ToolResult::Failure(err.to_string()).into());
            }
            Ok(criteria) => criteria,
        };
        if criteria.is_empty() {
            return Ok("[]".into());
        }
        Ok(format!(
            "- {}",
            criteria
                .into_iter()
                .map(|c| c.as_ref().to_string())
                .collect::<Vec<_>>()
                .join("\n- ")
        ))
    }
}

impl<Ch> Tool for RequireClarification<Ch>
where
    Ch: ClarificationReqHandler + Send + Sync + 'static,
    Ch::Error: Send + Sync + 'static,
{
    type Params = RequestClarificationInputParams;

    fn name() -> &'static str {
        "request_clarification"
    }

    fn description() -> &'static str {
        "ask the user a question, and get text answers"
    }

    async fn call(&mut self, parameters: Self::Params) -> SystemResult<String> {
        event!(Level::TRACE, "RequireClarification tool called");
        if let Some(clarification) = self
            .handler
            .on_request(parameters.question.as_str())
            .await
            .map_err(anyhow::Error::from)
            .map_err(ToolHandlerError::ClarificationRequest)?
        {
            event!(Level::TRACE, "got clarification: {clarification}");
            Ok(clarification.into())
        } else {
            event!(Level::TRACE, "got clarification: rejected");
            Ok("the user actively rejected your request".into())
        }
    }
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
struct AddCriteriaInputParams {
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

impl<T, E> From<Result<T, E>> for ToolResult
where
    T: Display,
    E: Display,
{
    fn from(value: Result<T, E>) -> Self {
        match value {
            Ok(t) => Self::Success(t.to_string()),
            Err(e) => Self::Failure(e.to_string()),
        }
    }
}

impl Into<String> for ToolResult {
    fn into(self) -> String {
        self.to_string()
    }
}
