#!/usr/bin/env bash
# Oracle integration tests. Starts Oracle and MongoDB in a separate Compose
# project, resets the test schema, and runs the named test target in the bank
# dev image on the project's data network.
# Usage: scripts/dev/it-oracle.sh <target>|all [--fresh]
#   targets: mongo-users, migrations, listeners, auth, accounts, transfers, outbox, outbox-outage, observability, e2e (more are added per plan step)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
project="saltation-it"

# State lives outside the checkout (CI checkouts delete untracked files), so the
# test database is created once and reused across runs (plan oracle-ci-speed).
state_dir="${SALTATION_IT_DIR:-$HOME/.cache/saltation-it}"
mkdir -p "$state_dir" && chmod 700 "$state_dir"
env_file="$state_dir/env"
compose=(docker compose -p "$project" -f "$root/infra/compose/compose.yml" --env-file "$env_file" --profile ci)

# One run at a time per host: flock where available (Linux runner), otherwise a
# directory lock that is taken over when its holder process is gone.
exec 9>"$state_dir/lock"
if command -v flock >/dev/null 2>&1; then
  flock 9
else
  until mkdir "$state_dir/lock.d" 2>/dev/null; do
    holder=$(cat "$state_dir/lock.d/pid" 2>/dev/null || echo)
    if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then rm -rf "$state_dir/lock.d"; continue; fi
    sleep 2
  done
  echo $$ >"$state_dir/lock.d/pid"
  trap 'rm -rf "$state_dir/lock.d"' EXIT
fi

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

target="${1:-all}"
if [ "${2:-}" = "--fresh" ] || [ "$recreate" = yes ] || [ "${SETUP_KEY:-}" != "$setup_key" ]; then
  echo "it-oracle: creating fresh test volumes"
  "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  python3 - "$env_file" "$setup_key" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
lines = [l for l in p.read_text().splitlines() if not l.startswith("SETUP_KEY=")]
p.write_text("\n".join(lines + [f"SETUP_KEY={sys.argv[2]}"]) + "\n")
PY
else
  echo "it-oracle: reusing the test database"
fi

# Leftovers from a cancelled run: the cargo container, a stopped MongoDB
# (outbox-outage), and application containers whose relay would drain the
# outbox under the database tests. Only observability and e2e run the bank.
docker rm -f saltation-it-cargo >/dev/null 2>&1 || true
case "$target" in
  observability|e2e) ;;
  *) "${compose[@]}" stop bank web alloy loki prometheus grafana >/dev/null 2>&1 || true ;;
esac
"${compose[@]}" up -d --wait --wait-timeout 3600 oracle mongo

reset_schema() {
  { echo "SET VERIFY OFF"; echo "DEFINE pw = \"$SALTATION_DB_PASSWORD\""; cat "$root/infra/oracle/reset_schema.sql"; } \
    | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB oracle sqlplus -s / as sysdba >/dev/null
  "${compose[@]}" exec -T mongo mongosh --quiet -u bank -p "$MONGO_BANK_PASSWORD" --authenticationDatabase bank_logs \
    --eval 'db.getSiblingDB("bank_logs").events.deleteMany({})' >/dev/null
}

dev_image_built=no
dev_image() {
  if [ "$dev_image_built" = no ]; then
    docker build -q --target dev -t saltation/bank-dev:local "$root/bank" >/dev/null
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
  [ -n "${E2E_SINCE:-}" ] && extra_env=(-e E2E_SINCE -e MONGO_TRUTH_URI)
  dev_image
  docker run --rm --name saltation-it-cargo --network "${project}_data" \
    -e SALTATION_DB_USER=saltation -e SALTATION_DB_PASSWORD -e SALTATION_DB_CONNECT=oracle:1521/ORCLPDB1 -e SALTATION_DB_ACQUIRE_MS=10000 -e SALTATION_CORS_ORIGINS=http://localhost:8088 \
    -e MONGO_URI="mongodb://bank:${MONGO_BANK_PASSWORD}@mongo:27017/bank_logs" ${extra_env[@]+"${extra_env[@]}"} \
    -v "$root/bank":/src -w /src -v "$root/contracts":/contracts:ro -e CONTRACTS_DIR=/contracts -v saltation-cargo-registry:/root/.cargo/registry -v saltation-cargo-target:/target \
    saltation/bank-dev:local cargo test --locked "${tests[@]}" -- --test-threads=1 ${args[@]+"${args[@]}"}
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

# Plan step 10: the whole stack, 1,000 seeded customers, 10 minutes of normal
# transfers from the generator, the Rust end-to-end checks, Prometheus data,
# then a person's flow in the mobile web app with Playwright.
e2e() {
  local since start gen roster phone password to_account
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  start=$(( ($(date +%s) % 90000) * 1000 ))
  gen=("${compose[@]}" --profile tools run --rm generator)
  "${compose[@]}" up -d --build --wait --wait-timeout 1800
  "${compose[@]}" --profile tools build generator >/dev/null
  echo "e2e: seeding 1,000 customers from #$start"
  "${gen[@]}" seed --customers 1000 --start "$start"
  echo "e2e: 10 minutes of normal transfers at 5 per second"
  "${gen[@]}" run-normal --minutes "${E2E_MINUTES:-10}" --rate 5
  MONGO_TRUTH_URI="mongodb://generator:${MONGO_GENERATOR_PASSWORD}@mongo:27017/truth" E2E_SINCE="$since" \
    cargo_test e2e -- generator_run_is_labelled_logged_and_consistent --nocapture
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
  docker run --rm --network "${project}_edge" -v "$root/web":/web -w /web/apps/mobile \
    -v saltation-pnpm-store:/pnpm-store -e npm_config_store_dir=/pnpm-store -e CI=true \
    -e E2E_BASE_URL=http://web:8088 -e E2E_PHONE="$phone" -e E2E_PASSWORD="$password" -e E2E_TO_ACCOUNT="$to_account" \
    mcr.microsoft.com/playwright:v1.55.0-noble sh -c "corepack enable >/dev/null 2>&1 && cd /web && pnpm install --frozen-lockfile >/dev/null && cd apps/mobile && pnpm exec playwright test"
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
    e2e)         e2e ;;
    mongo-users) mongo_users ;;
    *) echo "unknown target: $1" >&2; exit 2 ;;
  esac
}

if [ "$target" = all ]; then
  run_target mongo-users
  run_target migrations
  cargo_test listeners auth accounts transfers outbox
  run_target observability
else
  run_target "$target"
fi
