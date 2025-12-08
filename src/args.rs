//
// args.rs
// GitHub Replicant (Rust)
//
// Defines the CLI surface (username, source mode, forks flag, concurrency) and enforces mutual exclusivity between network-based modes to drive the rest of the application.
//
// Thales Matheus Mendonça Santos - November 2025

use clap::{ArgGroup, Parser};

/// Tool to locally synchronize repositories from a GitHub profile.
/// Modes: own repos (default), starred repos, repos from followers, following, or watching.
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about,
    long_about = None,
    group(
        ArgGroup::new("source")
            .args(["stars", "following", "followers", "watching"])
            .multiple(false)
    )
)]
pub struct Cli {
    /// The GitHub username
    pub username: String,

    /// Sync repositories the user has starred
    #[arg(long, default_value_t = false)]
    pub stars: bool,

    /// Sync repositories from users this profile follows
    #[arg(long, default_value_t = false)]
    pub following: bool,

    /// Sync repositories from this profile's followers
    #[arg(long, default_value_t = false)]
    pub followers: bool,

    /// Sync repositories the user is watching
    #[arg(long, default_value_t = false)]
    pub watching: bool,

    /// Include forked repositories in synchronization
    #[arg(long, default_value_t = false)]
    pub include_forks: bool,

    /// GitHub token for authenticated API requests (env: GITHUB_TOKEN)
    #[arg(long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// Maximum number of concurrent git operations (clone/pull)
    #[arg(short, long, default_value_t = 8)]
    pub concurrency: usize,

    /// Remove local repos not returned by the current GitHub query to mirror exactly
    #[arg(long, default_value_t = false)]
    pub exact_mirror: bool,

    /// Force update existing repositories, discarding local changes and divergent history
    #[arg(long, default_value_t = false)]
    pub force: bool,
}
