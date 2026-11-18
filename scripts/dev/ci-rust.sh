#!/usr/bin/env bash
# Runs a Rust gate for bank/ inside the bank dev image (no host toolchain).
# Usage: scripts/dev/ci-rust.sh fmt|fmt-write|clippy|test|lock
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
image="saltation/bank-dev:local"
. "$root/scripts/dev/cargo-container.sh"
docker build -q --target dev -t "$image" "$root/bank" >/dev/null
cargo_cache_capacity "$image" "$root/scripts/dev"
run() {
  cargo_container --rm -v "$root/bank":/src -w /src \
    -v "$root/contracts":/contracts:ro -e CONTRACTS_DIR=/contracts \
    -v "$root/scripts/dev":/repo-scripts:ro \
    -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target-guarded-v1:/target \
    "$image" "$@"
}
case "${1:-}" in
  fmt)    run cargo fmt --check ;;
  fmt-write) run cargo fmt ;;
  clippy) run python3 /repo-scripts/rust-cache.py --bank /src --contracts /contracts --profile debug -- cargo clippy --locked --all-targets -- -D warnings ;;
  test)
    run python3 /repo-scripts/test_rust_cache.py
    python3 "$root/scripts/dev/test_rust_cache_image.py"
    bash "$root/scripts/dev/test_cargo_container.sh"
    run python3 /repo-scripts/rust-cache.py --bank /src --contracts /contracts --profile debug -- cargo test --locked --lib --bins
    ;;
  lock)   run cargo generate-lockfile ;;
  *) echo "usage: $0 fmt|fmt-write|clippy|test|lock" >&2; exit 2 ;;
esac
