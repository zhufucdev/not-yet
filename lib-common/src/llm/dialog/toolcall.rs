use std::collections::HashMap;
use std::hash::Hash;

use futures::future::BoxFuture;

pub struct ToolHandler<'a, Args, Res, Error>(
    Box<dyn Fn(Args) -> BoxFuture<'a, Result<Res, Error>> + Send + Sync + 'a>,
);

pub struct ToolCall<Key, Args> {
    pub tool: Key,
    pub args: Args,
}

pub trait ToolNotFound<Key> {
    fn not_found(tool: Key) -> Self;
}

pub trait FromKeyAndResult<Key, Res> {
    fn from(key: Key, res: Res) -> Self;
}

pub async fn handle_tool_call<'a, ToolKey, Args, Resp, ToolResult, InternalError>(
    tool_call: ToolCall<ToolKey, Args>,
    handlers: &HashMap<ToolKey, ToolHandler<'a, Args, ToolResult, InternalError>>,
) -> Result<Resp, InternalError>
where
    ToolKey: Eq + Hash + Clone,
    Resp: ToolNotFound<ToolKey> + FromKeyAndResult<ToolKey, ToolResult>,
{
    let Some(handler) = handlers.get(&tool_call.tool) else {
        return Ok(Resp::not_found(tool_call.tool));
    };
    Ok(Resp::from(tool_call.tool, handler.0(tool_call.args).await?))
}

impl<'a, Args, Res, Err> ToolHandler<'a, Args, Res, Err> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(Args) -> Fut + Send + Sync + 'a,
        Fut: Future<Output = Result<Res, Err>> + Send + 'a,
    {
        Self(Box::new(move |args| Box::pin(f(args))))
    }
}
