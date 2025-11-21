use clap::Parser;

/// Tool to locally synchronize repositories from a GitHub profile.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// The GitHub username
    pub username: String,

    /// Include forked repositories in synchronization
    #[arg(long, default_value_t = false)]
    pub include_forks: bool,

    /// Maximum number of concurrent git operations (clone/pull)
    #[arg(short, long, default_value_t = 8)]
    pub concurrency: usize,
}
