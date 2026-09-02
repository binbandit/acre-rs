# Validation

`./scripts/check.sh` is the bar for every change, and CI runs it on Linux, macOS, and Windows:

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

## What the tests cover

- Unit tests sit next to the code: ref, status, and worktree parsing; markup; fingerprint normalisation; cache-root classification against a real Git repository.
- `tests/recovery.rs` drives the built binary through throwaway repositories: opening and returning workspaces, nested cache roots, recovery after lost state, repair accounting, and CLI flag ordering.

## What is exercised by hand

- Pull request resolution needs `gh` and a GitHub remote.
- Process detection is platform specific: `lsof` on macOS, `/proc` on Linux, nothing on Windows.
- Shell integration runs inside a real interactive shell.

Start with a disposable repository or a branch whose work is already committed. Acre is deliberately conservative, but filesystem and worktree lifecycle software should not be trialled first against irreplaceable uncommitted work.
