use anyhow::Context;
use llama_runner::Gemma3VisionRunner;

use crate::agent::ConditionMatcher;

mod agent;
mod polling;
mod secure;
mod source;

#[cfg(feature = "tgbot")]
mod bot;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    Ok(())
}
