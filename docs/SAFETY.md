# Safety model

Acre's safety promise is narrower and stronger than a generic `--force` flag.

## Ownership

Only worktrees beneath Acre's configured repository `workspaces/` and `slots/` roots can be Acre-owned. Existing worktrees discovered elsewhere are external and are never moved, reset, detached, removed, or recycled.

## No branch deletion

`acre done` detaches a branch from a directory but never deletes the branch ref. Commits remain reachable through the branch exactly as before. Newly created detached commits block return until the user attaches them to a branch; recovered detached workspaces are also retained.

## No automatic repository execution

Workspace preparation never runs package installation, build commands, checkout hooks, or code from `.acre.json`. Internal Git commands disable hooks and filesystem-monitor commands. Git still uses locally configured filters, including Git LFS; the local Git configuration must be trusted. Explicit commands such as `acre <target> -- <command>` and `acre config edit` run only when requested.

## Return assessment

Acre performs the assessment before mutation and, for the current shell, again after the shell has moved. Failure retains the workspace.

Tracked and untracked status comes from Git porcelain v2. Ignored state is listed separately with approved cache roots excluded. Unknown ignored data is blocked by default.

## Seed file preservation

Trusted seed paths are copied after the target is bound. Acre snapshots their content recursively. Any modification blocks return. Idle slots are scrubbed of all seed paths, preventing secret carry-over into untrusted targets. Copies are staged before publication; existing seed paths, including dangling symlinks, are preserved. Cache and seed operations do not follow symlinked parent directories.

## Pull requests

Cross-repository PRs are untrusted. No seed file is copied. Approved cache roots may be cloned into an untrusted review workspace, but that workspace is destroyed on `done` and never becomes a trusted cache source. No repository command is executed.

## Locks

Repository state and the shared repository index use OS-backed exclusive file locks. The OS releases locks when a process exits, including crashes. Lock files remain on disk so concurrent processes always lock the same file. Do not run older versions using directory locks concurrently with this version.

## Recovery bias

When metadata is uncertain:

- active Acre paths are reconstructed conservatively;
- unknown ignored state blocks return;
- missing Git registrations become broken records;
- repair reports state rather than inventing branch identity;
- data retention wins over pool capacity.

Pool reuse, eviction, and garbage collection require an unlocked, detached, clean slot with known cache metadata, an unchanged recorded commit, no unknown ignored data, and no detected processes using the slot. Workspaces stay at their installation paths; opening an idle slot by path takes it out of the pool.

Activation removes a slot from the persisted pool before changing files. If activation or its final state write fails, recovery retains the directory without trusting the old cache fingerprint. Legacy idle slots without a recorded commit remain unverified; explicitly reopen them, attach a branch if detached, and return them to establish a safe baseline.
