use std::sync::{Arc, RwLock};

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
    pub(super) history: Arc<RwLock<Vec<minijinja::Value>>>,
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
            history: Arc::new(RwLock::new(history.into_iter().collect())),
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
        messages.iter().for_each(|(role, cnt)| match role {
            MessageRole::User | MessageRole::System => {
                self.history
                    .write()
                    .unwrap()
                    .push(minijinja::Value::from_iter([
                        ("role", role.to_string()),
                        ("content", cnt.clone()),
                    ]));
            }
            MessageRole::Custom(ROLE_TOOL) => {
                let mut history = self.history.write().unwrap();
                let Some(last) = history.last_mut() else {
                    panic!("tool response following nothing")
                };
                let DialogTurn::ToolResponses(res) = &self.message else {
                    panic!("tool message is not tool turn")
                };
                *last = last
                    .as_object()
                    .unwrap()
                    .try_iter_pairs()
                    .unwrap()
                    .chain(
                        [(
                            "tool_responses".into(),
                            res.iter()
                                .map(minijinja::Value::from_serialize)
                                .collect::<minijinja::Value>(),
                        )]
                        .into_iter(),
                    )
                    .collect();
            }
            MessageRole::Custom(_) => panic!("unsupported role"),
            MessageRole::Assistant => {
                let DialogTurn::Assistant(res) = &self.message else {
                    panic!("assistant message is not assistant turn")
                };
                self.history.write().unwrap().push(res.into());
            }
        });

        let render = template
            .render(context! { messages => self.history.read().unwrap().as_slice(), add_generation_prompt => true })
            .map_err(JinjaTemplateError::Render)?;
        Ok(render)
    }
}

impl Gemma4ApplicableChatTemplate for DialogTemplate {}
