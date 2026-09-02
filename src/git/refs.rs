//! Listing and resolving refs.

use std::path::Path;

use crate::error::Result;
use crate::git::runner::{RunOptions, decode_stdout, run_git, run_git_with};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitRef {
    pub full_name: String,
    pub short_name: String,
    pub oid: String,
    pub kind: GitRefKind,
    pub remote: Option<String>,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GitRefKind {
    Local,
    Remote,
}

pub fn list_refs(cwd: &Path) -> Result<Vec<GitRef>> {
    let format = "%(refname)%00%(objectname)%00%(upstream)%00";
    let result = run_git(
        cwd,
        &[
            "for-each-ref",
            &format!("--format={format}"),
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse_refs(&result.stdout))
}

pub fn parse_refs(buffer: &[u8]) -> Vec<GitRef> {
    let values: Vec<String> = buffer
        .split(|byte| *byte == 0)
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect();
    let mut refs = Vec::new();
    let mut index = 0;
    while index + 2 < values.len() {
        let full_name = values[index].trim_start_matches(['\r', '\n']).to_owned();
        let oid = values[index + 1].clone();
        let upstream = values[index + 2].trim_end_matches(['\r', '\n']).to_owned();
        index += 3;
        if full_name.is_empty() || oid.is_empty() || full_name.ends_with("/HEAD") {
            continue;
        }
        if let Some(short_name) = full_name.strip_prefix("refs/heads/") {
            let short_name = short_name.to_owned();
            refs.push(GitRef {
                full_name,
                short_name,
                oid,
                kind: GitRefKind::Local,
                remote: None,
                upstream: if upstream.is_empty() {
                    None
                } else {
                    Some(
                        upstream
                            .strip_prefix("refs/remotes/")
                            .unwrap_or(&upstream)
                            .to_owned(),
                    )
                },
            });
        } else if let Some(short) = full_name.strip_prefix("refs/remotes/") {
            if let Some((remote, short_name)) = short.split_once('/') {
                let remote = remote.to_owned();
                let short_name = short_name.to_owned();
                refs.push(GitRef {
                    full_name,
                    short_name,
                    oid,
                    kind: GitRefKind::Remote,
                    remote: Some(remote),
                    upstream: None,
                });
            }
        }
    }
    refs
}

pub fn validate_branch_name(cwd: &Path, branch: &str) -> Result<bool> {
    let result = run_git_with(
        cwd,
        &["check-ref-format", "--branch", branch],
        RunOptions {
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

pub fn resolve_oid(cwd: &Path, reference: &str) -> Result<Option<String>> {
    let result = run_git_with(
        cwd,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
        RunOptions {
            accepted_statuses: &[0, 128],
            ..RunOptions::default()
        },
    )?;
    Ok((result.status == 0).then(|| decode_stdout(&result)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_and_remote_refs() {
        let input =
            b"refs/heads/main\0abc\0refs/remotes/origin/main\0\nrefs/remotes/origin/feature/x\0def\0\0";
        let refs = parse_refs(input);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].short_name, "main");
        assert_eq!(refs[1].remote.as_deref(), Some("origin"));
        assert_eq!(refs[1].short_name, "feature/x");
    }
}
