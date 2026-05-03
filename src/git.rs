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

fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/');
    let path = if let Some(remote) = trimmed.strip_prefix("https://") {
        github_remote_path(remote, '/', false)?
    } else if let Some(remote) = trimmed.strip_prefix("git@") {
        github_remote_path(remote, ':', false)?
    } else if let Some(remote) = trimmed.strip_prefix("ssh://") {
        github_remote_path(remote, '/', true)?
    } else {
        return None;
    };

    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let repo_segment = parts.next()?.trim();
    let repo = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);

    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }

    Some((owner.to_string(), repo.to_string()))
}

fn github_remote_path(remote: &str, separator: char, allow_port: bool) -> Option<&str> {
    let (host, path) = remote.split_once(separator)?;
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = strip_optional_www(host);
    let host = if allow_port {
        strip_optional_port(host)?
    } else {
        host
    };

    host.eq_ignore_ascii_case("github.com").then_some(path)
}

fn strip_optional_www(host: &str) -> &str {
    match host.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("www.") => &host[4..],
        _ => host,
    }
}

fn strip_optional_port(host: &str) -> Option<&str> {
    match host.split_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            Some(host)
        }
        Some(_) => None,
        None => Some(host),
    }
}

fn remotes_match(local_url: &str, expected_repo: &Repo) -> bool {
    let Some((owner, repo)) = parse_github_remote(local_url) else {
        return false;
    };

    owner.eq_ignore_ascii_case(&expected_repo.owner.login)
        && repo.eq_ignore_ascii_case(&expected_repo.name)
}

async fn get_origin_url(repo_path: &Path) -> Result<String> {
    run_git_command_output(["remote", "get-url", "origin"], Some(repo_path))
        .await
        .with_context(|| format!("Failed to read origin remote URL for {:?}", repo_path))
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
        let origin_url = get_origin_url(repo_path).await?;
        if !remotes_match(&origin_url, &repo) {
            return Err(anyhow::anyhow!(
                "Refusing to update {:?}: origin remote does not match the expected GitHub repository. Expected {}, found {}. Inspect the checkout manually or remove it before retrying.",
                repo_path,
                repo.clone_url,
                origin_url
            ));
        }

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
    match current_upstream(repo_path)
        .await
        .context("Unable to determine upstream branch for forced update")?
    {
        Some(upstream) => {
            // Reset hard to the upstream ref to drop local divergence or uncommitted changes.
            run_git_command(["reset", "--hard", upstream.as_str()], Some(repo_path)).await
        }
        None => {
            // Empty repositories (no commits yet) have no upstream; nothing to reset.
            Ok(())
        }
    }
}

// Resolve the current branch's upstream reference (e.g., origin/main).
async fn current_upstream(repo_path: &Path) -> Result<Option<String>> {
    // Prefer git's upstream resolution.
    if let Ok(upstream) = run_git_command_output(
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        Some(repo_path),
    )
    .await
    {
        return Ok(Some(upstream));
    }

    // Next, try to use the current branch name to build origin/<branch> when it exists remotely.
    if let Ok(branch) = run_git_command_output(["branch", "--show-current"], Some(repo_path)).await
    {
        if !branch.is_empty() {
            let candidate = format!("origin/{}", branch);
            if run_git_command(
                ["rev-parse", "--verify", candidate.as_str()],
                Some(repo_path),
            )
            .await
            .is_ok()
            {
                return Ok(Some(candidate));
            }
        }
    }

    // Fallback to the remote HEAD if configured.
    if let Ok(origin_head) = run_git_command_output(
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        Some(repo_path),
    )
    .await
    {
        if !origin_head.is_empty() {
            return Ok(Some(origin_head));
        }
    }

    // If no remote HEAD is configured, pick the most recently updated remote branch when present.
    if let Ok(remote_branch) = run_git_command_output(
        [
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "--count=1",
            "refs/remotes/origin",
        ],
        Some(repo_path),
    )
    .await
    {
        if let Some(branch) = remote_branch.lines().find(|b| !b.trim().is_empty()) {
            return Ok(Some(branch.trim().to_string()));
        }
    }

    // If the repository has no commits yet, treat it as having no upstream.
    if !has_commits(repo_path).await? {
        return Ok(None);
    }

    Err(anyhow::anyhow!(
        "No upstream branch configured and no remote branches found"
    ))
}

// Detect whether the repository already contains commits.
async fn has_commits(repo_path: &Path) -> Result<bool> {
    let status = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("HEAD")
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("Failed to check repository commit status")?;

    Ok(status.success())
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
        if is_lfs_error(err) {
            println!(
                "⚠️ LFS error for {}/{}. Retrying without LFS smudge filter.",
                repo.owner.login, repo.name
            );
            tokio::fs::remove_dir_all(repo_path).await.ok();
            return clone_without_lfs(repo, repo_path).await;
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

// Detect Git LFS smudge filter failures.
fn is_lfs_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("smudge filter lfs failed") || msg.contains("git lfs")
}

// Clone with GIT_LFS_SKIP_SMUDGE=1, then attempt git lfs pull (warn on failure).
async fn clone_without_lfs(repo: &Repo, repo_path: &Path) -> Result<()> {
    if repo_path.exists() {
        tokio::fs::remove_dir_all(repo_path)
            .await
            .context("Failed to remove incomplete directory before LFS-skip clone")?;
    }

    let path_str = repo_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid destination path"))?;

    let mut command = Command::new("git");
    command.env("GIT_LFS_SKIP_SMUDGE", "1");
    command.args(["clone", repo.clone_url.as_str(), path_str]);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = command
        .output()
        .await
        .context("Failed to execute 'git clone' with LFS skip")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tokio::fs::remove_dir_all(repo_path).await.ok();
        return Err(anyhow::anyhow!("Git clone (LFS-skip) failed: {}", stderr));
    }

    // Try to fetch LFS objects; warn but don't fail if it doesn't work.
    match run_git_command(["lfs", "pull"], Some(repo_path)).await {
        Ok(()) => println!(
            "✅ LFS objects fetched for {}/{}.",
            repo.owner.login, repo.name
        ),
        Err(_) => println!(
            "⚠️ Could not fetch LFS objects for {}/{}. LFS files remain as pointers.",
            repo.owner.login, repo.name
        ),
    }

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::github::Owner;
    use std::ffi::OsString;
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;
    use std::sync::LazyLock;
    use std::{env, fs};
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    static GIT_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn repo(owner: &str, name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            clone_url: format!("https://github.com/{}/{}.git", owner, name),
            fork: false,
            full_name: format!("{}/{}", owner, name),
            owner: Owner {
                login: owner.to_string(),
            },
        }
    }

    fn run_git(args: &[&str], cwd: &Path) {
        let status = StdCommand::new(real_git_path())
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("git command should run");
        assert!(status.success(), "git command failed: {:?}", args);
    }

    fn real_git_path() -> PathBuf {
        let output = StdCommand::new("sh")
            .arg("-c")
            .arg("command -v git")
            .output()
            .expect("git should be discoverable");
        assert!(output.status.success(), "git should be on PATH");

        PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("git path should be UTF-8")
                .trim(),
        )
    }

    fn temporary_git_repo_with_origin(origin_url: &str) -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("temp dir should be created");
        let repo_path = temp.path().join("Repo");
        fs::create_dir(&repo_path).expect("repo dir should be created");
        run_git(&["init"], &repo_path);
        run_git(&["remote", "add", "origin", origin_url], &repo_path);
        (temp, repo_path)
    }

    fn install_fake_git(bin_dir: &Path) {
        fs::create_dir_all(bin_dir).expect("fake git bin dir should be created");
        let fake_git = bin_dir.join("git");
        fs::write(
            &fake_git,
            r#"#!/bin/sh
if [ "$1" = "remote" ] && [ "$2" = "get-url" ] && [ "$3" = "origin" ]; then
  exec "$REAL_GIT" "$@"
fi

if [ "$1" = "pull" ]; then
  exit 0
fi

exec "$REAL_GIT" "$@"
"#,
        )
        .expect("fake git script should be written");

        let mut permissions = fs::metadata(&fake_git)
            .expect("fake git metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_git, permissions).expect("fake git should be executable");
    }

    struct EnvGuard {
        previous_path: Option<OsString>,
        previous_real_git: Option<OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous_path {
                Some(path) => env::set_var("PATH", path),
                None => env::remove_var("PATH"),
            }

            match &self.previous_real_git {
                Some(path) => env::set_var("REAL_GIT", path),
                None => env::remove_var("REAL_GIT"),
            }
        }
    }

    async fn with_fake_git<T>(future: impl Future<Output = T>) -> T {
        let _lock = GIT_ENV_LOCK.lock().await;
        let real_git = real_git_path();
        let fake_git_dir = TempDir::new().expect("fake git temp dir should be created");
        let fake_bin = fake_git_dir.path().join("bin");
        install_fake_git(&fake_bin);

        let previous_path = env::var_os("PATH");
        let previous_real_git = env::var_os("REAL_GIT");
        let mut paths = vec![fake_bin];
        if let Some(path) = previous_path.as_ref() {
            paths.extend(env::split_paths(path));
        }

        env::set_var(
            "PATH",
            env::join_paths(paths).expect("test PATH should be valid"),
        );
        env::set_var("REAL_GIT", real_git);
        let _env_guard = EnvGuard {
            previous_path,
            previous_real_git,
        };

        future.await
    }

    #[test]
    fn parses_supported_github_remote_urls() {
        assert_eq!(
            parse_github_remote("https://github.com/Owner/Repo"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("git@github.com:Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("git@github.com:Owner/Repo"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("https://user@github.com/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("https://token:x-oauth-basic@www.github.com/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("git@www.github.com:Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com:22/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@www.github.com:22/Owner/Repo.git"),
            Some(("Owner".to_string(), "Repo".to_string()))
        );
    }

    #[test]
    fn rejects_non_github_or_malformed_remote_urls() {
        assert_eq!(
            parse_github_remote("https://example.com/Owner/Repo.git"),
            None
        );
        assert_eq!(parse_github_remote("https://github.com/Owner"), None);
        assert_eq!(
            parse_github_remote("https://github.com/Owner/Repo/tree/main"),
            None
        );
        assert_eq!(parse_github_remote(""), None);
        assert_eq!(parse_github_remote("not-a-url"), None);
        assert_eq!(parse_github_remote("https://github.com//Repo.git"), None);
        assert_eq!(parse_github_remote("https://github.com/Owner/.git"), None);
        assert_eq!(
            parse_github_remote("ssh://git@github.com:/Owner/Repo.git"),
            None
        );
    }

    #[test]
    fn remotes_match_accepts_matching_and_equivalent_forms() {
        let expected = repo("Owner", "Repo");

        assert!(remotes_match(
            "https://github.com/Owner/Repo.git",
            &expected
        ));
        assert!(remotes_match("git@github.com:owner/repo.git", &expected));
        assert!(remotes_match("ssh://git@github.com/OWNER/REPO", &expected));
        assert!(remotes_match(
            "https://github.com/OWNER/REPO.git",
            &expected
        ));
        assert!(remotes_match(
            "https://user@www.github.com/Owner/Repo.git",
            &expected
        ));
        assert!(remotes_match(
            "ssh://git@github.com:22/Owner/Repo.git",
            &expected
        ));
    }

    #[test]
    fn remotes_match_rejects_mismatched_origin() {
        let expected = repo("Owner", "Repo");

        assert!(!remotes_match(
            "https://github.com/OtherOwner/Repo.git",
            &expected
        ));
        assert!(!remotes_match(
            "https://github.com/Owner/OtherRepo.git",
            &expected
        ));
        assert!(!remotes_match(
            "https://example.com/Owner/Repo.git",
            &expected
        ));
    }

    #[tokio::test]
    async fn sync_repository_updates_existing_checkout_with_matching_remote() {
        let (_temp, repo_path) =
            temporary_git_repo_with_origin("https://github.com/Owner/Repo.git");

        with_fake_git(sync_repository(repo("Owner", "Repo"), &repo_path, false))
            .await
            .expect("matching origin should allow sync to proceed");
    }

    #[tokio::test]
    async fn sync_repository_rejects_mismatched_existing_checkout_before_updates() {
        let (_temp, repo_path) =
            temporary_git_repo_with_origin("https://github.com/OtherOwner/Repo.git");

        for force_reset in [false, true] {
            let err = with_fake_git(sync_repository(
                repo("Owner", "Repo"),
                &repo_path,
                force_reset,
            ))
            .await
            .expect_err("mismatched origin should fail");
            let msg = err.to_string();

            assert!(msg.contains("Refusing to update"));
            assert!(msg.contains("https://github.com/Owner/Repo.git"));
            assert!(msg.contains("https://github.com/OtherOwner/Repo.git"));
        }
    }

    #[tokio::test]
    async fn sync_repository_updates_existing_checkout_with_matching_ssh_remote() {
        let (_temp, repo_path) = temporary_git_repo_with_origin("git@github.com:Owner/Repo.git");

        with_fake_git(sync_repository(repo("Owner", "Repo"), &repo_path, false))
            .await
            .expect("canonically matching SSH origin should allow sync to proceed");
    }

    #[tokio::test]
    async fn sync_repository_updates_existing_checkout_with_matching_ssh_port_remote() {
        let (_temp, repo_path) =
            temporary_git_repo_with_origin("ssh://git@www.github.com:22/Owner/Repo.git");

        with_fake_git(sync_repository(repo("Owner", "Repo"), &repo_path, false))
            .await
            .expect("canonically matching SSH origin with port should allow sync to proceed");
    }
}
