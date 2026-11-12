//! Ordered, restartable daily jobs, run under the clock's exclusive barrier.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
use serde_json::json;

use crate::shared::clock::kst;
use crate::shared::db::{Db, DbError, Session, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::sql_stmt;

const FINISHED: SqlStmt = sql_stmt!("rollover.finished", "sql/finished.sql");
const START: SqlStmt = sql_stmt!("rollover.start", "sql/start.sql");
const FINISH: SqlStmt = sql_stmt!("rollover.finish", "sql/finish.sql");
const ROLLED: SqlStmt = sql_stmt!("rollover.rolled", "sql/rolled.sql");

pub type JobFuture<'a> = Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>>;

pub trait RolloverJob: Send + Sync {
    fn name(&self) -> &'static str;
    fn run<'a>(&'a self, db: &'a Db, date: NaiveDate, at: DateTime<FixedOffset>) -> JobFuture<'a>;
}

#[derive(Clone, Default)]
pub struct Registry {
    jobs: Vec<Arc<dyn RolloverJob>>,
}

impl Registry {
    pub fn register(&mut self, job: Arc<dyn RolloverJob>) {
        self.jobs.push(job);
    }

    pub async fn roll_date(&self, db: &Db, date: NaiveDate) -> Result<(), DbError> {
        for job in &self.jobs {
            let name = job.name();
            let date_text = date.to_string();
            let finished: i64 = db
                .run(move |s| s.query_row(&FINISHED, &[&date_text, &name]))
                .await?;
            if finished > 0 {
                continue;
            }
            let date_text = date.to_string();
            db.run(move |s| {
                s.execute(&START, &[&date_text, &name])?;
                s.commit()
            })
            .await?;
            let at = last_instant(date);
            job.run(db, date, at).await?;
            db.run(move |s| {
                mark_finished(s, date, name, at)?;
                s.commit()
            })
            .await?;
        }
        let date = date.to_string();
        db.run(move |s| {
            s.execute(&ROLLED, &[&date])?;
            s.commit()
        })
        .await
    }
}

pub fn last_instant(date: NaiveDate) -> DateTime<FixedOffset> {
    kst()
        .from_local_datetime(&date.and_hms_micro_opt(23, 59, 59, 999_999).expect("time"))
        .single()
        .expect("fixed offset")
}

/// Finish and emit exactly once, in the caller's transaction. Never commits.
pub fn mark_finished(
    s: &mut Session,
    date: NaiveDate,
    job: &str,
    at: DateTime<FixedOffset>,
) -> Result<(), DbError> {
    let date = date.to_string();
    if s.execute(&FINISH, &[&date, &job])? > 0 {
        outbox::record(
            s,
            &Event::system("rollover.job_completed", at, "completed")
                .payload(json!({"business_date": date, "job": job})),
        )?;
    }
    Ok(())
}
