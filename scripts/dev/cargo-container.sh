#!/usr/bin/env bash
# Sourced by canonical Rust runners. Creation finishes before cancellation
# cleanup; only its immutable CID is removed, and removal is verified.
cargo_container() {
  local cargo_state cargo_pid="" cargo_status cargo_phase=create
  local cargo_old_int cargo_old_term cargo_old_hup
  cargo_state=$(mktemp -d "${TMPDIR:-/tmp}/saltation-cargo.XXXXXX") || return
  cargo_old_int=$(trap -p INT)
  cargo_old_term=$(trap -p TERM)
  cargo_old_hup=$(trap -p HUP)
  _cargo_restore() {
    trap - INT TERM HUP
    [ -z "$cargo_old_int" ] || eval "$cargo_old_int"
    [ -z "$cargo_old_term" ] || eval "$cargo_old_term"
    [ -z "$cargo_old_hup" ] || eval "$cargo_old_hup"
    return 0
  }
  _cargo_cleanup() {
    local cid="" remaining warned=no attempts=0
    [ ! -s "$cargo_state/cid" ] || cid=$(cat "$cargo_state/cid")
    if [[ "$cid" =~ ^[0-9a-f]{64}$ ]]; then
      # A failed removal or unavailable daemon cannot establish teardown.
      # Retry up to 60s before surfacing a bounded diagnostic rather than hanging forever.
      while [ "$attempts" -lt 300 ]; do
        docker rm -f "$cid" >/dev/null 2>&1 || true
        if remaining=$(docker ps -aq --no-trunc --filter "id=$cid") && [ -z "$remaining" ]; then
          break
        fi
        if [ "$warned" = no ]; then
          echo 'cargo-container: waiting to confirm invocation container removal' >&2
          warned=yes
        fi
        attempts=$((attempts + 1))
        sleep .2
      done
      if [ "$attempts" -ge 300 ]; then
        echo "cargo-container: timed out after 60s removing container $cid" >&2
      fi
    fi
    rm -f "$cargo_state/cid" "$cargo_state/create-output"
    rmdir "$cargo_state" || true
  }
  _cargo_interrupt() {
    local status="$1"
    trap '' INT TERM HUP
    if [ "$cargo_phase" = create ] && [ -n "$cargo_pid" ]; then
      # Docker may still create a container after its client is interrupted.
      # Let this create request finish and publish its CID before cleanup.
      wait "$cargo_pid" 2>/dev/null || true
      cargo_pid=""
    fi
    _cargo_cleanup
    if [ -n "$cargo_pid" ]; then
      wait "$cargo_pid" 2>/dev/null || true
    fi
    _cargo_restore
    exit "$status"
  }
  trap '_cargo_interrupt 130' INT
  trap '_cargo_interrupt 143' TERM
  trap '_cargo_interrupt 129' HUP
  # Shield creation from group cancellation too. The small launcher inherits
  # ignored signals and puts the Docker client in a separate process session.
  (
    trap '' INT TERM HUP
    exec python3 -c 'import subprocess, sys; sys.exit(subprocess.call(sys.argv[1:], start_new_session=True))' \
      docker create --cidfile "$cargo_state/cid" "$@"
  ) >"$cargo_state/create-output" &
  cargo_pid=$!
  if wait "$cargo_pid"; then cargo_status=0; else cargo_status=$?; fi
  cargo_pid=""
  if [ "$cargo_status" != 0 ]; then
    _cargo_cleanup
    _cargo_restore
    return "$cargo_status"
  fi
  local cargo_cid
  cargo_cid=$(cat "$cargo_state/cid")
  cargo_phase=run
  if [ -n "${CARGO_TEST_TIMEOUT:-}" ]; then
    timeout --kill-after=5s "$CARGO_TEST_TIMEOUT" docker start --attach "$cargo_cid" &
  else
    docker start --attach "$cargo_cid" &
  fi
  cargo_pid=$!
  if wait "$cargo_pid"; then cargo_status=0; else cargo_status=$?; fi
  cargo_pid=""
  _cargo_cleanup
  _cargo_restore
  return "$cargo_status"
}

cargo_cache_capacity() {
  local image="$1" scripts="$2" target=saltation-cargo-target-guarded-v1 budget
  if docker volume inspect "$target" >/dev/null 2>&1 \
    && docker run --rm -v "$target":/target:ro "$image" test -f /target/.saltation-capacity-v1.json; then
    return 0
  fi
  local legacy=()
  if docker volume inspect saltation-cargo-target >/dev/null 2>&1; then
    legacy=(-v saltation-cargo-target:/legacy:ro)
  fi
  budget=$(docker run --rm ${legacy[@]+"${legacy[@]}"} \
    -v saltation-cargo-registry:/registry:ro -v "$scripts":/repo-scripts:ro \
    "$image" python3 /repo-scripts/rust-cache-capacity.py) || return
  docker volume create "$target" >/dev/null || return
  printf '%s\n' "$budget" | docker run --rm -i -v "$target":/target \
    -v "$scripts":/repo-scripts:ro "$image" python3 /repo-scripts/rust-cache-capacity.py record
}
