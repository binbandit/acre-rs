#!/usr/bin/env bash
set -euo pipefail

# Honor rust-toolchain.toml even when a standalone Cargo precedes rustup on PATH.
export PATH="$(dirname "$(rustup which cargo)"):$PATH"

cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
