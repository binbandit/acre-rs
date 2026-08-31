# Architecture

Acre is one executable crate with explicit internal boundaries. The design favours readable domain code over frameworks, service containers, generic repositories, or a micro-crate graph.

```text
CLI / commands
      ↓
pool + target + environment
      ↓
git + state + provider
      ↓
filesystem and child processes
```

## Rules

1. `commands` parses user intent and renders outcomes. It does not invoke Git directly.
2. `git` is the only module allowed to run or parse Git.
3. `pool` owns the warm-workspace lifecycle and safety decisions.
4. `environment` owns compatibility fingerprints, cache roots, copy-on-write attempts, and seed-file integrity.
5. `state` owns persisted JSON, locks, shell history, leases, and pending two-phase operations.
6. `ui` owns human, plain, and JSON rendering. Business logic does not print.
7. External worktrees are observable but never mutated by Acre.
8. Uncertainty retains a workspace instead of recycling it.

## Source layout

```text
src/
  main.rs                process entrypoint
  cli.rs                 clap declarations and dispatch
  error.rs               stable error codes and exit classes
  model.rs               domain and persisted models
  target.rs              exact target resolution

  commands/              daily, machine, and administrative handlers
  git/                   process runner, discovery, porcelain parsers, mutations
  environment/           detection, fingerprint, inspection, clone, seed integrity
  pool/                  broker, assessment, process evidence, recovery, maintenance
  provider/              forge target resolution
  shell/                 directive protocol and integration generation
  state/                 paths, atomic JSON, config, locks, index, leases, sessions
  ui/                    semantic rendering, prompts, focused picker
```

## Core lifecycle

```text
open/create
  → resolve exact Git target
  → lock repository state
  → reconcile state against Git
  → fingerprint target environment
  → choose matching idle slot or create one
  → move slot to stable active path
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
  → move workspace to its idle slot
  → save state atomically
```

## Why Git stays a subprocess

Git is authoritative for repository discovery, worktree registrations, refs, status, and mutations. Acre uses stable porcelain formats and controls subprocess count rather than replacing installed Git semantics with a second implementation.

## State

Acre state lives outside repositories under `$ACRE_HOME`, or the configured root. Writes use a temporary file in the same directory, `sync_all`, atomic rename, and parent-directory sync where supported.

Persisted state is reconstructable from Git worktree registrations and Acre-owned path boundaries. Recovered ownership is treated conservatively.

## No daemon

Acre is correct command-by-command. Pool replenishment may use a short-lived detached Acre process, but correctness never depends on a permanently running service.
