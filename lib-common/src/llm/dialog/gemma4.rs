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

#[derive(Clone, Debug, Default)]
pub struct ExtraReqParams {
    pub tools: Vec<Tool>,
    pub enable_thinking: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantResponse {
    pub reasoning: Option<String>,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCall {
    name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
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
    type ExtraReq = ExtraReqParams;

    async fn get_dialog_continued(
        self: Arc<Self>,
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
            DialogTurn::ToolResponse(_) => vec![(MessageRole::Custom(ROLE_TOOL), "".into())],
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
        let res_turn = parse::assistant_response(res);
        dialog.turns.push(DialogTurn::Assistant(res_turn.clone()));
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

/// This module is partially generated by Claude
mod parse {
    use serde_json::{Map, Value};
    use tracing::{Level, event};

    use crate::llm::dialog;

    pub fn assistant_response(res: impl AsRef<str>) -> super::AssistantResponse {
        let (mut rest, reasoning) =
            dialog::parse::tag(res.as_ref(), "<|channel>thought", "<channel|>")
                .map(|(rest, rea)| (rest, Some(rea.to_string())))
                .unwrap_or((res.as_ref().to_string(), None));
        let mut tool_calls = Vec::new();
        while let Some((rest_, tool_call)) =
            dialog::parse::tag(&rest, "<|tool_call>call:", "<tool_call|>")
        {
            if let Some(tool_call) = parse_function_call(tool_call) {
                tool_calls.push(tool_call);
            } else {
                event!(Level::WARN, "failed to parse tool call: {tool_call}");
            }
            rest = rest_;
        }
        super::AssistantResponse {
            reasoning,
            content: rest,
            tool_calls,
        }
    }

    fn parse_function_call(res: impl AsRef<str>) -> Option<super::ToolCall> {
        let res = res.as_ref();
        let name_arg_rep = regex::Regex::new(r#"call:([^{]*)\{([^}]*)\}"#).unwrap();
        let name_arg_groups = name_arg_rep.captures(res)?;
        let (name, args) = (&name_arg_groups[1], &name_arg_groups[2]);
        Some(super::ToolCall {
            name: name.to_string(),
            arguments: parse_argument(args),
        })
    }

    /// Parses a string encoded by the Jinja `format_argument` macro.
    /// The macro is essentially JSON but with `<|"|>` used instead of `"` for string delimiters.
    fn parse_argument(res: impl AsRef<str>) -> Map<String, Value> {
        let input = res.as_ref().trim();
        let (value, _) = parse_value(input);
        match value {
            Value::Object(map) => map,
            _ => panic!("Top-level value must be an object, got: {input}"),
        }
    }

    /// Recursively parse a single value from the front of `s`.
    /// Returns `(parsed_value, remaining_input)`.
    fn parse_value(s: &str) -> (Value, &str) {
        let s = s.trim_start();

        if s.starts_with("<|\"|>") {
            // String value
            let rest = &s["<|\"|>".len()..];
            let end = rest.find("<|\"|>").expect("Unterminated string");
            let string_val = &rest[..end];
            let remaining = &rest[end + "<|\"|>".len()..];
            (Value::String(string_val.to_string()), remaining)
        } else if s.starts_with('{') {
            // Object / mapping
            parse_object(&s[1..])
        } else if s.starts_with('[') {
            // Array / sequence
            parse_array(&s[1..])
        } else if s.starts_with("true") {
            (Value::Bool(true), &s["true".len()..])
        } else if s.starts_with("false") {
            (Value::Bool(false), &s["false".len()..])
        } else {
            // Numeric fallback — consume until a structural character
            let end = s
                .find(|c: char| matches!(c, ',' | '}' | ']'))
                .unwrap_or(s.len());
            let token = s[..end].trim();
            let remaining = &s[end..];
            if let Ok(n) = token.parse::<i64>() {
                (Value::Number(n.into()), remaining)
            } else if let Ok(f) = token.parse::<f64>() {
                (
                    Value::Number(serde_json::Number::from_f64(f).expect("finite float")),
                    remaining,
                )
            } else if token == "null" || token.is_empty() {
                (Value::Null, remaining)
            } else {
                panic!("Unexpected token: {token:?}");
            }
        }
    }

    /// Parse the inside of an object after the opening `{`.
    /// Returns `(Value::Object(...), remaining_after_closing_brace)`.
    fn parse_object(mut s: &str) -> (Value, &str) {
        let mut map = Map::new();

        loop {
            s = s.trim_start();

            if s.starts_with('}') {
                return (Value::Object(map), &s[1..]);
            }

            // Keys are always wrapped in `<|"|>` (escape_keys=True by default)
            assert!(
                s.starts_with("<|\"|>"),
                "Expected key delimiter, got: {s:?}"
            );
            let key_start = "<|\"|>".len();
            let rest = &s[key_start..];
            let key_end = rest.find("<|\"|>").expect("Unterminated key");
            let key = rest[..key_end].to_string();
            s = &rest[key_end + "<|\"|>".len()..];

            // Consume the `:` separator
            s = s.trim_start();
            assert!(s.starts_with(':'), "Expected ':', got: {s:?}");
            s = &s[1..];

            // Parse the value
            let (value, rest) = parse_value(s);
            map.insert(key, value);
            s = rest.trim_start();

            // Optional comma between entries
            if s.starts_with(',') {
                s = &s[1..];
            }
        }
    }

    /// Parse the inside of an array after the opening `[`.
    /// Returns `(Value::Array(...), remaining_after_closing_bracket)`.
    fn parse_array(mut s: &str) -> (Value, &str) {
        let mut items = Vec::new();

        loop {
            s = s.trim_start();

            if s.starts_with(']') {
                return (Value::Array(items), &s[1..]);
            }

            let (item, rest) = parse_value(s);
            items.push(item);
            s = rest.trim_start();

            // Optional comma between items
            if s.starts_with(',') {
                s = &s[1..];
            }
        }
    }

    // ─── tests ───────────────────────────────────────────────────────────────────

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn obj(v: serde_json::Value) -> Map<String, serde_json::Value> {
            match v {
                Value::Object(m) => m,
                _ => panic!("not an object"),
            }
        }

        #[test]
        fn simple_string_value() {
            // { "key": "hello" }  →  {<|"|>key<|"|>:<|"|>hello<|"|>}
            let input = r#"{<|"|>key<|"|>:<|"|>hello<|"|>}"#;
            let result = parse_argument(input);
            assert_eq!(result["key"], json!("hello"));
        }

        #[test]
        fn boolean_values() {
            let input = r#"{<|"|>a<|"|>:true,<|"|>b<|"|>:false}"#;
            let result = parse_argument(input);
            assert_eq!(result["a"], json!(true));
            assert_eq!(result["b"], json!(false));
        }

        #[test]
        fn numeric_value() {
            let input = r#"{<|"|>count<|"|>:42}"#;
            let result = parse_argument(input);
            assert_eq!(result["count"], json!(42));
        }

        #[test]
        fn nested_object() {
            // { "outer": { "inner": "val" } }
            let input = r#"{<|"|>outer<|"|>:{<|"|>inner<|"|>:<|"|>val<|"|>}}"#;
            let result = parse_argument(input);
            assert_eq!(result["outer"], json!({"inner": "val"}));
        }

        #[test]
        fn array_of_strings() {
            let input = r#"{<|"|>items<|"|>:[<|"|>x<|"|>,<|"|>y<|"|>,<|"|>z<|"|>]}"#;
            let result = parse_argument(input);
            assert_eq!(result["items"], json!(["x", "y", "z"]));
        }

        #[test]
        fn complex_nested() {
            // Mirrors what format_argument would produce for:
            // { "name": "tool", "args": { "flag": true, "values": [1, 2] } }
            let input = r#"{<|"|>args<|"|>:{<|"|>flag<|"|>:true,<|"|>values<|"|>:[1,2]},<|"|>name<|"|>:<|"|>tool<|"|>}"#;
            let result = parse_argument(input);
            assert_eq!(result["name"], json!("tool"));
            assert_eq!(result["args"], json!({"flag": true, "values": [1, 2]}));
        }
    }
}
