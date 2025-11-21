# GitHub Replicant (Rust)

A high-performance, asynchronous CLI tool written in Rust to backup (clone or pull) all public repositories from a specific GitHub user.

## Features

*   **Async & Concurrent:** Uses `tokio` and `futures` to perform multiple git operations simultaneously (cloning/pulling).
*   **Smart Sync:** Automatically detects if a repository exists to decide between `git clone` and `git pull`.
*   **Filtration:** Option to include or exclude forked repositories (excludes forks by default).
*   **Visual Feedback:** Real-time progress bar using `indicatif`.

## Installation

Ensure you have [Rust and Cargo](https://rustup.rs/) installed.

```bash
git clone https://github.com/thalesmendonca/GitHub-Replicant-rs.git
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

### Output
Repositories are downloaded to an `output/<username>` directory within the project folder.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
