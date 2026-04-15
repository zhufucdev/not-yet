use std::{cell::RefCell, sync::Arc};

use llama_cpp_2::model::{LlamaChatTemplate, LlamaModel};
use llama_runner::{
    Gemma4ApplicableChatTemplate, GenericRunnerRequest, MessageRole, VisionLmRunner,
    error::GenericRunnerError,
    mcp::{
        Gemma4Tool,
        error::{JinjaTemplateError, ParseToolError},
        model::Tool,
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
    dialog::{DialogRequest, MultiTurnDialog, MultiTurnDialogEnabled, parse},
};

const ROLE_TOOL: &'static str = "tool";

#[derive(Clone, Debug)]
pub enum DialogTurn {
    System(Vec<SharedImageOrText>),
    User(Vec<SharedImageOrText>),
    Assistant(AssistantResponse),
    ToolResponses(Vec<ToolResponse>),
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolResponse {
    name: String,
    response: minijinja::Value,
}

#[derive(Clone, Debug, Default)]
pub struct ExtraReqParams {
    pub tools: Vec<Tool>,
    pub enable_thinking: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantResponse {
    pub reasoning: Option<String>,
    pub content: String,
    pub tool_calls: Vec<parse::gemmma4::Result<ToolCall>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
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

impl<'d, Runner> MultiTurnDialogEnabled<'d, DialogTemplate> for Runner
where
    for<'s, 'req> Runner: VisionLmRunner<'s, 'req, DialogTemplate> + Send + Sync + 'static,
{
    type Error = Error;
    type Turn = DialogTurn;
    type Response = AssistantResponse;
    type History = Vec<minijinja::Value>;
    type ExtraReq = ExtraReqParams;

    async fn get_dialog_continued(
        self: &Arc<Self>,
        req: &'d DialogRequest<Self::Turn, Self::ExtraReq>,
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
            DialogTurn::Assistant(_) => vec![(MessageRole::Assistant, "".into())],
            DialogTurn::ToolResponses(_) => vec![(MessageRole::Custom(ROLE_TOOL), "".into())],
        };
        let tmpl = DialogTemplate::new(
            req.extra
                .tools
                .iter()
                .map(|tool| {
                    tool.try_into()
                        .map_err(|err| Error::ParseTool(tool.name.clone().into(), err))
                })
                .collect::<Result<Vec<_>, _>>()?,
            req.extra.enable_thinking,
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
        let res_turn = parse::gemmma4::assistant_response(res);
        dialog.turns.push(DialogTurn::Assistant(res_turn.clone()));
        dialog
            .history
            .borrow_mut()
            .push((&res_turn).into());
        Ok(res_turn)
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
        enable_thinking: bool,
        message: DialogTurn,
        history: Arc<RefCell<Vec<minijinja::Value>>>,
    ) -> Self {
        let mut env = minijinja::Environment::new();
        minijinja_contrib::add_to_environment(&mut env);
        env.add_global(
            "tools",
            Value::from_serialize(tools.into_iter().collect::<Vec<_>>()),
        );
        if enable_thinking {
            env.add_global("enable_thinking", true);
        }
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
        self.history
            .borrow_mut()
            .extend(messages.iter().map(|(role, cnt)| {
                match role {
                    MessageRole::User | MessageRole::System => {
                        [("role", role.to_string()), ("content", cnt.clone())]
                            .into_iter()
                            .collect()
                    }
                    MessageRole::Custom(ROLE_TOOL) => {
                        let DialogTurn::ToolResponses(res) = &self.message else {
                            panic!("tool message is not tool turn")
                        };
                        [
                            ("role", ROLE_TOOL.into()),
                            (
                                "tool_responses",
                                res.iter()
                                    .map(minijinja::Value::from_serialize)
                                    .collect::<minijinja::Value>(),
                            ),
                        ]
                        .into_iter()
                        .collect()
                    }
                    MessageRole::Custom(_) => panic!("unsupported role"),
                    MessageRole::Assistant => {
                        let DialogTurn::Assistant(res) = &self.message else {
                            panic!("assistant message is not assistant turn")
                        };
                        res.into()
                    }
                }
            }));
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
        [
            (
                "tool_cals",
                self.tool_calls
                    .iter()
                    .filter_map(|call| call.as_ref().ok())
                    .map(minijinja::Value::from_serialize)
                    .collect::<minijinja::Value>(),
            ),
            ("content", self.content.clone().into()),
            ("reasoning", self.reasoning.clone().into()),
            ("role", MessageRole::Assistant.to_string().into()),
        ]
        .into_iter()
        .collect()
    }
}

impl Default for DialogTurn {
    fn default() -> Self {
        Self::User(vec![])
    }
}

impl ToolResponse {
    pub fn new<R>(name: impl ToString, response: R) -> Self
    where
        R: Serialize,
    {
        Self {
            name: name.to_string(),
            response: minijinja::Value::from_serialize(response),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use llama_runner::Gemma4VisionRunner;
    use rmcp::{handler::server::tool::schema_for_type, schemars::JsonSchema};
    use serde::Deserialize;
    use serde_json::json;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn tool_use() {
        let runner = Arc::new(Gemma4VisionRunner::default().await.unwrap());
        let mut dialog = MultiTurnDialog::new();
        #[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
        struct UpdateFavNumberParams {
            /// User's favorite number. Must be an integer.
            favorite_number: i32,
        }
        let mut req = DialogRequest {
            message: DialogTurn::User(vec!["My fav number is 420".into()]),
            extra: ExtraReqParams {
                tools: vec![Tool::new(
                    "update_fav_number",
                    "Update the user's favorite number",
                    schema_for_type::<UpdateFavNumberParams>(),
                )],
                enable_thinking: true,
            },
            ..Default::default()
        };
        let res = runner
            .get_dialog_continued(&req, &mut dialog)
            .await
            .unwrap();
        let AssistantResponse {
            reasoning: _,
            content,
            tool_calls,
        } = res;
        println!("model: {:?}", content);
        let tool_call = tool_calls.first().unwrap().as_ref().unwrap();
        assert!(tool_call.name == "update_fav_number");
        assert!(tool_call.arguments.get("favorite_number").is_some());
        assert_eq!(tool_call.arguments["favorite_number"], json!(420));

        assert_eq!(dialog.turns().len(), 2);

        #[derive(Debug, Clone, Serialize, Deserialize)]
        enum ToolResult<'s> {
            Success(&'s str),
            Failure(&'s str),
        }
        req.message = DialogTurn::ToolResponses(vec![ToolResponse::new(
            "update_fav_number",
            ToolResult::Failure(
                "420 is beyond 99! The favorite_number paramter should be between 0 and 99.",
            ),
        )]);
        let AssistantResponse {
            reasoning: _,
            content,
            tool_calls,
        } = runner
            .get_dialog_continued(&req, &mut dialog)
            .await
            .unwrap();
        println!("model: {:?}", content);
        assert!(tool_calls.is_empty());

        assert_eq!(dialog.turns().len(), 4);

        req.message = DialogTurn::User(vec!["Already! Then 42, please.".into()]);
        let AssistantResponse {
            reasoning: _,
            content,
            tool_calls,
        } = runner
            .get_dialog_continued(&req, &mut dialog)
            .await
            .unwrap();
        println!("model: {:?}", content);
        let tool_call = tool_calls.first().unwrap().as_ref().unwrap();
        assert_eq!(tool_call.name, "update_fav_number");
        assert_eq!(tool_call.arguments["favorite_number"], json!(42));
    }
}
