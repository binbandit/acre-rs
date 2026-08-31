# Safety model

Acre's safety promise is narrower and stronger than a generic `--force` flag.

## Ownership

Only worktrees beneath Acre's configured repository `workspaces/` and `slots/` roots can be Acre-owned. Existing worktrees discovered elsewhere are external and are never moved, reset, detached, removed, or recycled.

## No branch deletion

`acre done` detaches a branch from a directory but never deletes the branch ref. Commits remain reachable through the branch exactly as before.

## No automatic repository execution

Acre never runs package installation, build commands, shell hooks, editor commands, or code from `.acre.json`. Configuration is data-only.

## Return assessment

Acre performs the assessment before mutation and, for the current shell, again after the shell has moved. Failure retains the workspace.

Tracked and untracked status comes from Git porcelain v2. Ignored state is listed separately with approved cache roots excluded. Unknown ignored data is blocked by default.

## Seed file preservation

Trusted seed paths are copied after the target is bound. Acre snapshots their content recursively. Any modification blocks return. Idle slots are scrubbed of all seed paths, preventing secret carry-over into untrusted targets.

## Pull requests

Cross-repository PRs are untrusted. No seed file is copied. Approved cache roots may be cloned into an untrusted review workspace, but that workspace is destroyed on `done` and never becomes a trusted cache source. No repository command is executed.

## Locks

Repository locks use an exclusive directory and owner token. Only the token owner can release a lock. A live PID is never evicted based only on elapsed time.

## Recovery bias

When metadata is uncertain:

- active Acre paths are reconstructed conservatively;
- unknown ignored state blocks return;
- missing Git registrations become broken records;
- repair reports state rather than inventing branch identity;
- data retention wins over pool capacity.
