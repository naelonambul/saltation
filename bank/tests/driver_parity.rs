//! Public adapter contracts, exercised before and after the driver replacement.
mod common;

use bank::shared::db::{Db, DbConfig, DbError, Session, SqlStmt};
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

type NullableRow = (Option<String>, Option<i64>, Option<DateTime<FixedOffset>>);

const SETUP: SqlStmt = sql_stmt!("parity.setup", "sql/driver_parity_setup.sql");
const INSERT: SqlStmt = sql_stmt!("parity.insert", "sql/driver_parity_insert.sql");
const RETURNING: SqlStmt = sql_stmt!("parity.returning", "sql/driver_parity_returning.sql");
const READ: SqlStmt = sql_stmt!("parity.read", "sql/driver_parity_read.sql");
const ID: SqlStmt = sql_stmt!("parity.id", "sql/driver_parity_id.sql");
const NULLS: SqlStmt = sql_stmt!("parity.nulls", "sql/driver_parity_nulls.sql");
const COUNT: SqlStmt = sql_stmt!("parity.count", "sql/driver_parity_count.sql");
const CLEAR: SqlStmt = sql_stmt!("parity.clear", "sql/driver_parity_clear.sql");
const STAMP: SqlStmt = sql_stmt!("parity.stamp", "sql/driver_parity_stamp.sql");
const DATE_INSERT: SqlStmt = sql_stmt!("parity.date_insert", "sql/driver_parity_date_insert.sql");
const DATE_READ: SqlStmt = sql_stmt!("parity.date_read", "sql/driver_parity_date_read.sql");
const LOOKUP: SqlStmt = sql_stmt!("parity.lookup", "sql/driver_parity_lookup.sql");
const CURSOR: SqlStmt = sql_stmt!("parity.cursor", "sql/driver_parity_cursor.sql");
const PLAN: SqlStmt = sql_stmt!("parity.plan", "sql/driver_parity_plan.sql");

fn count(s: &mut Session) -> Result<i64, DbError> {
    s.query_row(&COUNT, &[])
}

#[tokio::test]
async fn driver_parity() {
    let api = common::api().await;
    let customer = common::customer(&api, 0).await;
    let db = api.state.db.clone();
    db.run(move |s| {
        let owned = customer.account;
        let borrowed = owned.as_str();
        let id: i64 = s.query_row(&LOOKUP, &[&owned])?;
        assert_eq!(s.query_row::<i64>(&LOOKUP, &[&borrowed])?, id);
        let cursors: Vec<(String, i64)> = s.query_all(&CURSOR, &[])?;
        assert!(!cursors.is_empty(), "lookup cursor missing");
        for (sql_id, child) in cursors {
            let plan = s.query_all::<String>(&PLAN, &[&sql_id, &child])?.join("\n");
            assert!(
                plan.contains("INDEX UNIQUE SCAN") || plan.contains("INDEX RANGE SCAN"),
                "{plan}"
            );
            assert!(!plan.to_uppercase().contains("SYS_OP_C2C"), "{plan}");
        }
        s.execute(&SETUP, &[])?;
        for (key, input, expected) in [
            (
                "before",
                "2026-01-01T23:59:59.999999+09:00",
                "2026-01-01 23:59:59.999999 +09:00",
            ),
            (
                "after",
                "2026-01-02T00:00:00.000001+09:00",
                "2026-01-02 00:00:00.000001 +09:00",
            ),
            ("utc", "2026-01-01T15:00:00.123456+00:00", ""),
        ] {
            let value = DateTime::parse_from_rfc3339(input).unwrap();
            s.execute(&INSERT, &[&key, &42_i64, &value])?;
            let (read, wall, utc, default): (
                DateTime<FixedOffset>,
                String,
                String,
                DateTime<FixedOffset>,
            ) = s.query_row(&STAMP, &[&key])?;
            assert_eq!(read, value);
            assert_eq!(
                utc,
                value
                    .with_timezone(&Utc)
                    .format("%Y-%m-%d %H:%M:%S%.6f")
                    .to_string()
            );
            if key != "utc" {
                assert_eq!(wall, expected);
                assert_eq!(read.offset().local_minus_utc(), 9 * 3600);
            }
            assert!(
                (Utc::now() - default.with_timezone(&Utc))
                    .num_milliseconds()
                    .abs()
                    < 60_000
            );
        }
        let date = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        s.execute(&DATE_INSERT, &[&"date", &date])?;
        assert_eq!(s.query_row::<NaiveDate>(&DATE_READ, &[&"date"])?, date);
        s.execute(&CLEAR, &[])?;
        let no_text = Option::<&str>::None;
        let no_number = Option::<i64>::None;
        let no_stamp = Option::<DateTime<FixedOffset>>::None;
        let stamp = DateTime::parse_from_rfc3339("2026-01-02T00:00:00.000001+09:00").unwrap();
        for key in [None, Some("successive"), None] {
            let number = key.map(|_| 7_i64);
            let time = key.map(|_| stamp);
            s.execute(&INSERT, &[&key, &number, &time])?;
        }
        let rows: Vec<NullableRow> = s.query_all(&NULLS, &[])?;
        assert_eq!(
            rows,
            vec![
                (None, None, None),
                (Some("successive".into()), Some(7), Some(stamp)),
                (None, None, None)
            ]
        );
        s.execute(&CLEAR, &[])?;
        s.execute_batch(
            &INSERT,
            &[
                vec![&no_text, &no_number, &no_stamp],
                vec![&Some("batch_some"), &Some(8_i64), &Some(stamp)],
                vec![&no_text, &no_number, &no_stamp],
            ],
        )?;
        let rows: Vec<NullableRow> = s.query_all(&NULLS, &[])?;
        assert_eq!(
            rows,
            vec![
                (None, None, None),
                (Some("batch_some".into()), Some(8), Some(stamp)),
                (None, None, None)
            ]
        );
        assert_eq!(count(s)?, 3);
        s.execute(&CLEAR, &[])?;
        let id = s.insert_returning_id(&RETURNING, &[&"returned", &99_i64, &stamp])?;
        assert_eq!(
            s.query_row::<(String, i64, DateTime<FixedOffset>)>(&READ, &[&id])?,
            ("returned".into(), 99, stamp)
        );
        assert_eq!(s.query_row::<i64>(&ID, &[&"returned"])?, id);
        assert_eq!(s.query_opt::<i64>(&ID, &[&"missing"])?, None);
        let error = s
            .execute(&INSERT, &[&"returned", &1_i64, &stamp])
            .unwrap_err();
        assert_eq!(error.ora_code(), Some(1));
        s.rollback()?;
        assert_eq!(count(s)?, 0);
        let error = s
            .execute_batch(
                &INSERT,
                &[
                    vec![&"fail_one", &1_i64, &stamp],
                    vec![&"fail_two", &2_i64, &stamp],
                    vec![&"fail_one", &3_i64, &stamp],
                ],
            )
            .unwrap_err();
        assert_eq!(error.ora_code(), Some(1));
        s.rollback()?;
        assert_eq!(count(s)?, 0);
        Ok(())
    })
    .await
    .unwrap();

    // A separate pool guarantees that the observer uses a different session.
    let observer = Db::connect(&DbConfig::from_env().unwrap()).unwrap();
    for (key, commit) in [("rolled_back", false), ("committed", true)] {
        db.run(move |s| {
            s.execute(
                &INSERT,
                &[&key, &1_i64, &Option::<DateTime<FixedOffset>>::None],
            )?;
            if commit {
                s.commit()?;
            } else {
                s.rollback()?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let found = observer
            .run(move |s| s.query_opt::<i64>(&ID, &[&key]))
            .await
            .unwrap();
        assert_eq!(found.is_some(), commit);
    }
    db.run(|s| {
        s.execute(&CLEAR, &[])?;
        s.commit()
    })
    .await
    .unwrap();
}
