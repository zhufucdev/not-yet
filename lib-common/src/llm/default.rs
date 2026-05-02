use std::{cell::LazyCell, time::Duration};

use llama_runner::{
    Gemma4VisionRunner, RunnerWithRecommendedSampling, error::CreateLlamaCppRunnerError,
};

use crate::llm::timeout::{ModelProducer, TimedModel};

pub const DEFAULT_MODEL: LazyCell<
    TimedModel<RunnerWithRecommendedSampling<Gemma4VisionRunner>, CreateLlamaCppRunnerError>,
> = LazyCell::new(|| {
    TimedModel::new(
        "gemma4",
        Duration::from_mins(5),
        ModelProducer::new(async || Gemma4VisionRunner::default().await),
    )
});
