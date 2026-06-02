use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    pin::Pin,
    sync::Arc,
};

use ollama_rs::{
    error::OllamaError,
    generation::{
        chat::{ChatMessage, MessageRole},
        parameters::ThinkType,
        tools::{
            Parameters, Tool as OllamaTool, ToolCallFunction, ToolFunctionInfo,
            ToolHolder as OllamaToolHolder, ToolInfo, ToolType,
        },
    },
};
use rmcp::model::EmptyObject;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};

use tracing::{Level, event};

pub use ollama_rs::generation::tools::Result as SystemResult;

use crate::{
    agent::{
        memory::{criteria::CriteriaMemory, dialog::DialogMemory},
        optimize::{
            Actor, ApproveOrDeny, BasicOptimizerAction, ClarificationReqHandler, OptimizationCallback, Optimizer, OptimizerAction, ScheduleParamterAccessor, ScheduleParamters
        },
        template,
    },
    channel::mpsc::MapSender,
    error::NaE,
    ollama::OllamaSharedChatHistory,
    runner::{
        OllamaRunner,
        ollama::{self, SystemPromptAwareChatHistory},
    },
};

pub struct LlmOptimizer<Runner, DiaMem, CriMem, ClarHandler, Schedule, ExtraAction> {
    runner: Runner,
    dialog_memory: Arc<RwLock<DiaMem>>,
    criteria_memory: Arc<RwLock<CriMem>>,
    clarification_handler: Arc<ClarHandler>,
    schedule: Arc<RwLock<Schedule>>,
    extra_tools: HashMap<&'static str, (ExtraToolHolder<ExtraAction>, ToolInfo, ExtraToolInfo)>,
}

#[derive(Debug, Clone)]
struct ExtraToolInfo {
    is_action: bool,
    retriever_tool_name: Option<&'static str>,
}

#[derive(Clone)]
struct ExtraToolHolder<Action>(Arc<RwLock<Box<dyn ToolHolder<Action> + 'static>>>);

#[derive(Clone)]
struct StatefulExtraToolHolder<Action> {
    action: Actor<Action>,
    inner: Arc<RwLock<Box<dyn ToolHolder<Action> + 'static>>>,
    state: Arc<RwLock<State>>,
    info: ExtraToolInfo,
    name: &'static str,
}

pub trait Tool<Action>: Send + Sync {
    const IS_ACTION: bool;
    const RETRIEVER_TOOL_NAME: Option<&'static str>;

    type Params: Parameters;

    fn name() -> &'static str;
    fn description() -> &'static str;

    /// Call the tool.
    /// Note that returning an Err will cause it to be bubbled up. If you want the LLM to handle the error,
    /// return that error as a string.
    fn call(
        &mut self,
        parameters: Self::Params,
        action: Actor<Action>,
    ) -> impl Future<Output = SystemResult<ToolResult>> + Send;
}

trait ToolHolder<Action>: Send + Sync {
    fn call(
        &mut self,
        parameters: Value,
        action: Actor<Action>,
    ) -> Pin<Box<dyn Future<Output = SystemResult<ToolResult>> + '_ + Send>>;
}

#[derive(Debug, Default, Clone)]
struct State {
    checked: HashSet<ToolcallKind>,
    mod_rejected: HashSet<ToolcallKind>,
    actions_count: u32,
}

impl<Runner, DiaMem, CriMem, ClarHandler, Schedule, ExtraAction>
    LlmOptimizer<Runner, DiaMem, CriMem, ClarHandler, Schedule, ExtraAction>
where
    ClarHandler: ClarificationReqHandler,
    Schedule: ScheduleParamterAccessor,
    DiaMem: DialogMemory,
    ExtraAction: Send + 'static,
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
            extra_tools: HashMap::new(),
        }
    }

    pub fn add_tool<T: Tool<ExtraAction> + 'static>(mut self, tool: T) -> Self {
        let mut settings = SchemaSettings::draft07();
        settings.inline_subschemas = true;
        let generator = settings.into_generator();

        let parameters = generator.into_root_schema_for::<T::Params>();
        event!(
            Level::DEBUG,
            "extra tool: {}, schema: {parameters:#?}",
            T::name()
        );

        let info = ToolInfo {
            tool_type: ToolType::Function,
            function: ToolFunctionInfo {
                name: T::name().to_string(),
                description: T::description().to_string(),
                parameters,
            },
        };

        self.extra_tools.insert(
            T::name(),
            (
                ExtraToolHolder(Arc::new(RwLock::new(Box::new(tool)))),
                info,
                ExtraToolInfo {
                    is_action: T::IS_ACTION,
                    retriever_tool_name: T::RETRIEVER_TOOL_NAME,
                },
            ),
        );
        self
    }
}

// --- Tools ---
struct SetPollingInterval<Schedule> {
    state: Arc<RwLock<State>>,
    action: Actor<BasicOptimizerAction>,
    schedule: Arc<RwLock<Schedule>>,
}

struct GetPollingInterval<Schedule> {
    state: Arc<RwLock<State>>,
    schedule: Arc<RwLock<Schedule>>,
}

struct SetBufferSize<Schedule> {
    state: Arc<RwLock<State>>,
    action: Actor<BasicOptimizerAction>,
    schedule: Arc<RwLock<Schedule>>,
}

struct GetBufferSize<Schedule> {
    state: Arc<RwLock<State>>,
    schedule: Arc<RwLock<Schedule>>,
}

struct AddCriterion<Criteria> {
    state: Arc<RwLock<State>>,
    action: Actor<BasicOptimizerAction>,
    criteria: Arc<RwLock<Criteria>>,
}

struct GetCriteria<Criteria> {
    state: Arc<RwLock<State>>,
    criteria: Arc<RwLock<Criteria>>,
}

struct RequireClarification<Ch> {
    handler: Arc<Ch>,
}

impl<History, DiaMem, CriMem, ClarHandler, Schedule, ExtraAction> Optimizer<History>
    for LlmOptimizer<OllamaRunner, DiaMem, CriMem, ClarHandler, Schedule, ExtraAction>
where
    History: SystemPromptAwareChatHistory + Default + Clone + Debug + Send + Sync + 'static,
    ClarHandler: ClarificationReqHandler + Sync + 'static,
    ClarHandler::Error: Send + Sync + 'static,
    Schedule: ScheduleParamterAccessor + Sync + 'static,
    DiaMem: DialogMemory<Dialog = History> + Send + Sync + 'static,
    DiaMem::Error: Display + Send + Sync + 'static,
    CriMem: CriteriaMemory + Send + Sync + 'static,
    CriMem::Error: Display,
    ExtraAction: Send + Clone + 'static,
{
    type Error = Error<DiaMem::Error>;
    type ExtraAction = ExtraAction;

    fn optimize(
        &self,
        prompt: Option<impl ToString + Send>,
        dialog: History,
    ) -> OptimizationCallback<Self::Error, Self::ExtraAction> {
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
        let extra_tools = self.extra_tools.clone();
        OptimizationCallback::new(async move |action| {
            let basic_action = action.clone().map(
                |(boa, re): (BasicOptimizerAction, mpsc::Sender<ApproveOrDeny>)| {
                    (OptimizerAction::Basic(boa), re)
                },
            );
            coordinator = coordinator
                .add_tool(SetPollingInterval {
                    state: state.clone(),
                    action: basic_action.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(GetPollingInterval {
                    state: state.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(SetBufferSize {
                    state: state.clone(),
                    action: basic_action.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(GetBufferSize {
                    state: state.clone(),
                    schedule: schedule.clone(),
                })
                .add_tool(AddCriterion {
                    state: state.clone(),
                    action: basic_action.clone(),
                    criteria: criteria_mem.clone(),
                })
                .add_tool(GetCriteria {
                    state: state.clone(),
                    criteria: criteria_mem.clone(),
                })
                .add_tool(RequireClarification {
                    handler: clarification_handler.clone(),
                });
            let extra_action =
                action
                    .clone()
                    .map(|(eoa, re): (ExtraAction, mpsc::Sender<ApproveOrDeny>)| {
                        (OptimizerAction::Extra(eoa), re)
                    });
            for (name, (holder, tool_info, extra)) in extra_tools.iter() {
                coordinator = coordinator.add_tool_holder(
                    name,
                    tool_info.clone(),
                    Box::new(StatefulExtraToolHolder::<ExtraAction> {
                        action: extra_action.clone(),
                        inner: holder.0.clone(),
                        info: extra.clone(),
                        state: state.clone(),
                        name: name,
                    }),
                );
            }

            if history.borrow().system_prompt().is_none() {
                let system_prompt = ollama::chat_message_from_shared(
                    template::expand_prompt::<NaE>(
                        include_str!("./system_prompt.xml"),
                        &Default::default(),
                        &Default::default(),
                    )
                    .await
                    .expect("system prompt failed"),
                    MessageRole::System,
                )
                .content;
                history.borrow_mut().update_system_prompt(system_prompt);
            }

            let mut res = coordinator
                .chat({
                    vec![ChatMessage::user(
                        prompt
                            .map(|p| p.to_string())
                            .unwrap_or(DEFAULT_PROMPT.into()),
                    )]
                })
                .await?;

            loop {
                dialog_mem
                    .write()
                    .await
                    // this unsafe is fine because
                    // 1. History is Send
                    // 2. Lifecycle is known
                    .update(unsafe { history.borrow_unguraded().unwrap() })
                    .await
                    .map_err(Error::Dialog)?;
                let gave_up = res.message.content.to_lowercase().contains("give up");
                let state = state.read().await.clone();
                if state.mod_rejected.is_empty() && state.actions_count > 0 || gave_up {
                    event!(Level::DEBUG, "optimization finished");
                    event!(Level::TRACE, "history: {:#?}", &history);
                    break;
                } else if !state.mod_rejected.is_empty() {
                    let msg = format!(
                        concat!(
                            "System: there {} {} error{} to be resolved. Presumably you did not finish your tasks well. ",
                            "Please try again. However, if this is intentional, respond with `give up`"
                        ),
                        if state.mod_rejected.len() == 1 {
                            "is"
                        } else {
                            "are"
                        },
                        state.mod_rejected.len(),
                        if state.mod_rejected.len() == 1 {
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

pub enum ToolResult {
    Success(String),
    Failure(String),
    Rejected { reason: Option<String> },
}

impl<Schedule> OllamaTool for SetPollingInterval<Schedule>
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
                .mod_rejected
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
                BasicOptimizerAction::Schedule(ScheduleParamters {
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

impl<Schedule> OllamaTool for GetPollingInterval<Schedule>
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
        state.mod_rejected.remove(&ToolcallKind::PollingInterval);
        Ok(self
            .schedule
            .read()
            .await
            .get_interval_mins()
            .await
            .to_string())
    }
}

impl<Schedule> OllamaTool for SetBufferSize<Schedule>
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
                .mod_rejected
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
                BasicOptimizerAction::Schedule(ScheduleParamters {
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

impl<Schedule> OllamaTool for GetBufferSize<Schedule>
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
        state.mod_rejected.remove(&ToolcallKind::BufferSize);
        Ok(self
            .schedule
            .read()
            .await
            .get_buffer_size()
            .await
            .to_string())
    }
}

impl<Criteria> OllamaTool for AddCriterion<Criteria>
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
                .mod_rejected
                .insert(ToolcallKind::Criteria);
            return Ok(ToolResult::Failure(
                                "you must check the criteria list before changing it. please use the `get_criteria` tool first".into(),
                            )
                            .into());
        }
        let content = parameters.content;
        let (tx, mut rx) = mpsc::channel(1);
        self.action
            .send((
                BasicOptimizerAction::ContextPrefill(vec![content.clone()]),
                tx,
            ))
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

impl<Criteria> OllamaTool for GetCriteria<Criteria>
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
        state.mod_rejected.remove(&ToolcallKind::Criteria);
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

impl<Ch> OllamaTool for RequireClarification<Ch>
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ToolcallKind {
    Criteria,
    PollingInterval,
    BufferSize,
    Extra(&'static str),
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

impl<T: Tool<Action>, Action> ToolHolder<Action> for T
where
    Action: Send + 'static,
{
    fn call(
        &mut self,
        parameters: Value,
        action: Actor<Action>,
    ) -> Pin<Box<dyn Future<Output = SystemResult<ToolResult>> + '_ + Send>> {
        event!(Level::DEBUG, "model used parameters {parameters}");
        Box::pin(async move {
            // Json returned from the model can sometimes be in different formats, see https://github.com/pepperoni21/ollama-rs/issues/210
            // This is a work-around for this issue.
            let param_value = match serde_json::from_value(parameters.clone()) {
                // We first try with the ToolCallFunction format
                Ok(ToolCallFunction { name: _, arguments }) => arguments,
                Err(_err) => match serde_json::from_value::<ToolInfo>(parameters.clone()) {
                    Ok(ti) => ti.function.parameters.to_value(),
                    Err(_err) => parameters,
                },
            };

            let param = serde_json::from_value(param_value)?;

            T::call(self, param, action).await.map(|r| r.into())
        })
    }
}

impl<Action> OllamaToolHolder for StatefulExtraToolHolder<Action>
where
    Action: Send + 'static,
{
    fn call(
        &mut self,
        parameters: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = SystemResult<String>> + '_ + Send>> {
        Box::pin(async move {
            if let Some(prior) = self.info.retriever_tool_name
                && !self
                    .state
                    .read()
                    .await
                    .checked
                    .contains(&ToolcallKind::Extra(prior))
            {
                return Ok(format!("rejected: you must tool `{prior}` before this one").into());
            }
            match self
                .inner
                .write()
                .await
                .call(parameters, self.action.clone())
                .await
            {
                Ok(res) => {
                    if !self.info.is_action {
                        if let ToolResult::Success(_) = res {
                            self.state
                                .write()
                                .await
                                .checked
                                .insert(ToolcallKind::Extra(self.name));
                        }
                    } else {
                        if let ToolResult::Failure(_) = res {
                            self.state
                                .write()
                                .await
                                .mod_rejected
                                .insert(ToolcallKind::Extra(self.name));
                        } else {
                            self.state.write().await.actions_count += 1;
                        }
                    }
                    Ok(res.into())
                }
                Err(err) => Err(err),
            }
        })
    }
}

pub trait ToToolResult {
    type Success;
    fn map_ok(self, ok: impl FnOnce(Self::Success) -> String) -> ToolResult;
}

impl ToToolResult for ApproveOrDeny {
    type Success = ();

    fn map_ok(self, ok: impl FnOnce(Self::Success) -> String) -> ToolResult {
        match self {
            ApproveOrDeny::Approve => ToolResult::Success(ok(())),
            ApproveOrDeny::Deny { reason } => ToolResult::Rejected { reason },
        }
    }
}

impl Display for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolResult::Success(s) => write!(f, "success: {s}"),
            ToolResult::Failure(e) => write!(f, "failure: {e}"),
            ToolResult::Rejected { reason } => match reason {
                Some(reason) => write!(f, "rejected: {reason}"),
                None => write!(f, "the user actively rejected your request"),
            },
        }
    }
}
