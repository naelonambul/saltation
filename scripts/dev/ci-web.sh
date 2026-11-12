#!/usr/bin/env bash
# Runs a web gate inside a Node container (no host toolchain).
# Usage: scripts/dev/ci-web.sh install|lint|typecheck|test|build|sh <args>
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
image="node:22.20.0-bookworm-slim"
run() {
  docker run --rm -v "$root":/repo -w /repo/web \
    -v saltation-pnpm-store:/pnpm-store -e npm_config_store_dir=/pnpm-store -e CI=true \
    "$image" sh -c "corepack enable >/dev/null 2>&1 && $*"
}
case "${1:-}" in
  install)   run "pnpm install --frozen-lockfile" ;;
  lint)      run "pnpm install --frozen-lockfile >/dev/null && pnpm lint" ;;
  typecheck) run "pnpm install --frozen-lockfile >/dev/null && pnpm typecheck" ;;
  test)      run "pnpm install --frozen-lockfile >/dev/null && pnpm test" ;;
  build)     run "pnpm install --frozen-lockfile >/dev/null && pnpm build" ;;
  sh)        shift; run "$*" ;;
  *) echo "usage: $0 install|lint|typecheck|test|build|sh <cmd>" >&2; exit 2 ;;
esac
