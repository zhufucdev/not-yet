use std::{
    collections::BTreeMap,
    fmt::Display,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Write, stdout},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, anyhow};
use app_common::{config::ParseConfigPath, feed};
use clap::Parser;
use futures::{StreamExt, future, pin_mut};
use llama_runner::{
    Gemma3VisionRunner, RunnerWithRecommendedSampling, error::CreateLlamaCppRunnerError,
};
use reqwest::header::HeaderMap;
use sea_orm::DatabaseConnection;
use serde::{Serialize, de::DeserializeOwned};
use smol_str::ToSmolStr;
use tracing::{Instrument, Level, debug_span, event, info_span};

use crate::{
    args::Args,
    config::{Config, RssConfig, RunMode, Subscription, ToFeed},
};
use lib_common::{
    agent::{LlmConditionMatcher, memory::sqlite::SqliteDecisionMemory},
    llm::timeout::{ModelProducer, TimedModel},
    polling::{self, Scheduler, trigger::ScheduleTrigger},
    source::{DefaultMetadata, Feed, LlmComprehendable, RssFeed},
    update::{accept::AcceptUpdatePersistence, sqlite::SqliteUpdatePersistence},
};

mod args;
mod config;

pub async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_max_level(args.verbosity)
        .init();
    let data_path = args.config.parse_config()?;
    let config = app_common::config::parse_config::<Config>(
        &data_path,
        include_bytes!("../asset/default_config.toml"),
    )
    .await?;

    async {
        let model = TimedModel::new(
            "decider_vlm",
            config.drop_model_in.clone(),
            ModelProducer::new(|| async { Gemma3VisionRunner::default().await }),
        );
        let db = app_common::config::setup_db(&data_path).await?;
        let scheduler = Scheduler::new();
        let (run_mode, subscriptions) = parse_args_config(args, config)?;

        event!(Level::INFO, "run mode: {run_mode:?}");
        match run_mode {
            config::RunMode::Oneshot => {
                for sub in subscriptions.as_ref() {
                    event!(Level::INFO, "checking subscription {sub:?}");
                    match &sub.feed {
                        config::Feed::Rss(conf) => {
                            oneshot(&scheduler, sub, conf.to_feed()?, &model, &db, &data_path).await?
                        }
                        config::Feed::Atom(conf) => {
                            oneshot(&scheduler, sub, conf.to_feed()?, &model, &db, &data_path).await?
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

                            match &sub.feed {
                                config::Feed::Rss(conf) => {
                                    daemon(
                                        &scheduler,
                                        sub,
                                        sub_id,
                                        conf.to_feed()?,
                                        &model,
                                        &db,
                                        &data_path,
                                    )
                                    .await?;
                                }
                                config::Feed::Atom(conf) => {
                                    daemon(
                                        &scheduler,
                                        sub,
                                        sub_id,
                                        conf.to_feed()?,
                                        &model,
                                        &db,
                                        &data_path,
                                    )
                                    .await?
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

async fn oneshot<Feed_>(
    scheduler: &Scheduler<Subscription>,
    sub: &Subscription,
    feed: Feed_,
    model: &TimedModel<RunnerWithRecommendedSampling<Gemma3VisionRunner>, CreateLlamaCppRunnerError>,
    db: &DatabaseConnection,
    data_path: &PathBuf,
) -> anyhow::Result<()>
where
    Feed_: Feed + Send + Sync + 'static,
    Feed_::Item: LlmComprehendable + Hash + Serialize + DeserializeOwned,
    Feed_::Error: Display,
{
    let _sc = scheduler
        .add_schedule(ScheduleTrigger::Interval(Duration::ZERO), sub.clone())
        .await?;
    let decider = LlmConditionMatcher::new(
        model.clone(),
        &sub.condition,
        SqliteDecisionMemory::new(db.clone(), &data_path, None)?,
    );
    let persistence = AcceptUpdatePersistence::new();
    // TODO:oneshot should return immediately when there is no new update
    let updates = app_common::feed::check(sub, &feed, &decider, &scheduler, persistence);
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
    Ok(())
}

async fn daemon<Feed_>(
    scheduler: &Scheduler<Subscription>,
    sub: &Subscription,
    sub_id: usize,
    feed: Feed_,
    model: &TimedModel<
        RunnerWithRecommendedSampling<Gemma3VisionRunner>,
        CreateLlamaCppRunnerError,
    >,
    db: &DatabaseConnection,
    data_path: &PathBuf,
) -> anyhow::Result<()>
where
    Feed_: Feed<Metadata = DefaultMetadata> + Send + Sync + 'static,
    Feed_::Item: LlmComprehendable + Hash + Serialize + DeserializeOwned,
    Feed_::Error: Display,
{
    let decider = LlmConditionMatcher::new(
        model.clone(),
        sub.condition.clone(),
        SqliteDecisionMemory::new(db.clone(), data_path.clone(), None)?,
    );
    let persistence = SqliteUpdatePersistence::new(db.clone(), {
        let mut hasher = DefaultHasher::new();
        sub.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    })?;
    let updates = app_common::feed::check(sub, &feed, &decider, &scheduler, persistence);
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
                            feed: config::Feed::Rss(RssConfig {
                                url: url.into(),
                                headers: headers.clone(),
                            }),
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
