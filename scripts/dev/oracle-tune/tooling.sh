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
CREATE TABLE saltation_tune.tune_windows(workload VARCHAR2(20),concurrency NUMBER,run NUMBER,started_at TIMESTAMP WITH TIME ZONE,ended_at TIMESTAMP WITH TIME ZONE,PRIMARY KEY(workload,concurrency,run));
CREATE TABLE saltation_tune.tune_plan_bounds(entry VARCHAR2(40),started_at TIMESTAMP WITH TIME ZONE);
SQL
  for entry in transfer:8 transfer:32 transfer:64 transfer_hot:32 transfer_hot:64 history:8 balance:8 lookup:8 outbox:1; do
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
