# Validation

`./scripts/check.sh` is the bar for every change. CI runs the same checks on the pinned toolchain from `rust-toolchain.toml` across Linux, macOS, and Windows, and runs tests and strict Clippy on current stable on Linux:

```text
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

## What the tests cover

- Unit tests sit next to the code: ref, status, and worktree parsing; markup; fingerprint normalisation; cache-root classification against a real Git repository.
- `tests/safety.rs` reproduces lease bypass, untrusted cache reuse, symlink escapes, unsafe pool reuse and garbage collection, configuration normalization, nested manifest changes, hook execution, session path traversal, detached commits in active and idle workspaces, failed activation, changed cache-source manifests, missing PR trust metadata, and independent-clone state isolation.
- `tests/state.rs` covers concurrent processes, crash release, failed writes, and legacy state-path compatibility. `tests/process.rs` covers simultaneous large input and output, inherited-pipe deadlines, and descendant cancellation.
- `tests/replenish.rs` pauses a real replenisher before cache copying and verifies foreground progress, concurrent state updates, and preservation of worktrees opened or edited during preparation.
- `tests/end_to_end.rs` drives the built binary through throwaway repositories: opening and returning workspaces, the shell directive round trip including `acre -`, the two-phase `done` that resumes after the shell moves, the machine acquire and release cycle, warm-pool reuse across branches, Python environments staying at their installation path, nested cache roots, recovery after lost state, repair accounting, and CLI flag ordering.

## What is exercised by hand

- Pull request resolution needs `gh` and a GitHub remote.
- Process detection is platform specific: `lsof` on macOS, `/proc` on Linux, nothing on Windows.
- The generated shell functions themselves run inside a real interactive shell; the tests exercise the directive protocol beneath them.

## Performance playground

Build a release binary, then run:

```sh
python3 scripts/performance-playground.py --binary target/release/acre
```

The script creates a persistent temporary playground with 1,000 branches spanning 20 commits,
24 registered worktrees, 2,000 tracked files, 5,000 dependency-cache files, and eight warm slots.
It measures cold and warm `acre new` calls and `acre -` shell directive round trips. On Unix it
also holds the repository lock for two seconds to reproduce navigation during background
workspace preparation. Each scenario saves individual timings, its five slowest Git commands,
and full Git process traces, with medians in `results.json`. Pool replenishment is disabled
during ordinary measurements so
background copying cannot contaminate the comparison. No network or user Acre state is used.
Cold creation means an empty pool; the primary checkout still has dependency caches available
to copy into a newly created worktree.
Every creation checks cache readiness and sample file contents. A separate scenario starts the
real `__replenish` worker and measures opening `main` while it prepares the next cache. This
catches lock contention hidden by isolated timings. Use `--cache-files 100000 --slots 1 --runs 1`
to exercise large copies without preparing many duplicate cache trees.

Compare two saved binaries with identical fixture sizes:

```sh
python3 scripts/performance-playground.py --compare /tmp/acre-before --binary target/release/acre
```

Use `--branches`, `--worktrees`, `--files`, `--cache-files`, `--slots`, and `--runs` to vary scale.
`--output` selects a new directory; existing directories are refused. The playground keeps
the exact binaries and repositories for further investigation. On Unix each fixture has an
`acre` launcher that selects its isolated configuration. From the fixture's `repo` directory,
enable navigation in a disposable Zsh session with the following (use `bash` instead of `zsh`
for Bash):

```sh
export ACRE_EXECUTABLE="$(cd .. && pwd)/acre"
eval "$("$ACRE_EXECUTABLE" shell init zsh)"
```

Remove the printed playground directory when finished.

The integration tests also verify that navigation completes while a repository lock remains
held, moves leases correctly between repositories, and that pool selection skips unsafe
preferred slots without scanning unused candidates. Creation must use exact ref lookups
instead of enumerating every branch, while preserving local, remote, explicit, and fresh bases.
These check the cause of the regression without imposing tight machine-dependent timings.

## Open limitations

- Process groups are resource cleanup, not a sandbox. Unix descendants that deliberately leave the group can survive cancellation; a captured process without a timeout can wait indefinitely for inherited pipes.
- Linux and Windows behavior must pass the CI matrix. Cross-compilation checks Windows types, but local macOS tests do not execute Windows job objects or verify every supported shell.

Start with a disposable repository or a branch whose work is already committed. Acre is deliberately conservative, but filesystem and worktree lifecycle software should not be trialled first against irreplaceable uncommitted work.

## Dependencies

CI audits the committed lockfile against RustSec on pull requests, main pushes, and weekly. Run `cargo audit --deny warnings` locally when changing dependencies. CI actions use full commit SHAs; Dependabot proposes weekly updates with a seven-day cooldown. Review lockfile changes, new maintainers, build scripts, and transitive additions before merging. Advisory checks identify known issues, not undisclosed compromises.

## Audit regressions

`tests/regressions.rs` exercises the safety, state identity, selector, environment, and output fixes through the CLI on every CI platform. They also cover `done` with a directory override, exact subdirectory history after returning a workspace, remote names containing slashes, and directory seed normalization. The replenishment suite also changes the source manifests and caches while a real background copy is paused, then verifies the incompatible copy is discarded.

On macOS, run `python3 tests/audit_regressions.py target/debug/acre` after building. CI runs these 41 additional regressions with the system Bash and Zsh, Node/npm, and `gh`. Fixtures isolate Git configuration, Acre state, and setup home directories. GitHub replies are stubbed; the real Enterprise routing probe uses placeholder credentials and a local proxy that refuses forwarding. Failed process probes are injected deliberately. Real child processes also verify detection in directories containing newlines, backslashes, control characters, and Unicode. Seed tests cover repeated separators and directory-only ignore rules, and provider tests cover owners and repositories named `pull`. The runner returns nonzero on any failure and prints the directory containing subprocess transcripts.

The package-rename regression refreshes the trusted primary installation and verifies that opening a new generation replaces old workspace links and successfully imports the renamed package. The pnpm regression removes the primary cache to isolate invalidation of the old pool generation. Neither test treats directory-presence readiness as proof of installed-package health.
