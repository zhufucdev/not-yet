use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the configuration and data files,
    /// defaults to $XDG_CONFIG/notyet, where for the former
    /// the programs recognizes config.toml
    #[clap(short, long)]
    pub config: Option<PathBuf>,

    #[clap(short, long)]
    pub verbose: bool,

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
    },
    Daemon,
}
