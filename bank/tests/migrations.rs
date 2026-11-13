//! Step 3 proof: migrations apply to an empty schema, are recorded, and a
//! second run applies nothing. Needs a reset schema (scripts/dev/it-oracle.sh).

use bank::shared::db::{Db, DbConfig, MIGRATIONS, SqlStmt};
use bank::sql_stmt;

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
        (1..=14).collect::<Vec<_>>()
    );
    let first = db.migrate().await.expect("first run");
    assert_eq!(
        first,
        MIGRATIONS.len(),
        "an empty schema applies every migration"
    );

    let second = db.migrate().await.expect("second run");
    assert_eq!(second, 0, "a second run applies nothing");

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

    assert_eq!(recorded, MIGRATIONS.len() as i64);
    assert_eq!(
        outbox_ts, "OUTBOX_TS/OUTBOX_TS",
        "outbox and its index live in the capped tablespace"
    );
    assert!(
        tables >= 29,
        "expected the stage-1 and stage-2 tables, found {tables}"
    );
}
