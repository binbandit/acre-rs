# Developing Acre itself

Use this only when editing the Acre source repository. Paths below are relative to that checkout, not to the installed skill.

Acre is one Rust executable crate. Start with the checkout's `README.md`, `docs/ARCHITECTURE.md`, and any applicable repository instructions. Requirements are Rust 1.85+ and Git 2.36+; respect `rust-toolchain.toml` when building and testing.

## Follow the ownership boundaries

| Area | Responsibility |
| --- | --- |
| `src/cli.rs`, `src/commands/` | Parse intent, dispatch, render command outcomes. |
| `src/workspace/` | Target resolution, activation, warm pool, leases, safety assessment, and return. |
| `src/environment/` | Fingerprints, approved roots, cache cloning, seed integrity. |
| `src/git/` | The only boundary that runs and parses Git subprocesses. |
| `src/state/` | Atomic persistence, locks, configuration, shell sessions, pending operations. |
| `src/provider/`, `src/shell/`, `src/ui/` | PR resolution, shell navigation protocol, and human/JSON presentation. |

Keep business logic out of rendering and direct Git calls out of command handlers. Persisted types live in `src/model.rs`; other types live with their owning modules. Follow the existing per-file `//!` purpose comment convention.

For an open/return change, trace the command through `workspace/activate.rs` or `workspace/release.rs`, its safety assessment, and persistence. Preserve external worktrees, fixed slot paths, branch refs, trusted-seed boundaries, and conservative retention on uncertainty. Do not turn recovery into silent deletion or automatic repository-code execution.

## Verify at the CLI boundary

Use `tests/common/mod.rs` fixtures for isolated repositories and configuration. For manual reproductions, set `ACRE_CONFIG` to a temporary config whose `root` also points to temporary storage; changing only the config path still leaves the default root at `~/.acre`. Disable background replenishment in a fixture when deterministic pool state matters. Avoid the user's real Acre state and primary working files.

For changed behavior, reproduce the command through the built binary and cover the relevant retention/ownership boundary. JSON output, exit codes, leases, files, and Git refs are observable contracts, not just the human success message.

Run the repository's checks:

```bash
./scripts/check.sh
```

This runs formatting, all-target checks and tests, clippy with warnings denied, and a release build using the pinned toolchain. On macOS, CI additionally runs `python3 tests/audit_regressions.py target/debug/acre`. Consult `docs/VALIDATION.md` for coverage and platform limits.

Update affected command, machine API, environment, and safety documentation alongside behavior changes. If the change alters agent guidance, update `skills/acre/` too. Keep installed-skill references inside that folder; links to source checkout documentation should be explicit upstream URLs unless the instructions are specifically for development in this repo.
