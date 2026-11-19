# Sourced by it-oracle.sh; uses its compose, root and cargo_test functions.
# Capture uses SYSDBA, so diagnostic privileges never become application grants.
tune_sql() {
  { printf '%s\n' 'WHENEVER SQLERROR EXIT FAILURE' 'WHENEVER OSERROR EXIT FAILURE' \
      'SET VERIFY OFF ECHO OFF PAGES 50000 LINES 240 TRIMSPOOL ON' \
      'ALTER SESSION SET CONTAINER=ORCLPDB1;' 'ALTER SESSION SET CURRENT_SCHEMA=SALTATION_TUNE;'; cat; printf '\nEXIT\n'; } \
    | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB -e NLS_LANG=American_America.AL32UTF8 oracle sqlplus -s / as sysdba
}

tune_seed() {
  local customers="${TUNE_CUSTOMERS:-20000}" years="${TUNE_YEARS:-3}" max_gb="${TUNE_MAX_GB:-25}" value available output
  for value in "$customers" "$years" "$max_gb"; do
    case "$value" in ''|*[!0-9]*|0) echo 'tune-seed: parameters must be positive integers' >&2; return 2 ;; esac
  done
  [ "$customers" -ge 2 ] && [ "$customers" -le 1000000 ] && [ "$years" -le 20 ] && [ "$max_gb" -le 25 ] \
    || { echo 'tune-seed: require 2..1000000 customers, 1..20 years, TUNE_MAX_GB <=25' >&2; return 2; }
  available=$("${compose[@]}" exec -T oracle df -Pk /opt/oracle/oradata | awk 'END {print $4}')
  [ "$available" -ge 31457280 ] || { echo 'tune-seed: Oracle data filesystem has less than 30 GB free; aborting before schema reset' >&2; return 1; }
  output="$root/.evidence/oracle-tuning/$(date -u +%Y%m%dT%H%M%SZ)-seed"
  mkdir -p "$output"
  # Password is inherited by name, never a process argument or console output.
  if ! { printf 'SET VERIFY OFF ECHO OFF\nDEFINE pw = "%s"\n' "$SALTATION_DB_PASSWORD"; cat <<'SQL'
WHENEVER SQLERROR EXIT FAILURE
ALTER SESSION SET CONTAINER=ORCLPDB1;
BEGIN
  FOR f IN (SELECT file_name FROM dba_data_files WHERE tablespace_name LIKE 'UNDOTBS%' AND bytes <= 4*1024*1024*1024) LOOP
    EXECUTE IMMEDIATE 'ALTER DATABASE DATAFILE ''' || f.file_name || ''' AUTOEXTEND ON MAXSIZE 4G';
  END LOOP;
END;
/
BEGIN
  EXECUTE IMMEDIATE 'DROP USER saltation_tune CASCADE';
EXCEPTION WHEN OTHERS THEN IF SQLCODE<>-1918 THEN RAISE; END IF;
END;
/
CREATE USER saltation_tune IDENTIFIED BY "&pw" DEFAULT TABLESPACE saltation_data
 QUOTA UNLIMITED ON saltation_data QUOTA UNLIMITED ON outbox_ts;
GRANT CREATE SESSION,CREATE TABLE,CREATE SEQUENCE,CREATE VIEW,CREATE PROCEDURE TO saltation_tune;
GRANT SELECT ON SYS.V_$SQL TO saltation_tune;
GRANT SELECT ON SYS.V_$SQL_PLAN TO saltation_tune;
GRANT SELECT ON SYS.V_$SQL_PLAN_STATISTICS_ALL TO saltation_tune;
GRANT SELECT ON SYS.V_$SESSION TO saltation_tune;
EXIT
SQL
  } | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB -e NLS_LANG=American_America.AL32UTF8 oracle sqlplus -s / as sysdba >/dev/null; then
    echo 'tune-seed: could not recreate SALTATION_TUNE' >&2; return 1
  fi
  IT_DB_USER=saltation_tune cargo_test driver_bench -- --ignored --exact tune_prepare --nocapture >"$output/migrations.txt" 2>&1
  {
    printf 'DEFINE customers=%s\nDEFINE years=%s\nDEFINE max_gb=%s\n' "$customers" "$years" "$max_gb"
    cat <<'SQL'
CREATE TABLE tune_seed(customers NUMBER,years NUMBER,max_gb NUMBER,first_day DATE,last_day DATE,hot_pairs NUMBER,hot_per_day NUMBER);
CREATE TABLE tune_moves(tid NUMBER,a1 NUMBER,a2 NUMBER,amount NUMBER,k NUMBER);
SQL
    cat "$root/scripts/dev/oracle-tune/seed.sql"
  } | tune_sql | tee "$output/seed.txt"
  IT_DB_USER=saltation_tune cargo_test driver_bench -- --ignored --exact tune_credentials --nocapture >"$output/credentials.txt" 2>&1
  tune_sql <"$root/scripts/dev/oracle-tune/check.sql" | tee "$output/consistency.txt"
  { echo "EXEC DBMS_STATS.GATHER_SCHEMA_STATS('SALTATION_TUNE',options=>'GATHER',cascade=>TRUE);"; cat "$root/scripts/dev/oracle-tune/segments.sql"; } \
    | tune_sql | tee "$output/segments.txt"
  echo "tune-seed evidence: $output"
}

# Drop a leftover trigger before any normal-statistics measurement.
tune_normal_stats() {
  tune_sql <<'SQL'
BEGIN
  EXECUTE IMMEDIATE 'DROP TRIGGER saltation_tune.tune_statistics';
EXCEPTION WHEN OTHERS THEN IF SQLCODE<>-4080 THEN RAISE; END IF;
END;
/
SQL
}

tune_plan_stats() {
  local label="$1"
  tune_sql <<SQL
INSERT INTO saltation_tune.tune_plan_bounds VALUES('$label',SYSTIMESTAMP);
COMMIT;
CREATE OR REPLACE TRIGGER saltation_tune.tune_statistics AFTER LOGON ON saltation_tune.SCHEMA
BEGIN EXECUTE IMMEDIATE 'ALTER SESSION SET STATISTICS_LEVEL=ALL'; END;
/
SQL
}

tune_reset_outbox() {
  local label="$1"
  IT_DB_USER=saltation_tune cargo_test driver_bench -- --ignored --exact tune_drain --nocapture >"$output/drain-$label.txt" 2>&1 || return
  printf 'TRUNCATE TABLE saltation_tune.outbox;\nPROMPT drained and truncated SALTATION_TUNE.OUTBOX (%s)\n' "$label" \
    | tune_sql >>"$output/resets.txt"
}

tune_capture() (
  local output entry label run failed=0
  python3 -B "$root/scripts/dev/oracle-tune/test_counters.py" || return
  output="$root/.evidence/oracle-tuning/$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$output"
  { uname -a; echo 'driver container: --cpus 2 --memory 4g';
    "${compose[@]}" ps -q oracle mongo | xargs docker inspect --format '{{.Name}} NanoCpus={{.HostConfig.NanoCpus}} Memory={{.HostConfig.Memory}}';
  } >"$output/host.txt"
  tune_normal_stats >"$output/statistics.txt" || return
  trap 'tune_normal_stats >"$output/statistics-cleanup.txt"' EXIT
  tune_sql >>"$output/statistics.txt" <<'SQL'
EXEC DBMS_STATS.GATHER_SCHEMA_STATS('SALTATION_TUNE',options=>'GATHER',cascade=>TRUE);
BEGIN
  FOR t IN (SELECT table_name FROM dba_tables WHERE owner='SALTATION_TUNE' AND table_name IN ('TUNE_WINDOWS','TUNE_PLAN_BOUNDS')) LOOP
    EXECUTE IMMEDIATE 'DROP TABLE saltation_tune.'||t.table_name||' PURGE';
  END LOOP;
END;
/
CREATE TABLE saltation_tune.tune_windows(workload VARCHAR2(40),concurrency NUMBER,run NUMBER,started_at TIMESTAMP WITH TIME ZONE,ended_at TIMESTAMP WITH TIME ZONE,PRIMARY KEY(workload,concurrency,run));
CREATE TABLE saltation_tune.tune_plan_bounds(entry VARCHAR2(40),started_at TIMESTAMP WITH TIME ZONE);
SQL
  for entry in transfer:8 transfer:32 transfer:64 transfer_hot:32 transfer_hot:64 history:8 balance:8 lookup:8 outbox:1 history_recent_first_shallow:8 history_recent_first_deep:8 history_recent_deep_shallow:8 history_recent_deep_deep:8 history_old_first_shallow:8 history_old_first_deep:8 history_old_deep_shallow:8 history_old_deep_deep:8 history_filtered_deep:8 outbox_long:1; do
    label="${entry/:/-}"
    BENCH_CAPTURE=1 BENCH_POPULATED=1 BENCH_ORA_SNAP=1 BENCH_DRIVER="${BENCH_DRIVER:-check}" BENCH_RUNS="${BENCH_RUNS:-1}" BENCH_ONLY="$entry" BENCH_OUTPUT="$output" driver_bench >"$output/bench-$label.log" 2>&1 || { failed=1; break; }
    for ((run=1; run<=${BENCH_RUNS:-1}; run++)); do
      python3 "$root/scripts/dev/oracle-tune/sqldelta.py" "$output/snap-$label-run$run-start.txt" "$output/snap-$label-run$run-end.txt" >"$output/sqldelta-$label-run$run.txt" || { failed=1; break; }
    done
    [ "$failed" = 0 ] || break
    if [[ "$entry" = transfer:* || "$entry" = transfer_hot:* ]]; then
      python3 "$root/scripts/dev/oracle-tune/capture.py" "$root" "$label" ash | tune_sql >"$output/ash-$label.txt" || { failed=1; break; }
    fi
    tune_reset_outbox "$label-plan" || { failed=1; break; }
    tune_plan_stats "$label" >>"$output/statistics.txt" || { failed=1; break; }
    mkdir -p "$output/plan-$label"
    BENCH_RESULT_NAME=plan.jsonl BENCH_CAPTURE=0 BENCH_POPULATED=1 BENCH_ORA_SNAP=0 BENCH_SECS=1 BENCH_WARMUP_SECS=1 BENCH_DRIVER="${BENCH_DRIVER:-check}" BENCH_RUNS=1 BENCH_ONLY="$entry" BENCH_OUTPUT="$output/plan-$label" driver_bench >"$output/plan-$label.log" 2>&1 || { failed=1; break; }
    python3 "$root/scripts/dev/oracle-tune/capture.py" "$root" "$label" xplan | tune_sql >"$output/xplan-$label.txt" || { failed=1; break; }
    tune_normal_stats >>"$output/statistics.txt" || { failed=1; break; }
  done
  if [ "$failed" = 0 ]; then
    TUNE_COVERAGE_OUT="$output/xplan-coverage.txt" IT_DB_USER=saltation_tune cargo_test driver_bench -- --ignored --exact tune_coverage --nocapture >"$output/coverage.txt" 2>&1 || failed=1
  fi
  cat "$root/scripts/dev/oracle-tune/segments.sql" | tune_sql >"$output/segments.txt" || failed=1
  python3 "$root/scripts/dev/oracle-tune/counters.py" "$output" || failed=1
  echo "tune-capture evidence: $output"
  return "$failed"
)

# Record Oracle server time immediately before acknowledging each request.
tune_window() {
  local stem="$1" label workload concurrency run phase
  label="${stem##*/snap-}"
  workload="${label%%-*}"; label="${label#*-}"
  concurrency="${label%%-*}"; label="${label#*-run}"
  run="${label%%-*}"; phase="${label##*-}"
  if [ "$phase" = start ]; then
    printf "INSERT INTO saltation_tune.tune_windows(workload,concurrency,run,started_at) VALUES('%s',%s,%s,SYSTIMESTAMP);\nCOMMIT;\n" "$workload" "$concurrency" "$run"
  else
    printf "UPDATE saltation_tune.tune_windows SET ended_at=SYSTIMESTAMP WHERE workload='%s' AND concurrency=%s AND run=%s;\nCOMMIT;\n" "$workload" "$concurrency" "$run"
  fi | tune_sql >"$stem-window.txt"
}

# Read only. A Compose project name is not proof of the volumes it mounted.
tune_restored_outbox_stack() {
  local service volume destination container actual oracle_id mongo_id bank_id current_key
  current_key="$(volumes_key)" || {
    echo 'restored outbox: could not identify both prepared data volumes' >&2
    return 1
  }
  if [[ "$image_id" == missing || "${SETUP_KEY:-}" != "$setup_key" ||
        -z "${VOLUMES_KEY:-}" || "$VOLUMES_KEY" != "$current_key" ]]; then
    echo 'restored outbox: prepared setup or volume identity does not match' >&2
    return 1
  fi
  for service in oracle mongo; do
    case "$service" in
      oracle) volume=oradata; destination=/opt/oracle/oradata ;;
      mongo) volume=mongodata; destination=/data/db ;;
    esac
    container="$("${compose[@]}" ps -q "$service")" || return
    if [[ ! "$container" =~ ^[0-9a-f]{64}$ ]]; then
      echo "restored outbox: expected one running $service container" >&2
      return 1
    fi
    actual="$(docker inspect --format       '{{.State.Running}} {{range .Mounts}}{{if eq .Destination "'"$destination"'"}}{{.Type}} {{.Name}}{{end}}{{end}}'       "$container")" || return
    if [[ "$actual" != "true volume ${project}_$volume" ]]; then
      echo "restored outbox: unexpected $service data mount" >&2
      return 1
    fi
    case "$service" in
      oracle) oracle_id="$container" ;;
      mongo) mongo_id="$container" ;;
    esac
  done
  bank_id="$("${compose[@]}" ps -q bank)" || return
  if [[ -n "$bank_id" ]]; then
    echo 'restored outbox: stop the bank service before capturing relay work' >&2
    return 1
  fi
  printf '%s %s %s\n' "$oracle_id" "$mongo_id" "$current_key"
}

# Reserve time from the request write, even when the watcher notices it late.
tune_outbox_budget() {
  local request="$1" cutoff="$2" maximum="$3"
  shift 3
  python3 -c '
import os, subprocess, sys, time
path, cutoff, maximum, *command = sys.argv[1:]
age = time.time() - os.stat(path).st_mtime
budget = min(float(maximum), float(cutoff) - age)
if age < 0 or budget <= 0:
    sys.exit("restored outbox: request deadline expired")
try:
    sys.exit(subprocess.run(command, input=sys.stdin.buffer.read(), timeout=budget).returncode)
except subprocess.TimeoutExpired:
    sys.exit("restored outbox: SQL client deadline expired; confirm Oracle work stopped before discarding this clone")
' "$request" "$cutoff" "$maximum" "$@"
}

tune_outbox_stats() {
  local request="$1" hostname="$2" pid="$3"
  {
    tune_outbox_sessions "$hostname" "$pid"
    cat <<'SQL'
SET SERVEROUTPUT ON
DECLARE
  fixture_rows NUMBER;
BEGIN
  SELECT COUNT(*) INTO fixture_rows FROM saltation_tune.outbox;
  IF fixture_rows <> 50000 THEN
    RAISE_APPLICATION_ERROR(-20001, 'ordinary outbox fixture must have 50000 committed rows');
  END IF;
END;
/
SQL
    if [[ "$restored_outbox_mode" == 1 ]]; then
      cat <<'SQL'
DECLARE
  gather_start DATE := SYSDATE;
  preference VARCHAR2(4000);
  missing_stats NUMBER;
  outbox_pk_stats NUMBER;
BEGIN
  preference := DBMS_STATS.GET_PREFS('METHOD_OPT','SALTATION_TUNE','OUTBOX');
  DBMS_OUTPUT.PUT_LINE('method_opt_before|' || preference);
  DBMS_STATS.GATHER_TABLE_STATS('SALTATION_TUNE','OUTBOX',estimate_percent=>100,
    degree=>1,cascade=>TRUE,no_invalidate=>FALSE,force=>FALSE);
  IF preference <> DBMS_STATS.GET_PREFS('METHOD_OPT','SALTATION_TUNE','OUTBOX') THEN
    RAISE_APPLICATION_ERROR(-20002, 'outbox histogram preference changed');
  END IF;
  DBMS_OUTPUT.PUT_LINE('method_opt_after|' || DBMS_STATS.GET_PREFS('METHOD_OPT','SALTATION_TUNE','OUTBOX'));
  SELECT COUNT(*) INTO missing_stats FROM dba_tab_statistics
    WHERE owner='SALTATION_TUNE' AND table_name='OUTBOX' AND partition_name IS NULL
      AND num_rows=50000 AND sample_size=50000 AND last_analyzed>=gather_start;
  IF missing_stats <> 1 THEN
    RAISE_APPLICATION_ERROR(-20003, 'fresh full-sample outbox table statistics missing');
  END IF;
  SELECT COUNT(*) INTO missing_stats FROM dba_ind_statistics
    WHERE table_owner='SALTATION_TUNE' AND table_name='OUTBOX' AND partition_name IS NULL
      AND (last_analyzed IS NULL OR last_analyzed<gather_start);
  SELECT COUNT(*) INTO outbox_pk_stats FROM dba_ind_statistics
    WHERE table_owner='SALTATION_TUNE' AND table_name='OUTBOX' AND index_name='OUTBOX_PK'
      AND partition_name IS NULL AND num_rows=50000 AND last_analyzed>=gather_start;
  IF missing_stats <> 0 OR outbox_pk_stats <> 1 THEN
    RAISE_APPLICATION_ERROR(-20004, 'fresh outbox index statistics missing');
  END IF;
END;
/
SQL
    fi
    cat <<'SQL'
SELECT 'table_stats|'||num_rows||'|'||sample_size||'|'||TO_CHAR(last_analyzed,'YYYYMMDDHH24MISS')
  FROM dba_tab_statistics WHERE owner='SALTATION_TUNE' AND table_name='OUTBOX' AND partition_name IS NULL;
SELECT 'index_stats|'||index_name||'|'||num_rows||'|'||TO_CHAR(last_analyzed,'YYYYMMDDHH24MISS')
  FROM dba_ind_statistics WHERE table_owner='SALTATION_TUNE' AND table_name='OUTBOX' AND partition_name IS NULL;
SQL
  } | tune_outbox_sql "$request" 25 20
}

tune_outbox_sql() {
  local request="$1" cutoff="$2" maximum="$3" oracle_id="${restored_outbox_stack%% *}"
  {
    printf '%s\n' 'WHENEVER SQLERROR EXIT FAILURE' 'WHENEVER OSERROR EXIT FAILURE'       'SET PAGES 0 LINES 1000 FEEDBACK OFF HEADING OFF VERIFY OFF TRIMSPOOL ON'       'ALTER SESSION SET CONTAINER=ORCLPDB1;'
    cat
    printf '\nEXIT SUCCESS\n'
  } | tune_outbox_budget "$request" "$cutoff" "$maximum"     docker exec -i -e ORACLE_SID=ORCLCDB -e NLS_LANG=American_America.AL32UTF8 "$oracle_id" sqlplus -s / as sysdba
}

tune_outbox_sessions() {
  local hostname="$1" pid="$2"
  cat <<SQL
DECLARE
  matching_sessions NUMBER;
  other_sessions NUMBER;
BEGIN
  SELECT COUNT(*) INTO matching_sessions FROM v\$session
    WHERE username='SALTATION_TUNE' AND machine='$hostname' AND REGEXP_SUBSTR(process,'^[0-9]+')='$pid';
  SELECT COUNT(*) INTO other_sessions FROM v\$session
    WHERE username='SALTATION_TUNE' AND
      (NVL(machine,'-')<>'$hostname' OR NVL(REGEXP_SUBSTR(process,'^[0-9]+'),'-')<>'$pid');
  IF matching_sessions=0 OR other_sessions<>0 THEN
    RAISE_APPLICATION_ERROR(-20005, 'cannot establish exclusive benchmark sessions in SALTATION_TUNE');
  END IF;
END;
/
SELECT 'session|'||sid||'|'||serial#||'|'||machine||'|'||process||'|'||TO_CHAR(logon_time,'YYYYMMDDHH24MISS')
  FROM v\$session WHERE username='SALTATION_TUNE' ORDER BY sid,serial#;
DECLARE
  stats_triggers NUMBER;
BEGIN
  SELECT COUNT(*) INTO stats_triggers FROM dba_triggers
    WHERE owner='SALTATION_TUNE' AND trigger_name='TUNE_STATISTICS' AND status='ENABLED';
  IF stats_triggers<>0 THEN
    RAISE_APPLICATION_ERROR(-20006, 'ordinary capture requires normal session statistics');
  END IF;
END;
/
SQL
}

tune_outbox_cursor_snapshot() {
  local request="$1" phase="$2" hostname="$3" pid="$4" sql_text
  local logons="SELECT 'logons|'||value FROM v\$sysstat WHERE name='logons cumulative';"
  sql_text="$(python3 -c '
from pathlib import Path
import sys
sql = Path(sys.argv[1]).read_text().rstrip("\r\n")
print(" || CHR(10) || ".join(chr(39) + line.replace(chr(39), chr(39)*2) + chr(39) for line in sql.split("\n")))
' "$root/bank/src/features/outbox_relay/sql/select_batch.sql")" || return
  {
    if [[ "$phase" == start ]]; then
      printf '%s\n' "$logons"
    fi
    tune_outbox_sessions "$hostname" "$pid"
    cat <<SQL
DECLARE
  relay_sql CONSTANT VARCHAR2(4000) := $sql_text;
BEGIN
  FOR c IN (
    SELECT sql_id,child_number,child_address,object_status,last_load_time,
           plan_hash_value,executions,rows_processed,invalidations,loads,buffer_gets
    FROM v\$sql WHERE parsing_schema_name='SALTATION_TUNE'
      AND DBMS_LOB.GETLENGTH(sql_fulltext) BETWEEN LENGTH(relay_sql) AND LENGTH(relay_sql)+2
      AND RTRIM(DBMS_LOB.SUBSTR(sql_fulltext,4000,1),CHR(10)||CHR(13))=relay_sql
  ) LOOP
    DBMS_OUTPUT.PUT_LINE('cursor|'||c.sql_id||'|'||c.child_number||'|'||RAWTOHEX(c.child_address)||'|'||
      c.object_status||'|'||c.last_load_time||'|'||c.plan_hash_value||'|'||c.executions||'|'||
      c.rows_processed||'|'||c.invalidations||'|'||c.loads||'|'||c.buffer_gets);
    FOR p IN (SELECT id,operation,options,object_name FROM v\$sql_plan
        WHERE sql_id=c.sql_id AND child_address=c.child_address ORDER BY id) LOOP
      DBMS_OUTPUT.PUT_LINE('plan|'||c.sql_id||'|'||c.child_number||'|'||RAWTOHEX(c.child_address)||'|'||
        p.id||'|'||p.operation||'|'||p.options||'|'||p.object_name);
    END LOOP;
  END LOOP;
  FOR s IN (
    SELECT sql_id, executions, rows_processed, buffer_gets FROM v\$sqlstats
    WHERE DBMS_LOB.GETLENGTH(sql_fulltext) BETWEEN LENGTH(relay_sql) AND LENGTH(relay_sql)+2
      AND RTRIM(DBMS_LOB.SUBSTR(sql_fulltext,4000,1),CHR(10)||CHR(13))=relay_sql
  ) LOOP
    DBMS_OUTPUT.PUT_LINE('parent|'||s.sql_id||'|'||s.executions||'|'||s.rows_processed||'|'||s.buffer_gets);
  END LOOP;
END;
/
SQL
    if [[ "$phase" == end ]]; then
      printf '%s\n' "SELECT 'remaining|'||COUNT(*) FROM saltation_tune.outbox;" "$logons"
    fi
  } | {
    # SQL*Plus must emit the structured cursor rows.
    { printf '%s\n' 'SET SERVEROUTPUT ON SIZE UNLIMITED'; cat; } | tune_outbox_sql "$request" 28 28
  }
}

tune_outbox_attribution() {
  python3 - "$1" "$2" <<'PY'
from collections import defaultdict
from pathlib import Path
import sys

def read(path):
    cursors, plans, sessions, parents = {}, defaultdict(list), set(), {}
    remaining, logons = [], []
    for line in Path(path).read_text().splitlines():
        kind, *fields = line.split("|")
        if kind == "cursor":
            sql, child, address, status, loaded, plan, *values = fields
            cursors[sql, child, address] = (status, loaded, plan, *map(int, values))
        elif kind == "plan":
            plans[tuple(fields[:3])].append(" ".join(filter(None, fields[4:])))
        elif kind == "session":
            sessions.add(tuple(fields))
        elif kind == "parent":
            parents[fields[0]] = tuple(map(int, fields[1:]))
        elif kind == "logons":
            logons.extend(fields)
        elif kind == "remaining":
            remaining.extend(fields)
    return cursors, plans, sessions, remaining, logons, parents

def inconclusive(reason):
    sys.exit("restored outbox: inconclusive cursor attribution: " + reason)

before, _, start_sessions, _, start_logons, start_parents = read(sys.argv[1])
after, plans, end_sessions, remaining, end_logons, end_parents = read(sys.argv[2])
if (not start_sessions or start_sessions != end_sessions or remaining != ["0"]
        or len(start_logons) != 1 or start_logons != end_logons):
    inconclusive("session membership or login count changed, or rows remain")
def parent_delta(sql_id):
    if sql_id not in end_parents or (sql_id not in start_parents and before):
        inconclusive("missing parent counters for the relay SQL")
    return tuple(end - start for start, end in zip(start_parents.get(sql_id, (0, 0, 0)), end_parents[sql_id]))

measured = []
for identity, end in after.items():
    status, loaded, plan, executions, rows, invalidations, loads, gets = end
    old = before.get(identity)
    if old is not None and old[1] == loaded and old[6] == loads:
        if old[2] != plan or invalidations != old[5]:
            inconclusive("plan or invalidation changed within the window")
        executions, rows, gets = (executions-old[3], rows-old[4], gets-old[7])
    elif old is None and loads == 1 and invalidations == 0:
        pass
    elif old is not None and old[0] != "VALID" and loads == old[6]+1 and invalidations == old[5]:
        # Either Oracle reset this child or carried its counters through the first load.
        # Attribute it only if the retained SQL totals corroborate one interpretation.
        total = parent_delta(identity[0])
        if total not in ((executions, rows, gets), (executions-old[3], rows-old[4], gets-old[7])):
            inconclusive("post-invalidation counters could have reset")
        executions, rows, gets = total
    else:
        inconclusive("cursor reset or reload could not be attributed")
    if min(executions, rows, gets) < 0 or (not executions and (rows or gets)):
        inconclusive("cursor counters cannot be subtracted")
    if executions:
        measured.append((identity, plan, rows, gets, executions))
if (not measured or len({(entry[0][0], entry[1]) for entry in measured}) != 1
        or sum(entry[2] for entry in measured) != 50000
        or any(not plans[entry[0]] for entry in measured)):
    inconclusive("expected one SQL ID and plan accounting for all delivered rows")
if parent_delta(measured[0][0][0]) != tuple(sum(entry[i] for entry in measured) for i in (4, 2, 3)):
    inconclusive("retained SQL totals disagree with child deltas")
for identity, plan, rows, gets, _ in measured:
    print("|".join((*identity, plan, str(rows), str(gets), ", ".join(plans[identity]))))
PY
}

tune_outbox_client() {
  local cargo actual expected oracle_id mongo_id key hostname pid
  cargo="$(docker inspect -f '{{.Id}}' "$cargo_name")" || return
  read -r oracle_id mongo_id key <<<"$restored_outbox_stack"
  expected="$(printf '%s\n' "$oracle_id" "$mongo_id" "$cargo" | LC_ALL=C sort)"
  actual="$(docker network inspect --format '{{range $id, $v := .Containers}}{{$id}}{{println}}{{end}}' "${project}_data")" || return
  if [[ "$(printf '%s\n' "$actual" | LC_ALL=C sort)" != "$expected" ]]; then
    echo 'restored outbox: another client is attached to the benchmark data network' >&2
    return 1
  fi
  hostname="$(docker inspect -f '{{.Config.Hostname}}' "$cargo")" || return
  pid="$(docker exec "$cargo" sh -c '
    for entry in /proc/[0-9]*/exe; do
      binary="$(readlink "$entry" 2>/dev/null)" || continue
      case "$binary" in
        /target/release/deps/driver_bench-*) set -- "$@" "${entry#/proc/}" ;;
      esac
    done
    if [ "$#" -ne 1 ]; then
      echo "restored outbox: expected one live driver-bench process" >&2
      exit 1
    fi
    printf "%s\n" "${1%/exe}"
  ')" || return
  printf '%s %s\n' "$hostname" "$pid"
}

tune_restored_outbox_watch() {
  local prefix="$1" owner_pid="${2:-$$}" phase stem request client hostname pid
  for phase in start end; do
    stem="$prefix-$phase"
    request="$stem.req"
    while [[ ! -f "$request" ]]; do
      kill -0 "$owner_pid" 2>/dev/null || return 1
      sleep 0.1
    done
    if ! {
      kill -0 "$owner_pid" 2>/dev/null &&
      mkdir -- "$stem.claim" &&
      client="$(tune_outbox_client)" &&
      read -r hostname pid <<<"$client" &&
      [[ "$hostname" =~ ^[0-9a-f]{12}$ && "$pid" =~ ^[1-9][0-9]*$ ]] &&
      tune_outbox_boundary "$prefix" "$phase" "$request" "$hostname" "$pid"
    } >"$stem.restored-outbox.log" 2>&1; then
      echo "restored outbox: $phase handshake failed; see $stem.restored-outbox.log" >&2
      return 1
    fi
  done
}

tune_outbox_boundary() {
  local prefix="$1" phase="$2" request="$3" hostname="$4" pid="$5" stem="$1-$2"
  if [[ "$phase" == start ]]; then
    tune_outbox_stats "$request" "$hostname" "$pid" >"$stem.stats.txt" || {
      cat "$stem.stats.txt"
      return 1
    }
  fi
  bench_snapshot "$stem" tune_outbox_budget "$request" 28 28 || {
    cat "$stem.txt"
    return 1
  }
  tune_outbox_cursor_snapshot "$request" "$phase" "$hostname" "$pid" >"$stem.cursor.txt" || {
    cat "$stem.cursor.txt"
    return 1
  }
  if [[ "$phase" == end ]]; then
    tune_outbox_attribution "$prefix-start.cursor.txt" "$stem.cursor.txt" >"$prefix.attribution.txt" || return
  fi
  python3 - "$request" "$stem.ack" <<'PY'
from pathlib import Path
import sys
import time

request, ack = map(Path, sys.argv[1:])
pending = ack.with_suffix(".ack.tmp")
with pending.open("x"):
    pass
try:
    if not 0 <= time.time() - request.stat().st_mtime < 28:
        sys.exit("restored outbox: ACK cutoff exceeded")
    pending.rename(ack)
finally:
    pending.unlink(missing_ok=True)
PY
}

# A zero exit without the final handshake must not keep the project lock.
tune_finish_outbox_watcher() {
  local watcher="$1" final_ack="$2"
  if [[ ! -f "$final_ack" ]]; then
    kill "$watcher" 2>/dev/null || true
    wait "$watcher" 2>/dev/null || true
    echo 'restored outbox: benchmark exited without the final snapshot acknowledgment' >&2
    return 1
  fi
  wait "$watcher"
}
