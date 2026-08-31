# Validation status

## What was verified in the creation environment

- `Cargo.toml` parses as TOML.
- Every `mod` declaration resolves to a source file.
- All Rust source files pass a lexical delimiter and unterminated-string/comment scan.
- The source tree contains no `todo!`, `unimplemented!`, or placeholder implementation markers.
- The archive contains the complete source tree, documentation, schemas, checks, and CI definitions.
- ZIP and tar.gz archives were opened after creation and their manifests compared.
- SHA-256 checksums were generated from the final bytes.

## What was not verified in the creation environment

The environment did not contain `rustc`, `cargo`, or `rustfmt`, and outbound package access was unavailable. Consequently, the source was not compiled or executed here.

Do not treat the archive as compiler-verified until this succeeds on your machine:

```bash
./scripts/check.sh
```

That script runs:

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Start with a disposable repository or a branch whose work is already committed. Acre is deliberately conservative, but filesystem and worktree lifecycle software should not be trialled first against irreplaceable uncommitted work.
