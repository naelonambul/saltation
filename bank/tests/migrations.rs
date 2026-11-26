//! Step 3 proof: migrations apply to an empty schema, are recorded, and a
//! second run applies nothing. Needs a reset schema (scripts/dev/it-oracle.sh).

use bank::shared::db::{Db, DbConfig, MIGRATIONS, SqlStmt};
use bank::sql_stmt;

#[path = "migrations_tuning/mod.rs"]
mod tuning;

const COUNT_MIGRATIONS: SqlStmt = sql_stmt!("test.count_migrations", "sql/count_migrations.sql");
const OUTBOX_TABLESPACE: SqlStmt = sql_stmt!("test.outbox_tablespace", "sql/outbox_tablespace.sql");
const COUNT_TABLES: SqlStmt = sql_stmt!("test.count_tables", "sql/count_tables.sql");

const COUNT_PERFORMANCE_INDEXES: SqlStmt = sql_stmt!(
    "test.count_performance_indexes",
    "sql/count_performance_indexes.sql"
);

const INSERT_REQUEST_ATTEMPT: SqlStmt = sql_stmt!(
    "test.insert_request_attempt",
    "sql/insert_request_attempt.sql"
);

#[tokio::test]
async fn migrations_apply_once_and_are_recorded() {
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");

    assert_eq!(
        MIGRATIONS.iter().map(|m| m.version).collect::<Vec<_>>(),
        (1..=22).collect::<Vec<_>>()
    );
    let first = db.migrate().await.expect("first run");
    assert_eq!(
        first,
        MIGRATIONS.len(),
        "an empty schema applies every migration"
    );

    let objects: (i64, i64) = db.run(|s| s.query_row(&ADMIN_SCHEMA, &[])).await.unwrap();
    assert_eq!(objects, (1, 2), "analyst table and customer name index");

    let second = db.migrate().await.expect("second run");
    assert_eq!(second, 0, "a second run applies nothing");

    db.run(|s| {
        let columns = sql_stmt!("test.tune_index_columns", "sql/tune_index_columns.sql");
        for (index, expected) in [
            (
                "PASSWORD_RESET_CUSTOMER_IX",
                vec!["CUSTOMER_ID", "RESET_ID"],
            ),
            ("DAILY_CLOSE_PK", vec!["BUSINESS_DATE", "ACCOUNT_ID"]),
        ] {
            let actual: Vec<String> = s.query_all(&columns, &[&index])?;
            assert_eq!(actual, expected, "{index}");
        }
        let compression: (String, i64, String) = s.query_row(
            &sql_stmt!(
                "test.tune_index_compression",
                "sql/tune_index_compression.sql"
            ),
            &[],
        )?;
        assert_eq!(compression, ("ENABLED".into(), 1, "UNIQUE".into()));
        let removed: i64 = s.query_row(
            &sql_stmt!("test.tune_removed_indexes", "sql/tune_removed_indexes.sql"),
            &[],
        )?;
        assert_eq!(removed, 0);
        Ok(())
    })
    .await
    .expect("tuned index definitions");

    let (recorded, outbox_ts, tables) = db
        .run(|s| {
            let recorded: i64 = s.query_row(&COUNT_MIGRATIONS, &[])?;
            let outbox_ts: String = s.query_row(&OUTBOX_TABLESPACE, &[])?;
            let tables: i64 = s.query_row(&COUNT_TABLES, &[])?;
            let indexes: i64 = s.query_row(&COUNT_PERFORMANCE_INDEXES, &[])?;
            assert_eq!(indexes, 3, "all performance indexes exist");
            Ok((recorded, outbox_ts, tables))
        })
        .await
        .expect("inspect");
    db.run(|s| {
        let column: (String, i64, String) = s.query_row(
            &sql_stmt!(
                "test.reused_request_column",
                "sql/reused_request_column.sql"
            ),
            &[],
        )?;
        assert_eq!(column, ("VARCHAR2".into(), 26, "Y".into()));
        let absent: Option<&str> = None;
        for id in ["migration14-rejected-1", "migration14-rejected-2"] {
            s.execute(
                &INSERT_REQUEST_ATTEMPT,
                &[&id, &absent, &absent, &absent, &"REJECTED"],
            )?;
        }
        let done = s
            .execute(
                &INSERT_REQUEST_ATTEMPT,
                &[&"migration14-done", &absent, &absent, &absent, &"DONE"],
            )
            .expect_err("DONE must claim idempotency");
        assert_eq!(done.ora_code(), Some(2290));

        // Every partial-null combination must fail, regardless of which field is absent.
        for mask in 1..7 {
            let scope = (mask & 1 != 0).then_some("migration14");
            let key = (mask & 2 != 0).then_some("partial");
            let hash = (mask & 4 != 0).then_some("x");
            let partial = s
                .execute(
                    &INSERT_REQUEST_ATTEMPT,
                    &[&"migration14-partial", &scope, &key, &hash, &"REJECTED"],
                )
                .expect_err("a partial idempotency claim must fail");
            assert_eq!(partial.ora_code(), Some(2290));
        }
        s.execute(
            &INSERT_REQUEST_ATTEMPT,
            &[
                &"migration14-claimed-1",
                &"migration14",
                &"claimed",
                &"x",
                &"DONE",
            ],
        )?;
        let duplicate = s
            .execute(
                &INSERT_REQUEST_ATTEMPT,
                &[
                    &"migration14-claimed-2",
                    &"migration14",
                    &"claimed",
                    &"x",
                    &"REJECTED",
                ],
            )
            .expect_err("a claimed idempotency key must remain unique");
        assert_eq!(duplicate.ora_code(), Some(1));
        s.rollback()
    })
    .await
    .expect("request attempt constraints");

    db.run(|s| {
        let tables: Vec<String> =
            s.query_all(&sql_stmt!("test.atm_tables", "sql/atm_tables.sql"), &[])?;
        assert_eq!(tables, ["ATM_CREDENTIAL", "ATM_SESSION", "ATM_TERMINAL"]);
        s.execute(&sql_stmt!("test.atm_fixture", "sql/atm_fixture.sql"), &[])?;
        let insert = sql_stmt!("test.atm_insert_session", "sql/atm_insert_session.sql");
        s.execute(
            &insert,
            &[
                &"00000000000000000000000001",
                &"hash-1",
                &"00000000000000000000000001",
            ],
        )?;
        let duplicate = s
            .execute(
                &insert,
                &[
                    &"00000000000000000000000002",
                    &"hash-2",
                    &"00000000000000000000000002",
                ],
            )
            .expect_err("one open ATM session per terminal");
        assert_eq!(duplicate.ora_code(), Some(1));
        s.rollback()
    })
    .await
    .expect("ATM tables and open-session uniqueness");

    db.run(|s| {
        let tables: Vec<String> =
            s.query_all(&sql_stmt!("test.card_tables", "sql/card_tables.sql"), &[])?;
        assert_eq!(tables, ["CARD", "CARD_AUTHORIZATION"]);
        let indexes: Vec<String> =
            s.query_all(&sql_stmt!("test.card_indexes", "sql/card_indexes.sql"), &[])?;
        assert_eq!(
            indexes,
            [
                "CARD_ACCOUNT_IX",
                "CARD_AUTHORIZATION_CARD_IX",
                "CARD_NO_UK"
            ]
        );
        let kind: String = s.query_row(&sql_stmt!("test.card_kind", "sql/card_kind.sql"), &[])?;
        for value in [
            "transfer",
            "inbound",
            "atm_cash",
            "term_open",
            "term_close",
            "loan_disburse",
            "loan_repay",
            "card_payment",
        ] {
            assert!(
                kind.contains(&format!("'{value}'")),
                "missing {value}: {kind}"
            );
        }
        let nullable: String = s.query_row(
            &sql_stmt!(
                "test.transfer_kind_nullable",
                "sql/transfer_kind_nullable.sql"
            ),
            &[],
        )?;
        assert_eq!(nullable, "N", "transfer.kind must remain NOT NULL");
        s.execute(&sql_stmt!("test.card_fixture", "sql/card_fixture.sql"), &[])?;
        let null_kind = s
            .execute(
                &sql_stmt!("test.transfer_null_kind", "sql/transfer_null_kind.sql"),
                &[],
            )
            .expect_err("transfer.kind rejects NULL");
        assert_eq!(null_kind.ora_code(), Some(1400));
        let wrong_prefix = s
            .execute(
                &sql_stmt!("test.card_wrong_prefix", "sql/card_wrong_prefix.sql"),
                &[],
            )
            .expect_err("card_no_ck rejects another prefix");
        assert_eq!(wrong_prefix.ora_code(), Some(2290));
        assert!(wrong_prefix.to_string().contains("CARD_NO_CK"));
        let duplicate = s
            .execute(
                &sql_stmt!("test.card_duplicate", "sql/card_duplicate.sql"),
                &[],
            )
            .expect_err("card number must be unique");
        assert_eq!(duplicate.ora_code(), Some(1));
        let insert = sql_stmt!("test.card_authorization", "sql/card_authorization.sql");
        let none: Option<&str> = None;
        for (mode, lat, lon, valid) in [
            ("online", none, none, true),
            ("online", Some("0"), none, false),
            ("online", none, Some("0"), false),
            ("online", Some("0"), Some("0"), false),
            ("in_person", none, none, false),
            ("in_person", Some("0"), none, false),
            ("in_person", none, Some("0"), false),
            ("in_person", Some("-90"), Some("180"), true),
            ("in_person", Some("90"), Some("-180"), true),
            ("in_person", Some("90.000001"), Some("0"), false),
            ("in_person", Some("0"), Some("180.000001"), false),
        ] {
            let result = s.execute(&insert, &[&mode, &lat, &lon]);
            if valid {
                result?;
                s.execute(
                    &sql_stmt!(
                        "test.card_delete_authorization",
                        "sql/card_delete_authorization.sql"
                    ),
                    &[],
                )?;
            } else {
                assert_eq!(
                    result.expect_err("coordinate constraint").ora_code(),
                    Some(2290)
                );
            }
        }
        s.rollback()
    })
    .await
    .expect("card schema constraints");

    assert_eq!(recorded, MIGRATIONS.len() as i64);
    assert_eq!(
        outbox_ts, "OUTBOX_TS/OUTBOX_TS",
        "outbox and its index live in the capped tablespace"
    );
    assert!(
        tables >= 32,
        "expected the stage-1 and stage-2 tables, found {tables}"
    );
}

const ADMIN_SCHEMA: SqlStmt =
    sql_stmt!("test.admin_schema_objects", "sql/admin_schema_objects.sql");
const ADMIN_SNAPSHOT: SqlStmt = sql_stmt!(
    "test.admin_upgrade_snapshot",
    "sql/admin_upgrade_snapshot.sql"
);
const ADMIN_TOKENS: SqlStmt =
    sql_stmt!("test.admin_upgrade_tokens", "sql/admin_upgrade_tokens.sql");

#[tokio::test]
#[ignore = "requires a populated, isolated backfill schema at migration v20"]
async fn populated_admin_upgrade() {
    use chrono::{DateTime, FixedOffset};

    type Token = (
        String,
        String,
        i64,
        String,
        Option<String>,
        DateTime<FixedOffset>,
        Option<DateTime<FixedOffset>>,
    );
    type Snapshot = (i64, i64, i64, i64, i64);
    let db = Db::connect(&DbConfig::from_env().expect("config")).unwrap();
    let before: Snapshot = db.run(|s| s.query_row(&ADMIN_SNAPSHOT, &[])).await.unwrap();
    let tokens: Vec<Token> = db.run(|s| s.query_all(&ADMIN_TOKENS, &[])).await.unwrap();
    assert_eq!(before.4, 20);
    assert!(before.0 > 0 && before.1 > 0 && before.3 > 0);

    assert_eq!(db.migrate().await.unwrap(), 2);
    let after: Snapshot = db.run(|s| s.query_row(&ADMIN_SNAPSHOT, &[])).await.unwrap();
    assert_eq!(after, (before.0, before.1, before.2, before.3, 22));
    let current: Vec<Token> = db.run(|s| s.query_all(&ADMIN_TOKENS, &[])).await.unwrap();
    assert!(current == tokens, "existing access tokens changed");
    let objects: (i64, i64) = db.run(|s| s.query_row(&ADMIN_SCHEMA, &[])).await.unwrap();
    assert_eq!(objects, (1, 2), "analyst table and customer name index");
    assert_eq!(db.migrate().await.unwrap(), 0);
    db.close().unwrap();
}
