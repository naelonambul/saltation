//! SQL coverage only: invoked by driver_bench's ignored tune_coverage wrapper.
//! Requires an already migrated and populated SALTATION_TUNE schema. No commits or DDL.
use bank::shared::db::{Db, DbConfig, DbError, Session, SqlStmt, ToSql};
use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

type At = DateTime<FixedOffset>;
type TransactionRow = (i64, At, i64, i64, String, String, Option<String>);
type PayeeRow = (i64, String, String, String, String, Option<String>, String);

// Both the statement name and its bytes are the production ones. Report only
// after the adapter has executed (and, for queries, fetched) successfully.
macro_rules! production {
    ($s:expr, $method:ident, $feature:literal, $file:literal, $binds:expr) => {{
        let result = $s.$method(
            &SqlStmt {
                name: concat!($feature, ".", $file),
                sql: include_str!(concat!(
                    "../src/features/",
                    $feature,
                    "/sql/",
                    $file,
                    ".sql"
                )),
            },
            $binds,
        )?;
        capture_plan(
            $s,
            concat!("features/", $feature, "/sql/", $file, ".sql"),
            include_str!(concat!(
                "../src/features/",
                $feature,
                "/sql/",
                $file,
                ".sql"
            )),
        )?;
        println!(concat!(
            "coverage FILE features/",
            $feature,
            "/sql/",
            $file,
            ".sql executed"
        ));
        result
    }};
}

// Call before any other SQL can replace the session's previous cursor.
fn capture_plan(s: &mut Session, path: &str, sql: &str) -> Result<(), DbError> {
    let (id, child): (String, i64) = s.query_row(
        &SqlStmt {
            name: "tune.coverage.previous",
            sql: "SELECT prev_sql_id, prev_child_number FROM v$session WHERE sid = SYS_CONTEXT('USERENV','SID')",
        },
        &[],
    )?;
    let text: String = s.query_row(
        &SqlStmt {
            name: "tune.coverage.text",
            sql: "SELECT sql_text FROM v$sql WHERE sql_id = :id AND child_number = :child",
        },
        &[&id, &child],
    )?;
    let first = sql.lines().next().expect("production SQL first line");
    if !text.starts_with(first) {
        return Err(DbError::Config(format!(
            "previous cursor does not match {path}"
        )));
    }
    let lines: Vec<String> = s.query_all(
        &SqlStmt {
            name: "tune.coverage.plan",
            sql: "SELECT plan_table_output FROM TABLE(DBMS_XPLAN.DISPLAY_CURSOR(:id, :child, 'ALLSTATS LAST +PEEKED_BINDS'))",
        },
        &[&id, &child],
    )?;
    if !lines.iter().any(|line| line.contains("A-Rows")) {
        return Err(DbError::Config(format!(
            "no runtime plan statistics for {path}"
        )));
    }
    let output = std::env::var("TUNE_COVERAGE_OUT")
        .map_err(|_| DbError::Config("TUNE_COVERAGE_OUT is required".into()))?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(output)
        .map_err(|e| DbError::Config(format!("open coverage plans: {e}")))?;
    writeln!(file, "FILE {path}\n{}", lines.join("\n"))
        .map_err(|e| DbError::Config(format!("write coverage plans: {e}")))?;
    Ok(())
}

fn fixture(
    s: &mut Session,
    sql: &'static str,
    binds: &[&dyn bank::shared::db::ToSql],
) -> Result<u64, DbError> {
    s.execute(
        &SqlStmt {
            name: "tune.coverage.fixture",
            sql,
        },
        binds,
    )
}

/// Execute all coverage work on one adapter lease, then explicitly roll back.
/// The adapter also discards/cleans the lease on errors; no helper calls commit.
pub async fn run() {
    let output = std::env::var("TUNE_COVERAGE_OUT").expect("TUNE_COVERAGE_OUT is required");
    std::fs::File::create(output).expect("create coverage plans");
    let mut config = DbConfig::from_env().expect("coverage database configuration");
    config.user = "SALTATION_TUNE".into();
    let db = Db::connect(&config).expect("connect SALTATION_TUNE");
    let result = db
        .run(|s| {
            let work = cover(s);
            // Do this even when a statement fails, preserving its original error.
            let rollback = s.rollback();
            work.and(rollback)
        })
        .await;
    db.close().expect("close coverage pool");
    result.expect("production SQL coverage (fixtures rolled back)");
}

fn cover(s: &mut Session) -> Result<(), DbError> {
    s.execute(
        &SqlStmt {
            name: "tune.coverage.statistics",
            sql: "ALTER SESSION SET STATISTICS_LEVEL=ALL",
        },
        &[],
    )?;
    let (customer, phone, hash): (i64, String, String) = s.query_row(&SqlStmt {
        name: "tune.coverage.customer",
        sql: "SELECT c.customer_id, c.phone, cr.password_hash FROM customer c JOIN customer_credential cr ON cr.customer_id = c.customer_id ORDER BY c.customer_id FETCH FIRST 1 ROW ONLY",
    }, &[])?;
    let now: At = s.query_row(
        &SqlStmt {
            name: "tune.coverage.clock",
            sql: "SELECT business_time FROM sim_clock WHERE id = 'X'",
        },
        &[],
    )?;
    let date = now.date_naive();
    let start = now
        .timezone()
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .expect("fixed offset");
    let end = start + Duration::days(1);
    let old = now - Duration::days(730);
    let expires = now + Duration::minutes(5);
    let unique = ulid::Ulid::new().to_string();
    let account_no = format!("9{}", &unique[11..]);
    let terminal = format!("TC{}", &unique[8..]);
    let token_hash = format!("coverage-{unique}");
    let account: i64 = s.insert_returning_id(&SqlStmt {
        name: "tune.coverage.account",
        sql: "INSERT INTO account (account_no, customer_id, terms_id, subtype, status, opened_at) VALUES (:account_no, :customer, 1, 'DEMAND', 'ACTIVE', :at) RETURNING account_id INTO :id",
    }, &[&account_no, &customer, &old])?;

    // Malformed one-leg and empty transfers exercise the single daily-close
    // aggregation and account marking, without touching live rows.
    let transfer: i64 = s.insert_returning_id(&SqlStmt {
        name: "tune.coverage.transfer",
        sql: "INSERT INTO transfer (kind, amount, occurred_at) VALUES ('transfer', 1, :at) RETURNING transfer_id INTO :id",
    }, &[&now])?;
    let empty_transfer: i64 = s.insert_returning_id(&SqlStmt {
        name: "tune.coverage.empty_transfer",
        sql: "INSERT INTO transfer (kind, amount, occurred_at) VALUES ('transfer', 1, :at) RETURNING transfer_id INTO :id",
    }, &[&now])?;
    fixture(
        s,
        "INSERT INTO account_transaction (account_id, account_sequence, amount, balance_after, inout_type, tran_type, print_content, counterparty_bank_code, channel, transfer_id, business_date, occurred_at) VALUES (:account, 1, -1, 0, 'O', 'transfer', 'coverage', '998', 'MOBILE', :transfer, :business_date, :at)",
        &[&account, &transfer, &date, &now],
    )?;
    fixture(
        s,
        "INSERT INTO daily_close (account_id, business_date, opening_balance, closing_balance, verified) VALUES (:account, :business_date, 0, 0, 'Y')",
        &[&account, &date],
    )?;
    let rows: Vec<TransactionRow> = production!(
        s,
        query_all,
        "accounts",
        "select_transactions_asc",
        &[
            &account,
            &start,
            &end,
            &"A",
            &"A",
            &"A",
            &0i64,
            &i64::MAX,
            &26i64
        ]
    );
    assert_eq!(rows.len(), 1, "ascending history fixture");

    fixture(
        s,
        "UPDATE customer_credential SET failed_count = 4, locked_until = NULL WHERE customer_id = :id",
        &[&customer],
    )?;
    let failures = production!(
        s,
        insert_returning_id,
        "auth",
        "record_failure",
        &[&(now + Duration::minutes(30)), &customer]
    );
    assert_eq!(failures, 5);
    let (_, _, _, stored_failures, locked): (i64, String, String, i64, Option<At>) =
        production!(s, query_row, "auth", "select_login", &[&phone]);
    assert_eq!(stored_failures, failures);
    assert_eq!(locked, Some(now + Duration::minutes(30)));

    fixture(
        s,
        "INSERT INTO atm_terminal (terminal_id, region, location, latitude, longitude) VALUES (:terminal, 'SEOUL', 'coverage', 37.5, 127.0)",
        &[&terminal],
    )?;
    fixture(
        s,
        "INSERT INTO atm_credential (account_no, pin_hash) VALUES (:account, :hash)",
        &[&account_no, &hash],
    )?;
    fixture(
        s,
        "INSERT INTO atm_session (session_id, terminal_id, account_no, token_hash, started_at, expires_at, request_id) VALUES (:id, :terminal, :account, :hash, :at, :expires, :request)",
        &[
            &unique,
            &terminal,
            &account_no,
            &token_hash,
            &now,
            &expires,
            &unique,
        ],
    )?;
    let session: (String, String, At, Option<At>, String, String) =
        production!(s, query_row, "atm", "lock_session", &[&token_hash]);
    assert_eq!(session.0, unique);
    assert_eq!(session.1, account_no);
    assert_eq!(session.2, expires);
    assert!(session.3.is_none());
    assert_eq!(session.4, unique);
    assert_eq!(session.5, terminal);
    let credential: (String, i64, Option<At>) =
        production!(s, query_row, "atm", "lock_credential", &[&account_no]);
    assert_eq!(credential, ("ACTIVE".into(), 0, None));
    let locked: Option<At> = production!(s, query_row, "atm", "select_lock_status", &[&account_no]);
    assert!(locked.is_none());

    let _: (i64, String) =
        production!(s, query_row, "password_reset", "select_customer", &[&phone]);
    production!(
        s,
        execute,
        "password_reset",
        "insert_code",
        &[&customer, &hash, &now, &expires]
    );
    let (reset, _): (i64, String) =
        production!(s, query_row, "password_reset", "peek_code", &[&phone]);
    let selected: (i64, Option<At>, At, i64) =
        production!(s, query_row, "password_reset", "select_code", &[&customer]);
    assert_eq!(selected.0, reset);
    production!(s, execute, "password_reset", "fail_code", &[&reset]);
    production!(
        s,
        execute,
        "password_reset",
        "change_password",
        &[&hash, &now, &customer]
    );
    production!(s, execute, "password_reset", "use_code", &[&now, &reset]);
    production!(
        s,
        execute,
        "password_reset",
        "revoke_tokens",
        &[&now, &customer]
    );
    production!(
        s,
        execute,
        "password_reset",
        "end_sessions",
        &[&now, &customer]
    );
    production!(
        s,
        execute,
        "password_reset",
        "insert_sms",
        &[&phone, &"coverage", &now]
    );
    let _: (String, At) = production!(s, query_row, "password_reset", "select_sms", &[&phone]);

    let payee: i64 = production!(
        s,
        insert_returning_id,
        "payees",
        "insert_payee",
        &[
            &customer,
            &"998",
            &account_no,
            &"coverage",
            &None::<String>,
            &now,
            &"MOBILE",
            &None::<String>
        ]
    );
    for prefix in ["5", "4"] {
        let other = format!("{prefix}{}", &account_no[1..]);
        production!(
            s,
            insert_returning_id,
            "payees",
            "insert_payee",
            &[
                &customer,
                &"998",
                &other,
                &"coverage",
                &None::<String>,
                &now,
                &"MOBILE",
                &None::<String>
            ]
        );
    }
    let payees: Vec<PayeeRow> = production!(s, query_all, "payees", "list_payees", &[&customer]);
    assert!(payees.iter().any(|row| row.0 == payee));
    let _: PayeeRow = production!(s, query_row, "payees", "select_payee", &[&payee, &customer]);
    let _: (String, String, String) =
        production!(s, query_row, "payees", "select_internal", &[&account_no]);
    let (bank, external): (String, String) = s.query_row(
        &SqlStmt {
            name: "tune.coverage.counterparty",
            sql: "SELECT institution_code, account_no FROM counterparty FETCH FIRST 1 ROW ONLY",
        },
        &[],
    )?;
    let _: String = production!(
        s,
        query_row,
        "payees",
        "select_external",
        &[&bank, &external]
    );

    let deposit_no = format!("8{}", &unique[11..]);
    let deposit: i64 = production!(
        s,
        insert_returning_id,
        "deposits",
        "insert_account",
        &[&deposit_no, &customer, &2i64, &now]
    );
    production!(
        s,
        execute,
        "deposits",
        "insert_detail",
        &[&deposit, &account, &1i64, &date]
    );
    for (prefix, maturity) in [
        ("7", date - Duration::days(1)),
        ("6", date + Duration::days(1)),
    ] {
        let number = format!("{prefix}{}", &unique[11..]);
        let extra = production!(
            s,
            insert_returning_id,
            "deposits",
            "insert_account",
            &[&number, &customer, &2i64, &now]
        );
        production!(
            s,
            execute,
            "deposits",
            "insert_detail",
            &[&extra, &account, &1i64, &maturity]
        );
    }
    let deposits: Vec<(i64, i64, i64, String)> =
        production!(s, query_all, "deposits", "candidates", &[&date]);
    assert!(deposits.iter().any(|row| row.0 == deposit));

    let violations: Vec<(i64, String, Option<i64>)> =
        production!(s, query_all, "daily_close", "transfers", &[&start, &end]);
    assert!(violations.iter().any(|row| row.0 == transfer));
    assert_eq!(
        violations
            .iter()
            .filter(|row| row.0 == empty_transfer)
            .count(),
        1,
        "empty malformed transfer counted exactly once"
    );
    assert!(
        violations
            .iter()
            .any(|row| row.0 == empty_transfer && row.2.is_none())
    );
    let mut transfer_ids = BTreeSet::new();
    let mut details = BTreeMap::<i64, String>::new();
    for (id, kind, account) in violations {
        transfer_ids.insert(id);
        if let Some(account) = account {
            let detail = format!("transfer {id}: {kind} rows violate F-21");
            details
                .entry(account)
                .and_modify(|current| {
                    if detail < *current {
                        current.clone_from(&detail);
                    }
                })
                .or_insert(detail);
        }
    }
    assert!(transfer_ids.len() >= 2);
    assert_eq!(
        details.get(&account),
        Some(&format!("transfer {transfer}: transfer rows violate F-21"))
    );
    let date_text = date.to_string();
    let updates: Vec<Vec<&dyn ToSql>> = details
        .iter()
        .map(|(account, detail)| vec![detail as &dyn ToSql, detail, account, &date_text])
        .collect();
    production!(
        s,
        execute_batch,
        "daily_close",
        "mark_transfer_violations",
        &updates
    );
    let (changed, detail): (i64, String) = s.query_row(
        &SqlStmt {
            name: "tune.coverage.marked",
            sql: "SELECT COUNT(*), MIN(mismatch_detail) FROM daily_close WHERE account_id = :account AND business_date = :business_date AND verified = 'N'",
        },
        &[&account, &date],
    )?;
    assert!(changed >= 1);
    assert_eq!(
        detail,
        format!("transfer {transfer}: transfer rows violate F-21")
    );

    // Match DormancyJob's UTC NaiveDateTime bind, including its inclusive date rule.
    let cutoff = (date - Duration::days(365) + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        - Duration::hours(9);
    let dormant: Vec<i64> =
        production!(s, query_all, "accounts_lifecycle", "candidates", &[&cutoff]);
    assert!(dormant.contains(&account));
    let _: i64 = s.query_row(
        &SqlStmt {
            name: "tune.coverage.lock_account",
            sql: "SELECT account_id FROM account WHERE account_id = :id FOR UPDATE",
        },
        &[&account],
    )?;
    let eligible: i64 = production!(
        s,
        query_row,
        "accounts_lifecycle",
        "eligible",
        &[&account, &cutoff]
    );
    assert_eq!(eligible, 1);
    Ok(())
}
