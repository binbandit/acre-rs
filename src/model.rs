use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SupportedShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

#[derive(Debug, Clone)]
pub struct GlobalOptions {
    pub directory: Option<PathBuf>,
    pub json: bool,
    pub no_color: bool,
    pub plain: bool,
    pub verbose: bool,
}

#[derive(Debug, Clone)]
pub struct ShellBridge {
    pub active: bool,
    pub directive_file: Option<PathBuf>,
    pub session_id: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CommandContext {
    pub cwd: PathBuf,
    pub interactive: bool,
    pub global: GlobalOptions,
    pub shell: ShellBridge,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub top_level: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub remote: Option<String>,
    pub remote_url: Option<String>,
    pub default_branch: Option<String>,
    pub current_worktree: Option<GitWorktree>,
    pub worktrees: Vec<GitWorktree>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub branch_ref: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
    pub is_main: bool,
    pub exists: bool,
}

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

#[derive(Debug, Clone, Copy)]
pub struct EcosystemDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub fingerprint_files: &'static [&'static str],
    pub cache_roots: &'static [&'static str],
    pub required_roots: &'static [&'static str],
    pub seed_files: &'static [&'static str],
}

#[derive(Debug, Clone)]
pub struct EnvironmentPlan {
    pub fingerprint: String,
    pub ecosystem_ids: Vec<String>,
    pub cache_roots: Vec<String>,
    pub required_roots: Vec<String>,
    pub seed_files: Vec<String>,
    pub platform_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentState {
    Ready,
    Warm,
    Cold,
    Unknown,
}

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
    pub target_paths: BTreeMap<String, PathBuf>,
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
    pub slot_id: Option<String>,
    pub target: StoredTarget,
    pub trust: TrustLevel,
    pub environment: Option<EnvironmentSnapshot>,
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

#[derive(Debug, Clone)]
pub struct MaterializedWorkspace {
    pub repository: Repository,
    pub target: ResolvedTarget,
    pub workspace: WorkspaceRecord,
    pub path: PathBuf,
    pub created: bool,
    pub reused: bool,
    pub elapsed_ms: u128,
    pub lease: Option<WorkspaceLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeStatus {
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub entries: Vec<StatusEntry>,
    pub dirty: bool,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    pub original_path: Option<String>,
    pub index: String,
    pub worktree: String,
    pub kind: StatusEntryKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusEntryKind {
    Changed,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessUse {
    pub pid: u32,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoneAssessment {
    pub workspace: WorkspaceRecord,
    pub status: WorkingTreeStatus,
    pub operation: Option<String>,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub new_ignored: Vec<String>,
    pub changed_seed_files: Vec<String>,
    pub leases: Vec<WorkspaceLease>,
    pub processes: Vec<ProcessUse>,
    pub safe: bool,
    pub reasons: Vec<String>,
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
#[serde(rename_all = "camelCase")]
pub struct AcreConfig {
    pub schema_version: u32,
    pub root: PathBuf,
    pub pool: PoolConfig,
    pub environment: EnvironmentConfig,
    pub safety: SafetyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    pub min_slots: usize,
    pub max_slots: usize,
    pub replenish: bool,
    pub idle_retention_days: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentConfig {
    #[serde(default)]
    pub cache_roots: Vec<String>,
    #[serde(default)]
    pub required_roots: Vec<String>,
    #[serde(default)]
    pub seed_files: Vec<String>,
    #[serde(default)]
    pub excluded_roots: Vec<String>,
}

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
#[serde(rename_all = "camelCase")]
pub struct SafetyConfig {
    pub detect_processes: bool,
    pub block_unknown_ignored_files: bool,
}

#[derive(Debug, Clone)]
pub struct HostedRemote {
    pub host: String,
    pub owner: String,
    pub repo: String,
}
