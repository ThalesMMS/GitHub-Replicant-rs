//
// main.rs
// GitHub Replicant (Rust)
//
// Coordinates the CLI workflow: selects the sync source (own, stars, followers, following), fetches repositories from GitHub, filters them, and orchestrates concurrent git clone/pull operations with progress reporting.
//
// Thales Matheus Mendonça Santos - November 2025

mod args;
mod git;
mod github;

use anyhow::{Context, Result};
use args::Cli;
use clap::Parser;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Data source selector. Only one flag is allowed at a time.
enum SyncSource {
    Own,
    Stars,
    Following,
    Followers,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let username = &args.username;

    // Determine which dataset to sync based on CLI flags.
    let source = if args.stars {
        SyncSource::Stars
    } else if args.following {
        SyncSource::Following
    } else if args.followers {
        SyncSource::Followers
    } else {
        SyncSource::Own
    };

    // The GitHub API requires a valid User-Agent.
    let client = Client::builder()
        .user_agent("github-backup-rs-cli-v1")
        .build()?;

    // Fetch the requested repo set based on the selected source.
    let (all_repos, source_label) = match source {
        SyncSource::Own => {
            println!("🔍 Fetching repositories for: {}", username);
            let repos = github::fetch_all_repos(&client, username).await?;
            (repos, format!("{}'s repositories", username))
        }
        SyncSource::Stars => {
            println!("🔍 Fetching starred repositories for: {}", username);
            let repos = github::fetch_starred_repos(&client, username).await?;
            (repos, format!("starred repositories of {}", username))
        }
        SyncSource::Following => {
            println!("🔍 Fetching accounts followed by: {}", username);
            let following = github::fetch_following_users(&client, username).await?;

            if following.is_empty() {
                println!("ℹ️ No following accounts found for {}.", username);
                return Ok(());
            }

            // Fan-out: for each followed user, fetch their repos, deduplicating by full name.
            println!(
                "🔍 Fetching repositories for {} followed accounts.",
                following.len()
            );
            let repos = github::fetch_repos_for_users(&client, &following).await?;
            (
                repos,
                format!("repositories from accounts followed by {}", username),
            )
        }
        SyncSource::Followers => {
            println!("🔍 Fetching followers of: {}", username);
            let followers = github::fetch_followers(&client, username).await?;

            if followers.is_empty() {
                println!("ℹ️ No followers found for {}.", username);
                return Ok(());
            }

            // Fan-out: for each follower, fetch their repos, deduplicating by full name.
            println!(
                "🔍 Fetching repositories for {} followers.",
                followers.len()
            );
            let repos = github::fetch_repos_for_users(&client, &followers).await?;
            (
                repos,
                format!("repositories from followers of {}", username),
            )
        }
    };

    // Drop forks unless explicitly requested.
    let repos_to_sync: Vec<github::Repo> = all_repos
        .clone()
        .into_iter()
        .filter(|r| args.include_forks || !r.fork)
        .collect();

    let count = repos_to_sync.len();
    println!(
        "✅ Found {} repositories ({} selected for synchronization) from {}.",
        all_repos.len(),
        count,
        source_label
    );

    if count == 0 {
        return Ok(());
    }

    // Define and create output folder: output/<username>
    let output_dir = PathBuf::from("output").join(username);
    // Use tokio::fs for async file operations so we do not block the runtime.
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;

    // Progress Bar Configuration
    let pb = ProgressBar::new(count as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    // Concurrent Synchronization
    // Use Arc to safely share the output directory across tasks without cloning paths.
    let output_dir_arc = Arc::new(output_dir);
    let root_username = username.clone();

    let stream = stream::iter(repos_to_sync)
        .map(|repo| {
            let base_dir_clone = Arc::clone(&output_dir_arc);
            let pb_clone = pb.clone();
            let repo_name = repo.name.clone();
            let root_username = root_username.clone();
            // Create an async task for each repository
            async move {
                pb_clone.set_message(format!("🔄 {}", repo.name));
                // Compute destination path respecting owner to avoid collisions.
                let destination = destination_path(base_dir_clone.as_ref(), &repo, &root_username);
                let result = git::sync_repository(repo.clone(), &destination).await;
                pb_clone.inc(1);
                (repo_name, result)
            }
        })
        // Control how many tasks run simultaneously
        .buffer_unordered(args.concurrency);

    // Execute stream and collect results
    let results: Vec<(String, Result<()>)> = stream.collect().await;

    pb.finish_with_message("🎉 Synchronization complete!");

    // Error Summary
    let errors: Vec<_> = results.iter().filter(|(_, res)| res.is_err()).collect();
    if !errors.is_empty() {
        println!("\n⚠️ {} operations failed:", errors.len());
        for (name, result) in errors {
            if let Err(e) = result {
                eprintln!("[FAILED] {}: {}", name, e);
            }
        }
        // Return a general error if something failed
        return Err(anyhow::anyhow!("Synchronization finished with errors."));
    }

    Ok(())
}

// Build the filesystem target path for a repo. If it belongs to the root user, place it directly
// under output/<root>/<repo>; otherwise nest under output/<root>/<owner>/<repo> to prevent clashes.
fn destination_path(base_dir: &Path, repo: &github::Repo, root_username: &str) -> PathBuf {
    if repo.owner.login.eq_ignore_ascii_case(root_username) {
        base_dir.join(&repo.name)
    } else {
        base_dir.join(&repo.owner.login).join(&repo.name)
    }
}
