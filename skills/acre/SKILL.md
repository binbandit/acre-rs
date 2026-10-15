---
name: acre
description: Use Acre to open or create Git workspaces, reuse dependency environments and build caches, manage agent leases, review pull requests in isolation, and safely return workspaces to the warm pool. Use when the user mentions Acre or the repository uses Acre for workspace management, including troubleshooting or developing Acre itself.
---

# Acre

Acre separates a Git checkout from its prepared environment. It opens existing work in a fixed workspace directory and reuses compatible ignored dependencies and caches. Use the project's normal tools for installing dependencies, testing, committing, merging, and pushing; Acre does not perform those operations.

## Choose the right interface

- For an agent working across multiple tool calls, use JSON `acquire` and `release`. Keep the returned path and lease ID for the whole task.
- For a single command in existing work, use `acre <target> -- <program> [args...]`. Acre holds a temporary lease while the command runs and releases it afterward; the workspace remains active.
- For a human terminal, `acre setup` installs shell navigation. Bare `acre` opens a picker, `acre <target>` navigates, and `acre -` returns to the previous exact location. These depend on the shell session; a subprocess cannot change the agent's later working directory.
- If the task is already in the intended workspace, use it. A second acquisition of the same target shares that checkout; it does not create an independent copy. Concurrent independent edits need separate branches/workspaces.

Start with `acre --version` and select the repository explicitly with `-C` when the tool's working directory is uncertain. Use `acre -C /path/to/repo --json system inspect` to discover workspaces, ownership, slots, and leases. Use `system doctor` for diagnosis, not as a prerequisite to every open.

If Acre is missing, follow the source repository's installation instructions (`cargo install --path .` from an Acre checkout). Installing this skill does not install the executable. Shell setup is unnecessary for the machine API and modifies shell configuration, so only use it when shell integration is wanted.

## Agent workspace lifecycle

The paths, branch names, and holder labels below are examples; substitute the actual task's values.

### Open existing work

```bash
acre -C /path/to/repo --json acquire feature/refunds \
  --holder agent:refund-task
```

Targets include local branches, remote branches such as `origin/feature/refunds`, `pr:1842`, GitHub pull-request URLs, and existing worktree paths. PR resolution needs an authenticated `gh`. For ambiguous or command-shaped names, use `branch:done`, `remote:origin/new`, or `worktree:/absolute/path`.

An unknown target is an error. Check spelling, available refs, and the response's suggestions; do not turn a failed lookup into branch creation.

### Create new work deliberately

```bash
acre -C /path/to/repo --json acquire feature/new-work \
  --new --from origin/main --holder agent:new-work-task
```

Choose a base that exists and matches the user's intent. `--from .` uses HEAD in the selected repository directory. `--fresh` fetches the exact remote base before creation when freshness is needed. Only `acquire --new` and `acre new` create new task branches; a plain open never invents one from an unknown name.

### Work in the returned path

1. Check the process exit status and parse stdout as JSON. Continue only on `ok: true`.
2. Record `path`, `lease_id`, `ownership`, `branch`, and `environment`. `branch` and `environment` can be null, including for some external/detached worktrees.
3. Set every subsequent editing, build, test, and Git tool's working directory to the returned absolute `path`. Do not infer a directory from the branch name; slot paths remain fixed across reuse.
4. Inspect the checkout's instructions and Git status before editing. Opening an already active checkout preserves its existing changes.
5. Use `environment.state`, `requiredRoots`, and `presentRoots` to decide whether the project's normal setup is needed. `ready` means roots exist, not that tests pass. See [environment and recovery](references/environment-and-recovery.md) for cache behavior and tuning.

Every successful `acquire` creates a distinct lease, even with the same holder label. Release every lease you create. Supply `--pid` only when you know the long-lived process responsible for the task; otherwise omit it. A short-lived tool shell's PID can expire while the agent is still using the workspace.

A lease prevents recycling; it is not an exclusive editing lock. If an acquire response is lost, inspect leases before retrying so you do not leak another lease.

### Finish or hand off

Stop task-owned servers/watchers and move the calling process out of the workspace before asking Acre to return it. Preserve task edits with the user's chosen Git workflow; do not commit, discard files, or kill unrelated processes merely to satisfy cleanup.

```bash
# Run from the primary checkout or another directory outside the workspace.
acre --json release --lease-id <returned-lease-id>
```

Release removes that exact lease and, when it was the last lease, attempts a safe return. Interpret the response:

| Result | Meaning and next action |
| --- | --- |
| `ok: true`, `pooled: true` | The workspace returned to the warm pool; stop using its path. The branch and commits remain. |
| `ok: true`, `retained: true` | The lease is gone but the workspace remains. Read `assessment.reasons` or `return_error`; report unfinished work or the concrete blocker. |
| `external: true` | Acre left an external worktree untouched. |
| `ok: true`, `retained: false`, `pooled: false` | The workspace was removed, as with a safely returned untrusted PR checkout. Stop using its path. |
| `ok: false` | Inspect `error.code`, `message`, and `details`; do not assume cleanup succeeded. |

For a handoff that should keep the checkout active:

```bash
acre --json release --lease-id <returned-lease-id> --keep-active
```

Once release succeeds, do not repeatedly release the old ID to retry pooling. After resolving a retention reason, run `acre -C /path/to/primary --json done <workspace-path>` from outside that workspace. A human using shell integration can use `acre done` from inside it.

Report the branch/path, whether the lease was released, and whether Acre pooled, retained, or removed the workspace. Acquisition and release are separate outcomes from the task's tests and Git publication.

## Preserve Acre's safety boundaries

- Acre-owned workspaces can be pooled; primary and external worktrees stay in place. Do not manually remove, rename, reset, or scrub an external worktree as an Acre cleanup step.
- Dirty tracked/untracked work, Git operations, changed seeds, unknown ignored files, other leases, process use, or uncertain metadata can block return. Diagnose the reason instead of bypassing the checks with `git clean -fdx`, deleting state/locks, or disabling safety settings.
- Cross-repository PRs are untrusted: Acre withholds seed files, never uses their workspaces as trusted cache sources, and destroys their clean checkouts on return. Do not copy secrets into them to make tests pass. Opening a PR does not itself authorize executing its code.
- Acre never runs repository setup scripts or hooks during workspace preparation. Explicit child commands, dependency installation, and builds execute repository code and should follow the task's normal trust and authorization boundaries.
- Detached commits must be attached to a branch before return if they are to be kept. `done` preserves branch refs and commits; it does not merge, push, or delete branches.

## Less common tasks

- For warming, cache compatibility, configuration, or a retained/broken workspace, read [environment and recovery](references/environment-and-recovery.md).
- When changing Acre's Rust implementation, read [developing Acre](references/developing-acre.md).
- For additional version-specific details, use the installed command's help or the upstream [command reference](https://github.com/binbandit/acre-rs/blob/main/docs/COMMANDS.md) and [machine API](https://github.com/binbandit/acre-rs/blob/main/docs/MACHINE_API.md). The operational guidance above and bundled references remain usable when this skill is installed outside the Acre repository.
