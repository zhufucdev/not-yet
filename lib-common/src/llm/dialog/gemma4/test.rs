use std::sync::Arc;

use crate::llm::{
    self, Model,
    dialog::{
        DialogRequest as _, MultiTurnDialog, MultiTurnDialogEnabled, WithMaxSeq,
        gemma4::{
            DialogRequest, DialogTurn, ToolResponse, assistant::AssistantResponse, tool::ToolResult,
        },
    },
};

use rmcp::{handler::server::tool::schema_for_type, model::Tool, schemars::JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing_test::traced_test;

#[tokio::test]
#[traced_test]
async fn tool_use() {
    let runner = llm::DEFAULT_MODEL.clone().get_runner().await.unwrap();
    let mut dialog = MultiTurnDialog::new();
    #[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
    struct UpdateFavNumberParams {
        /// User's favorite number. Must be an integer.
        favorite_number: i32,
    }

    let mut req = DialogRequest::new(DialogTurn::User(vec!["My fav number is 420".into()]))
        .with_max_seq(3)
        .with_tools([Tool::new(
            "update_fav_number",
            "Change the user's favorite number",
            schema_for_type::<UpdateFavNumberParams>(),
        )]);

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

    req.set_message(DialogTurn::ToolResponses(vec![ToolResponse::new(
        "update_fav_number",
        ToolResult::Failure(
            "420 is beyond 99! The favorite_number paramter should be between 0 and 99.",
        ),
    )]));
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

    req.set_message(DialogTurn::User(vec!["Already! Then 42, please.".into()]));
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
