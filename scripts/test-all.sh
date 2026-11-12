#!/bin/sh
# Full local gate: fmt, clippy, unit tests, smoke. No root needed.
# Usage: ./scripts/test-all.sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-features
./scripts/smoke.sh
echo "ALL GREEN"
