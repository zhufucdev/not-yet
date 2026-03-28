use std::process::exit;
use tracing::error_span;

mod agent;
mod bot;
mod cli;
mod polling;
mod secure;
mod serde_utils;
mod source;
mod update;

#[cfg(feature = "tgbot")]
use bot as flavor;

#[cfg(not(feature = "tgbot"))]
use cli as flavor;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let span = error_span!("main");
    let _guard = span.enter();
    match flavor::main().await {
        Ok(()) => {}
        Err(err) => {
            tracing::error!("main thread existed unexpectedly: {err}");
            exit(1);
        }
    }
}
