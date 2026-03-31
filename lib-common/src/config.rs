use anyhow::{Context, Ok, anyhow};
use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use tracing::{Instrument, Level, debug_span, event};

pub trait ParseConfigPath {
    type Error;
    fn parse(&self) -> Result<PathBuf, Self::Error>;
}

impl ParseConfigPath for Option<PathBuf> {
    type Error = anyhow::Error;

    fn parse(&self) -> Result<PathBuf, Self::Error> {
        self.as_ref()
            .cloned()
            .or_else(|| dirs::config_dir())
            .map(|p| p.join("notyet"))
            .ok_or(anyhow!("failed to determine config path"))
    }
}

pub async fn parse_config<C>(
    data_path: impl AsRef<Path>,
    fallback: impl AsRef<[u8]>,
) -> anyhow::Result<C>
where
    C: DeserializeOwned,
{
    let data_path = data_path.as_ref();

    event!(Level::DEBUG, "creating config dir at {data_path:?}");
    tokio::fs::create_dir_all(&data_path).await?;

    let config: anyhow::Result<C> = async {
        let fp = data_path.join("config.toml");
        if fp.exists() {
            let buf = tokio::fs::read(&fp).await?;
            event!(Level::DEBUG, "read config from {fp:?}");
            Ok(toml::from_slice(&buf)?)
        } else {
            let default = fallback.as_ref();
            event!(Level::INFO, "config file does not exist, using default");
            tokio::fs::write(fp, default).await?;
            Ok(toml::from_slice(default)?)
        }
    }
    .instrument(debug_span!("config"))
    .await;

    Ok(config?)
}

pub async fn setup_db(working_dir: &Path) -> anyhow::Result<DatabaseConnection> {
    let fp = working_dir.join("app.db");
    event!(Level::DEBUG, "db path is {fp:?}");
    let fps = fp.to_str().ok_or(anyhow!("invalid working dir"))?;
    let db = Database::connect(format!("sqlite://{fps}?mode=rwc"))
        .await
        .context("failed to connect to database")?;
    Migrator::up(&db, None).await?;
    Ok(db)
}
