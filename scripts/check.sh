#!/usr/bin/env bash
set -euo pipefail

# Honor rust-toolchain.toml even when a standalone Cargo precedes rustup on PATH. Resolved on its
# own line so a missing rustup stops the script instead of silently using an unpinned Cargo.
cargo_path="$(rustup which cargo)"
export PATH="$(dirname "$cargo_path"):$PATH"

cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
