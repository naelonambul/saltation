# Sourced by it-oracle.sh; uses its compose, root and cargo_test functions.
# Capture uses SYSDBA, so diagnostic privileges never become application grants.
tune_sql() {
  { printf '%s\n' 'WHENEVER SQLERROR EXIT FAILURE' 'WHENEVER OSERROR EXIT FAILURE' \
      'SET VERIFY OFF ECHO OFF PAGES 50000 LINES 240 TRIMSPOOL ON' \
      'ALTER SESSION SET CONTAINER=ORCLPDB1;' 'ALTER SESSION SET CURRENT_SCHEMA=SALTATION_TUNE;'; cat; printf '\nEXIT\n'; } \
    | "${compose[@]}" exec -T -e ORACLE_SID=ORCLCDB -e NLS_LANG=American_America.AL32UTF8 oracle sqlplus -s / as sysdba
}

tune_seed() {
  local customers="${TUNE_CUSTOMERS:-20000}" years="${TUNE_YEARS:-3}" max_gb="${TUNE_MAX_GB:-25}" value docker_root available output
  for value in "$customers" "$years" "$max_gb"; do
    case "$value" in ''|*[!0-9]*|0) echo 'tune-seed: parameters must be positive integers' >&2; return 2 ;; esac
  done
  [ "$customers" -ge 2 ] && [ "$customers" -le 1000000 ] && [ "$years" -le 20 ] && [ "$max_gb" -le 25 ] \
    || { echo 'tune-seed: require 2..1000000 customers, 1..20 years, TUNE_MAX_GB <=25' >&2; return 2; }
  docker_root=$(docker info -f '{{.DockerRootDir}}')
  available=$(df -Pk "$docker_root" | awk 'END {print $4}')
  [ "$available" -ge 31457280 ] || { echo 'tune-seed: Docker data filesystem has less than 30 GB free; aborting before schema reset' >&2; return 1; }
  output="$root/.evidence/oracle-tuning/$(date -u +%Y%m%dT%H%M%SZ)-seed"
  mkdir -p "$output"
  # Password is inherited by name, never a process argument or console output.
  if ! { printf 'SET VERIFY OFF ECHO OFF\nDEFINE pw = "%s"\n' "$SALTATION_DB_PASSWORD"; cat <<'SQL'
WHENEVER SQLERROR EXIT FAILURE
ALTER SESSION SET CONTAINER=ORCLPDB1;
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

tune_capture() (
  local output entry failed=0
  output="$root/.evidence/oracle-tuning/$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$output"
  # Schema logon trigger sets ALL only for capture connections, including pool replacements.
  tune_sql >"$output/statistics.txt" <<'SQL'
BEGIN
  EXECUTE IMMEDIATE 'DROP TABLE saltation_tune.tune_windows PURGE';
EXCEPTION WHEN OTHERS THEN IF SQLCODE<>-942 THEN RAISE; END IF;
END;
/
CREATE TABLE saltation_tune.tune_windows(workload VARCHAR2(20),concurrency NUMBER,run NUMBER,started_at TIMESTAMP WITH TIME ZONE,ended_at TIMESTAMP WITH TIME ZONE,PRIMARY KEY(workload,concurrency,run));
CREATE OR REPLACE TRIGGER saltation_tune.tune_statistics AFTER LOGON ON saltation_tune.SCHEMA
BEGIN EXECUTE IMMEDIATE 'ALTER SESSION SET STATISTICS_LEVEL=ALL'; END;
/
SQL
  trap 'printf "DROP TRIGGER saltation_tune.tune_statistics;\n" | tune_sql >"$output/statistics-cleanup.txt"' EXIT
  for entry in transfer:8 transfer:32 transfer:64 history:8 balance:8 lookup:8 outbox:1; do
    BENCH_CAPTURE=1 BENCH_POPULATED=1 BENCH_ORA_SNAP=1 BENCH_DRIVER="${BENCH_DRIVER:-check}" BENCH_RUNS="${BENCH_RUNS:-1}" BENCH_ONLY="$entry" BENCH_OUTPUT="$output" driver_bench || { failed=1; break; }
    if [[ "$entry" = transfer:* ]]; then
      python3 "$root/scripts/dev/oracle-tune/capture.py" "$root" "${entry/:/-}" ash | tune_sql >"$output/ash-${entry/:/-}.txt" || { failed=1; break; }
    fi
    # Capture before another workload can age these cursors out.
    python3 "$root/scripts/dev/oracle-tune/capture.py" "$root" "${entry/:/-}" xplan | tune_sql >"$output/xplan-${entry/:/-}.txt" || { failed=1; break; }
  done
  { echo "EXEC DBMS_STATS.GATHER_SCHEMA_STATS('SALTATION_TUNE',options=>'GATHER',cascade=>TRUE);"; cat "$root/scripts/dev/oracle-tune/segments.sql"; } | tune_sql >"$output/segments.txt"
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
