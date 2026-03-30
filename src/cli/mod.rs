use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Write, stdout},
    path::Path,
    time::Duration,
};

use anyhow::{Context, anyhow};
use clap::Parser;
use futures::{Stream, StreamExt, TryStreamExt, future, pin_mut, stream};
use llama_runner::Gemma3VisionRunner;
use migration::{Migrator, MigratorTrait};
use reqwest::header::HeaderMap;
use sea_orm::{Database, DatabaseConnection};
use serde::{Serialize, de::DeserializeOwned};
use smol_str::ToSmolStr;
use tracing::{Instrument, Level, debug_span, event, info_span, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, util::SubscriberInitExt};

use crate::{
    agent::{Decider, LlmConditionMatcher, memory::sqlite::SqliteDecisionMemory},
    cli::{
        args::Args,
        config::{Config, RunMode, Subscription},
    },
    polling::{self, Scheduler, schedule, task::Task, trigger::ScheduleTrigger},
    source::{Feed, LlmComprehendable, LlmRssItem, RssFeed},
    update::{
        UpdatePersistence, UpdateWakerExt, accept::AcceptUpdatePersistence,
        sqlite::SqliteUpdatePersistence,
    },
};

mod args;
mod config;

pub async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(if args.verbose {
                    "not_yet=debug".parse().unwrap()
                } else {
                    LevelFilter::ERROR.into()
                })
                .from_env()
                .unwrap(),
        )
        .with_level(true)
        .finish()
        .init();

    let Some(data_path) = args
        .config
        .clone()
        .or_else(|| dirs::config_dir())
        .map(|p| p.join("notyet"))
    else {
        return Err(anyhow!("failed to determine config path"));
    };

    event!(Level::DEBUG, "creating config dir at {data_path:?}");
    tokio::fs::create_dir_all(&data_path).await?;

    let config: anyhow::Result<Config> = async {
        let fp = data_path.join("config.toml");
        if fp.exists() {
            let buf = tokio::fs::read(&fp).await?;
            event!(Level::DEBUG, "read config from {fp:?}");
            Ok(toml::from_slice(&buf)?)
        } else {
            let default_config = include_bytes!("default_config.toml");
            event!(Level::INFO, "config file does not exist, using default");
            tokio::fs::write(fp, default_config).await?;
            Ok(toml::from_slice(default_config)?)
        }
    }
    .instrument(debug_span!("config"))
    .await;

    async {
        let (run_mode, subscriptions) = parse_args_config(args, config?)?;
        let db = setup_db(&data_path).await?;
        let mut scheduler = Scheduler::new();
        let runner = Gemma3VisionRunner::default().await?;

        event!(Level::INFO, "run mode: {run_mode:?}");
        match run_mode {
            config::RunMode::Oneshot => {
                for sub in subscriptions.as_ref() {
                    event!(Level::INFO, "checking subscription {sub:?}");
                    let feed = create_feed(sub)?;
                    let _sc = scheduler
                        .add_schedule(ScheduleTrigger::Interval(Duration::ZERO), sub.clone())
                        .await?;
                    let decider = LlmConditionMatcher::new(
                        &runner,
                        &sub.condition,
                        SqliteDecisionMemory::new(db.clone(), &data_path)?,
                    );
                    let persistence = AcceptUpdatePersistence::new();
                    // TODO:oneshot should return immediately when there is no new update
                    let updates = check_feed(sub, &feed, &decider, &scheduler, persistence);
                    pin_mut!(updates);
                    let mut stdout_guard = stdout().lock();
                    if let Some(result) = updates.next().await {
                        let (task, is_truthy) = result?;
                        event!(
                            Level::INFO,
                            "schedule id = {}, is_truthy = {is_truthy}",
                            task.schedule().id()
                        );
                        if is_truthy {
                            writeln!(&mut stdout_guard, "vivid")?;
                        } else {
                            writeln!(&mut stdout_guard, "statsis")?;
                        }
                    }
                }
            }
            config::RunMode::Daemon { schedules } => {
                let scheduler = schedules
                    .into_iter()
                    .enumerate()
                    .map(|(id, s)| {
                        Ok(polling::Schedule::new(
                            id,
                            subscriptions
                                .as_ref()
                                .get(s.for_ - 1)
                                .cloned()
                                .ok_or(anyhow!(
                                    "schedule {} looks for subscription {} but not found",
                                    id + 1,
                                    s.for_
                                ))?,
                            s.trigger,
                        )?)
                    })
                    .collect::<anyhow::Result<Scheduler<_>>>()?;
                future::try_join_all(subscriptions.as_ref().iter().enumerate().map(
                    async |(sub_id, sub)| -> anyhow::Result<()> {
                        async {
                            event!(Level::INFO, "registered subscription {sub:?}");

                            let feed = create_feed(sub)?;
                            let decider = LlmConditionMatcher::new(
                                &runner,
                                sub.condition.clone(),
                                SqliteDecisionMemory::new(db.clone(), data_path.clone())?,
                            );
                            let persistence = SqliteUpdatePersistence::new(db.clone(), {
                                let mut hasher = DefaultHasher::new();
                                sub.hash(&mut hasher);
                                format!("{:x}", hasher.finish())
                            })?;
                            let updates = check_feed(sub, &feed, &decider, &scheduler, persistence);
                            pin_mut!(updates);
                            while let Some(result) = updates.next().await {
                                match result {
                                    Ok((task, is_truthy)) => {
                                        let mut stdout_guard = stdout().lock();
                                        if is_truthy {
                                            write!(&mut stdout_guard, "{sub_id}")?;
                                            if let Ok(metadata) = feed.get_metadata().await {
                                                write!(&mut stdout_guard, "\t{}", metadata.name)?;
                                            }
                                            writeln!(&mut stdout_guard)?;
                                        } else {
                                            event!(
                                                Level::INFO,
                                                "the LLM decided that an update for {} is stasis",
                                                task.schedule().key()
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        if let Ok(metadata) = feed.get_metadata().await {
                                            event!(
                                                Level::ERROR,
                                                "failed to poll update, checking feed {:?}: {err}",
                                                metadata.name
                                            );
                                        } else {
                                            event!(Level::ERROR, "failed to poll update: {err}");
                                        }
                                    }
                                }
                            }
                            Ok(())
                        }
                        .instrument(info_span!("daemon", sub = sub_id))
                        .await
                    },
                ))
                .await?;
            }
        }
        Ok(())
    }
    .instrument(debug_span!("run"))
    .await
}

async fn setup_db(working_dir: &Path) -> anyhow::Result<DatabaseConnection> {
    let fp = working_dir.join("app.db");
    event!(Level::DEBUG, "db path is {fp:?}");
    let fps = fp.to_str().ok_or(anyhow!("invalid working dir"))?;
    let db = Database::connect(format!("sqlite://{fps}?mode=rwc"))
        .await
        .context("failed to connect to database")?;
    Migrator::up(&db, None).await?;
    Ok(db)
}

fn create_feed(sub: &Subscription) -> anyhow::Result<RssFeed> {
    Ok(match &sub.feed {
        config::Feed::Rss { url, headers } => RssFeed::new(
            url,
            &headers
                .as_ref()
                .map(|map| -> anyhow::Result<_> {
                    Ok(HeaderMap::from_iter(
                        map.iter()
                            .map(|(k, v)| Ok((k.parse()?, v.parse()?)))
                            .collect::<anyhow::Result<Vec<_>>>()
                            .context("invalid header")?,
                    ))
                })
                .transpose()?,
        )?,
    })
}

fn check_feed<'f, Item, FeedError, Feed_, DeciderError, Decider_, PersistenceError, Persistence>(
    key: &'f Subscription,
    feed: &'f Feed_,
    decider: &'f Decider_,
    scheduler: &Scheduler<Subscription>,
    persistence: Persistence,
) -> impl Stream<Item = anyhow::Result<(Task<Subscription>, bool)>>
where
    Item: LlmComprehendable + Hash + Serialize + DeserializeOwned + Send + Sync + Unpin + 'static,
    FeedError: Display,
    Feed_: Feed<Item = Item, Error = FeedError>,
    DeciderError: std::error::Error + Send + Sync + 'static,
    Decider_: Decider<Material = Item, Error = DeciderError> + ?Sized,
    PersistenceError: Display,
    Persistence: UpdatePersistence<Item = Item, Error = PersistenceError>,
{
    scheduler
        .start_polling(Some(key))
        .wake_update(feed, persistence)
        .map_ok(move |(update, task)| {
            event!(
                Level::DEBUG,
                "woke for update, schedule id = {}, key = {}",
                task.schedule().id(),
                task.schedule().key()
            );
            (update, task)
        })
        .map_err(|err| anyhow!("update error: {err}"))
        .then(
            async move |result| -> anyhow::Result<(Task<Subscription>, bool)> {
                match result {
                    Ok((update, task)) => {
                        let Some(material) = update else {
                            return Ok((task, false));
                        };
                        Ok((task, decider.get_truth_value(material).await?))
                    }
                    Err(err) => Err(err),
                }
            },
        )
}

fn parse_args_config(
    args: Args,
    config: Config,
) -> anyhow::Result<(RunMode, impl AsRef<[Subscription]>)> {
    match args.command {
        Some(cmd) => match cmd {
            args::Command::Rss {
                url,
                conditions,
                headers,
            } => {
                let headers = if headers.is_empty() {
                    None
                } else {
                    Some(BTreeMap::from_iter(
                        headers
                            .into_iter()
                            .map(|v| {
                                let Some((k, v)) = v.split_once(": ") else {
                                    return Err(anyhow!("invalid header format: {v}"));
                                };
                                Ok((k.to_smolstr(), v.to_smolstr()))
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    ))
                };
                Ok((
                    RunMode::Oneshot,
                    url.into_iter()
                        .zip(conditions.into_iter())
                        .map(|(url, condition)| Subscription {
                            feed: config::Feed::Rss {
                                url: url.into(),
                                headers: headers.clone(),
                            },
                            condition: condition.to_smolstr(),
                        })
                        .collect(),
                ))
            }
            args::Command::Daemon => Ok((
                RunMode::Daemon {
                    schedules: match config.mode {
                        RunMode::Oneshot => {
                            return Err(anyhow!("daemon mode requires a trigger"));
                        }
                        RunMode::Daemon { schedules } => schedules,
                    },
                },
                config.subscriptions,
            )),
        },
        None => Ok((config.mode, config.subscriptions)),
    }
}
