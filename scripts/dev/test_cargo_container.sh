#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
. "$root/scripts/dev/cargo-container.sh"
name="saltation-cargo-exit-$RANDOM-$$"
if cargo_container --rm --name "$name" saltation/bank-dev:local sh -c 'exit 23'; then status=0; else status=$?; fi
[ "$status" = 23 ]
[ -z "$(docker ps -aq --filter "name=^/${name}$")" ]
printf '%s\n' 'PASS Cargo exit status and container removal'
name="saltation-cargo-timeout-$RANDOM-$$"
started=$SECONDS
if CARGO_TEST_TIMEOUT=1 cargo_container --rm --name "$name" saltation/bank-dev:local sh -c 'trap "" TERM; sleep 30'; then status=0; else status=$?; fi
[ "$status" = 124 ] || [ "$status" = 137 ]
[ "$((SECONDS-started))" -lt 15 ]
[ -z "$(docker ps -aq --filter "name=^/${name}$")" ]
printf '%s\n' 'PASS bounded timeout and container removal'
name="saltation-cargo-signal-$RANDOM-$$"
cargo_container --rm --name "$name" saltation/bank-dev:local sh -c 'trap "" TERM; sleep 30' &
caller=$!
for attempt in {1..100}; do
  [ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || true)" != true ] || break
  sleep .05
done
[ "$(docker inspect -f '{{.State.Running}}' "$name")" = true ]
started=$SECONDS
kill -TERM "$caller"
if wait "$caller"; then status=0; else status=$?; fi
[ "$status" = 143 ]
[ "$((SECONDS-started))" -lt 15 ]
[ -z "$(docker ps -aq --filter "name=^/${name}$")" ]
printf '%s\n' 'PASS caller cancellation stops the invocation container'

# A delayed create must complete before cancellation resolves its immutable CID.
shim=$(mktemp -d "${TMPDIR:-/tmp}/saltation-docker-fixture.XXXXXX")
real_docker=$(command -v docker)
export CACHE_TEST_REAL_DOCKER="$real_docker" CACHE_TEST_SHIM="$shim"
trap 'rm -rf "$shim"' EXIT
cat >"$shim/docker" <<'SH'
#!/usr/bin/env bash
set -eu
if [ "$1" = create ] && [ "${CACHE_TEST_DELAY_CREATE:-}" = 1 ]; then
  touch "$CACHE_TEST_SHIM/creating"
  sleep 1
fi
if [ "$1" = rm ] && [ "${CACHE_TEST_FAIL_REMOVE:-}" = 1 ] && [ ! -f "$CACHE_TEST_SHIM/remove-failed" ]; then
  touch "$CACHE_TEST_SHIM/remove-failed"
  exit 1
fi
exec "$CACHE_TEST_REAL_DOCKER" "$@"
SH
chmod +x "$shim/docker"
name="saltation-cargo-startup-$RANDOM-$$"
PATH="$shim:$PATH" CACHE_TEST_DELAY_CREATE=1 cargo_container --rm --name "$name" saltation/bank-dev:local sh -c 'sleep 30' &
caller=$!
for attempt in {1..100}; do
  [ ! -f "$shim/creating" ] || break
  sleep .02
done
[ -f "$shim/creating" ]
kill -TERM "$caller"
if wait "$caller"; then status=0; else status=$?; fi
[ "$status" = 143 ]
[ -z "$(docker ps -aq --filter "name=^/${name}$")" ]
printf '%s\n' 'PASS cancellation waits for startup before removing its container'
name="saltation-cargo-remove-retry-$RANDOM-$$"
PATH="$shim:$PATH" CACHE_TEST_FAIL_REMOVE=1 cargo_container --name "$name" saltation/bank-dev:local sh -c 'exit 23' && status=0 || status=$?
[ "$status" = 23 ]
[ -f "$shim/remove-failed" ]
[ -z "$(docker ps -aq --filter "name=^/${name}$")" ]
printf '%s\n' 'PASS removal failure retries before returning'

rm -f "$shim/creating"
PATH="$shim:$PATH" CACHE_TEST_DELAY_CREATE=1 python3 - "$root/scripts/dev/cargo-container.sh" "$shim/creating" <<'PYTEST'
import os, signal, subprocess, sys, time, uuid
name = 'saltation-cargo-group-' + uuid.uuid4().hex
command = '. "$1"; cargo_container --rm --name "$2" saltation/bank-dev:local sh -c "sleep 30"'
p = subprocess.Popen(['bash', '-c', command, '_', sys.argv[1], name], start_new_session=True)
try:
    deadline = time.monotonic() + 10
    while not os.path.exists(sys.argv[2]) and p.poll() is None and time.monotonic() < deadline:
        time.sleep(.02)
    assert os.path.exists(sys.argv[2])
    os.killpg(p.pid, signal.SIGTERM)
    assert p.wait(timeout=15) == 143
    remaining = subprocess.check_output(['docker', 'ps', '-aq', '--filter', 'name=^/' + name + '$'], text=True)
    assert not remaining.strip()
    print('PASS process-group cancellation during startup removes its container')
finally:
    if p.poll() is None:
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
PYTEST
