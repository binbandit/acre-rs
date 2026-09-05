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
- `tests/end_to_end.rs` drives the built binary through throwaway repositories: opening and returning workspaces, the shell directive round trip including `acre -`, the two-phase `done` that resumes after the shell moves, the machine acquire and release cycle, warm-pool reuse across branches, Python environments staying at their installation path, nested cache roots, recovery after lost state, repair accounting, and CLI flag ordering.

## What is exercised by hand

- Pull request resolution needs `gh` and a GitHub remote.
- Process detection is platform specific: `lsof` on macOS, `/proc` on Linux, nothing on Windows.
- The generated shell functions themselves run inside a real interactive shell; the tests exercise the directive protocol beneath them.

## Open limitations

- Process groups are resource cleanup, not a sandbox. Unix descendants that deliberately leave the group can survive cancellation; a captured process without a timeout can wait indefinitely for inherited pipes.
- Linux and Windows behavior must pass the CI matrix. Cross-compilation checks Windows types, but local macOS tests do not execute Windows job objects or verify every supported shell.

Start with a disposable repository or a branch whose work is already committed. Acre is deliberately conservative, but filesystem and worktree lifecycle software should not be trialled first against irreplaceable uncommitted work.

## Dependencies

CI audits the committed lockfile against RustSec on pull requests, main pushes, and weekly. Run `cargo audit --deny warnings` locally when changing dependencies. CI actions use full commit SHAs; Dependabot proposes weekly updates with a seven-day cooldown. Review lockfile changes, new maintainers, build scripts, and transitive additions before merging. Advisory checks identify known issues, not undisclosed compromises.
