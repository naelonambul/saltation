//! Step 3 proof: migrations apply to an empty schema, are recorded, and a
//! second run applies nothing. Needs a reset schema (scripts/dev/it-oracle.sh).

use bank::shared::db::{Db, DbConfig, MIGRATIONS, SqlStmt};
use bank::sql_stmt;

const COUNT_MIGRATIONS: SqlStmt = sql_stmt!("test.count_migrations", "sql/count_migrations.sql");
const OUTBOX_TABLESPACE: SqlStmt = sql_stmt!("test.outbox_tablespace", "sql/outbox_tablespace.sql");
const COUNT_TABLES: SqlStmt = sql_stmt!("test.count_tables", "sql/count_tables.sql");

#[tokio::test]
async fn migrations_apply_once_and_are_recorded() {
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");

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
            Ok((recorded, outbox_ts, tables))
        })
        .await
        .expect("inspect");
    assert_eq!(recorded, MIGRATIONS.len() as i64);
    assert_eq!(
        outbox_ts, "OUTBOX_TS/OUTBOX_TS",
        "outbox and its index live in the capped tablespace"
    );
    assert!(tables >= 20, "expected the stage-1 tables, found {tables}");
}
