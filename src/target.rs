use std::collections::BTreeSet;
use std::path::Path;

use crate::error::{AcreError, Result, exit};
use crate::git::operations::fetch_ref;
use crate::git::refs::{list_refs, resolve_oid, validate_branch_name};
use crate::model::{
    GitRefKind, Repository, RepositoryState, ResolvedTarget, StoredTarget, TargetKind, TrustLevel,
};
use crate::provider::github::{
    ensure_pull_request_object, parse_pull_request_selector, resolve_pull_request,
};
use crate::util::{canonical_or_absolute, is_subsequence};

pub fn resolve_existing_target(
    repository: &Repository,
    state: &RepositoryState,
    selector: &str,
) -> Result<ResolvedTarget> {
    let typed = parse_typed_selector(selector)?;
    let value = typed.value.as_str();

    if typed.kind == SelectorKind::Auto {
        if let Some(number) = parse_pull_request_selector(value) {
            if let Some(existing) = state.workspaces.iter().find(|workspace| {
                workspace.status != crate::model::WorkspaceStatus::Broken
                    && workspace
                        .target
                        .pull_request
                        .as_ref()
                        .is_some_and(|pull_request| pull_request.number == number)
            }) {
                if let Some(worktree) = repository.worktree_at(&existing.path) {
                    let mut target = from_stored(&existing.target, existing.trust);
                    target.kind = TargetKind::Worktree;
                    target.existing_worktree = Some(worktree.clone());
                    return Ok(target);
                }
            }
            let mut pull_request = resolve_pull_request(repository, &number.to_string())?;
            let oid = ensure_pull_request_object(repository, &pull_request)?;
            pull_request.head_oid = oid.clone();
            return Ok(ResolvedTarget {
                kind: TargetKind::PullRequest,
                display_name: format!("PR #{} · {}", pull_request.number, pull_request.title),
                oid,
                trust: if pull_request.cross_repository {
                    TrustLevel::Untrusted
                } else {
                    TrustLevel::Trusted
                },
                existing_worktree: None,
                local_branch: None,
                remote_branch: None,
                remote: None,
                base_ref: None,
                pull_request: Some(pull_request),
            });
        }
    }

    let path_value = Path::new(value);
    if let Some(worktree) = repository.worktrees.iter().find(|worktree| match typed.kind {
        SelectorKind::Worktree => canonical_or_absolute(&worktree.path) == canonical_or_absolute(path_value),
        SelectorKind::Remote => false,
        _ => {
            worktree.branch.as_deref() == Some(value)
                || (typed.kind == SelectorKind::Auto
                    && canonical_or_absolute(&worktree.path) == canonical_or_absolute(path_value))
        }
    }) {
        let stored = state.workspace_at(&worktree.path);
        return Ok(ResolvedTarget {
            kind: TargetKind::Worktree,
            display_name: worktree
                .branch
                .clone()
                .or_else(|| stored.map(|workspace| workspace.target.display_name.clone()))
                .unwrap_or_else(|| value.to_owned()),
            oid: worktree.head.clone(),
            trust: stored
                .map(|workspace| workspace.trust)
                .unwrap_or(TrustLevel::Trusted),
            existing_worktree: Some(worktree.clone()),
            local_branch: worktree.branch.clone(),
            remote_branch: None,
            remote: None,
            base_ref: None,
            pull_request: stored.and_then(|workspace| workspace.target.pull_request.clone()),
        });
    }

    let refs = list_refs(&repository.top_level)?;
    if typed.kind != SelectorKind::Remote {
        if let Some(local) = refs
            .iter()
            .find(|reference| reference.kind == GitRefKind::Local && reference.short_name == value)
        {
            return Ok(ResolvedTarget {
                kind: TargetKind::LocalBranch,
                display_name: local.short_name.clone(),
                oid: local.oid.clone(),
                trust: TrustLevel::Trusted,
                existing_worktree: None,
                local_branch: Some(local.short_name.clone()),
                remote_branch: None,
                remote: None,
                base_ref: None,
                pull_request: None,
            });
        }
    }

    let explicit = refs.iter().find(|reference| {
        reference.kind == GitRefKind::Remote
            && reference
                .remote
                .as_ref()
                .is_some_and(|remote| format!("{remote}/{}", reference.short_name) == value)
    });
    let matches: Vec<_> = if let Some(explicit) = explicit {
        vec![explicit]
    } else if typed.kind == SelectorKind::Worktree {
        Vec::new()
    } else {
        refs.iter()
            .filter(|reference| reference.kind == GitRefKind::Remote && reference.short_name == value)
            .collect()
    };

    if matches.len() == 1 {
        let remote = matches[0];
        let remote_name = remote.remote.clone().unwrap_or_default();
        return Ok(ResolvedTarget {
            kind: TargetKind::RemoteBranch,
            display_name: remote.short_name.clone(),
            oid: remote.oid.clone(),
            trust: TrustLevel::Trusted,
            existing_worktree: None,
            local_branch: Some(remote.short_name.clone()),
            remote_branch: Some(format!("{remote_name}/{}", remote.short_name)),
            remote: Some(remote_name),
            base_ref: None,
            pull_request: None,
        });
    }
    if matches.len() > 1 {
        return Err(AcreError::new(
            "ACRE_TARGET_AMBIGUOUS",
            format!("{value} exists on more than one remote."),
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({
            "matches": matches.iter().map(|reference| format!("{}/{}", reference.remote.as_deref().unwrap_or(""), reference.short_name)).collect::<Vec<_>>()
        })));
    }

    let candidates: BTreeSet<String> = repository
        .worktrees
        .iter()
        .filter_map(|worktree| worktree.branch.clone())
        .chain(refs.iter().map(|reference| match reference.kind {
            GitRefKind::Local => reference.short_name.clone(),
            GitRefKind::Remote => format!(
                "{}/{}",
                reference.remote.as_deref().unwrap_or(""),
                reference.short_name
            ),
        }))
        .collect();
    Err(AcreError::new(
        "ACRE_TARGET_NOT_FOUND",
        format!("No existing work is named {selector}."),
        exit::NOT_FOUND,
    )
    .with_details(serde_json::json!({
        "selector": selector,
        "suggestions": closest(value, candidates.into_iter().collect::<Vec<_>>().as_slice())
    })))
}

pub fn resolve_new_target(
    repository: &Repository,
    branch: &str,
    from: Option<&str>,
    fresh: bool,
) -> Result<ResolvedTarget> {
    if !validate_branch_name(&repository.top_level, branch)? {
        return Err(AcreError::new(
            "ACRE_INVALID_BRANCH",
            format!("{branch} is not a valid Git branch name."),
            exit::USAGE,
        )
        .with_details(serde_json::json!({ "branch": branch })));
    }
    let refs = list_refs(&repository.top_level)?;
    if repository
        .worktrees
        .iter()
        .any(|worktree| worktree.branch.as_deref() == Some(branch))
        || refs
            .iter()
            .any(|reference| reference.kind == GitRefKind::Local && reference.short_name == branch)
    {
        return Err(AcreError::new(
            "ACRE_BRANCH_EXISTS",
            format!("{branch} already exists."),
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "branch": branch })));
    }

    let base_ref = if from == Some(".") {
        repository
            .current_worktree
            .as_ref()
            .map(|worktree| worktree.head.clone())
            .unwrap_or_else(|| "HEAD".to_owned())
    } else if let Some(from) = from {
        from.to_owned()
    } else if let Some(default_branch) = &repository.default_branch {
        let remote_candidate = repository
            .remote
            .as_ref()
            .map(|remote| format!("{remote}/{default_branch}"));
        remote_candidate
            .filter(|candidate| {
                refs.iter().any(|reference| {
                    reference.kind == GitRefKind::Remote
                        && reference.remote.as_ref().is_some_and(|remote| {
                            format!("{remote}/{}", reference.short_name) == candidate.as_str()
                        })
                })
            })
            .unwrap_or_else(|| default_branch.clone())
    } else {
        "HEAD".to_owned()
    };

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
        kind: TargetKind::NewBranch,
        display_name: branch.to_owned(),
        oid,
        trust: TrustLevel::Trusted,
        existing_worktree: None,
        local_branch: Some(branch.to_owned()),
        remote_branch: None,
        remote: None,
        base_ref: Some(base_ref),
        pull_request: None,
    })
}

fn from_stored(target: &StoredTarget, trust: TrustLevel) -> ResolvedTarget {
    ResolvedTarget {
        kind: target.kind,
        display_name: target.display_name.clone(),
        oid: target.oid.clone(),
        trust,
        existing_worktree: None,
        local_branch: target.local_branch.clone(),
        remote_branch: target.remote_branch.clone(),
        remote: target
            .remote_branch
            .as_ref()
            .and_then(|value| value.split_once('/').map(|(remote, _)| remote.to_owned())),
        base_ref: None,
        pull_request: target.pull_request.clone(),
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
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    matches
        .into_iter()
        .take(5)
        .map(|(_, value)| value.clone())
        .collect()
}

fn similarity(needle: &str, candidate: &str) -> i32 {
    if candidate.contains(needle) {
        100 - candidate.len() as i32
    } else if is_subsequence(needle, candidate) {
        50 - candidate.len() as i32
    } else {
        0
    }
}
