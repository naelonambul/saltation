#!/bin/bash
# Sets the Saltation schema password from the container environment.
set -euo pipefail
: "${SALTATION_DB_PASSWORD:?SALTATION_DB_PASSWORD must be set}"
"$ORACLE_HOME"/bin/sqlplus -s "/ as sysdba" <<SQL
WHENEVER SQLERROR EXIT SQL.SQLCODE
ALTER SESSION SET CONTAINER = ORCLPDB1;
ALTER USER saltation IDENTIFIED BY "${SALTATION_DB_PASSWORD}";
EXIT
SQL

# Setup is complete: the image's own health check reports healthy while the
# database is still being created, so readiness is this marker plus that check.
touch /opt/oracle/oradata/.saltation-ready
