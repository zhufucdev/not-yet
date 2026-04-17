use std::sync::Arc;

use llama_runner::{GenericRunnerRequest, MessageRole, VisionLmRunner};
use serde::{Deserialize, Serialize};

use crate::llm::{
    SharedImageOrText,
    async_runner::RunnerAsyncExt,
    dialog::{
        DialogRequest as _, MultiTurnDialog, MultiTurnDialogEnabled, WithLlguidance, WithMaxSeq,
        WithPrefill, WithSampling,
        gemma4::{
            DialogRequest, ROLE_TOOL, assistant::AssistantResponse, error::Error,
            template::DialogTemplate, tool::ToolResponse,
        },
        parse,
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DialogTurn {
    System(Vec<SharedImageOrText>),
    User(Vec<SharedImageOrText>),
    Assistant(AssistantResponse),
    ToolResponses(Vec<ToolResponse>),
}

impl<'d, Runner> MultiTurnDialogEnabled<'d, DialogTemplate> for Runner
where
    for<'s, 'req> Runner: VisionLmRunner<'s, 'req, DialogTemplate> + Send + Sync + 'static,
{
    type Error = Error;
    type Turn = DialogTurn;
    type Response = AssistantResponse;
    type History = Vec<minijinja::Value>;
    type Request = DialogRequest;

    async fn get_dialog_continued(
        self: &Arc<Self>,
        req: &'d Self::Request,
        dialog: &'d mut MultiTurnDialog<Self::Turn, Self::History>,
    ) -> Result<Self::Response, Self::Error> {
        let new_messages = match req.get_message() {
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
            req.get_tools()
                .iter()
                .map(|tool| {
                    tool.try_into()
                        .map_err(|err| Error::ParseTool(tool.name.clone().into(), err))
                })
                .collect::<Result<Vec<_>, _>>()?,
            req.is_thinking(),
            req.get_message().clone(),
            dialog.history().clone(),
        );
        let res = self
            .get_vlm_response_async(GenericRunnerRequest {
                tmpl: tmpl,
                messages: new_messages,
                sampling: req.get_sampling().clone(),
                llguidance: req.get_llguidance().cloned(),
                max_seq: req.get_max_seq().clone(),
                prefill: req.get_prefill().cloned(),
            })
            .await?;
        dialog.turns.push(req.get_message().clone());
        let res_turn = parse::gemmma4::assistant_response(res);
        dialog.turns.push(DialogTurn::Assistant(res_turn.clone()));
        dialog.history.push((&res_turn).into());
        Ok(res_turn)
    }
}

impl Default for DialogTurn {
    fn default() -> Self {
        Self::User(vec![])
    }
}
