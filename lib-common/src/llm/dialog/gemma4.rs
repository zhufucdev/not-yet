use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, RwLock},
};

use llama_cpp_2::model::{LlamaChatTemplate, LlamaModel};
use llama_runner::{
    Gemma4ApplicableChatTemplate, GenericRunnerRequest, MessageRole, VisionLmRunner,
    error::GenericRunnerError,
    mcp::{
        Gemma4Tool,
        error::{JinjaTemplateError, ParseToolError},
    },
    template::ChatTemplate,
};
use minijinja::{Value, context};
use minijinja_contrib::pycompat::unknown_method_callback;
use serde::Serialize;
use thiserror::Error;

use crate::llm::{
    SharedImageOrText,
    async_runner::RunnerAsyncExt,
    dialog::{DialogRequest, MultiTurnDialog, MultiTurnDialogEnabled},
};

const ROLE_TOOL: &'static str = "tool";

#[derive(Clone, Debug)]
pub enum DialogTurn {
    System(Vec<SharedImageOrText>),
    User(Vec<SharedImageOrText>),
    Assistant(AssistantResponse),
    ToolResponse(minijinja::Value),
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantResponse {
    pub reasoning: Option<String>,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolCall {
    name: String,
    arguments: HashMap<String, String>,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported role")]
    UnsupportedRole,
    #[error(transparent)]
    Runner(#[from] GenericRunnerError<JinjaTemplateError>),
    #[error("parse tool {0}: {1}")]
    ParseTool(String, #[source] ParseToolError),
}

#[derive(Serialize)]
struct MsgProxy<'s> {
    role: &'s str,
    content: &'s str,
    /// tool response only
    name: Option<&'s str>,
    /// assistant only
    reasoning: Option<&'s str>,
}

impl<'d, Runner> MultiTurnDialogEnabled<'d, DialogTemplate> for Runner
where
    for<'s, 'req> Runner: VisionLmRunner<'s, 'req, DialogTemplate> + Send + Sync + 'static,
{
    type Error = Error;
    type Turn = DialogTurn;
    type Response = AssistantResponse;
    type History = Vec<minijinja::Value>;

    async fn get_dialog_continued(
        self: Arc<Self>,
        req: &'d DialogRequest<Self::Turn>,
        dialog: &'d mut MultiTurnDialog<Self::Turn, Self::History>,
    ) -> Result<Self::Response, Self::Error> {
        let new_messages = match &req.message {
            DialogTurn::System(msg) => msg
                .into_iter()
                .map(|m| (MessageRole::System, m.clone()))
                .collect::<Vec<_>>(),
            DialogTurn::User(msg) => msg
                .into_iter()
                .map(|m| (MessageRole::User, m.clone()))
                .collect(),
            DialogTurn::Assistant(res) => vec![(MessageRole::Assistant, "".into())],
            DialogTurn::ToolResponse(value) => vec![(MessageRole::Custom(ROLE_TOOL), "".into())],
        };
        let tmpl = DialogTemplate::new(
            req.tools
                .iter()
                .map(|tool| {
                    tool.try_into()
                        .map_err(|err| Error::ParseTool(tool.name.clone().into(), err))
                })
                .collect::<Result<Vec<_>, _>>()?,
            req.message.clone(),
            dialog.history(),
        );
        let res = self
            .get_vlm_response_async(GenericRunnerRequest {
                tmpl: tmpl,
                messages: new_messages,
                sampling: req.sampling.clone(),
                llguidance: req.llguidance.clone(),
                max_seq: req.max_seq.clone(),
                prefill: req.prefill.clone(),
            })
            .await?;
        dialog.turns.push(req.message.clone());

        todo!()
    }
}

#[derive(Clone)]
pub struct DialogTemplate {
    env: minijinja::Environment<'static>,
    message: DialogTurn,
    history: Arc<RefCell<Vec<minijinja::Value>>>,
}

impl DialogTemplate {
    fn new(
        tools: impl IntoIterator<Item = Gemma4Tool>,
        message: DialogTurn,
        history: Arc<RefCell<Vec<minijinja::Value>>>,
    ) -> Self {
        let mut env = minijinja::Environment::new();
        minijinja_contrib::add_to_environment(&mut env);
        env.add_global(
            "tools",
            Value::from_serialize(tools.into_iter().collect::<Vec<_>>()),
        );
        env.set_unknown_method_callback(unknown_method_callback);
        Self {
            env,
            message,
            history,
        }
    }
}

impl ChatTemplate for DialogTemplate {
    type Error = JinjaTemplateError;

    fn apply_template(
        &self,
        _model: &LlamaModel,
        model_tmpl: &LlamaChatTemplate,
        messages: &[(MessageRole, String)],
    ) -> Result<String, Self::Error> {
        let template = self
            .env
            .template_from_str(model_tmpl.to_str()?)
            .map_err(JinjaTemplateError::Parse)?;
        self.history.borrow_mut().extend(
            messages
                .iter()
                .map(|(role, cnt)| match role {
                    MessageRole::User | MessageRole::System => {
                        [("role", role.to_string()), ("content", cnt.clone())]
                            .into_iter()
                            .collect()
                    }
                    MessageRole::Custom("tool") => {
                        let DialogTurn::ToolResponse(tool) = &self.message else {
                            panic!("tool message is not tool turn")
                        };
                        tool.clone()
                    }
                    MessageRole::Custom(_) => panic!("unsupported role"),
                    MessageRole::Assistant => {
                        let DialogTurn::Assistant(res) = &self.message else {
                            panic!("assistant message is not assistant turn")
                        };
                        res.into()
                    }
                })
                .collect::<Vec<_>>(),
        );
        let msg_guard = self.history.borrow();
        let messages = msg_guard.as_slice();

        let render = template
            .render(context! { messages => messages, add_generation_prompt => true })
            .map_err(JinjaTemplateError::Render)?;
        Ok(render)
    }
}

impl Gemma4ApplicableChatTemplate for DialogTemplate {}

impl Into<minijinja::Value> for &AssistantResponse {
    fn into(self) -> minijinja::Value {
        minijinja::Value::from_serialize(self)
            .as_object()
            .unwrap()
            .try_iter_pairs()
            .unwrap()
            .chain([("role".into(), MessageRole::Assistant.to_string().into())])
            .into_iter()
            .collect()
    }
}

impl Default for DialogTurn {
    fn default() -> Self {
        Self::User(vec![])
    }
}
