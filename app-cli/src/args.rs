use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;

#[derive(Debug, Parser)]
#[command(version = app_common::meta::VERSION, name = "not-yet-cli")]
pub struct Args {
    /// Path to the configuration and data files,
    /// defaults to $XDG_CONFIG/notyet, where for the former
    /// the program recognizes config.toml
    #[clap(short, long)]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub verbosity: Verbosity,

    /// Override the run mode specified by configuration,
    /// usually used for testing
    #[clap(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Rss {
        /// URL of the RSS feeds, overriding configuration set
        #[clap(short, long)]
        url: Vec<String>,
        /// Under what circumstances to see the feed as "vivid",
        /// should zip with URLs
        #[clap(short = 'c', long = "condition")]
        conditions: Vec<String>,
        /// Extra headers, cURL style
        #[clap(short = 'H', long = "header")]
        headers: Vec<String>,
        #[clap(short, long, default_value = "usize::MAX")]
        buffer_size: usize,
    },
    Daemon,
}
