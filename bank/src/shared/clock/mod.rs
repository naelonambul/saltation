//! Persisted business time and the bank-wide operation barrier.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

use crate::shared::db::{Db, DbError, SqlStmt};
use crate::shared::rollover::Registry;
use crate::sql_stmt;

const SELECT_CLOCK: SqlStmt = sql_stmt!("clock.select", "sql/select_clock.sql");
const INIT_CLOCK: SqlStmt = sql_stmt!("clock.init", "sql/init_clock.sql");
const INIT_ROLLED: SqlStmt = sql_stmt!("clock.init_rolled", "sql/init_rolled.sql");
const SET_TIME: SqlStmt = sql_stmt!("clock.set_time", "sql/set_time.sql");
const SET_BACK: SqlStmt = sql_stmt!("clock.set_back", "sql/set_back.sql");
const SET_LIVE: SqlStmt = sql_stmt!("clock.set_live", "sql/set_live.sql");
const FORGET_ROLLOVER: SqlStmt = sql_stmt!("clock.forget_rollover", "sql/forget_rollover.sql");
const HAS_CUSTOMER: SqlStmt = sql_stmt!("clock.has_customer", "sql/has_customer.sql");

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
    pub rolled_through: NaiveDate,
    #[serde(skip)]
    pub pending_rollover: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ClockError {
    #[error("clock is not in backfill mode")]
    NotBackfill,
    #[error("cannot move backward with customers")]
    Backward,
    #[error("wall time is behind business time")]
    WallBehind,
    #[error("operations did not finish in time")]
    Busy,
    #[error(transparent)]
    Db(#[from] DbError),
}

pub struct Ticket {
    pub now: DateTime<FixedOffset>,
    _guard: OwnedRwLockReadGuard<()>,
}

#[derive(Clone)]
pub struct Clock {
    db: Db,
    registry: Registry,
    barrier: Arc<RwLock<()>>,
    barrier_wait: Duration,
}

impl Clock {
    pub async fn init(db: Db, registry: Registry) -> Result<Self, DbError> {
        db.run(|s| {
            s.execute(&INIT_CLOCK, &[])?;
            s.execute(&INIT_ROLLED, &[])?;
            s.commit()
        })
        .await?;
        Ok(Self {
            db,
            registry,
            barrier: Arc::new(RwLock::new(())),
            barrier_wait: Duration::from_secs(10),
        })
    }

    pub fn with_barrier_wait(mut self, d: Duration) -> Self {
        self.barrier_wait = d;
        self
    }

    pub async fn state(&self) -> Result<ClockState, DbError> {
        let (mode, stored, rolled, pending): (String, DateTime<FixedOffset>, String, i64) =
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
        let rolled_through = NaiveDate::parse_from_str(&rolled, "%Y-%m-%d")
            .map_err(|e| DbError::Config(format!("rolled_through: {e}")))?;
        Ok(ClockState {
            mode,
            business_time,
            rolled_through,
            pending_rollover: pending > 0,
        })
    }

    pub async fn enter(&self) -> Result<Ticket, DbError> {
        let mut caught_up = false;
        loop {
            let guard = self.barrier.clone().read_owned().await;
            let state = self.state().await?;
            let overdue =
                business_date(state.business_time) > state.rolled_through.succ_opt().expect("date");
            if state.mode == ClockMode::Live && (state.pending_rollover || overdue) && !caught_up {
                drop(guard);
                if let Err(error) = self.catch_up().await {
                    if self.state().await?.pending_rollover {
                        return Err(DbError::Busy);
                    }
                    return Err(error);
                }
                caught_up = true;
                continue;
            }
            if state.pending_rollover
                || (state.mode == ClockMode::Backfill
                    && business_date(state.business_time) <= state.rolled_through)
                || (state.mode == ClockMode::Live && overdue)
            {
                drop(guard);
                return Err(DbError::Busy);
            }
            return Ok(Ticket {
                now: state.business_time,
                _guard: guard,
            });
        }
    }

    async fn roll_to(
        &self,
        rolled: NaiveDate,
        target: DateTime<FixedOffset>,
    ) -> Result<(), DbError> {
        let mut date = rolled.succ_opt().expect("date");
        while date < business_date(target) {
            self.registry.roll_date(&self.db, date).await?;
            date = date.succ_opt().expect("date");
        }
        Ok(())
    }

    pub async fn advance(&self, seconds: i64) -> Result<ClockState, ClockError> {
        let clock = self.clone();
        tokio::spawn(async move {
            let _guard = tokio::time::timeout(clock.barrier_wait, clock.barrier.write())
                .await
                .map_err(|_| ClockError::Busy)?;
            let state = clock.state().await?;
            if state.mode != ClockMode::Backfill {
                return Err(ClockError::NotBackfill);
            }
            if seconds < 1 {
                return Err(ClockError::Backward);
            }
            let delta = chrono::Duration::try_seconds(seconds).ok_or(ClockError::Backward)?;
            let target = state
                .business_time
                .checked_add_signed(delta)
                .ok_or(ClockError::Backward)?;
            clock.set_locked(state, target).await
        })
        .await
        .map_err(|_| ClockError::Db(DbError::Config("clock task failed".into())))?
    }

    pub async fn set(&self, to: DateTime<FixedOffset>) -> Result<ClockState, ClockError> {
        let clock = self.clone();
        tokio::spawn(async move {
            let _guard = tokio::time::timeout(clock.barrier_wait, clock.barrier.write())
                .await
                .map_err(|_| ClockError::Busy)?;
            let state = clock.state().await?;
            if state.mode != ClockMode::Backfill {
                return Err(ClockError::NotBackfill);
            }
            clock.set_locked(state, to.with_timezone(&kst())).await
        })
        .await
        .map_err(|_| ClockError::Db(DbError::Config("clock task failed".into())))?
    }

    async fn set_locked(
        &self,
        state: ClockState,
        to: DateTime<FixedOffset>,
    ) -> Result<ClockState, ClockError> {
        if to < state.business_time {
            let customers: i64 = self.db.run(|s| s.query_row(&HAS_CUSTOMER, &[])).await?;
            if customers > 0 {
                return Err(ClockError::Backward);
            }
            let rolled = business_date(to).pred_opt().expect("date").to_string();
            self.db
                .run(move |s| {
                    s.execute(&SET_BACK, &[&to, &rolled])?;
                    s.execute(&FORGET_ROLLOVER, &[&rolled])?;
                    s.commit()
                })
                .await?;
        } else {
            self.roll_to(state.rolled_through, to).await?;
            self.db
                .run(move |s| {
                    s.execute(&SET_TIME, &[&to])?;
                    s.commit()
                })
                .await?;
        }
        Ok(self.state().await?)
    }

    pub async fn go_live(&self) -> Result<ClockState, ClockError> {
        let clock = self.clone();
        tokio::spawn(async move {
            let _guard = clock.barrier.write().await;
            let state = clock.state().await?;
            if state.mode == ClockMode::Live {
                return Ok(state);
            }
            let wall = Utc::now().with_timezone(&kst());
            if !live_switch_allowed(state.business_time, wall) {
                return Err(ClockError::WallBehind);
            }
            clock.roll_to(state.rolled_through, wall).await?;
            clock
                .db
                .run(|s| {
                    s.execute(&SET_LIVE, &[])?;
                    s.commit()
                })
                .await?;
            Ok(clock.state().await?)
        })
        .await
        .map_err(|_| ClockError::Db(DbError::Config("clock task failed".into())))?
    }

    /// Rolls the dates the wall clock has left (live mode). Takes the
    /// exclusive barrier only when a date is due, so an idle check never
    /// stalls requests.
    pub async fn catch_up(&self) -> Result<(), DbError> {
        let clock = self.clone();
        tokio::spawn(async move {
            let state = clock.state().await?;
            if state.mode != ClockMode::Live
                || (!state.pending_rollover
                    && business_date(state.business_time)
                        <= state.rolled_through.succ_opt().expect("date"))
            {
                return Ok(());
            }
            let _guard = clock.barrier.write().await;
            let state = clock.state().await?;
            if state.mode == ClockMode::Live {
                clock
                    .roll_to(state.rolled_through, Utc::now().with_timezone(&kst()))
                    .await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| DbError::Config("clock task failed".into()))?
    }

    pub fn spawn_live_scheduler(&self) {
        let clock = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(error) = clock.catch_up().await {
                    tracing::error!(%error, "live clock rollover failed");
                }
            }
        });
    }
}

pub fn live_switch_allowed(
    business_time: DateTime<FixedOffset>,
    wall: DateTime<FixedOffset>,
) -> bool {
    wall >= business_time
}

pub fn business_date(t: DateTime<FixedOffset>) -> NaiveDate {
    t.with_timezone(&kst()).date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_date_uses_kst() {
        let t = DateTime::parse_from_rfc3339("2026-03-01T16:30:00Z").expect("time");
        assert_eq!(
            business_date(t),
            NaiveDate::from_ymd_opt(2026, 3, 2).expect("date")
        );
    }

    #[test]
    fn live_switch_checks_wall_time() {
        let t = DateTime::parse_from_rfc3339("2026-03-01T12:00:00+09:00").expect("time");
        assert!(!live_switch_allowed(t, t - chrono::Duration::seconds(1)));
        assert!(live_switch_allowed(t, t));
        assert!(live_switch_allowed(t, t + chrono::Duration::seconds(1)));
    }

    #[test]
    fn last_instant_is_microsecond_before_kst_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("date");
        assert_eq!(
            crate::shared::rollover::last_instant(date).to_rfc3339(),
            "2026-03-01T23:59:59.999999+09:00"
        );
    }
}
