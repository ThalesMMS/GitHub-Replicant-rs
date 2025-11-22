//
// git.rs
// GitHub Replicant (Rust)
//
// Wraps git operations for repositories: ensures destination paths exist, then clones new repos or pulls updates on existing ones using async process execution and error surfacing.
//
// Thales Matheus Mendonça Santos - November 2025

use crate::github::Repo;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Executes a git command asynchronously and captures the output.
async fn run_git_command(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    // Use tokio::process::Command for non-blocking execution
    let mut command = Command::new("git");
    if let Some(path) = cwd {
        command.current_dir(path);
    }
    command.args(args);

    // Capture stdout and stderr to avoid mixing output in the terminal
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = command
        .output()
        .await
        .context("Failed to execute 'git' command. Is Git installed?")?;

    if output.status.success() {
        Ok(())
    } else {
        // If failed, return stderr for diagnosis
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("Git command failed: {}", stderr))
    }
}

/// Clones the repository if it doesn't exist, or runs 'git pull' if it does.
pub async fn sync_repository(repo: Repo, repo_path: &Path) -> Result<()> {
    // Ensure the parent directories exist before cloning/pulling.
    if let Some(parent) = repo_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to ensure parent directory for {:?}", repo_path))?;
    }

    // Check if directory exists AND contains a .git folder (indicating a valid repo)
    if repo_path.exists() && repo_path.join(".git").exists() {
        // Repository exists: Update (git pull)
        run_git_command(&["pull"], Some(&repo_path)).await
    } else {
        // Repository doesn't exist or is incomplete: Clone (git clone)

        // If directory exists but no .git, remove it before cloning
        if repo_path.exists() {
            tokio::fs::remove_dir_all(&repo_path)
                .await
                .context("Failed to remove incomplete directory before cloning")?;
        }

        // Clone passing the full path as the last argument
        let path_str = repo_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid destination path"))?;

        let result = run_git_command(&["clone", &repo.clone_url, path_str], None).await;

        // If clone fails, try to clean up the partially created directory
        if result.is_err() {
            tokio::fs::remove_dir_all(&repo_path).await.ok();
        }
        result
    }
}
