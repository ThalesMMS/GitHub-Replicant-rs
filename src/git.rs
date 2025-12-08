//
// git.rs
// GitHub Replicant (Rust)
//
// Wraps git operations for repositories: ensures destination paths exist, then clones new repos or pulls updates on existing ones using async process execution and error surfacing.
//
// Thales Matheus Mendonça Santos - November 2025

use crate::github::Repo;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Executes a git command asynchronously and captures the output.
async fn run_git_command<I, S>(args: I, cwd: Option<&Path>) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
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

/// Executes a git command and returns stdout as String.
async fn run_git_command_output<I, S>(args: I, cwd: Option<&Path>) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    if let Some(path) = cwd {
        command.current_dir(path);
    }
    command.args(args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = command
        .output()
        .await
        .context("Failed to execute 'git' command. Is Git installed?")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("Git command failed: {}", stderr))
    }
}

/// Clones the repository if it doesn't exist, or runs 'git pull' if it does.
pub async fn sync_repository(repo: Repo, repo_path: &Path, force_reset: bool) -> Result<()> {
    // Ensure the parent directories exist before cloning/pulling.
    if let Some(parent) = repo_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to ensure parent directory for {:?}", repo_path))?;
    }

    // Check if directory exists AND contains a .git folder (indicating a valid repo)
    if repo_path.exists() && repo_path.join(".git").exists() {
        // Repository exists: Update (git pull or forced reset)
        if force_reset {
            force_update(repo_path).await
        } else {
            match run_git_command(["pull"], Some(repo_path)).await {
                Ok(()) => Ok(()),
                Err(err) if is_default_branch_error(&err) => {
                    println!(
                        "ℹ️ Default branch changed for {}. Re-cloning to match remote.",
                        repo.full_name
                    );
                    if let Err(remove_err) = tokio::fs::remove_dir_all(repo_path).await {
                        return Err(err.context(format!(
                            "Failed to remove repository before re-clone: {:?}",
                            remove_err
                        )));
                    }
                    clone_repository(&repo, repo_path).await
                }
                Err(err) => Err(err),
            }
        }
    } else {
        clone_repository(&repo, repo_path).await
    }
}

// Forcefully update a repository by fetching all remotes and resetting to the upstream branch.
async fn force_update(repo_path: &Path) -> Result<()> {
    // Fetch latest changes and prune removed branches.
    run_git_command(["fetch", "--all", "--prune"], Some(repo_path)).await?;

    // Determine the upstream branch to hard reset against.
    let upstream = current_upstream(repo_path)
        .await
        .context("Unable to determine upstream branch for forced update")?;

    // Reset hard to the upstream ref to drop local divergence or uncommitted changes.
    run_git_command(["reset", "--hard", upstream.as_str()], Some(repo_path)).await
}

// Resolve the current branch's upstream reference (e.g., origin/main).
async fn current_upstream(repo_path: &Path) -> Result<String> {
    // Prefer git's upstream resolution.
    if let Ok(upstream) = run_git_command_output(
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        Some(repo_path),
    )
    .await
    {
        return Ok(upstream);
    }

    // Fallback: use the current branch name to build origin/<branch>.
    let branch =
        run_git_command_output(["rev-parse", "--abbrev-ref", "HEAD"], Some(repo_path)).await?;

    if branch == "HEAD" {
        return Err(anyhow::anyhow!(
            "Repository is in a detached HEAD state; cannot determine upstream."
        ));
    }

    Ok(format!("origin/{}", branch))
}

// Clone the repository, handling DMCA errors gracefully.
async fn clone_repository(repo: &Repo, repo_path: &Path) -> Result<()> {
    // If directory exists but no .git, remove it before cloning
    if repo_path.exists() {
        tokio::fs::remove_dir_all(repo_path)
            .await
            .context("Failed to remove incomplete directory before cloning")?;
    }

    // Clone passing the full path as the last argument
    let path_str = repo_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid destination path"))?;

    let result = run_git_command(["clone", repo.clone_url.as_str(), path_str], None).await;

    // If clone fails, try to clean up the partially created directory
    if let Err(err) = &result {
        if is_dmca_error(err) {
            println!(
                "⚠️ Repo {} from user {} skipped due to DMCA takedown.",
                repo.name, repo.owner.login
            );
            tokio::fs::remove_dir_all(repo_path).await.ok();
            return Ok(());
        }
        tokio::fs::remove_dir_all(repo_path).await.ok();
    }
    result
}

// Detect default-branch mismatch errors reported by git pull.
fn is_default_branch_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("Your configuration specifies to merge with the ref")
        && msg.contains("no such ref was fetched")
}

// Detect DMCA-related errors.
fn is_dmca_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("dmca")
}
