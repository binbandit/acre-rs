//! The schema Acre persists: repository state, configuration, and the records inside them.
//!
//! Types that only live in memory sit next to the code that produces them.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Untrusted means a fork PR: no seed files in, no caches out to the pool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    Worktree,
    LocalBranch,
    RemoteBranch,
    PullRequest,
    NewBranch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTarget {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub head_oid: String,
    pub repository: String,
    pub cross_repository: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredTarget {
    pub kind: TargetKind,
    pub display_name: String,
    pub oid: String,
    pub local_branch: Option<String>,
    pub remote_branch: Option<String>,
    pub pull_request: Option<PullRequestTarget>,
}

// Ready: required roots present. Warm: some caches. Cold: nothing. Unknown: recovered, unverified.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentState {
    Ready,
    Warm,
    Cold,
    Unknown,
}

// How caches arrived: reflinked, copied, reused in place, or never cloned.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloneMode {
    Reflink,
    Copy,
    Reuse,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSnapshot {
    pub fingerprint: String,
    pub state: EnvironmentState,
    pub cache_roots: Vec<String>,
    pub required_roots: Vec<String>,
    pub present_roots: Vec<String>,
    pub source: Option<PathBuf>,
    pub cloned_files: Option<u64>,
    pub cloned_bytes: Option<u64>,
    pub clone_mode: Option<CloneMode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceOwnership {
    Acre,
    External,
}

// Idle is a pool slot; Retained is a recovered workspace kept out of caution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStatus {
    Active,
    Idle,
    Retained,
    Broken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryState {
    pub schema_version: u32,
    pub repository_id: String,
    pub repository_common_dir: PathBuf,
    pub repository_name: String,
    pub updated_at: String,
    #[serde(default)]
    pub slots: Vec<WorkspaceSlot>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceRecord>,
    #[serde(default)]
    pub leases: Vec<WorkspaceLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSlot {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub head: Option<String>,
    pub status: WorkspaceStatus,
    pub environment: Option<EnvironmentSnapshot>,
    pub created_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub id: String,
    pub repository_id: String,
    pub path: PathBuf,
    pub ownership: WorkspaceOwnership,
    pub status: WorkspaceStatus,
    // None for external and recovered workspaces, which are never pooled.
    pub slot_id: Option<String>,
    pub target: StoredTarget,
    pub trust: TrustLevel,
    pub environment: Option<EnvironmentSnapshot>,
    // Ignored paths present at activation; anything beyond these blocks `done`.
    #[serde(default)]
    pub baseline_ignored: Vec<String>,
    #[serde(default)]
    pub seeded_paths: Vec<String>,
    #[serde(default)]
    pub baseline_seed_files: Vec<SeedFileSnapshot>,
    pub activated_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedFileSnapshot {
    pub path: String,
    pub hash: String,
}

// Who is using a workspace; `done` refuses while another holder remains.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLease {
    pub id: String,
    pub workspace_id: String,
    pub holder: String,
    pub pid: Option<u32>,
    pub session_id: Option<String>,
    pub acquired_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDoneOperation {
    pub schema_version: u32,
    pub kind: String,
    pub token: String,
    pub repository_common_dir: PathBuf,
    pub workspace_id: String,
    pub expected_head: String,
    // Checked when the resumed `done` comes back: the tree must be exactly as assessed.
    pub expected_status_fingerprint: String,
    pub safe_destination: PathBuf,
    pub current_session_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownRepository {
    pub id: String,
    pub name: String,
    pub common_dir: PathBuf,
    pub top_level: PathBuf,
    pub remote_url: Option<String>,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSessionState {
    pub schema_version: u32,
    pub id: String,
    pub current_directory: Option<PathBuf>,
    pub previous_directory: Option<PathBuf>,
    pub pid: Option<u32>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AcreConfig {
    pub schema_version: u32,
    pub root: PathBuf,
    pub pool: PoolConfig,
    pub environment: EnvironmentConfig,
    pub safety: SafetyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PoolConfig {
    pub min_slots: usize,
    pub max_slots: usize,
    pub replenish: bool,
    pub idle_retention_days: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvironmentConfig {
    pub cache_roots: Vec<String>,
    pub required_roots: Vec<String>,
    pub seed_files: Vec<String>,
    pub excluded_roots: Vec<String>,
}

// Checked in as .acre.json; data only, so a branch can widen cache roots but never run anything.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoConfig {
    pub environment: Option<RepoEnvironmentConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoEnvironmentConfig {
    #[serde(default)]
    pub cache_roots: Vec<String>,
    #[serde(default)]
    pub required_roots: Vec<String>,
    #[serde(default)]
    pub seed_files: Vec<String>,
    #[serde(default)]
    pub excluded_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SafetyConfig {
    pub detect_processes: bool,
    pub block_unknown_ignored_files: bool,
}

impl fmt::Display for EnvironmentState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ready => "ready",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Unknown => "unknown",
        })
    }
}

impl fmt::Display for WorkspaceOwnership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Acre => "acre",
            Self::External => "external",
        })
    }
}
