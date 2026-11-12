#!/usr/bin/env bash
# Oracle integration tests. Starts Oracle and MongoDB in a separate Compose
# project, resets the test schema, and runs the named test target in the bank
# dev image on the project's data network.
# Usage: scripts/dev/it-oracle.sh <target>|all [--fresh]
#   targets: mongo-users, migrations, listeners, auth, accounts, transfers, outbox, outbox-outage, observability (more are added per plan step)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
project="saltation-it"
env_file="$root/infra/compose/.env.it"
compose=(docker compose -p "$project" -f "$root/infra/compose/compose.yml" --env-file "$env_file" --profile ci)

if [ ! -f "$env_file" ]; then
  umask 077
  gen() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 20; }
  cat >"$env_file" <<ENV
ORACLE_PWD=It$(gen)1
SALTATION_DB_PASSWORD=It$(gen)1
MONGO_ROOT_PASSWORD=$(gen)
MONGO_BANK_PASSWORD=$(gen)
MONGO_GENERATOR_PASSWORD=$(gen)
GRAFANA_ADMIN_PASSWORD=$(gen)
ENV
fi
set -a; . "$env_file"; set +a

target="${1:-all}"
[ "${2:-}" = "--fresh" ] && "${compose[@]}" down -v

"${compose[@]}" up -d --wait --wait-timeout 3600 oracle mongo
# Database tests inspect the outbox directly; a running bank container's relay
# would drain it under them. Only the observability target runs the bank.
[ "$target" = observability ] || "${compose[@]}" stop bank >/dev/null 2>&1 || true

reset_schema() {
  { echo "SET VERIFY OFF"; echo "DEFINE pw = \"$SALTATION_DB_PASSWORD\""; cat "$root/infra/oracle/reset_schema.sql"; } \
    | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB oracle sqlplus -s / as sysdba >/dev/null
}

cargo_test() {
  local extra_env=()
  [ -n "${OUTBOX_OUTAGE_TEST:-}" ] && extra_env=(-e OUTBOX_OUTAGE_TEST)
  docker build -q --target dev -t saltation/bank-dev:local "$root/bank" >/dev/null
  docker run --rm --network "${project}_data" \
    -e SALTATION_DB_USER=saltation -e SALTATION_DB_PASSWORD -e SALTATION_DB_CONNECT=oracle:1521/ORCLPDB1 -e SALTATION_DB_ACQUIRE_MS=10000 \
    -e MONGO_URI="mongodb://bank:${MONGO_BANK_PASSWORD}@mongo:27017/bank_logs" ${extra_env[@]+"${extra_env[@]}"} \
    -v "$root/bank":/src -w /src -v "$root/contracts":/contracts:ro -e CONTRACTS_DIR=/contracts -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target:/target \
    saltation/bank-dev:local cargo test --locked --test "$1" -- --test-threads=1 "${@:2}"
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
  OUTBOX_OUTAGE_TEST=1 cargo_test outbox outage_then_full_delivery --nocapture
  wait "$chaos"
}

# Runs the whole stack: a request to the bank must appear in Loki (via
# app.log and Alloy) and in Prometheus, and Grafana must serve the dashboard.
observability() {
  "${compose[@]}" up -d --build --wait --wait-timeout 900
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

run_target() {
  case "$1" in
    migrations)  reset_schema; cargo_test migrations ;;
    listeners)   cargo_test listeners ;;
    auth)        cargo_test auth ;;
    accounts)    cargo_test accounts ;;
    transfers)   cargo_test transfers ;;
    outbox)      cargo_test outbox ;;
    outbox-outage) outbox_outage ;;
    observability) observability ;;
    mongo-users) mongo_users ;;
    *) echo "unknown target: $1" >&2; exit 2 ;;
  esac
}

if [ "$target" = all ]; then
  for t in mongo-users migrations listeners auth accounts transfers outbox; do run_target "$t"; done
  run_target observability
else
  run_target "$target"
fi
