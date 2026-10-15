# Environment reuse and recovery

Read this when improving warm reuse, configuring a project, or investigating why Acre retained a workspace.

## Get the benefit of warm workspaces

Prefer reusing a compatible slot at its fixed path. Acre fingerprints recognized manifests, lockfiles, package-manager settings, toolchain declarations, patches, and platform/runtime facts at the target commit, including nested projects. Similar branch names do not imply compatible environments.

| Environment state | How to act |
| --- | --- |
| `ready` | All required roots exist. Run the task's normal checks; this does not validate installed package health. |
| `warm` | Optional reusable caches exist, but required roots may be absent. Inspect the root lists before installing. |
| `cold` | No approved reusable roots exist. Run the normal project setup/build if the task needs it. |
| `unknown` | Recovered environment metadata could not be verified. Inspect it rather than assuming reuse is safe. |

Node projects normally require `node_modules`; Python projects normally require `.venv`. Rust's `target` is optional, so Rust workspaces are usable when reported as cold or warm. Required roots come from top-most projects: sibling `web` and `api` projects may each need their own environment, while nested helper projects can have optional caches.

To build up reuse, prepare a workspace with the project's normal install/build command, preserve any source changes through the task's Git workflow, then return it safely. Matching work can reuse those ignored roots. `acre system warm --slots 2` prepares idle slots; it does not install dependencies and cannot guarantee a ready environment when no compatible prepared source exists.

Whole-slot reuse is preferred. Concurrent workspaces can receive cloned or copied caches, and `environment.cloneMode` records what actually happened. Do not claim a speedup or copy-on-write clone based on `reused` alone.

Specific traps:

- Python virtual environments with `pyvenv.cfg` are reused in place, not copied between workspaces. Create each concurrent workspace's environment at its own returned path.
- A GUI agent without the shell's `node` on `PATH` gets a different Node generation. Align the caller's runtime environment before expecting shared warm slots.
- Source changes to fingerprint files disqualify a cache source. Do not manually copy its dependencies into another generation to work around that decision.
- Only approved Git-ignored cache roots qualify. Tracked paths and symlinked cache roots are not copied or cleared.
- `.env` and `.env.local` are trusted local seeds, not caches. Acre copies them from the trusted primary checkout only when ignored and absent at the destination, records their content, and removes unchanged copies on return. Modified seeds block return.

## Configure only what the project needs

Inspect current settings first:

```bash
acre --json config show
acre config path
acre -C /path/to/repo --json system inspect
```

User configuration can be updated with `acre config set <dotted-key> <json-or-string>`, for example `acre config set pool.maxSlots 6` when more concurrency is wanted. `pool.minSlots` must not exceed `pool.maxSlots`.

For repository-specific environment rules, `acre -C /path/to/repo config repo-init` creates `.acre.json`. Read an existing file before editing it; do not force-overwrite it. Its `environment` object supports `cacheRoots`, `requiredRoots`, `seedFiles`, and `excludedRoots`. Use repository-relative paths and keep seed paths separate from cache paths. Cache roots can match nested projects too.

Add cache roots only for reproducible, disposable outputs. Do not approve a broad ignored directory containing user data or secrets just to make `done` succeed. Unknown keys, duplicate paths, and overlapping seed/cache paths are rejected. In automation use `config set` rather than interactive `config edit`.

## Diagnose a refusal

Start with the actual error or release assessment, then inspect the affected checkout with `git status` and `acre -C /path/to/repo --json system inspect`. `acre -C /path/to/repo --json system doctor` checks configuration, Git, and state; an empty pool can be a warning even when it exits successfully.

| Evidence | Response |
| --- | --- |
| Dirty files or changed seeds | Preserve and report the changes. Finish them through the user's workflow or keep the workspace active. |
| Unknown ignored files | Inspect the named data. Move/preserve needed files; approve a cache root only if it really is disposable output. |
| Other leases | Let their owners finish. Release only your own IDs. |
| Process use | Stop your task's processes and leave the directory. `-C` alone does not move a caller whose real working directory is still inside it. |
| Merge/rebase/bisect or detached commits | Resolve the Git workflow or preserve commits on a branch. Do not silently abort an operation. |
| Busy/conflicting repository state | Inspect the competing operation. Retry after it finishes; avoid rapid or indefinite retries. Persistent lock files are normal OS-lock anchors, not evidence of a stale lock. |
| Unreadable/broken state | Inspect with `doctor`; use `system repair` when state recovery is the intended action. Repair preserves unreadable state for inspection and reconstructs conservatively from Git. |

`system gc` removes eligible unused infrastructure after safety assessment. It is not a way to bypass a retained workspace. Process-inspection failures on macOS/Linux retain workspaces; Windows lacks a working-directory process scanner, so lease discipline remains important.

JSON failures have `error.code`, `error.message`, and `error.details`. Exit classes are 1 internal, 2 usage/configuration, 3 environment unavailable, 4 target not found, 5 conflict, 6 safe refusal, 7 Git, 8 network/provider, and 130 interruption. A successful release can still report retention; do not use exit code 0 as proof of recycling. Exit 194 and the `__resume` protocol belong to shell integration, not machine clients.
