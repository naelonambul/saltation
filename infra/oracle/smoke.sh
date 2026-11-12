#!/usr/bin/env bash
# Starts a throwaway container from the Saltation Oracle image, waits for the
# database, and prints the settings the spec requires (N-7, F-36a).
set -euo pipefail
IMAGE_TAG="${IMAGE_TAG:-saltation/oracle-ee:19.19}"
name="saltation-oracle-smoke-$$"
here="$(cd "$(dirname "$0")" && pwd)"
cleanup() { docker rm -f "$name" >/dev/null 2>&1 || true; docker volume rm "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# Throwaway credentials, generated per run; the container is removed on exit.
sys_pw="S$(python3 -c 'import secrets; print(secrets.token_hex(12))')1"
app_pw="A$(python3 -c 'import secrets; print(secrets.token_hex(12))')1"

docker run -d --name "$name" \
  -e ORACLE_PWD="$sys_pw" -e SALTATION_DB_PASSWORD="$app_pw" \
  -e INIT_SGA_SIZE=3584 -e INIT_PGA_SIZE=1024 \
  -v "$name":/opt/oracle/oradata \
  -v "$here/setup":/opt/oracle/scripts/setup:ro \
  "$IMAGE_TAG" >/dev/null

echo "smoke: waiting for the database (first start creates it; this takes a while)"
for _ in $(seq 1 180); do
  status=starting
  docker exec "$name" test -f /opt/oracle/oradata/.saltation-ready 2>/dev/null && status=healthy && break
  if [ "$(docker inspect -f '{{.State.Running}}' "$name")" != true ]; then docker logs "$name" | tail -40; exit 1; fi
  sleep 10
done
[ "$status" = healthy ] || { docker logs "$name" | tail -40; echo "smoke: not healthy" >&2; exit 1; }
if docker logs "$name" 2>&1 | grep -E "ORA-[0-9]+|SP2-[0-9]+" >/dev/null; then
  docker logs "$name" 2>&1 | grep -E "ORA-[0-9]+|SP2-[0-9]+" >&2; echo "smoke: errors during setup" >&2; exit 1
fi

docker exec -i -e ORACLE_SID=ORCLCDB "$name" bash -c 'sqlplus -s / as sysdba' <<'SQL'
SET HEADING OFF FEEDBACK OFF PAGESIZE 0 LINESIZE 200
SELECT 'banner: ' || banner_full FROM v$version;
SELECT 'log_mode: ' || log_mode FROM v$database;
SELECT 'redo: ' || COUNT(*) || ' groups of ' || MIN(bytes)/1024/1024 || ' MB' FROM v$log;
SELECT 'pga_aggregate_limit: ' || value/1024/1024 || ' MB' FROM v$parameter WHERE name = 'pga_aggregate_limit';
SELECT 'undo max: ' || SUM(maxbytes)/1024/1024/1024 || ' GB' FROM dba_data_files WHERE tablespace_name LIKE 'UNDOTBS%';
ALTER SESSION SET CONTAINER = ORCLPDB1;
SELECT 'outbox_ts max: ' || maxbytes/1024/1024/1024 || ' GB' FROM dba_data_files WHERE tablespace_name = 'OUTBOX_TS';
SELECT 'schema user: ' || username FROM dba_users WHERE username = 'SALTATION';
EXIT
SQL
login="$(docker exec -e ORACLE_SID=ORCLCDB "$name" bash -c "printf 'SET HEADING OFF FEEDBACK OFF\nSELECT 40+2 FROM dual;\nEXIT\n' | sqlplus -s saltation/$app_pw@//localhost:1521/ORCLPDB1" | tr -d '[:space:]')"
[ "$login" = 42 ] || { echo "smoke: schema login failed: $login" >&2; exit 1; }
echo "smoke: schema login ok"
