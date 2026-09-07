//! Turns what the user typed into an exact Git target, never guessing and never creating.

use crate::error::{AcreError, Result, exit};
use crate::git::operations::fetch_ref;
use crate::git::refs::{GitRef, GitRefKind, list_refs, resolve_oid, validate_branch_name};
use crate::git::repository::Repository;
use crate::git::worktrees::GitWorktree;
use crate::model::WorkspaceStatus;
use crate::model::{PullRequestTarget, RepositoryState, StoredTarget, TargetKind, TrustLevel};
use crate::provider::github::{
    ensure_pull_request_object, parse_pull_request_selector, resolve_pull_request,
};
use crate::util::is_subsequence;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub kind: TargetKind,
    pub display_name: String,
    pub oid: String,
    pub trust: TrustLevel,
    pub existing_worktree: Option<GitWorktree>,
    pub local_branch: Option<String>,
    pub remote_branch: Option<String>,
    pub remote: Option<String>,
    pub base_ref: Option<String>,
    pub pull_request: Option<PullRequestTarget>,
}

impl ResolvedTarget {
    /// A target with only its identity filled in; each kind adds what it needs on top.
    fn new(
        kind: TargetKind,
        display_name: impl Into<String>,
        oid: impl Into<String>,
        trust: TrustLevel,
    ) -> Self {
        Self {
            kind,
            display_name: display_name.into(),
            oid: oid.into(),
            trust,
            existing_worktree: None,
            local_branch: None,
            remote_branch: None,
            remote: None,
            base_ref: None,
            pull_request: None,
        }
    }
}

impl From<&ResolvedTarget> for StoredTarget {
    fn from(target: &ResolvedTarget) -> Self {
        Self {
            kind: target.kind,
            display_name: target.display_name.clone(),
            oid: target.oid.clone(),
            local_branch: target.local_branch.clone(),
            remote_branch: target.remote_branch.clone(),
            pull_request: target.pull_request.clone(),
        }
    }
}

/// Resolves a selector to existing work, in order: a pull request, a checked-out worktree, a
/// local branch, a branch on exactly one remote. Anything else is an error with suggestions.
pub fn resolve_existing_target(
    repository: &Repository,
    state: &RepositoryState,
    selector: &str,
) -> Result<ResolvedTarget> {
    let typed = parse_typed_selector(selector)?;
    let value = typed.value.as_str();

    // Only a bare selector can be a PR; `branch:pr:12` means a branch literally named that.
    if typed.kind == SelectorKind::Auto {
        if let Some(number) = parse_pull_request_selector(value) {
            return pull_request_target(repository, state, number);
        }
    }
    if let Some(target) = worktree_target(repository, state, &typed) {
        return Ok(target);
    }
    let refs = list_refs(&repository.top_level)?;
    // `remote:` skips local branches on purpose: the user is asking for the remote's copy.
    if matches!(typed.kind, SelectorKind::Auto | SelectorKind::Branch) {
        if let Some(local) = refs
            .iter()
            .find(|reference| reference.kind == GitRefKind::Local && reference.short_name == value)
        {
            return Ok(ResolvedTarget {
                local_branch: Some(local.short_name.clone()),
                ..ResolvedTarget::new(
                    TargetKind::LocalBranch,
                    &local.short_name,
                    &local.oid,
                    TrustLevel::Trusted,
                )
            });
        }
    }
    if let Some(target) = remote_branch_target(&refs, &typed)? {
        return Ok(target);
    }
    Err(not_found(repository, &refs, selector, value))
}

/// The workspace already holding this pull request, or its head fetched fresh.
fn pull_request_target(
    repository: &Repository,
    state: &RepositoryState,
    number: u64,
) -> Result<ResolvedTarget> {
    let held = state.workspaces.iter().find(|workspace| {
        workspace.status != WorkspaceStatus::Broken
            && workspace
                .target
                .pull_request
                .as_ref()
                .is_some_and(|pull_request| pull_request.number == number)
    });
    if let Some(existing) = held {
        if let Some(worktree) = repository.worktree_at(&existing.path) {
            return Ok(ResolvedTarget {
                kind: TargetKind::Worktree,
                existing_worktree: Some(worktree.clone()),
                ..from_stored(&existing.target, existing.trust)
            });
        }
    }
    let mut pull_request = resolve_pull_request(repository, &number.to_string())?;
    let oid = ensure_pull_request_object(repository, &pull_request)?;
    // gh reports the head it knows; the object we actually fetched is what we bind to.
    pull_request.head_oid = oid.clone();
    let trust = if pull_request.cross_repository {
        TrustLevel::Untrusted
    } else {
        TrustLevel::Trusted
    };
    let display_name = format!("PR #{} · {}", pull_request.number, pull_request.title);
    Ok(ResolvedTarget {
        pull_request: Some(pull_request),
        ..ResolvedTarget::new(TargetKind::PullRequest, display_name, oid, trust)
    })
}

/// A worktree already checked out, found by branch name or by path depending on the selector.
fn worktree_target(
    repository: &Repository,
    state: &RepositoryState,
    typed: &TypedSelector,
) -> Option<ResolvedTarget> {
    let value = typed.value.as_str();
    let by_path = || repository.worktree_at(Path::new(value));
    let by_branch = || {
        repository
            .worktrees
            .iter()
            .find(|worktree| worktree.branch.as_deref() == Some(value))
    };
    let worktree = match typed.kind {
        SelectorKind::Worktree => by_path(),
        SelectorKind::Branch => by_branch(),
        // Branch names win over paths: "main" is almost never a directory the user meant.
        SelectorKind::Auto => by_branch().or_else(by_path),
        SelectorKind::Remote => None,
    }?;
    let stored = state.workspace_at(&worktree.path);
    let display_name = worktree
        .branch
        .clone()
        .or_else(|| stored.map(|workspace| workspace.target.display_name.clone()))
        .unwrap_or_else(|| value.to_owned());
    // An untrusted PR workspace stays untrusted when reopened by path.
    let trust = stored.map_or(TrustLevel::Trusted, |workspace| workspace.trust);
    Some(ResolvedTarget {
        existing_worktree: Some(worktree.clone()),
        local_branch: worktree.branch.clone(),
        pull_request: stored.and_then(|workspace| workspace.target.pull_request.clone()),
        ..ResolvedTarget::new(TargetKind::Worktree, display_name, &worktree.head, trust)
    })
}

/// A branch on exactly one remote, named either `remote/branch` or just `branch`.
fn remote_branch_target(refs: &[GitRef], typed: &TypedSelector) -> Result<Option<ResolvedTarget>> {
    if typed.kind == SelectorKind::Worktree {
        return Ok(None);
    }
    let value = typed.value.as_str();
    let remotes = || {
        refs.iter()
            .filter(|reference| reference.kind == GitRefKind::Remote)
    };
    let matches: Vec<&GitRef> = match remotes().find(|reference| reference.qualified_name() == value) {
        Some(explicit) => vec![explicit],
        None => remotes()
            .filter(|reference| reference.short_name == value)
            .collect(),
    };
    match matches.as_slice() {
        [] => Ok(None),
        [remote] => {
            let remote_name = remote.remote.clone().unwrap_or_default();
            Ok(Some(ResolvedTarget {
                local_branch: Some(remote.short_name.clone()),
                remote_branch: Some(remote.qualified_name()),
                remote: Some(remote_name),
                ..ResolvedTarget::new(
                    TargetKind::RemoteBranch,
                    &remote.short_name,
                    &remote.oid,
                    TrustLevel::Trusted,
                )
            }))
        }
        _ => Err(AcreError::new(
            "ACRE_TARGET_AMBIGUOUS",
            format!("{value} exists on more than one remote."),
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({
            "matches": matches.iter().map(|reference| reference.qualified_name()).collect::<Vec<_>>()
        }))),
    }
}

fn not_found(repository: &Repository, refs: &[GitRef], selector: &str, value: &str) -> AcreError {
    let candidates: Vec<String> = repository
        .worktrees
        .iter()
        .filter_map(|worktree| worktree.branch.clone())
        .chain(refs.iter().map(GitRef::qualified_name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    AcreError::new(
        "ACRE_TARGET_NOT_FOUND",
        format!("No existing work is named {selector}."),
        exit::NOT_FOUND,
    )
    .with_details(serde_json::json!({
        "selector": selector,
        "suggestions": closest(value, &candidates),
    }))
}

pub fn resolve_new_target(
    repository: &Repository,
    branch: &str,
    from: Option<&str>,
    fresh: bool,
) -> Result<ResolvedTarget> {
    // Ask git rather than guessing the ref-name rules ourselves.
    if !validate_branch_name(&repository.top_level, branch)? {
        return Err(AcreError::new(
            "ACRE_INVALID_BRANCH",
            format!("{branch} is not a valid Git branch name."),
            exit::USAGE,
        )
        .with_details(serde_json::json!({ "branch": branch })));
    }
    if repository
        .worktrees
        .iter()
        .any(|worktree| worktree.branch.as_deref() == Some(branch))
        || resolve_oid(&repository.top_level, &format!("refs/heads/{branch}"))?.is_some()
    {
        return Err(AcreError::new(
            "ACRE_BRANCH_EXISTS",
            format!("{branch} already exists."),
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "branch": branch })));
    }

    // "." means "from here", whatever commit this shell is standing on.
    let base_ref = if from == Some(".") {
        repository
            .current_worktree
            .as_ref()
            .map(|worktree| worktree.head.clone())
            .unwrap_or_else(|| "HEAD".to_owned())
    } else if let Some(from) = from {
        from.to_owned()
    } else if let Some(default_branch) = &repository.default_branch {
        // Prefer origin/main over local main: the local one may be stale.
        match &repository.remote {
            Some(remote)
                if resolve_oid(
                    &repository.top_level,
                    &format!("refs/remotes/{remote}/{default_branch}"),
                )?
                .is_some() =>
            {
                format!("{remote}/{default_branch}")
            }
            _ => default_branch.clone(),
        }
    } else {
        "HEAD".to_owned()
    };

    // --fresh refetches the base so the new branch starts from the remote's latest commit.
    if fresh {
        if let Some(remote) = &repository.remote {
            if let Some(remote_branch) = base_ref.strip_prefix(&format!("{remote}/")) {
                fetch_ref(
                    repository,
                    remote,
                    &format!("refs/heads/{remote_branch}"),
                    Some(&format!("refs/remotes/{remote}/{remote_branch}")),
                )?;
            }
        }
    }
    let oid = resolve_oid(&repository.top_level, &base_ref)?.ok_or_else(|| {
        AcreError::new(
            "ACRE_BASE_NOT_FOUND",
            format!("Acre could not resolve the base {base_ref}."),
            exit::NOT_FOUND,
        )
        .with_details(serde_json::json!({ "baseRef": base_ref }))
    })?;
    Ok(ResolvedTarget {
        local_branch: Some(branch.to_owned()),
        base_ref: Some(base_ref),
        ..ResolvedTarget::new(TargetKind::NewBranch, branch, oid, TrustLevel::Trusted)
    })
}

fn from_stored(target: &StoredTarget, trust: TrustLevel) -> ResolvedTarget {
    ResolvedTarget {
        local_branch: target.local_branch.clone(),
        remote_branch: target.remote_branch.clone(),
        remote: target
            .remote_branch
            .as_ref()
            .and_then(|value| value.split_once('/').map(|(remote, _)| remote.to_owned())),
        pull_request: target.pull_request.clone(),
        ..ResolvedTarget::new(target.kind, &target.display_name, &target.oid, trust)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorKind {
    Auto,
    Branch,
    Remote,
    Worktree,
}

struct TypedSelector {
    kind: SelectorKind,
    value: String,
}

fn parse_typed_selector(selector: &str) -> Result<TypedSelector> {
    for (prefix, kind) in [
        ("branch:", SelectorKind::Branch),
        ("remote:", SelectorKind::Remote),
        ("worktree:", SelectorKind::Worktree),
    ] {
        if let Some(value) = selector.strip_prefix(prefix) {
            if value.is_empty() {
                return Err(AcreError::new(
                    "ACRE_TARGET_INVALID",
                    format!("{} requires a value.", prefix.trim_end_matches(':')),
                    exit::USAGE,
                ));
            }
            return Ok(TypedSelector {
                kind,
                value: value.to_owned(),
            });
        }
    }
    Ok(TypedSelector {
        kind: SelectorKind::Auto,
        value: selector.to_owned(),
    })
}

fn closest(needle: &str, candidates: &[String]) -> Vec<String> {
    let needle = needle.to_ascii_lowercase();
    let mut matches: Vec<(i32, &String)> = candidates
        .iter()
        .filter_map(|candidate| {
            let score = similarity(&needle, &candidate.to_ascii_lowercase());
            (score > 0).then_some((score, candidate))
        })
        .collect();
    // Best score first, then alphabetical so the list is stable between runs.
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    matches
        .into_iter()
        .take(5)
        .map(|(_, value)| value.clone())
        .collect()
}

fn similarity(needle: &str, candidate: &str) -> i32 {
    // Shorter candidates rank higher within a tier, so "main" beats "maintenance".
    if candidate.contains(needle) {
        100 - candidate.len() as i32
    } else if is_subsequence(needle, candidate) {
        50 - candidate.len() as i32
    } else {
        0
    }
}
