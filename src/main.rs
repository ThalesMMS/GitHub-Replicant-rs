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
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT},
    Client,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Data source selector. Only one flag is allowed at a time.
enum SyncSource {
    Own,
    Stars,
    Following,
    Followers,
    Watching,
}

/// CLI entry point that fetches repositories from GitHub (based on CLI flags) and synchronizes them into a local output directory.
///
/// Parses command-line arguments, selects the dataset to sync (own repos, stars, following, followers, or watching), constructs an HTTP client (including optional authorization), fetches and filters repositories, validates and creates the target output directory, and concurrently clones/updates each repository. Optionally prunes local repositories not present in the latest fetch when `--exact-mirror` is used. On completion, prints a summary and returns an error if any repository synchronization failed.
///
/// # Returns
///
/// `Ok(())` on success, or an error if any step (HTTP client construction, fetching data, filesystem operations, or repository synchronizations) fails.
///
/// # Examples
///
/// ```no_run
/// // Run the compiled binary from the command line, e.g.:
/// // cargo run -- alice --stars --output-dir ./backup
/// ```
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
    } else if args.watching {
        SyncSource::Watching
    } else {
        SyncSource::Own
    };

    // The GitHub API requires a valid User-Agent; include Authorization when provided.
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("github-backup-rs-cli-v1"),
    );
    if let Some(token) = args.token.as_deref() {
        let token_value = format!("Bearer {}", token);
        let header_value = HeaderValue::from_str(&token_value)
            .context("Invalid characters in GITHUB_TOKEN for Authorization header")?;
        headers.insert(AUTHORIZATION, header_value);
    }

    let client = Client::builder()
        .default_headers(headers)
        .build()
        .context("Failed to build HTTP client")?;

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
        SyncSource::Watching => {
            let is_authenticated = args.token.is_some();
            if is_authenticated {
                println!(
                    "🔍 Fetching watched repositories (including Custom) for authenticated user"
                );
            } else {
                println!("🔍 Fetching watched repositories for: {}", username);
            }
            let repos = github::fetch_watched_repos(&client, username, is_authenticated).await?;
            (repos, format!("watched repositories of {}", username))
        }
    };

    // Drop forks unless explicitly requested.
    let repos_to_sync: Vec<github::Repo> = all_repos
        .clone()
        .into_iter()
        .filter(|r| args.include_forks || !r.fork)
        .collect();

    // Compute the target output folder name based on the source type.
    let output_dir_name = output_dir_name(username, &source);
    let output_root = args
        .output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("output"));
    let output_dir = output_root.join(&output_dir_name);
    validate_output_dir(&output_dir)?;

    // Pre-compute the desired destination paths for mirroring and sync.
    let desired_paths: HashSet<PathBuf> = repos_to_sync
        .iter()
        .map(|repo| destination_path(&output_dir, repo, username))
        .collect();

    let count = repos_to_sync.len();
    println!(
        "✅ Found {} repositories ({} selected for synchronization) from {}.",
        all_repos.len(),
        count,
        source_label
    );

    if count == 0 {
        // Allow --exact-mirror to clean up when there are no repos to sync.
        if args.exact_mirror {
            tokio::fs::create_dir_all(&output_dir)
                .await
                .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;
            prune_extra_repos(&output_dir, &desired_paths).await?;
        }
        return Ok(());
    }

    // Define and create output folder: output/<username> or the source-specific suffix.
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
    let force_update = args.force;

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
                let result = git::sync_repository(repo.clone(), &destination, force_update).await;
                pb_clone.inc(1);
                (repo_name, result)
            }
        })
        // Control how many tasks run simultaneously
        .buffer_unordered(args.concurrency);

    // Execute stream and collect results
    let results: Vec<(String, Result<()>)> = stream.collect().await;

    pb.finish_with_message("🎉 Synchronization complete!");

    // If requested, remove repositories not present in the latest fetch.
    if args.exact_mirror {
        prune_extra_repos(output_dir_arc.as_ref(), &desired_paths).await?;
    }

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

// Derive the output folder name based on the selected source.
/// Derives the output directory leaf name for a given GitHub username and sync source.
///
/// The leaf is the plain `username` for `SyncSource::Own`, or `"<username>-<suffix>"`
/// where `<suffix>` is `stars`, `following`, `followers`, or `watching` for the
/// corresponding sources.
///
/// # Returns
///
/// The directory name to use for the given username and source.
///
/// # Examples
///
/// ```ignore
/// let name = output_dir_name("alice", &SyncSource::Stars);
/// assert_eq!(name, "alice-stars");
/// ```
fn output_dir_name(username: &str, source: &SyncSource) -> String {
    match source {
        SyncSource::Own => username.to_string(),
        SyncSource::Stars => format!("{}-stars", username),
        SyncSource::Following => format!("{}-following", username),
        SyncSource::Followers => format!("{}-followers", username),
        SyncSource::Watching => format!("{}-watching", username),
    }
}

// Validate absolute output paths early; relative paths are created later if needed.
/// Validates an absolute output directory path; relative paths are accepted without checks.
///
/// For absolute `output_dir`, this function ensures one of:
/// - the path exists and is a directory, or
/// - the path does not exist but its nearest existing ancestor is a directory.
///
/// If these conditions are not met, an error with contextual information is returned.
///
/// # Examples
///
/// ```ignore
/// use std::path::Path;
/// // Relative path is allowed (no validation performed)
/// assert!(validate_output_dir(Path::new("relative/output")).is_ok());
/// ```
fn validate_output_dir(output_dir: &Path) -> Result<()> {
    if !output_dir.is_absolute() {
        return Ok(());
    }

    match std::fs::metadata(output_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                anyhow::bail!(
                    "Invalid output directory path {:?}: path exists but is not a directory",
                    output_dir
                );
            }
            std::fs::canonicalize(output_dir).with_context(|| {
                format!(
                    "Failed to validate output directory path {:?}: path cannot be accessed",
                    output_dir
                )
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut parent = output_dir.parent();
            let (ancestor, metadata) = loop {
                let candidate = parent.with_context(|| {
                    format!(
                        "Invalid output directory path {:?}: no existing ancestor found",
                        output_dir
                    )
                })?;

                match std::fs::metadata(candidate) {
                    Ok(metadata) => break (candidate, metadata),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        parent = candidate.parent();
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "Invalid output directory path {:?}: ancestor {:?} cannot be accessed",
                                output_dir, candidate
                            )
                        });
                    }
                }
            };

            if !metadata.is_dir() {
                anyhow::bail!(
                    "Invalid output directory path {:?}: ancestor {:?} is not a directory",
                    output_dir,
                    ancestor
                );
            }
            std::fs::canonicalize(ancestor).with_context(|| {
                format!(
                    "Failed to validate output directory path {:?}: ancestor {:?} cannot be resolved",
                    output_dir, ancestor
                )
            })?;
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Invalid output directory path {:?}: path cannot be accessed",
                    output_dir
                )
            });
        }
    }

    Ok(())
}

// Determine if a path is a git repository by checking for a .git directory.
/// Determines whether the given path is a Git repository by checking for a `.git` directory.
///
/// # Returns
///
/// `true` if a `.git` entry exists at `path` and is a directory, `false` otherwise.
///
/// # Examples
///
/// ```ignore
/// let path = std::path::Path::new(".");
/// let _is_repo = is_git_repo(path).await;
/// ```
async fn is_git_repo(path: &Path) -> bool {
    tokio::fs::metadata(path.join(".git"))
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

// Collect existing repository directories under the output folder (direct or nested).
async fn existing_repo_paths(base_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut repos = Vec::new();
    if !base_dir.exists() {
        return Ok(repos);
    }

    let mut entries = tokio::fs::read_dir(base_dir)
        .await
        .with_context(|| format!("Failed to read directory {:?}", base_dir))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await?.is_dir() {
            continue;
        }

        if is_git_repo(&path).await {
            repos.push(path);
            continue;
        }

        let mut inner = tokio::fs::read_dir(&path).await?;
        while let Some(repo_entry) = inner.next_entry().await? {
            let repo_path = repo_entry.path();
            if repo_entry.file_type().await?.is_dir() && is_git_repo(&repo_path).await {
                repos.push(repo_path);
            }
        }
    }

    Ok(repos)
}

// Remove repositories not present in the desired set and clean empty owner directories.
/// Removes repository directories under `base_dir` that are not present in `desired`, then removes any empty owner directories left behind.
///
/// The function mutates the filesystem: it deletes any repository directory that is not included in `desired` and then removes owner-level directories that become empty as a result.
///
/// # Examples
///
/// ```ignore
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// use std::collections::HashSet;
/// use std::path::PathBuf;
///
/// let base = std::path::Path::new("/tmp/example-output");
/// let desired: HashSet<PathBuf> = HashSet::new(); // no repos desired => all repos under `base` are removed
///
/// // Removes repos under `base` not listed in `desired` and prunes empty owner directories.
/// prune_extra_repos(base, &desired).await.unwrap();
/// # });
/// ```
async fn prune_extra_repos(base_dir: &Path, desired: &HashSet<PathBuf>) -> Result<()> {
    let existing = existing_repo_paths(base_dir).await?;
    for repo_path in existing {
        if !desired.contains(&repo_path) {
            tokio::fs::remove_dir_all(&repo_path)
                .await
                .with_context(|| {
                    format!("Failed to remove outdated repository at {:?}", repo_path)
                })?;
        }
    }

    // Clean up empty owner directories left after pruning.
    if base_dir.exists() {
        let mut entries = tokio::fs::read_dir(base_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !entry.file_type().await?.is_dir() || is_git_repo(&path).await {
                continue;
            }

            let mut inner = tokio::fs::read_dir(&path).await?;
            if inner.next_entry().await?.is_none() {
                tokio::fs::remove_dir_all(&path).await.ok();
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn relative_output_dir_is_allowed() {
        assert!(validate_output_dir(Path::new("output/torvalds")).is_ok());
    }

    #[test]
    fn absolute_output_dir_with_existing_parent_is_allowed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let output_dir = temp_dir.path().join("torvalds");

        assert!(validate_output_dir(&output_dir).is_ok());
    }

    #[test]
    fn absolute_output_dir_with_missing_parent_is_allowed_if_ancestor_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let output_dir = temp_dir.path().join("missing").join("torvalds");

        assert!(validate_output_dir(&output_dir).is_ok());
    }

    #[test]
    fn existing_output_path_must_be_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let output_dir = temp_dir.path().join("torvalds");
        File::create(&output_dir).unwrap();

        assert!(validate_output_dir(&output_dir).is_err());
    }
}
