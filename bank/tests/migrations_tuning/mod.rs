//! Index access witnesses for the approved predicate and date-key groups.
use super::*;
use bank::shared::db::Session;

type IndexAccess = (String, Option<String>, Option<String>);

#[tokio::test]
#[ignore = "run before fresh migrations, against the populated integration schema"]
async fn populated_migrations_preserve_data() {
    let db = Db::connect(&DbConfig::from_env().unwrap()).unwrap();
    let snapshot = sql_stmt!(
        "test.tune_migration_snapshot",
        "../sql/tune_migration_snapshot.sql"
    );
    type Snapshot = (i64, i64, i64, i64, i64, i64, i64, i64);
    let before: Snapshot = db.run(move |s| s.query_row(&snapshot, &[])).await.unwrap();
    assert!(
        before.0 > 0 && before.1 > 0 && before.3 > 0 && before.4 > 0 && before.5 > 0,
        "populated migration requires customers, ledger rows, resets, payees and closes"
    );
    assert_eq!(
        db.migrate().await.unwrap(),
        3,
        "apply exactly groups A/B/C on the populated schema"
    );
    let after: Snapshot = db.run(move |s| s.query_row(&snapshot, &[])).await.unwrap();
    assert_eq!(
        before, after,
        "index migration preserves persisted records and balances"
    );
    assert_eq!(db.migrate().await.unwrap(), 0);
    db.close().unwrap();
}

const PREVIOUS: SqlStmt = sql_stmt!("test.tune_previous", "../sql/accounts_tune_previous.sql");
const ACCESS: SqlStmt = sql_stmt!("test.tune_access_plan", "../sql/tune_access_plan.sql");
const REGISTERED: SqlStmt = sql_stmt!(
    "transfers.registered",
    "../../src/features/transfers/sql/registered.sql"
);
const RESET: SqlStmt = sql_stmt!(
    "password_reset.peek_code",
    "../../src/features/password_reset/sql/peek_code.sql"
);
const REPORT: SqlStmt = sql_stmt!(
    "daily_close.report",
    "../../src/features/daily_close/sql/report.sql"
);

fn access(s: &mut Session) -> Result<Vec<IndexAccess>, bank::shared::db::DbError> {
    let (id, child): (String, i64) = s.query_row(&PREVIOUS, &[])?;
    s.query_all(&ACCESS, &[&id, &child])
}

#[tokio::test]
async fn predicates_use_active_payee_and_reset_and_date_keys() {
    let db = Db::connect(&DbConfig::from_env().unwrap()).unwrap();
    db.migrate().await.unwrap();
    db.run(|s| {
        let _: i64 = s.query_row(&REGISTERED, &[&1_i64, &"998", &"960000000001"])?;
        let plan = access(s)?;
        assert!(
            plan.iter()
                .any(|(_, _, index)| index.as_deref() == Some("PAYEE_ACTIVE_UK")),
            "{plan:?}"
        );
        let _: Option<(i64, String)> = s.query_opt(&RESET, &[&"01000000000"])?;
        let plan = access(s)?;
        assert!(
            plan.iter().any(|(_, options, index)| index.as_deref()
                == Some("PASSWORD_RESET_CUSTOMER_IX")
                && options
                    .as_ref()
                    .is_some_and(|options| options.contains("MIN/MAX"))),
            "{plan:?}"
        );
        let _: (i64, i64) = s.query_row(&REPORT, &[&"2026-11-17"])?;
        let plan = access(s)?;
        assert!(
            plan.iter().any(
                |(_, options, index)| index.as_deref() == Some("DAILY_CLOSE_PK")
                    && options
                        .as_ref()
                        .is_some_and(|options| options.contains("RANGE SCAN"))
            ),
            "{plan:?}"
        );
        Ok(())
    })
    .await
    .unwrap();
    db.close().unwrap();
}
