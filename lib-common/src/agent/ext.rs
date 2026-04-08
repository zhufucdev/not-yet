use std::sync::Arc;

use async_stream::try_stream;
use futures::{Stream, TryStreamExt};
use llama_runner::{
    GenericRunnerRequest, GenericVisionLmRequest, VisionLmRunner, error::GenericRunnerError,
    template::ChatTemplate,
};

use crate::{agent::template::AsBorrowedMessages, llm::SharedImageOrText};

pub trait RunnerAsyncExt<Tmpl: ChatTemplate> {
    fn stream_vlm_response_async(
        self: Arc<Self>,
        req: GenericRunnerRequest<SharedImageOrText, Tmpl>,
    ) -> impl Stream<Item = Result<String, GenericRunnerError<Tmpl::Error>>>;

    async fn get_vlm_response_async(
        self: Arc<Self>,
        req: GenericRunnerRequest<SharedImageOrText, Tmpl>,
    ) -> Result<String, GenericRunnerError<Tmpl::Error>>;
}

impl<T, Tmpl> RunnerAsyncExt<Tmpl> for T
where
    for<'r, 'req> T: VisionLmRunner<'r, 'req> + 'static,
    Tmpl: ChatTemplate + Send + 'static,
    Tmpl::Error: Send,
{
    fn stream_vlm_response_async(
        self: Arc<Self>,
        req: GenericRunnerRequest<SharedImageOrText, Tmpl>,
    ) -> impl Stream<Item = Result<String, GenericRunnerError<Tmpl::Error>>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let task = {
            let this: UnsafeBox<Arc<T>> = UnsafeBox(self.clone());
            tokio::spawn(async move {
                let mut iter =
                    UnsafeBox(this.as_ref().stream_vlm_response(GenericVisionLmRequest {
                        messages: req.messages.as_ref_msg(),
                        sampling: req.sampling,
                        llguidance: req.llguidance,
                        max_seq: req.max_seq,
                        prefill: req.prefill,
                        tmpl: req.tmpl,
                    }));

                while let Some(result) = iter.as_mut().next() {
                    if let Err(_) = tx.send(result).await {
                        break;
                    }
                }
            })
        };
        try_stream! {
            while let Some(result) = rx.recv().await {
                yield result?;
            }
            task.await.unwrap();
        }
    }

    async fn get_vlm_response_async(
        self: Arc<Self>,
        req: GenericRunnerRequest<SharedImageOrText, Tmpl>,
    ) -> Result<String, GenericRunnerError<Tmpl::Error>>
    where
        Tmpl: ChatTemplate,
    {
        self.stream_vlm_response_async(req).try_collect().await
    }
}

struct UnsafeBox<T>(T);
unsafe impl<T> Send for UnsafeBox<T> {}

impl<T> AsMut<T> for UnsafeBox<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> AsRef<T> for UnsafeBox<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}
