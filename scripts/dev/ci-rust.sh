#!/usr/bin/env bash
# Runs a Rust gate for bank/ inside the bank dev image (no host toolchain).
# Usage: scripts/dev/ci-rust.sh fmt|fmt-write|clippy|test|lock
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
image="saltation/bank-dev:local"
docker build -q --target dev -t "$image" "$root/bank" >/dev/null
run() {
  docker run --rm -v "$root/bank":/src -w /src \
    -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target:/target \
    "$image" "$@"
}
case "${1:-}" in
  fmt)    run cargo fmt --check ;;
  fmt-write) run cargo fmt ;;
  clippy) run cargo clippy --locked --all-targets -- -D warnings ;;
  test)   run cargo test --locked --lib --bins ;;
  lock)   run cargo generate-lockfile ;;
  *) echo "usage: $0 fmt|fmt-write|clippy|test|lock" >&2; exit 2 ;;
esac
