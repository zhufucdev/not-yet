use std::process::exit;
use tracing::error_span;

#[cfg(feature = "bot")]
use lib_bot as flavor;

#[cfg(not(feature = "bot"))]
use lib_cli as flavor;

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
