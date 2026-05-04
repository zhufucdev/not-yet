use std::sync::Arc;

use llama_runner::Gemma4VisionRunner;
use ntest::timeout;
use tokio::sync::{RwLock, mpsc};
use tracing::event;
use tracing_test::traced_test;

use crate::{
    agent::{
        memory::{
            criteria::{CriteriaMemory, debug::DebugCriteriaMemory},
            dialog::{DialogMemory, debug::DebugDialogMemory},
        },
        optimize::{
            self, ApproveOrDeny, OptimizationCallback, Optimizer, OptimizerAction,
            gemma4::{ClarificationReqHandler, Gemma4Optimizer, ScheduleParamterAccessor},
        },
    },
    error::NaE,
    llm::{
        dialog::{
            gemma4::{self, ToolResponse},
            toolcall,
        },
        owned::OwnedModel,
    },
    polling::schedule,
};

struct ChannelClarreqHandler {
    req_rx: RwLock<mpsc::Receiver<String>>,
    req_tx: mpsc::Sender<String>,
    res_rx: RwLock<mpsc::Receiver<Option<String>>>,
    res_tx: mpsc::Sender<Option<String>>,
}

impl ChannelClarreqHandler {
    fn new() -> Self {
        let (req_tx, req_rx) = mpsc::channel(1);
        let (res_tx, res_rx) = mpsc::channel(1);
        Self {
            req_rx: RwLock::new(req_rx),
            req_tx,
            res_rx: RwLock::new(res_rx),
            res_tx,
        }
    }
}

impl ClarificationReqHandler for ChannelClarreqHandler {
    type Error = NaE;

    async fn on_request(&self, prompt: &str) -> Result<Option<String>, Self::Error> {
        self.req_tx.send(prompt.to_string()).await;
        Ok(self.res_rx.write().await.recv().await.unwrap())
    }
}

struct DummySchedule {
    interval_mins: u32,
    buffer_size: usize,
}

impl ScheduleParamterAccessor for DummySchedule {
    type Error = NaE;

    async fn get_interval_mins(&self) -> u32 {
        self.interval_mins
    }
    async fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    async fn set_interval_mins(&mut self, new_value: u32) -> Result<(), Self::Error> {
        self.interval_mins = new_value;
        Ok(())
    }

    async fn set_buffer_size(&mut self, new_value: usize) -> Result<(), Self::Error> {
        self.buffer_size = new_value;
        Ok(())
    }
}

#[tokio::test]
#[traced_test]
#[timeout(1000)]
async fn optimization_callback() {
    let mut callback: OptimizationCallback<()> = OptimizationCallback::new(async |action| {
        let tool_handler = gemma4::ToolHandler::<NaE>::new(|_| {
            let action = action.clone();
            async move {
                let (tx, mut rx) = mpsc::channel(1);
                action
                    .clone()
                    .send((OptimizerAction::ContextPrefill(vec![]), tx))
                    .await
                    .unwrap();
                assert_eq!(rx.recv().await.unwrap(), ApproveOrDeny::Approve);
                Ok(gemma4::ToolResult::Success("success").into())
            }
        });
        let tool_call = toolcall::ToolCall {
            tool: "".to_string(),
            args: serde_json::Map::default(),
        };
        let res: ToolResponse = toolcall::handle_tool_call(
            tool_call,
            &[("".to_string(), tool_handler)].into_iter().collect(),
        )
        .await
        .unwrap();
        assert_eq!(res.name, "");
        Ok(())
    });
    let (opt, app) = callback.accept().await.unwrap().unwrap();
    assert!(matches!(opt, OptimizerAction::ContextPrefill(_)));
    app.send(ApproveOrDeny::Approve).await.unwrap();
}

#[tokio::test]
#[traced_test]
async fn optimize_criteria() {
    let mut dialog_mem = DebugDialogMemory::new();
    dialog_mem
        .update(&rmp_serde::from_slice::<gemma4::Dialog>(include_bytes!("dialog-hn.rmp")).unwrap())
        .await
        .unwrap();
    let criteria_mem = DebugCriteriaMemory::new();
    let clarreq = ChannelClarreqHandler::new();
    let mut schedule = DummySchedule {
        interval_mins: 60,
        buffer_size: usize::MAX,
    };

    let optimizer = Arc::new(Gemma4Optimizer::new(
        OwnedModel::new(Gemma4VisionRunner::default().await.unwrap()),
        dialog_mem,
        criteria_mem,
        clarreq,
        schedule,
    ));

    let mut optimization = optimizer
        .optimize_inplace(
            "this is more related to programming jobs rather than the actvitiy itself".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let (action, approve) = optimization
        .accept()
        .await
        .unwrap()
        .expect("early end of optimization");
    match action {
        OptimizerAction::ContextPrefill(items) => {
            assert!(!items.is_empty());
            approve.send(ApproveOrDeny::Approve).await.unwrap();
        }
        OptimizerAction::Schedule(schedule_paramters) => {
            panic!("should not go schedule branch")
        }
    }
    assert!(optimization.accept().await.unwrap().is_none());
}
