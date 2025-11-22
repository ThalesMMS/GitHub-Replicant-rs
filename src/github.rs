//
// github.rs
// GitHub Replicant (Rust)
//
// Handles GitHub API interactions: paginated fetch helpers, repo/star/follower/following queries, and aggregation/deduplication of repositories with owner metadata for downstream syncing.
//
// Thales Matheus Mendonça Santos - November 2025

use anyhow::{Context, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// Relevant data from the GitHub API response.
/// Includes owner info so we can build nested paths and deduplicate by full_name.
#[derive(Debug, Deserialize, Clone)]
pub struct Repo {
    pub name: String,
    pub clone_url: String,
    pub fork: bool,
    pub full_name: String,
    pub owner: Owner,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Owner {
    pub login: String,
}

#[derive(Debug, Deserialize)]
struct User {
    login: String,
}

/// Generic helper to fetch paginated GitHub resources.
/// Accepts a URL builder for each page and a label used in error messages.
async fn fetch_paginated<T, F>(client: &Client, build_url: F, context_label: &str) -> Result<Vec<T>>
where
    T: DeserializeOwned,
    F: Fn(usize) -> String,
{
    let mut items = Vec::new();
    let mut page = 1;

    loop {
        let url = build_url(page);

        let response = client.get(&url).send().await.with_context(|| {
            format!(
                "Failed to connect to GitHub API on page {} for {}",
                page, context_label
            )
        })?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "GitHub API error {} while fetching {}.",
                response.status(),
                context_label
            ));
        }

        let page_items: Vec<T> = response.json().await.with_context(|| {
            format!(
                "Failed to parse GitHub API JSON response for {}",
                context_label
            )
        })?;

        // GitHub pagination ends when a page returns an empty array.
        if page_items.is_empty() {
            break;
        }

        items.extend(page_items);
        page += 1;
    }

    Ok(items)
}

/// Fetches all repositories for the user, handling GitHub API pagination.
pub async fn fetch_all_repos(client: &Client, username: &str) -> Result<Vec<Repo>> {
    fetch_paginated(
        client,
        |page| {
            format!(
                "https://api.github.com/users/{}/repos?per_page=100&page={}",
                username, page
            )
        },
        &format!("repositories for user {}", username),
    )
    .await
}

/// Fetches all repositories starred by the user.
pub async fn fetch_starred_repos(client: &Client, username: &str) -> Result<Vec<Repo>> {
    fetch_paginated(
        client,
        |page| {
            format!(
                "https://api.github.com/users/{}/starred?per_page=100&page={}",
                username, page
            )
        },
        &format!("starred repositories for user {}", username),
    )
    .await
}

/// Fetches the list of usernames this profile follows.
pub async fn fetch_following_users(client: &Client, username: &str) -> Result<Vec<String>> {
    let users: Vec<User> = fetch_paginated(
        client,
        |page| {
            format!(
                "https://api.github.com/users/{}/following?per_page=100&page={}",
                username, page
            )
        },
        &format!("following list for user {}", username),
    )
    .await?;

    Ok(users.into_iter().map(|u| u.login).collect())
}

/// Fetches the list of usernames that follow this profile.
pub async fn fetch_followers(client: &Client, username: &str) -> Result<Vec<String>> {
    let users: Vec<User> = fetch_paginated(
        client,
        |page| {
            format!(
                "https://api.github.com/users/{}/followers?per_page=100&page={}",
                username, page
            )
        },
        &format!("followers list for user {}", username),
    )
    .await?;

    Ok(users.into_iter().map(|u| u.login).collect())
}

/// Fetch all repositories for a list of usernames, deduplicating by full name.
pub async fn fetch_repos_for_users(client: &Client, usernames: &[String]) -> Result<Vec<Repo>> {
    let mut repos_by_full_name = HashMap::new();
    let mut seen_users = HashSet::new();

    for username in usernames {
        // Avoid redundant API calls if a username repeats in the list.
        if !seen_users.insert(username.clone()) {
            continue;
        }

        // Reuse the single-user fetcher so pagination/error handling stays in one place.
        let repos = fetch_all_repos(client, username)
            .await
            .with_context(|| format!("Failed to fetch repositories for user '{}'", username))?;

        for repo in repos {
            repos_by_full_name
                .entry(repo.full_name.clone())
                .or_insert(repo);
        }
    }

    // Stable ordering for deterministic progress/order.
    let mut repos: Vec<Repo> = repos_by_full_name.into_values().collect();
    repos.sort_by(|a, b| a.full_name.cmp(&b.full_name));

    Ok(repos)
}
