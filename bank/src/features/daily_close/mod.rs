//! Atomic daily ledger reconciliation under the clock's exclusive barrier.
use crate::shared::clock::kst;
use crate::shared::db::{Db, DbError, SqlStmt};
use crate::shared::rollover::{self, JobFuture, RolloverJob};
use crate::sql_stmt;
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
const DELETE: SqlStmt = sql_stmt!("daily_close.delete", "sql/delete.sql");
const INSERT: SqlStmt = sql_stmt!("daily_close.insert", "sql/insert.sql");
const TRANSFERS: SqlStmt = sql_stmt!("daily_close.transfers", "sql/transfers.sql");
const COUNT_TRANSFERS: SqlStmt =
    sql_stmt!("daily_close.count_transfers", "sql/count_transfers.sql");
const REPORT: SqlStmt = sql_stmt!("daily_close.report", "sql/report.sql");

pub struct DailyCloseJob;
#[derive(Debug, PartialEq, Eq)]
pub struct CloseReport {
    pub accounts: i64,
    pub mismatches: i64,
    pub transfer_violations: i64,
}
impl RolloverJob for DailyCloseJob {
    fn name(&self) -> &'static str {
        "daily_close"
    }
    fn run<'a>(&'a self, db: &'a Db, date: NaiveDate, at: DateTime<FixedOffset>) -> JobFuture<'a> {
        Box::pin(async move { close_date(db, date, at).await.map(|_| ()) })
    }
}
/// Replace a date's reconciliation and finish the rollover in the same transaction.
/// The caller must hold the clock barrier so no posting can race this snapshot.
/// Compares against the live stored balance: valid only for the date the clock
/// has just left, or a date with no later postings. A mismatch carries into later
/// dates' opening balances until the data is corrected and those dates re-closed.
pub async fn close_date(
    db: &Db,
    date: NaiveDate,
    at: DateTime<FixedOffset>,
) -> Result<CloseReport, DbError> {
    let start = kst()
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .expect("KST");
    let end = start + chrono::Duration::days(1);
    let date_text = date.to_string();
    let report = db
        .run(move |s| {
            s.execute(&DELETE, &[&date_text])?;
            s.execute(&INSERT, &[&date_text, &start, &end])?;
            s.execute(&TRANSFERS, &[&start, &end, &date_text])?;
            let transfer_violations: i64 = s.query_row(&COUNT_TRANSFERS, &[&start, &end])?;
            let (accounts, mismatches): (i64, i64) = s.query_row(&REPORT, &[&date_text])?;
            rollover::mark_finished(s, date, "daily_close", at)?;
            s.commit()?;
            Ok(CloseReport {
                accounts,
                mismatches,
                transfer_violations,
            })
        })
        .await?;
    let CloseReport {
        accounts,
        mismatches,
        transfer_violations,
    } = report;
    if mismatches + transfer_violations > 0 {
        tracing::warn!(business_date = %date, accounts, mismatches, transfer_violations, "daily close");
    } else {
        tracing::info!(business_date = %date, accounts, mismatches, transfer_violations, "daily close");
    }
    metrics::gauge!("bank_daily_close_mismatches").set((mismatches + transfer_violations) as f64);
    metrics::counter!("bank_daily_close_runs_total").increment(1);
    Ok(report)
}
