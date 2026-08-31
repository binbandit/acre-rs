# Acre

> **Every branch, already ready.**

Acre is a warm workspace runtime for Git. It opens an existing worktree, local branch, remote branch, or pull request in an isolated workspace while reusing a compatible dependency environment and build caches.

Acre is deliberately not a general Git workflow suite. It does not merge branches, write commits, push, run repository hooks, or replace your editor or agent harness.

```console
$ acre new feature/refunds

Created feature/refunds
from origin/main

→ feature/refunds
workspace ready · reused environment 52ba538f · 284 ms
```

## Daily interface

```text
acre
acre <target>
acre new <branch>
acre -
acre done
acre <target> -- <program> [args...]
```

- `acre` chooses existing work.
- `acre <target>` opens an existing worktree, local branch, remote branch, or pull request.
- `acre new <branch>` is the only operation that creates a branch.
- `acre -` returns to the previous exact location for the current shell.
- `acre done` safely returns an Acre-owned workspace to the warm pool.
- `acre <target> -- <program>` runs a command there without navigating.

An unknown target is never silently turned into a new branch.

## Requirements

- Rust 1.85 or newer
- Git 2.31 or newer
- `gh` only for `pr:<number>` and GitHub pull-request URLs

## Build and install

```bash
cargo build --release
cargo install --path .
acre setup
exec "$SHELL" -l
```

Run the complete local checks before using a development checkout:

```bash
./scripts/check.sh
```

## First trial

Start in a disposable or safely committed repository:

```bash
acre system doctor
acre system warm
acre new acre/trial
```

When Acre reports a cold environment, run the project's normal setup command yourself, such as:

```bash
pnpm install
```

Acre never executes repository-controlled setup scripts automatically. When the workspace is clean:

```bash
acre done
acre acre/trial
```

See [the work trial guide](docs/WORK_TRIAL.md) before using Acre against important uncommitted work.

## What Acre owns

Acre keeps a small pool of detached reusable Git worktrees per repository. It fingerprints dependency generations from lockfiles and toolchain declarations, selects a compatible warm slot, moves the slot to a stable branch-derived path, binds the requested target, reuses approved cache roots, and copies trusted local seed files only for trusted targets.

When `acre done` is safe, Acre preserves the branch and commits, detaches the worktree, removes trusted seed copies, and returns the prepared environment to the pool.

Acre refuses recycling when it finds dirty work, an in-progress Git operation, changed seed files, unexplained ignored files, another lease, a process using the directory, or state it cannot prove safe.

External worktrees created by Git, Worktrunk, Cursor, Claude, Codex, an IDE, or another tool are addressable but never moved, reset, removed, scrubbed, or pooled by Acre.

## Environment generations

Built-in definitions cover pnpm, npm, Yarn, Bun, Turborepo, Next.js, Rust, Python, uv, Poetry, and Go. Fingerprints include relevant project files plus platform and runtime facts. An incompatible slot is never presented as compatible.

Copy-on-write reuse is attempted where the host tools and filesystem support it. Acre records the actual clone mode and falls back to an ordinary copy rather than pretending.

Default trusted seed files are `.env` and `.env.local`. They are copied only from the trusted primary checkout, only when ignored by Git, and never into untrusted cross-repository pull-request workspaces.

## Machine API

Editors, agents, and automation can use the same broker:

```bash
acre --json acquire feature/refunds \
  --holder codex:session-218 \
  --pid 48122

acre --json release --lease-id <lease-id>
```

Explicit creation through the machine API requires `--new`:

```bash
acre --json acquire feature/new-work \
  --new \
  --from origin/main \
  --holder editor:zed
```

See [the machine API reference](docs/MACHINE_API.md).

## Administrative commands

```bash
acre system warm
acre system inspect
acre system doctor
acre system repair
acre system gc

acre config show
acre config path
acre config init
acre config set pool.maxSlots 6
acre config edit
acre config repo-init
```

These stay outside the daily product vocabulary.

## Architecture

This repository is intentionally one Rust crate with clear internal modules rather than a micro-crate graph:

```text
commands       thin CLI handlers
git            the only Git process boundary
environment    fingerprints, cache inspection, COW seeding, seed integrity
pool           allocation, activation, leases, assessment, return, recovery
state          atomic JSON, locks, repository index, sessions, pending operations
shell          parent-shell directive protocol and generated integrations
ui             semantic output, prompts, and the focused inline picker
```

Read [the architecture](docs/ARCHITECTURE.md) for the dependency rules and lifecycle.

## Safety status

This source reconstruction was created in an environment without a Rust toolchain, so its archive was statically checked but could not be compiled here. Run `./scripts/check.sh` locally before trialling it. The exact validation boundary is recorded in [docs/VALIDATION.md](docs/VALIDATION.md).

## License

MIT or Apache-2.0, at your option.
