//! Pull request resolution through the `gh` CLI.

use serde::Deserialize;

use crate::error::{AcreError, Result, exit};
use crate::git::operations::fetch_ref;
use crate::git::refs::resolve_oid;
use crate::git::repository::Repository;
use crate::git::runner::{RunOptions, run_process};
use crate::model::PullRequestTarget;
use crate::provider::remote::parse_hosted_remote;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    author: Option<GhAuthor>,
    url: String,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    is_cross_repository: Option<bool>,
    head_repository: Option<GhRepository>,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepository {
    name_with_owner: Option<String>,
}

pub fn parse_pull_request_selector(selector: &str) -> Option<u64> {
    let lower = selector.to_ascii_lowercase();
    if let Some(value) = lower.strip_prefix("pr:") {
        return value.parse().ok();
    }
    let marker = "/pull/";
    let index = lower.find(marker)? + marker.len();
    lower[index..]
        .split('/')
        .next()
        .and_then(|value| value.parse().ok())
}

pub fn resolve_pull_request(repository: &Repository, selector: &str) -> Result<PullRequestTarget> {
    let hosted = repository
        .remote_url
        .as_deref()
        .and_then(parse_hosted_remote)
        // GitHub Enterprise hosts qualify too; gh handles them.
        .filter(|remote| remote.host.to_ascii_lowercase().contains("github"))
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_PR_PROVIDER_UNSUPPORTED",
                "Pull request targets currently require a GitHub remote.",
                exit::ENVIRONMENT,
            )
            .with_details(serde_json::json!({ "remoteUrl": repository.remote_url }))
        })?;
    let repo_slug = format!("{}/{}", hosted.owner, hosted.repo);
    let result = run_process(
        "gh",
        &[
            "pr",
            "view",
            selector,
            "--repo",
            &repo_slug,
            "--json",
            "number,title,author,url,baseRefName,headRefName,headRefOid,isCrossRepository,headRepository",
        ],
        RunOptions {
            cwd: Some(repository.top_level.clone()),
            timeout: Some(std::time::Duration::from_secs(60)),
            ..RunOptions::default()
        },
    )?;
    let parsed: GhPullRequest = serde_json::from_slice(&result.stdout).map_err(|error| {
        AcreError::new(
            "ACRE_PR_INVALID_RESPONSE",
            format!("GitHub returned an unreadable pull request response: {error}"),
            exit::NETWORK,
        )
    })?;
    Ok(PullRequestTarget {
        number: parsed.number,
        title: parsed.title,
        author: parsed
            .author
            .and_then(|author| author.login.or(author.name))
            .unwrap_or_else(|| "unknown".to_owned()),
        url: parsed.url,
        base_ref_name: parsed.base_ref_name,
        head_ref_name: parsed.head_ref_name,
        head_oid: parsed.head_ref_oid,
        repository: parsed
            .head_repository
            .and_then(|repository| repository.name_with_owner)
            .unwrap_or(repo_slug),
        // A missing field would read as trusted, so keep isCrossRepository in the --json list above.
        cross_repository: parsed.is_cross_repository.unwrap_or(false),
    })
}

pub fn ensure_pull_request_object(
    repository: &Repository,
    pull_request: &PullRequestTarget,
) -> Result<String> {
    // Already fetched (or the head is in our history); no network needed.
    if resolve_oid(&repository.top_level, &pull_request.head_oid)?.is_some() {
        return Ok(pull_request.head_oid.clone());
    }
    let remote = repository.remote.as_deref().ok_or_else(|| {
        AcreError::new(
            "ACRE_PR_FETCH_UNAVAILABLE",
            "Acre could not fetch this pull request because the repository has no remote.",
            exit::NETWORK,
        )
    })?;
    // Our own ref namespace, so nothing collides with the user's remote-tracking refs.
    let destination = format!("refs/acre/pull/{}", pull_request.number);
    fetch_ref(
        repository,
        remote,
        &format!("refs/pull/{}/head", pull_request.number),
        Some(&destination),
    )?;
    Ok(resolve_oid(&repository.top_level, &destination)?.unwrap_or_else(|| pull_request.head_oid.clone()))
}
