#!/usr/bin/env bash
# Runs a generator gate inside a Python container with uv (no host toolchain).
# Usage: scripts/dev/ci-py.sh lock|lint|test
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
image="ghcr.io/astral-sh/uv:0.8.17-python3.13-bookworm-slim"
run() {
  docker run --rm -v "$root/generator":/app -w /app \
    -v saltation-uv-cache:/root/.cache/uv -e UV_PROJECT_ENVIRONMENT=/tmp/venv -e UV_LINK_MODE=copy \
    "$image" sh -c "$*"
}
case "${1:-}" in
  lock) run "uv lock" ;;
  lint) run "uv sync --frozen --quiet && uv run --frozen ruff check . && uv run --frozen ruff format --check . && uv run --frozen lint-imports" ;;
  test) run "uv sync --frozen --quiet && uv run --frozen pytest -q" ;;
  *) echo "usage: $0 lock|lint|test" >&2; exit 2 ;;
esac
