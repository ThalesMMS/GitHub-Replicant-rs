# GitHub Replicant (Rust)

A high-performance, asynchronous CLI tool written in Rust to backup (clone or pull) all public repositories from a specific GitHub user.

## Features

*   **Async & Concurrent:** Uses `tokio` and `futures` to perform multiple git operations simultaneously (cloning/pulling).
*   **Smart Sync:** Automatically detects if a repository exists to decide between `git clone` and `git pull`.
*   **Filtration:** Option to include or exclude forked repositories (excludes forks by default).
*   **Visual Feedback:** Real-time progress bar using `indicatif`.
*   **Starred/Network Backup:** Sync repositories you starred, from the accounts you follow, or from your followers.

## Installation

Ensure you have [Rust and Cargo](https://rustup.rs/) installed.

```bash
git clone https://github.com/ThalesMMS/GitHub-Replicant-rs.git
cd GitHub-Replicant-rs
cargo build --release
```

The binary will be available at `target/release/github-Replicant-rs`.

## Usage

You can run the tool directly via `cargo run` or using the compiled binary.

### Basic Usage
Backup all non-forked repositories for a user (e.g., `torvalds`):

```bash
cargo run -- torvalds
```

### Starred Repositories
Backup all repositories a user has starred:

```bash
cargo run -- torvalds --stars
```

### Repositories from Following
Backup repositories from every account a user follows:

```bash
cargo run -- torvalds --following
```

### Repositories from Followers
Backup repositories from every account that follows the user:

```bash
cargo run -- torvalds --followers
```

### Include Forks
To also backup forked repositories:

```bash
cargo run -- torvalds --include-forks
```

### Adjust Concurrency
By default, the tool processes 8 repositories in parallel. You can adjust this with `--concurrency` (or `-c`):

```bash
cargo run -- torvalds -c 16
```

### Force Update Divergent Repos
If a repository has diverged or has local changes, force-reset to the upstream branch:

```bash
cargo run -- torvalds --force
```

### Exact Mirror (remove stale repos)
To delete local repositories not returned in the current query (e.g., stars you unstarred), opt into exact mirroring:

```bash
cargo run -- torvalds --stars --exact-mirror
```

### Output
Repositories are downloaded to an `output` directory within the project folder. Folder naming depends on the mode you run:

* Own repositories: `output/<username>`
* Starred: `output/<username>-stars`
* Following: `output/<username>-follows`
* Followers: `output/<username>-followers`

When cloning repositories that belong to other owners (e.g., starred repos or repos from followers/following), they are organized under a nested owner folder to avoid name collisions:

```
output/<username>/<owner>/<repo-name>
```

Repositories belonging to `<username>` stay in `output/<username>/<repo-name>` as before.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
