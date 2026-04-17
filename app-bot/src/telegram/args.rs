use std::path::PathBuf;

use clap::Parser;
use clap_verbosity_flag::Verbosity;

#[derive(Debug, Parser)]
#[command(version = app_common::meta::VERSION, name = "not-yet-tg")]
pub struct Args {
    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[command(flatten)]
    pub verbosity: Verbosity,
    /// Telegram bot token, overridden by "BOT_TOKEN"
    /// environment variable if present
    #[clap(short, long)]
    pub bot_token: Option<String>,

    /// Path to the configuration and data files,
    /// defaults to $XDG_CONFIG/notyet, where for the former
    /// the program recognizes config.toml
    #[clap(short, long)]
    pub config: Option<PathBuf>,
}
