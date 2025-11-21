mod args;
mod git;
mod github;

use anyhow::{Context, Result};
use args::Cli;
use clap::Parser;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    let username = &args.username;

    // The GitHub API requires a valid User-Agent.
    let client = Client::builder()
        .user_agent("github-backup-rs-cli-v1")
        .build()?;

    println!("🔍 Fetching repositories for: {}", username);
    let all_repos = github::fetch_all_repos(&client, username).await?;

    // Filter repositories
    let repos_to_sync: Vec<github::Repo> = all_repos
        .clone()
        .into_iter()
        .filter(|r| args.include_forks || !r.fork)
        .collect();

    let count = repos_to_sync.len();
    println!(
        "✅ Found {} repositories ({} selected for synchronization).",
        all_repos.len(),
        count
    );

    if count == 0 {
        return Ok(());
    }

    // Define and create output folder: output/<username>
    let output_dir = PathBuf::from("output").join(username);
    // Use tokio::fs for async file operations
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;

    // Progress Bar Configuration
    let pb = ProgressBar::new(count as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}"
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    // Concurrent Synchronization
    // Use Arc to safely share the output directory across tasks.
    let output_dir_arc = Arc::new(output_dir);

    let stream = stream::iter(repos_to_sync)
        .map(|repo| {
            let base_dir_clone = Arc::clone(&output_dir_arc);
            let pb_clone = pb.clone();
            // Create an async task for each repository
            async move {
                pb_clone.set_message(format!("🔄 {}", repo.name));
                let result = git::sync_repository(repo.clone(), &base_dir_clone).await;
                pb_clone.inc(1);
                (repo.name, result)
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
