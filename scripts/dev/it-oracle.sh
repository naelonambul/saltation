#!/usr/bin/env bash
# Oracle integration tests. Starts Oracle and MongoDB in a separate Compose
# project, resets the test schema, and runs the named test target in the bank
# dev image on the project's data network.
# Usage: scripts/dev/it-oracle.sh <target>|all [--fresh]
#   targets: tune-seed, tune-capture, driver-bench, driver-parity, db-pool, mongo-users, migrations, backfill, listeners, auth, accounts, transfers, payees, inbound, password_reset, deposits, loans, outbox, outbox-outage, observability, e2e, browser-dates, generator-bc, generator-cards, generator-behavior
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
. "$root/scripts/dev/cargo-container.sh"
project="${SALTATION_IT_PROJECT:-saltation-it}"
case "$project" in
  *[!a-z0-9_-]*) echo "invalid isolated integration project: $project" >&2; exit 2 ;;
  saltation-it|saltation-it-[a-z0-9]*) ;;
  *) echo "invalid isolated integration project: $project" >&2; exit 2 ;;
esac
cargo_name="${project}-cargo"

# State lives outside the checkout (CI checkouts delete untracked files), so the
# test database is created once and reused across runs (plan oracle-ci-speed).
project_state="$HOME/.cache/$project"
state_dir="${SALTATION_IT_DIR:-$project_state}"
mkdir -p "$project_state" "$state_dir" && chmod 700 "$project_state" "$state_dir"
env_file="$state_dir/env"
compose=(docker compose -p "$project" -f "$root/infra/compose/compose.yml" --env-file "$env_file" --profile ci)

# The parent keeps fd 9 open for the entire run. The kernel releases its lock on exit.
exec 9>"$project_state/lock"
python3 -c 'import fcntl; fcntl.flock(9, fcntl.LOCK_EX)'

# Anything that only runs when the volumes are created is part of the key; a
# different key recreates the test volumes so setup changes are always proven.
image_id=$(docker image inspect -f '{{.Id}}' saltation/oracle-ee:19.19 2>/dev/null || echo missing)
setup_key=$(python3 - "$root" "$image_id" <<'PY'
import hashlib, pathlib, sys
root = pathlib.Path(sys.argv[1])
h = hashlib.sha256(sys.argv[2].encode())
for p in sorted([*root.glob("infra/oracle/setup/*"), *root.glob("infra/mongo/init/*"), root / "infra/oracle/build.sh"]):
    h.update(p.name.encode()); h.update(p.read_bytes())
print(h.hexdigest())
PY
)

recreate=no
if [ ! -f "$env_file" ]; then
  recreate=yes
  umask 077
  gen() { python3 -c 'import secrets, string; a = string.ascii_letters + string.digits; print("".join(secrets.choice(a) for _ in range(20)))'; }
  cat >"$env_file" <<ENV
ORACLE_PWD=It$(gen)1
SALTATION_DB_PASSWORD=It$(gen)1
MONGO_ROOT_PASSWORD=$(gen)
MONGO_BANK_PASSWORD=$(gen)
MONGO_GENERATOR_PASSWORD=$(gen)
GRAFANA_ADMIN_PASSWORD=$(gen)
SALTATION_API_BASE=http://bank:8080
SALTATION_CORS_ORIGINS=http://web:8088
ORACLE_PUBLISH=127.0.0.1:
MONGO_PUBLISH=127.0.0.1:
BANK_PUBLISH=127.0.0.1:
WEB_PUBLISH=127.0.0.1:
GRAFANA_PUBLISH=127.0.0.1:
SETUP_KEY=
ENV
fi
set -a; . "$env_file"; set +a

# set_env NAME VALUE: replaces one line of the env file.
set_env() {
  python3 - "$env_file" "$1" "$2" <<'PY'
import pathlib, sys
p, name, value = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
lines = [l for l in p.read_text().splitlines() if not l.startswith(name + "=")]
p.write_text("\n".join(lines + [f"{name}={value}"]) + "\n")
PY
}

# Identity of the database volumes. Volumes created by anything else (another
# version of this script, a manual `down -v`) hold other passwords, so a
# mismatch also recreates them.
volumes_key() {
  docker volume inspect -f '{{.Name}} {{.CreatedAt}}' "${project}_oradata" "${project}_mongodata" 2>/dev/null | cksum | cut -d' ' -f1
}

target="${1:-all}"
if [ "${2:-}" = "--fresh" ] || [ "$recreate" = yes ] || [ "${SETUP_KEY:-}" != "$setup_key" ] \
  || [ "${VOLUMES_KEY:-}" != "$(volumes_key)" ]; then
  echo "it-oracle: creating fresh test volumes"
  "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  set_env SETUP_KEY "$setup_key"
else
  echo "it-oracle: reusing the test database"
fi

# Leftovers from a cancelled run: the cargo container, a stopped MongoDB
# (outbox-outage), and application containers whose relay would drain the
# outbox under the database tests. Only observability and e2e run the bank.
docker rm -f "$cargo_name" >/dev/null 2>&1 || true
case "$target" in
  observability|e2e) ;;
  *) "${compose[@]}" stop bank web alloy loki prometheus grafana >/dev/null 2>&1 || true ;;
esac
"${compose[@]}" up -d --wait --wait-timeout 3600 oracle mongo
[ "${VOLUMES_KEY:-}" = "$(volumes_key)" ] || set_env VOLUMES_KEY "$(volumes_key)"

reset_schema() {
  { echo "SET VERIFY OFF"; echo "DEFINE pw = \"$SALTATION_DB_PASSWORD\""; cat "$root/infra/oracle/reset_schema.sql"; } \
    | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB oracle sqlplus -s / as sysdba >/dev/null
  "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB oracle sqlplus -s / as sysdba >/dev/null <<'SQL'
WHENEVER SQLERROR EXIT SQL.SQLCODE
ALTER SESSION SET CONTAINER = ORCLPDB1;
GRANT SELECT ON SYS.V_$SQL TO saltation;
GRANT SELECT ON SYS.V_$SQL_PLAN TO saltation;
GRANT SELECT ON SYS.V_$SQL_PLAN_STATISTICS_ALL TO saltation;
GRANT SELECT ON SYS.V_$SESSION TO saltation;
EXIT
SQL
  "${compose[@]}" exec -T mongo mongosh --quiet -u bank -p "$MONGO_BANK_PASSWORD" --authenticationDatabase bank_logs \
    --eval 'db.getSiblingDB("bank_logs").events.deleteMany({})' >/dev/null
}

dev_image_built=no
dev_image() {
  if [ "$dev_image_built" = no ]; then
    docker build -q --target dev -t saltation/bank-dev:local "$root/bank" >/dev/null || return
    cargo_cache_capacity saltation/bank-dev:local "$root/scripts/dev" || return
    dev_image_built=yes
  fi
}

# cargo_test <test>... [-- <test args>]: runs the named integration test targets
# in one cargo invocation (single-threaded) in a container named for cleanup.
cargo_test() {
  local tests=() args=()
  while [ $# -gt 0 ] && [ "$1" != -- ]; do tests+=(--test "$1"); shift; done
  [ "${1:-}" = -- ] && { shift; args=("$@"); }
  local extra_env=()
  [ -n "${OUTBOX_OUTAGE_TEST:-}" ] && extra_env=(-e OUTBOX_OUTAGE_TEST)
  [ -n "${E2E_SINCE:-}" ] && extra_env=(-e E2E_SINCE -e MONGO_TRUTH_URI -e E2E_ATM -v "${project}_applogs:/var/log/saltation:ro")
  if [ -n "${GENERATOR_BC_FIXTURE:-}" ]; then extra_env+=(-e GENERATOR_BC_FIXTURE -e MONGO_TRUTH_URI); fi
  if [ -n "${GENERATOR_CARDS_FIXTURE:-}" ]; then extra_env+=(-e GENERATOR_CARDS_FIXTURE -e MONGO_TRUTH_URI); fi
  if [ -n "${GENERATOR_BEHAVIOR_FIXTURE:-}" ]; then extra_env+=(-e GENERATOR_BEHAVIOR_FIXTURE -e MONGO_TRUTH_URI); fi
  [ -z "${CARGO_BUILD_JOBS:-}" ] || extra_env+=(-e "CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS")
  [ -z "${ORA_SDTZ:-}" ] || extra_env+=(-e ORA_SDTZ)
  [ -z "${TUNE_SEED_AT:-}" ] || extra_env+=(-e TUNE_SEED_AT)
  [ "${SALTATION_TEST_EXPECT_LIVE:-}" = 1 ] && extra_env+=(-e SALTATION_TEST_EXPECT_LIVE=1)
  if [ -n "${TUNE_COVERAGE_OUT:-}" ]; then
    extra_env+=(-e "TUNE_COVERAGE_OUT=/coverage/$(basename "$TUNE_COVERAGE_OUT")" -v "$(dirname "$TUNE_COVERAGE_OUT"):/coverage")
  fi
  dev_image || return
  cargo_container --rm --name "$cargo_name" --network "${project}_data" \
    -e SALTATION_DB_USER="${IT_DB_USER:-saltation}" -e SALTATION_DB_PASSWORD -e SALTATION_DB_CONNECT=oracle:1521/ORCLPDB1 -e SALTATION_DB_ACQUIRE_MS=10000 -e SALTATION_CORS_ORIGINS=http://localhost:8088 \
    -e MONGO_URI="mongodb://bank:${MONGO_BANK_PASSWORD}@mongo:27017/bank_logs" ${extra_env[@]+"${extra_env[@]}"} \
    -v "$root/bank":/src -w /src -v "$root/contracts":/contracts:ro -e CONTRACTS_DIR=/contracts -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target-guarded-v1:/target -v "$root/scripts/dev":/repo-scripts:ro \
    saltation/bank-dev:local python3 /repo-scripts/rust-cache.py --bank /src --contracts /contracts --profile debug -- cargo test --locked "${tests[@]}" -- --test-threads=1 ${args[@]+"${args[@]}"}
}

# Requests and acknowledgements delimit only the measured benchmark window.
bench_snapshot_watch() {
  local directory="$1" request stem
  while :; do
    for request in "$directory"/snap-*-run*-start.req "$directory"/snap-*-run*-end.req; do
      [ -f "$request" ] || continue
      stem="${request%.req}"
      [ ! -f "$stem.ack" ] || continue
      if "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB oracle sqlplus -s / as sysdba >"$stem.txt" <<SQL
WHENEVER SQLERROR EXIT FAILURE
WHENEVER OSERROR EXIT FAILURE
SET VERIFY OFF
DEFINE bench_user = '${BENCH_SNAPSHOT_USER:-SALTATION}'
SET PAGES 0 LINES 1000 FEEDBACK OFF HEADING OFF TRIMSPOOL ON
ALTER SESSION SET CONTAINER=ORCLPDB1;
SELECT 'startup '||TO_CHAR(startup_time,'YYYYMMDDHH24MISS') FROM v\$instance;
SELECT 'sys '||REPLACE(name,' ','_')||' '||value FROM v\$sysstat WHERE name IN ('user commits', 'user rollbacks', 'redo size', 'redo writes', 'redo synch writes', 'redo synch time', 'SQL*Net roundtrips to/from client', 'parse count (total)', 'parse count (hard)', 'execute count', 'logons cumulative', 'user calls');
SELECT 'sev '||s.sid||' '||s.serial#||' '||REPLACE(e.event,' ','_')||' '||e.total_waits||' '||e.time_waited_micro FROM v\$session s JOIN v\$session_event e ON e.sid=s.sid WHERE s.username='&bench_user';
SELECT 'sst '||s.sid||' '||s.serial#||' '||REPLACE(n.name,' ','_')||' '||t.value FROM v\$session s JOIN v\$sesstat t ON t.sid=s.sid JOIN v\$statname n ON n.statistic#=t.statistic# WHERE s.username='&bench_user' AND n.name IN ('user commits', 'user rollbacks', 'redo size', 'redo writes', 'redo synch writes', 'redo synch time', 'SQL*Net roundtrips to/from client', 'parse count (total)', 'parse count (hard)', 'execute count', 'logons cumulative', 'user calls');
SELECT 'hist '||REPLACE(event,' ','_')||' '||wait_time_milli||' '||wait_count FROM v\$event_histogram WHERE event IN ('log file sync','log file parallel write');
SELECT 'sysev '||REPLACE(event,' ','_')||' '||total_waits||' '||time_waited_micro FROM v\$system_event WHERE event IN ('log file sync','log file parallel write');
SELECT 'sql|'||sql_id||'|'||child_number||'|'||executions||'|'||buffer_gets||'|'||disk_reads||'|'||rows_processed||'|'||cpu_time||'|'||elapsed_time||'|'||RAWTOHEX(child_address)||':'||last_load_time||':'||invalidations||'|'||REPLACE(REPLACE(REPLACE(SUBSTR(sql_text,1,60),CHR(10),' '),CHR(13),' '),'|',' ') FROM v\$sql WHERE parsing_schema_name='&bench_user';
EXIT
SQL
      then
        if [ "${BENCH_CAPTURE:-}" = 1 ]; then
          tune_window "$stem" || continue
        fi
        touch "$stem.ack"
      fi
    done
    sleep 0.1
  done
}

# Each workload/run has its own resource-limited process. Only empty-schema runs reset.
driver_bench() {
  : "${BENCH_DRIVER:?BENCH_DRIVER is required}"
  local runs="${BENCH_RUNS:-3}" entries entry workload concurrency run failed=0 output
  case "$BENCH_DRIVER" in *[!a-zA-Z0-9._-]*|'') echo "invalid BENCH_DRIVER" >&2; return 2 ;; esac
  case "$runs" in *[!0-9]*|''|0) echo "BENCH_RUNS must be positive" >&2; return 2 ;; esac
  local oracle_container oracle_limits oracle_cpus oracle_memory
  oracle_container=$("${compose[@]}" ps -q oracle)
  oracle_limits=$(docker inspect --format '{{.HostConfig.NanoCpus}} {{.HostConfig.Memory}}' "$oracle_container")
  read -r oracle_cpus oracle_memory <<<"$(python3 - "$oracle_limits" <<'LIMITS'
from decimal import Decimal
import sys
cpu, memory = map(int, sys.argv[1].split())
print(format(Decimal(cpu) / Decimal(1000000000), 'f') if cpu else 'unlimited',
      str(memory) if memory else 'unlimited')
LIMITS
)"
  local bench_env=() watcher="" bench_user=saltation
  if [ "${BENCH_POPULATED:-}" = 1 ]; then
    bench_user=saltation_tune
    bench_env+=(-e BENCH_POPULATED=1 -e "TUNE_OUTBOX_LONG=${TUNE_OUTBOX_LONG:-500000}")
  fi
  if [ "${BENCH_RATE+x}" = x ]; then
    case "$BENCH_RATE" in *[!0-9]*|'') echo "BENCH_RATE must be a positive integer" >&2; return 2 ;; esac
    [[ "$BENCH_RATE" =~ [1-9] ]] || { echo "BENCH_RATE must be a positive integer" >&2; return 2; }
    bench_env+=(-e "BENCH_RATE=$BENCH_RATE")
  fi
  if [ "${BENCH_ORA_SNAP:-}" = 1 ]; then
    bench_env+=(-e BENCH_ORA_SNAP=1)
  fi
  [ -z "${BENCH_SECS:-}" ] || bench_env+=(-e "BENCH_SECS=$BENCH_SECS")
  [ -z "${BENCH_WARMUP_SECS:-}" ] || bench_env+=(-e "BENCH_WARMUP_SECS=$BENCH_WARMUP_SECS")
  entries=(lookup:8 balance:8 history:8 transfer:1 transfer:8 transfer:32 transfer:64 duplicate:200 outbox:1)
  if [ "${BENCH_POPULATED:-}" = 1 ]; then
    entries+=(transfer_hot:32 transfer_hot:64 history_recent_first_shallow:8 history_recent_first_deep:8 history_recent_deep_shallow:8 history_recent_deep_deep:8 history_old_first_shallow:8 history_old_first_deep:8 history_old_deep_shallow:8 history_old_deep_deep:8 history_filtered_deep:8 outbox_long:1)
  fi
  if [ -n "${BENCH_ONLY:-}" ]; then
    local found=no
    for entry in "${entries[@]}"; do [ "$entry" != "$BENCH_ONLY" ] || found=yes; done
    [ "$found" = yes ] || { echo "invalid BENCH_ONLY" >&2; return 2; }
    entries=("$BENCH_ONLY")
  fi
  if [ "${BENCH_RATE+x}" = x ] && [ -z "${BENCH_ONLY:-}" ]; then
    entries=(lookup:8 balance:8 history:8 transfer:1 transfer:8 transfer:32 transfer:64)
  fi
  output="${BENCH_OUTPUT:-$root/.evidence/driver-bench/$(date -u +%Y%m%dT%H%M%SZ)-$BENCH_DRIVER}"
  mkdir -p "$output"
  dev_image || return
  for entry in "${entries[@]}"; do
    workload="${entry%:*}"; concurrency="${entry#*:}"
    if [ "${BENCH_RATE+x}" = x ]; then
      case "$workload" in
        lookup|balance|history|transfer) ;;
        *) echo "BENCH_RATE only supports lookup, balance, history, transfer" >&2; return 2 ;;
      esac
    fi
    for ((run=1; run<=runs; run++)); do
      echo "driver-bench: $entry run $run"
      if [ "${BENCH_CAPTURE:-}" = 1 ]; then
        tune_reset_outbox "${entry/:/-}-run$run-timed" || { failed=1; break; }
      fi
      if [ "${BENCH_POPULATED:-}" != 1 ]; then
        if ! reset_schema; then failed=1; continue; fi
      fi
      if [ "${BENCH_ORA_SNAP:-}" = 1 ]; then
        BENCH_SNAPSHOT_USER="$(printf %s "$bench_user" | tr "[:lower:]" "[:upper:]")" bench_snapshot_watch "$output" &
        watcher=$!
      fi
      if ! cargo_container --rm --name "$cargo_name" --network "${project}_data" --cpus 2 --memory 4g \
        -e SALTATION_DB_USER="$bench_user" -e SALTATION_DB_PASSWORD -e SALTATION_DB_CONNECT=oracle:1521/ORCLPDB1 -e SALTATION_DB_ACQUIRE_MS=200 -e SALTATION_CORS_ORIGINS=http://localhost:8088 \
        -e MONGO_URI="mongodb://bank:${MONGO_BANK_PASSWORD}@mongo:27017/bank_logs" \
        ${bench_env[@]+"${bench_env[@]}"} -e BENCH_DRIVER -e BENCH_WORKLOAD="$workload" -e BENCH_CONCURRENCY="$concurrency" -e BENCH_RUN="$run" \
        -e BENCH_ORACLE_CPUS="$oracle_cpus" -e BENCH_ORACLE_MEMORY="$oracle_memory" \
        -e BENCH_CPUS=2 -e BENCH_MEMORY=4g -e BENCH_OUT="/out/${BENCH_RESULT_NAME:-bench.jsonl}" \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -v "$output":/out \
        -v "$root/bank":/src -w /src -v "$root/contracts":/contracts:ro -e CONTRACTS_DIR=/contracts \
        -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target-guarded-v1:/target -v "$root/scripts/dev":/repo-scripts:ro \
        saltation/bank-dev:local sh -c 'python3 /repo-scripts/rust-cache.py --bank /src --contracts /contracts --profile release -- cargo test --release --locked --test driver_bench -- --ignored --exact driver_bench --nocapture; status=$?; chown -R "$HOST_UID:$HOST_GID" /out; exit "$status"'; then
        failed=1
      fi
      if [ -n "$watcher" ]; then
        kill "$watcher" 2>/dev/null || true
        wait "$watcher" 2>/dev/null || true
        watcher=""
      fi
    done
  done
  echo "driver-bench output: $output/${BENCH_RESULT_NAME:-bench.jsonl}"
  return "$failed"
}

mongo_users() {
  local denied
  "${compose[@]}" exec -T mongo mongosh --quiet -u bank -p "$MONGO_BANK_PASSWORD" --authenticationDatabase bank_logs \
    --eval 'db.getSiblingDB("bank_logs").probe.insertOne({ok:1}); print("bank: bank_logs ok")'
  "${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval 'db.getSiblingDB("truth").probe.insertOne({ok:1}); print("generator: truth ok")'
  denied=$("${compose[@]}" exec -T mongo mongosh --quiet -u bank -p "$MONGO_BANK_PASSWORD" --authenticationDatabase bank_logs \
    --eval 'try { db.getSiblingDB("truth").probe.findOne(); print("ALLOWED") } catch (e) { print("DENIED") }')
  [ "$denied" = DENIED ] || { echo "bank user can read truth" >&2; return 1; }
  denied=$("${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval 'try { db.getSiblingDB("bank_logs").probe.findOne(); print("ALLOWED") } catch (e) { print("DENIED") }')
  [ "$denied" = DENIED ] || { echo "generator user can read bank_logs" >&2; return 1; }
  echo "mongo-users: each user is denied on the other database"
}

# Stops MongoDB 10 s into the outage test for 120 s, then starts it again.
outbox_outage() {
  ( sleep 10; "${compose[@]}" stop mongo >/dev/null; echo "outage: mongo stopped";
    sleep 120; "${compose[@]}" start mongo >/dev/null; echo "outage: mongo started" ) &
  local chaos=$!
  OUTBOX_OUTAGE_TEST=1 cargo_test outbox -- outage_then_full_delivery --nocapture
  wait "$chaos"
}

# Runs the whole stack: a request to the bank must appear in Loki (via
# app.log and Alloy) and in Prometheus, and Grafana must serve the dashboard.
observability() {
  "${compose[@]}" up -d --build --force-recreate --wait --wait-timeout 900
  local net_edge="${project}_edge" net_obs="${project}_obs" rid
  rid=$(python3 -c "import secrets; a='0123456789ABCDEFGHJKMNPQRSTVWXYZ'; print('0' + ''.join(secrets.choice(a) for _ in range(25)))")
  docker run --rm --network "$net_edge" curlimages/curl:8.15.0 -fsS -H "X-Request-Id: $rid" http://bank:8080/healthz >/dev/null
  echo "observability: sent request $rid"
  local found="" i
  for i in $(seq 1 30); do
    found=$(docker run --rm --network "$net_obs" curlimages/curl:8.15.0 -fsS -G http://loki:3100/loki/api/v1/query_range \
      --data-urlencode "query={job=\"bank\"} |= \"$rid\"" --data-urlencode "limit=1" | grep -c "$rid" || true)
    [ "$found" -gt 0 ] && break
    sleep 2
  done
  [ "$found" -gt 0 ] || { echo "observability: request $rid not found in Loki" >&2; return 1; }
  echo "observability: Loki has the app.log line for $rid"
  local up
  for i in $(seq 1 30); do
    up=$(docker run --rm --network "$net_obs" curlimages/curl:8.15.0 -fsS -G http://prometheus:9090/api/v1/query \
      --data-urlencode 'query=sum(bank_http_request_duration_seconds_count) > 0 and on() up{job="bank"} == 1' | grep -c '"value"' || true)
    [ "$up" -gt 0 ] && break
    sleep 2
  done
  [ "$up" -gt 0 ] || { echo "observability: bank metrics not in Prometheus" >&2; return 1; }
  echo "observability: Prometheus scrapes the bank and has request latency"
  docker run --rm --network "$net_obs" curlimages/curl:8.15.0 -fsS -u "admin:${GRAFANA_ADMIN_PASSWORD}" \
    "http://grafana:3000/api/dashboards/uid/saltation-bank" | grep -q '"title":"Saltation bank"' \
    || { echo "observability: Grafana dashboard missing" >&2; return 1; }
  echo "observability: Grafana serves the Saltation bank dashboard"
}

generator_bc() (
  local_dir=$(mktemp -d)
  trap 'rm -f "$local_dir/fault.jsonl" "$local_dir/resume.jsonl" "$local_dir/verifier.log"; rmdir "$local_dir"' EXIT
  "${compose[@]}" stop bank web alloy loki prometheus grafana >/dev/null
  reset_schema
  "${compose[@]}" up -d --build --wait --wait-timeout 1800 bank
  "${compose[@]}" --profile tools build generator >/dev/null
  harness="$root/generator/tests/bc_harness.py"
  gen=("${compose[@]}" --profile tools run --rm -T -v "$harness:/proof.py:ro" --entrypoint python generator /proof.py)
  run_id=$("${compose[@]}" --profile tools run --rm -T --entrypoint python generator -c \
    'from saltation_gen.client.bank import new_ulid; print(new_ulid())')
  owner="$run_id-fault"
  status=0
  "${gen[@]}" fault "$run_id" "$owner" >"$local_dir/fault.jsonl" || status=$?
  cat "$local_dir/fault.jsonl"
  [ "$status" -eq 75 ] || { echo "generator-bc: expected fault exit 75, got $status" >&2; exit 1; }
  grep -Fq '"fault": "discarded_committed_transfer_response"' "$local_dir/fault.jsonl"
  status=0
  "${gen[@]}" resume "$run_id" "$owner" >"$local_dir/resume.jsonl" || status=$?
  cat "$local_dir/resume.jsonl"
  [ "$status" -eq 0 ] || exit "$status"
  fixture=$(python3 "$harness" combine "$local_dir/fault.jsonl" "$local_dir/resume.jsonl" "$owner")
  GENERATOR_BC_FIXTURE="$fixture" MONGO_TRUTH_URI="mongodb://generator:${MONGO_GENERATOR_PASSWORD}@mongo:27017/truth" \
    cargo_test generator_bc -- --ignored --exact generator_bc_recovery_proof 2>&1 | tee "$local_dir/verifier.log"
  grep -Fxq 'test generator_bc_recovery_proof ... ok' "$local_dir/verifier.log"
)

generator_cards() (
  verifier=$(mktemp)
  trap 'rm -f "$verifier"' EXIT
  "${compose[@]}" stop bank web alloy loki prometheus grafana >/dev/null
  reset_schema
  "${compose[@]}" up -d --build --wait --wait-timeout 1800 bank
  "${compose[@]}" --profile tools build generator >/dev/null
  fixture=$("${compose[@]}" --profile tools run --rm -T \
    -v "$root/generator/tests/cards_harness.py:/proof.py:ro" --entrypoint python generator /proof.py)
  printf '%s\n' "$fixture"
  GENERATOR_CARDS_FIXTURE="$fixture" MONGO_TRUTH_URI="mongodb://generator:${MONGO_GENERATOR_PASSWORD}@mongo:27017/truth" \
    cargo_test generator_cards -- --ignored --exact generator_cards_proof 2>&1 | tee "$verifier"
  grep -Fxq 'test generator_cards_proof ... ok' "$verifier"
)

generator_behavior() (
  verifier=$(mktemp)
  trap 'rm -f "$verifier"' EXIT
  "${compose[@]}" stop bank web alloy loki prometheus grafana >/dev/null
  reset_schema
  "${compose[@]}" up -d --build --wait --wait-timeout 1800 bank
  "${compose[@]}" --profile tools build generator >/dev/null
  fixture=$("${compose[@]}" --profile tools run --rm -T \
    -v "$root/generator/tests/behavior_harness.py:/proof.py:ro" --entrypoint python generator /proof.py)
  printf '%s\n' "$fixture"
  GENERATOR_BEHAVIOR_FIXTURE="$fixture" MONGO_TRUTH_URI="mongodb://generator:${MONGO_GENERATOR_PASSWORD}@mongo:27017/truth" \
    cargo_test generator_behavior -- --ignored --exact generator_behavior_proof 2>&1 | tee "$verifier"
  grep -Fxq 'test generator_behavior_proof ... ok' "$verifier"
)

# Plan step 10: the whole stack, 1,000 seeded customers, 10 minutes of normal
# transfers from the generator, the Rust end-to-end checks, Prometheus data,
# then a person's flow in the mobile web app with Playwright.
e2e() {
  local since start gen roster phone password to_account atm
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  start=$(( ($(date +%s) % 90000) * 1000 ))
  gen=("${compose[@]}" --profile tools run --rm generator)
  "${compose[@]}" up -d --build --wait --wait-timeout 1800
  "${compose[@]}" --profile tools build generator >/dev/null
  # A new database starts in backfill mode; the run and the browser flow need live time.
  echo "e2e: seeding counterparties and switching to live"
  "${gen[@]}" seed-parties --counterparties 50
  "${gen[@]}" seed-terminals
  "${gen[@]}" clock live
  # Other targets reset the Oracle schema; a roster kept from an earlier run
  # would name customers the bank no longer has.
  "${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval 'db.getSiblingDB("truth").roster.deleteMany({})' >/dev/null
  echo "e2e: seeding 1,000 customers from #$start"
  "${gen[@]}" seed --customers 1000 --start "$start"
  echo "e2e: 10 minutes of normal transfers at 5 per second"
  "${gen[@]}" run-normal --minutes "${E2E_MINUTES:-10}" --rate 5
  atm=$("${gen[@]}" atm-smoke)
  E2E_ATM="$atm" MONGO_TRUTH_URI="mongodb://generator:${MONGO_GENERATOR_PASSWORD}@mongo:27017/truth" E2E_SINCE="$since" \
    cargo_test e2e -- --nocapture
  local count
  count=$(docker run --rm --network "${project}_obs" curlimages/curl:8.15.0 -fsS -G http://prometheus:9090/api/v1/query \
    --data-urlencode 'query=sum(bank_http_request_duration_seconds_count{endpoint="/v2.0/transfer/withdraw/fin_num"})' \
    | python3 -c 'import json,sys; r=json.load(sys.stdin)["data"]["result"]; print(int(float(r[0]["value"][1])) if r else 0)')
  [ "$count" -ge 2000 ] || { echo "e2e: Prometheus counted only $count transfers" >&2; return 1; }
  echo "e2e: Prometheus counted $count transfers"
  roster=$("${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval "const r = db.getSiblingDB('truth').roster.find({customer_no: {\$gte: 'C' + String($start).padStart(8, '0')}}).sort({customer_no: 1}).limit(2).toArray(); print(r[0].phone + ' ' + r[0].password + ' ' + r[1].account_no)")
  read -r phone password to_account <<<"$roster"
  echo "e2e: browser flow as $phone"
  browser_flow "$phone" "$password" "$to_account" "${E2E_TIMEZONE:-}"
}


# browser_flow <phone> <password> <to_account> [<timezone>]
browser_flow() {
  local phone="$1" password="$2" to_account="$3" timezone="${4:-}"
  local browser_env=()
  [ -z "$timezone" ] || browser_env+=(-e "E2E_TIMEZONE=$timezone")
  docker run --rm --network "${project}_edge" -v "$root/web":/web -w /web/apps/mobile \
    -v saltation-pnpm-store:/pnpm-store -e npm_config_store_dir=/pnpm-store -e CI=true ${browser_env[@]+"${browser_env[@]}"} \
    -e E2E_BASE_URL=http://web:8088 -e E2E_PHONE="$phone" -e E2E_PASSWORD="$password" -e E2E_TO_ACCOUNT="$to_account" \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    mcr.microsoft.com/playwright:v1.55.0-noble sh -c 'corepack enable >/dev/null 2>&1 && cd /web && pnpm install --frozen-lockfile >/dev/null && cd apps/mobile && pnpm exec playwright test; status=$?; chown -R "$HOST_UID:$HOST_GID" /web; exit "$status"'
}

browser_dates() {
  local start gen roster phone password to_account zone
  local failed=()
  start=$(( ($(date +%s) % 90000) * 1000 ))
  gen=("${compose[@]}" --profile tools run --rm generator)
  "${compose[@]}" up -d --build --wait --wait-timeout 1800
  "${compose[@]}" --profile tools build generator >/dev/null
  "${gen[@]}" seed-parties --counterparties 50
  "${gen[@]}" clock live
  "${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval 'db.getSiblingDB("truth").roster.deleteMany({})' >/dev/null
  "${gen[@]}" seed --customers 2 --start "$start"
  roster=$("${compose[@]}" exec -T mongo mongosh --quiet -u generator -p "$MONGO_GENERATOR_PASSWORD" --authenticationDatabase truth \
    --eval "const r = db.getSiblingDB('truth').roster.find({customer_no: {\$gte: 'C' + String($start).padStart(8, '0')}}).sort({customer_no: 1}).limit(2).toArray(); print(r[0].phone + ' ' + r[0].password + ' ' + r[1].account_no)")
  read -r phone password to_account <<<"$roster"
  for zone in Etc/GMT+12 Pacific/Kiritimati; do
    echo "browser-dates: browser zone $zone"
    if ! browser_flow "$phone" "$password" "$to_account" "$zone"; then
      failed+=("$zone")
    fi
  done
  if [ "${#failed[@]}" -gt 0 ]; then
    echo "browser-dates: failed zones: ${failed[*]}" >&2
    return 1
  fi
}

# The db-pool cases that need their own process: another session time zone, and a
# timed exit after closing a warmed pool.
db_pool_isolated() {
  ORA_SDTZ=+00:00 cargo_test db_pool -- --ignored --exact db_pool_rejects_other_time_zones
  # Compile and run once before timing the already-built test process.
  cargo_test db_pool -- --ignored --exact db_pool_close_exits
  local started=$SECONDS
  CARGO_TEST_TIMEOUT=60 cargo_test db_pool -- --ignored --exact db_pool_close_exits
  [ "$((SECONDS - started))" -le 5 ] || { echo "db-pool close process exceeded 5 s" >&2; return 1; }
}

# Plan test-speed step 1: show host load in CI logs.
host_load() {
  echo "it-oracle: host load ($1)"
  uptime || true
  docker stats --no-stream || true
}

run_target() {
  case "$1" in
    tune-seed) tune_seed ;;
    tune-capture) tune_capture ;;
    driver-bench) driver_bench ;;
    driver-parity) reset_schema; cargo_test driver_parity ;;
    db-pool)
      cargo_test db_pool
      db_pool_isolated
      ;;
    migrations)  reset_schema; cargo_test migrations ;;
    admin-migrations) cargo_test migrations -- --ignored --exact populated_admin_upgrade ;;
    admin)       cargo_test admin ;;
    tune-migrations) cargo_test migrations -- --ignored --exact tuning::populated_migrations_preserve_data ;;
    tune-coverage)
      local output="$root/.evidence/oracle-tuning/$(date -u +%Y%m%dT%H%M%SZ)-coverage"
      mkdir -p "$output"
      TUNE_COVERAGE_OUT="$output/xplan-coverage.txt" IT_DB_USER=saltation_tune cargo_test driver_bench -- --ignored --exact tune_coverage --nocapture >"$output/coverage.txt" 2>&1
      echo "tune-coverage evidence: $output"
      ;;
    backfill)    reset_schema; cargo_test backfill ;;
    listeners)   cargo_test listeners ;;
    auth)        cargo_test auth ;;
    accounts)    cargo_test accounts ;;
    transfers)   cargo_test transfers ;;
    payees)      cargo_test payees ;;
    outcomes)    cargo_test outcomes ;;
    atm)         cargo_test atm ;;
    card-payments) cargo_test card_payments ;;
    cards)       cargo_test cards ;;
    inbound)     cargo_test inbound ;;
    password_reset) cargo_test password_reset ;;
    deposits) cargo_test deposits ;;
    loans) cargo_test loans ;;
    outbox)      cargo_test outbox ;;
    outbox-outage) outbox_outage ;;
    observability) observability ;;
    e2e)         e2e ;;
    generator-bc) generator_bc ;;
    generator-cards) generator_cards ;;
    generator-behavior) generator_behavior ;;
    browser-dates) browser_dates ;;
    mongo-users) mongo_users ;;
    *) echo "unknown target: $1" >&2; exit 2 ;;
  esac
}

# shellcheck source=scripts/dev/oracle-tune/tooling.sh
. "$root/scripts/dev/oracle-tune/tooling.sh"

if [ "$target" = all ]; then
  host_load start
  run_target mongo-users
  run_target migrations
  cargo_test listeners auth accounts admin transfers payees cards card_payments inbound password_reset deposits loans outbox outcomes
  run_target backfill
  SALTATION_TEST_EXPECT_LIVE=1 cargo_test listeners auth accounts admin transfers payees cards card_payments inbound password_reset deposits loans outbox outcomes atm driver_parity db_pool
  db_pool_isolated
  run_target observability
  run_target generator-bc
  run_target generator-cards
  run_target generator-behavior
  host_load end
else
  run_target "$target"
fi
