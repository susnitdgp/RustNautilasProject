#!/usr/bin/env bash
set -euo pipefail
cd /home/ubuntu/RustNautilasProject
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p kite-node
./target/debug/kite-node native-smbc-sim config/production-smbc.json
