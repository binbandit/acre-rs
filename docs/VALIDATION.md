# Validation

`./scripts/check.sh` is the bar for every change. CI runs the same checks on the pinned toolchain from `rust-toolchain.toml` across Linux, macOS, and Windows, and again on current stable on Linux:

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## What the tests cover

- Unit tests sit next to the code: ref, status, and worktree parsing; markup; fingerprint normalisation; cache-root classification against a real Git repository.
- `tests/end_to_end.rs` drives the built binary through throwaway repositories: opening and returning workspaces, the shell directive round trip including `acre -`, the two-phase `done` that resumes after the shell moves, the machine acquire and release cycle, warm-pool reuse across branches, nested cache roots, recovery after lost state, repair accounting, and CLI flag ordering.

## What is exercised by hand

- Pull request resolution needs `gh` and a GitHub remote.
- Process detection is platform specific: `lsof` on macOS, `/proc` on Linux, nothing on Windows.
- The generated shell functions themselves run inside a real interactive shell; the tests exercise the directive protocol beneath them.

Start with a disposable repository or a branch whose work is already committed. Acre is deliberately conservative, but filesystem and worktree lifecycle software should not be trialled first against irreplaceable uncommitted work.
