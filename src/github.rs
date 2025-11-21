use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

/// Relevant data from the GitHub API response.
#[derive(Debug, Deserialize, Clone)]
pub struct Repo {
    pub name: String,
    pub clone_url: String,
    pub fork: bool,
}

/// Fetches all repositories for the user, handling GitHub API pagination.
pub async fn fetch_all_repos(client: &Client, username: &str) -> Result<Vec<Repo>> {
    let mut all_repos = Vec::new();
    let mut page = 1;

    // Loop to handle pagination (max 100 repos per page)
    loop {
        let url = format!(
            "https://api.github.com/users/{}/repos?per_page=100&page={}",
            username, page
        );

        let response = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to GitHub API on page {}", page))?;

        // Check if the request was successful (e.g., if the user exists)
        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "GitHub API Error: {}. Check if username '{}' is correct.",
                response.status(),
                username
            ));
        }

        let repos: Vec<Repo> = response
            .json()
            .await
            .context("Failed to parse GitHub API JSON response")?;

        if repos.is_empty() {
            break; // Pagination finished
        }

        all_repos.extend(repos);
        page += 1;
    }

    Ok(all_repos)
}
