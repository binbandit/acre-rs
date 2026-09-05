# Architecture

Acre is one executable crate with explicit internal boundaries. The design favours readable domain code over frameworks, service containers, generic repositories, or a micro-crate graph.

```text
CLI / commands
      ↓
workspace + environment
      ↓
git + state + provider + shell
      ↓
filesystem and child processes
```

## Rules

1. `commands` parses user intent and renders outcomes. It does not invoke Git directly.
2. `git` is the only module allowed to run or parse Git.
3. `workspace` owns the lifecycle: resolving targets, the warm pool, activation, assessment, return, and leases.
4. `environment` owns compatibility fingerprints, cache roots, copy-on-write attempts, and seed-file integrity.
5. `state` owns persisted JSON, locks, shell history, and pending two-phase operations.
6. `ui` owns human, plain, and JSON rendering. Business logic does not print.
7. External worktrees are observable but never mutated by Acre.
8. Uncertainty retains a workspace instead of recycling it.

## Source layout

```text
src/
  main.rs                process entrypoint
  cli.rs                 clap declarations, the invocation context, dispatch
  error.rs               stable error codes and exit classes
  model.rs               the persisted schema: state records and configuration
  util.rs                small shared helpers

  commands/              one handler per command, plus the post-open summary
  workspace/             resolve, pool, activate, assess, release, lease, maintain, process
  environment/           definitions, fingerprint, inspect, roots, clone, seed
  git/                   runner, discovery, porcelain parsers, mutations
  state/                 storage, paths, config, lock, repository, index, shell, operations
  provider/              remote URL parsing and GitHub pull requests
  shell/                 directive protocol, generated integrations, navigation
  ui/                    markup, output, prompts, picker, error rendering
```

Types live next to the code that produces them; `model.rs` holds only what is written to disk. Every file opens with a one-line `//!` comment saying what it is for.

To follow a command end to end, start at `cli.rs`, then the handler in `commands/`, then `workspace/activate.rs` for opening or `workspace/release.rs` for returning.

## Core lifecycle

```text
open/create
  → resolve exact Git target
  → lock repository state
  → reconcile state against Git
  → fingerprint target environment
  → choose matching idle slot or create one
  → persist its removal from the pool before changing files
  → bind branch or detached PR commit
  → reuse or seed compatible caches
  → copy trusted seed files when allowed
  → acquire shell or machine lease
  → save state atomically
```

```text
done/release
  → lock repository state
  → remove only the calling lease
  → read status and operation state
  → compare ignored paths and seed integrity
  → detect other leases and process use
  → retain on any uncertainty
  → detach clean workspace
  → scrub copied seed files
  → mark the workspace idle without moving its directory
  → save state atomically
```

## Why Git stays a subprocess

Git is authoritative for repository discovery, worktree registrations, refs, status, and mutations. Acre uses stable porcelain formats and controls subprocess count rather than replacing installed Git semantics with a second implementation.

## State

Acre state lives outside repositories under the configured root (default `~/.acre`). `ACRE_CONFIG` selects the configuration file. Writes use a temporary file in the same directory, `sync_all`, atomic rename, and parent-directory sync where supported.

Persisted state is reconstructable from Git worktree registrations and Acre-owned path boundaries. Recovered ownership is treated conservatively.

## No daemon

Acre is correct command-by-command. Pool replenishment may use a short-lived detached Acre process, but correctness never depends on a permanently running service.

Repository identity comes from the canonical Git common directory, so independent clones do not share state. Existing remote-keyed state keeps its paths when it belongs to that common directory. Repository and index updates use OS-backed file locks via `fs4`; temporary files use `tempfile`.

Workspace directories identify reusable slots, not branches. New slots live under `workspaces/<slot-id>`; existing managed directories keep their locations. Branch and PR names are display labels. Missing state or an interrupted activation recovers orphaned worktrees as retained workspaces, since a detached checkout alone does not prove it safe to recycle. Each reusable slot records its commit so later detached commits cannot be lost through reuse or garbage collection.

Captured commands run in Unix process groups or Windows job objects using `process-wrap`. Input, output, and process exit share one timeout. Cancellation kills the group and reaps the immediate child; interactive passthrough commands retain normal terminal behavior.
