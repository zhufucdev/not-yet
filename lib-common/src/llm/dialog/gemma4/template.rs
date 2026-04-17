use std::cell::RefCell;

use llama_cpp_2::model::{LlamaChatTemplate, LlamaModel};
use llama_runner::{
    Gemma4ApplicableChatTemplate, MessageRole,
    mcp::{Gemma4Tool, error::JinjaTemplateError},
    template::ChatTemplate,
};
use minijinja::{Value, context};
use minijinja_contrib::pycompat::unknown_method_callback;

use crate::llm::dialog::gemma4::{DialogTurn, ROLE_TOOL};

#[derive(Clone)]
pub struct DialogTemplate {
    env: minijinja::Environment<'static>,
    message: DialogTurn,
    history: RefCell<Vec<minijinja::Value>>,
}

impl DialogTemplate {
    pub(super) fn new(
        tools: impl IntoIterator<Item = Gemma4Tool>,
        enable_thinking: bool,
        message: DialogTurn,
        history: impl IntoIterator<Item = minijinja::Value>,
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
            history: RefCell::new(history.into_iter().collect()),
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

        let render = template
            .render(context! { messages => self.history.borrow().as_slice(), add_generation_prompt => true })
            .map_err(JinjaTemplateError::Render)?;
        Ok(render)
    }
}

impl Gemma4ApplicableChatTemplate for DialogTemplate {}
