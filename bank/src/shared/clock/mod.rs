//! The simulated clock (spec F-25 to F-28). Business time is read once per
//! operation. Stage 1 runs in live mode only: business time follows the wall
//! clock. Backfill mode and the operation barrier arrive in stage 2.

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

use crate::shared::db::{Db, DbError, SqlStmt};
use crate::sql_stmt;

const SELECT_CLOCK: SqlStmt = sql_stmt!("clock.select", "sql/select_clock.sql");
const INIT_CLOCK: SqlStmt = sql_stmt!("clock.init", "sql/init_clock.sql");

/// Korea Standard Time, used for business dates (spec: daily limits, close).
pub fn kst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("valid offset")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClockMode {
    Backfill,
    Live,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClockState {
    pub mode: ClockMode,
    pub business_time: DateTime<FixedOffset>,
}

#[derive(Clone)]
pub struct Clock {
    db: Db,
}

impl Clock {
    /// Ensures the clock row exists (live mode on first start).
    pub async fn init(db: Db) -> Result<Self, DbError> {
        db.run(|s| {
            s.execute(&INIT_CLOCK, &[])?;
            s.commit()
        })
        .await?;
        Ok(Self { db })
    }

    pub async fn state(&self) -> Result<ClockState, DbError> {
        let (mode, stored): (String, DateTime<FixedOffset>) =
            self.db.run(|s| s.query_row(&SELECT_CLOCK, &[])).await?;
        let mode = if mode == "BACKFILL" {
            ClockMode::Backfill
        } else {
            ClockMode::Live
        };
        let business_time = match mode {
            ClockMode::Live => Utc::now().with_timezone(&kst()),
            ClockMode::Backfill => stored.with_timezone(&kst()),
        };
        Ok(ClockState {
            mode,
            business_time,
        })
    }

    /// Business time for one operation, read once at its start.
    pub async fn now(&self) -> Result<DateTime<FixedOffset>, DbError> {
        Ok(self.state().await?.business_time)
    }
}

/// The KST business date of a business time.
pub fn business_date(t: DateTime<FixedOffset>) -> NaiveDate {
    t.with_timezone(&kst()).date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_date_uses_kst() {
        let utc_evening = DateTime::parse_from_rfc3339("2026-03-01T16:30:00Z").expect("time");
        assert_eq!(
            business_date(utc_evening),
            NaiveDate::from_ymd_opt(2026, 3, 2).expect("date")
        );
    }
}
