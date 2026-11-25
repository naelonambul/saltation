//! Demand-account dormancy, run at the end of each business date.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate};

use crate::shared::db::{Db, DbError, SqlStmt};
use crate::shared::ledger;
use crate::shared::rollover::{JobFuture, RolloverJob};
use crate::sql_stmt;

const CANDIDATES: SqlStmt = sql_stmt!("accounts_lifecycle.candidates", "sql/candidates.sql");
const ELIGIBLE: SqlStmt = sql_stmt!("accounts_lifecycle.eligible", "sql/eligible.sql");
const DORMANCY_BATCH_SIZE: usize = 200;

pub struct DormancyJob;

impl RolloverJob for DormancyJob {
    fn name(&self) -> &'static str {
        "dormancy"
    }

    fn run<'a>(&'a self, db: &'a Db, date: NaiveDate, at: DateTime<FixedOffset>) -> JobFuture<'a> {
        Box::pin(async move {
            let cutoff = (date - chrono::Duration::days(365) + chrono::Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .expect("midnight")
                - chrono::Duration::hours(9);
            let ids: Vec<i64> = db
                .run(move |s| s.query_all(&CANDIDATES, &[&cutoff]))
                .await?;
            for chunk in ids.chunks(DORMANCY_BATCH_SIZE) {
                let batch = chunk.to_vec();
                db.run(move |s| {
                    for id in batch {
                        let mut locks = ledger::lock_accounts(s, &[id])?;
                        let eligible: i64 = s.query_row(&ELIGIBLE, &[&id, &cutoff])?;
                        if eligible == 1 {
                            ledger::set_status(
                                s, &mut locks, id, "DORMANT", "dormancy", at, None, "SYSTEM",
                            )
                            .map_err(|e| match e {
                                ledger::LedgerError::Db(e) => e,
                                other => DbError::Config(other.to_string()),
                            })?;
                        }
                    }
                    s.commit()
                })
                .await?;
            }
            Ok(())
        })
    }
}

pub fn jobs() -> Vec<Arc<dyn RolloverJob>> {
    vec![Arc::new(DormancyJob)]
}
