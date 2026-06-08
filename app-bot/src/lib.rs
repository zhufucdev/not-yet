mod authenticator;
mod config;
mod db;
mod init;
mod rss;
mod token;

#[cfg(feature = "telegram")]
pub mod telegram;

use std::sync::Arc;

use app_common::{
    config::ParseConfigPath,
    poller::{Poller, UpdateContext},
    rss::RssServer,
    verbosity::WithVerbosity,
};
use clap::Parser;
use futures::future;
use lib_common::polling::Scheduler;
#[cfg(feature = "telegram")]
pub use telegram as flavor;

pub use flavor::UserId;
use tracing::{Level, event};
use tracing_subscriber::EnvFilter;

use crate::{config::Config, init::InitResult};

macro_rules! assert_impls {
    ($trait:path, $value:ident) => {{
        #[allow(dead_code)]
        struct AssertImpls<'s, T>(&'s T)
        where
            T: $trait;
        AssertImpls(&$value);
    }};
}

pub async fn main() -> anyhow::Result<()> {
    let args = flavor::Args::parse();
    assert_impls!(WithVerbosity, args);
    assert_impls!(ParseConfigPath, args);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(args.get_verbosity().tracing_level_filter().into())
                .from_env()
                .unwrap_or_default(),
        )
        .init();

    let data_path = args.parse_config_path()?;
    let config = app_common::config::parse_config::<Config>(
        &data_path,
        include_bytes!("../asset/default_config.toml"),
    )
    .await?;

    let app = flavor::init(&args, &config).await?;
    assert_impls!(InitResult, app);

    let scheduler = Arc::new(Scheduler::from_iter(app.get_schedules().await?));
    let poller = Poller::new(UpdateContext {
        #[cfg(feature = "serve-rss")]
        rss_server: Arc::new(app.get_rss_broadcasts().await?.into_iter().collect()),
        #[cfg(not(feature = "serve-rss"))]
        rss_server: Default::default(),
    });
    future::try_join_all(scheduler.schedules().await.iter().map(async |schedule| {
        app.attach_to_poller(poller.transaction().await, schedule.key().clone())
            .await
    }))
    .await?;

    tokio::select! {
        r = app.main(Arc::clone(&scheduler)) => {
            let Err(err) = r else {
                return Ok(());
            };
            event!(Level::ERROR, "app exited: {err}");
        },
        r = poller.poll_all(scheduler) => {
            event!(Level::ERROR, "poller exited with {r:?}");
        },
        Err(err) = async {
            let Some(rss_config) = config.serve_rss.as_ref() else {
                return Ok(());
            };
            let server = RssServer::from_state(
                &rss_config.bind,
                rss_config.host.as_ref(),
                Arc::clone(&poller.context().rss_server),
            );
            server.run().await.map_err(|e| anyhow::anyhow!(e))
        } => {
            event!(Level::ERROR, "rss server exited with {err:?}");
        },
    }

    Ok(())
}
