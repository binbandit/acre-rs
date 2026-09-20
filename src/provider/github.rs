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
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: Option<String>,
    name: Option<String>,
}

pub fn parse_pull_request_selector(selector: &str) -> Option<u64> {
    let lower = selector.to_ascii_lowercase();
    if let Some(value) = lower.strip_prefix("pr:") {
        return value.parse().ok();
    }
    let (_, path) = selector
        .strip_prefix("https://")
        .or_else(|| selector.strip_prefix("http://"))?
        .split_once('/')?;
    let mut parts = path.split('/');
    parts.next()?;
    parts.next()?;
    if parts.next()? != "pull" {
        return None;
    }
    parts.next()?.split(['?', '#']).next()?.parse().ok()
}

pub fn pull_request_repository(repository: &Repository) -> Result<String> {
    let hosted = repository
        .remote_url
        .as_deref()
        .and_then(parse_hosted_remote)
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_PR_PROVIDER_UNSUPPORTED",
                "Pull request targets require a hosted GitHub remote.",
                exit::ENVIRONMENT,
            )
        })?;
    Ok(if hosted.host.eq_ignore_ascii_case("github.com") {
        format!("{}/{}", hosted.owner, hosted.repo)
    } else {
        format!("{}/{}/{}", hosted.host, hosted.owner, hosted.repo)
    })
}

pub fn validate_pull_request_url(repository: &Repository, selector: &str) -> Result<()> {
    if selector.starts_with("http://") || selector.starts_with("https://") {
        // Either the owner or repository can itself be named "pull".
        let prefix = selector.split('/').take(5).collect::<Vec<_>>().join("/");
        let requested = parse_hosted_remote(&prefix);
        let current = repository.remote_url.as_deref().and_then(parse_hosted_remote);
        if !requested.zip(current).is_some_and(|(requested, current)| {
            requested.host.eq_ignore_ascii_case(&current.host)
                && requested.owner.eq_ignore_ascii_case(&current.owner)
                && requested.repo.eq_ignore_ascii_case(&current.repo)
        }) {
            return Err(AcreError::new(
                "ACRE_PR_REPOSITORY_MISMATCH",
                "This pull request URL belongs to a different repository or host.",
                exit::USAGE,
            ));
        }
    }
    Ok(())
}

pub fn resolve_pull_request(repository: &Repository, selector: &str) -> Result<PullRequestTarget> {
    let repo_slug = pull_request_repository(repository)?;
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
            // gh reads the repo's own auth and host config from inside the checkout.
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
        repository: repo_slug,
        // Missing trust metadata must withhold secrets and keep caches out of the pool.
        cross_repository: parsed.is_cross_repository.unwrap_or(true),
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
