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

impl GitRef {
    /// `origin/feature` for a remote branch, `feature` for a local one.
    pub fn qualified_name(&self) -> String {
        match &self.remote {
            Some(remote) => format!("{remote}/{}", self.short_name),
            None => self.short_name.clone(),
        }
    }
}

pub fn list_refs(cwd: &Path) -> Result<Vec<GitRef>> {
    // NUL-separated so branch names with odd characters survive; the trailing %00 ends each record.
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

/// Parses `for-each-ref` output: NUL-separated name, object id, and upstream per ref, with a
/// newline between refs that lands at the start of the next name.
pub fn parse_refs(buffer: &[u8]) -> Vec<GitRef> {
    let values: Vec<String> = buffer
        .split(|byte| *byte == 0)
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect();
    values
        .chunks_exact(3)
        .filter_map(|record| {
            let full_name = record[0].trim_start_matches(['\r', '\n']);
            let oid = record[1].as_str();
            let upstream = record[2].trim_end_matches(['\r', '\n']);
            // origin/HEAD is a pointer, not a branch anyone opens.
            if full_name.is_empty() || oid.is_empty() || full_name.ends_with("/HEAD") {
                return None;
            }
            if let Some(short_name) = full_name.strip_prefix("refs/heads/") {
                return Some(GitRef {
                    full_name: full_name.to_owned(),
                    short_name: short_name.to_owned(),
                    oid: oid.to_owned(),
                    kind: GitRefKind::Local,
                    remote: None,
                    // Shortened to `origin/main`, which is how the picker and completion present it.
                    upstream: (!upstream.is_empty()).then(|| {
                        upstream
                            .strip_prefix("refs/remotes/")
                            .unwrap_or(upstream)
                            .to_owned()
                    }),
                });
            }
            let (remote, short_name) = full_name.strip_prefix("refs/remotes/")?.split_once('/')?;
            Some(GitRef {
                full_name: full_name.to_owned(),
                short_name: short_name.to_owned(),
                oid: oid.to_owned(),
                kind: GitRefKind::Remote,
                remote: Some(remote.to_owned()),
                upstream: None,
            })
        })
        .collect()
}

pub fn validate_branch_name(cwd: &Path, branch: &str) -> Result<bool> {
    let result = run_git_with(
        cwd,
        &["check-ref-format", "--branch", branch],
        RunOptions {
            // 1 means invalid, 128 means git itself objected; both are just "no".
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

pub fn resolve_oid(cwd: &Path, reference: &str) -> Result<Option<String>> {
    let result = run_git_with(
        cwd,
        // ^{commit} peels tags, so a tag name resolves to something we can check out detached.
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
