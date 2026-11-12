//! Oracle access: the connection pool, the blocking-work gate, the statement
//! runner and the migration runner. Every SQL statement the bank runs is a
//! named [`SqlStmt`] embedded from a `.sql` file; nothing assembles SQL text.

mod migrations;

use std::sync::Arc;
use std::time::{Duration, Instant};

use oracle::Connection;
use oracle::pool::{CloseMode, GetMode, Pool, PoolBuilder};
use oracle::sql_type::ToSql;
use tokio::sync::Semaphore;

use crate::shared::telemetry::RequestStats;

pub use migrations::{MIGRATIONS, Migration};

/// A named SQL statement loaded from a file at compile time.
/// How long warming the pool at start may take.
const WARM_UP_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy)]
pub struct SqlStmt {
    pub name: &'static str,
    pub sql: &'static str,
}

/// Declares a [`SqlStmt`] from a file next to the calling module.
/// Plain SQL files carry no trailing `;`; PL/SQL blocks keep their `END;`.
#[macro_export]
macro_rules! sql_stmt {
    ($name:literal, $path:literal) => {
        $crate::shared::db::SqlStmt {
            name: $name,
            sql: include_str!($path),
        }
    };
}

/// Connection settings, read from the environment.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub user: String,
    pub password: String,
    pub connect_string: String,
    pub min_sessions: u32,
    pub max_sessions: u32,
    pub stmt_cache_size: u32,
    pub acquire_timeout: Duration,
}

impl DbConfig {
    pub fn from_env() -> Result<Self, DbError> {
        fn var(name: &str) -> Result<String, DbError> {
            std::env::var(name).map_err(|_| DbError::Config(format!("{name} is not set")))
        }
        fn num(name: &str, default: u32) -> Result<u32, DbError> {
            match std::env::var(name) {
                Ok(v) => v
                    .parse()
                    .map_err(|_| DbError::Config(format!("{name} must be a number"))),
                Err(_) => Ok(default),
            }
        }
        Ok(Self {
            user: var("SALTATION_DB_USER")?,
            password: var("SALTATION_DB_PASSWORD")?,
            connect_string: var("SALTATION_DB_CONNECT")?,
            min_sessions: num("SALTATION_DB_MIN_SESSIONS", 4)?,
            max_sessions: num("SALTATION_DB_MAX_SESSIONS", 12)?,
            stmt_cache_size: num("SALTATION_DB_STMT_CACHE", 64)?,
            acquire_timeout: Duration::from_millis(u64::from(num("SALTATION_DB_ACQUIRE_MS", 200)?)),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("configuration: {0}")]
    Config(String),
    #[error("database busy: no session within the acquire timeout")]
    Busy,
    #[error("oracle error in {statement}: {source}")]
    Oracle {
        statement: &'static str,
        source: oracle::Error,
    },
    #[error("oracle error: {0}")]
    Pool(#[from] oracle::Error),
    #[error("blocking task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl DbError {
    /// The ORA- error number, when the failure came from the database.
    pub fn ora_code(&self) -> Option<i32> {
        let source = match self {
            DbError::Oracle { source, .. } | DbError::Pool(source) => source,
            _ => return None,
        };
        source.db_error().map(|e| e.code())
    }
}

/// The bank's handle on Oracle. Cheap to clone.
#[derive(Clone)]
pub struct Db {
    pool: Arc<Pool>,
    gate: Arc<Semaphore>,
    acquire_timeout: Duration,
}

impl Db {
    pub fn connect(config: &DbConfig) -> Result<Self, DbError> {
        // The pool is created empty and warmed here. Oracle client 23 would open a non-zero
        // minimum in background threads (kpuspExtendStartup), and that wait can hang forever
        // when another pool in the process is active.
        let pool = PoolBuilder::new(&config.user, &config.password, &config.connect_string)
            .min_connections(0)
            .max_connections(config.max_sessions)
            .connection_increment(1)
            .stmt_cache_size(config.stmt_cache_size)
            .get_mode(GetMode::TimedWait(config.acquire_timeout))
            .build()?;
        // The get mode cannot change after creation (ORA-24499), so a get that times out
        // while the pool is still opening the session (ORA-24459) is retried.
        let deadline = Instant::now() + WARM_UP_WAIT;
        let mut held: Vec<Connection> = Vec::new();
        while held.len() < config.min_sessions.min(config.max_sessions) as usize {
            match pool.get() {
                Ok(conn) => held.push(conn),
                Err(e)
                    if e.db_error().is_some_and(|d| d.code() == 24459)
                        && Instant::now() < deadline => {}
                Err(e) => return Err(e.into()),
            }
        }
        drop(held);
        Ok(Self {
            pool: Arc::new(pool),
            gate: Arc::new(Semaphore::new(config.max_sessions as usize)),
            acquire_timeout: config.acquire_timeout,
        })
    }

    /// Runs `work` with one pooled connection on a blocking thread. The whole
    /// unit of work, including commit or rollback, happens inside `work`.
    pub async fn run<T, F>(&self, work: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Session) -> Result<T, DbError> + Send + 'static,
    {
        self.run_collect(work).await.0
    }

    /// Like [`Db::run`], adding the statement statistics to a request's totals.
    pub async fn run_in<T, F>(&self, stats: &RequestStats, work: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Session) -> Result<T, DbError> + Send + 'static,
    {
        let (result, collected) = self.run_collect(work).await;
        if let Ok(mut total) = stats.lock() {
            total.merge(&collected);
        }
        result
    }

    async fn run_collect<T, F>(&self, work: F) -> (Result<T, DbError>, StatementStats)
    where
        T: Send + 'static,
        F: FnOnce(&mut Session) -> Result<T, DbError> + Send + 'static,
    {
        let permit =
            match tokio::time::timeout(self.acquire_timeout, self.gate.clone().acquire_owned())
                .await
            {
                Ok(Ok(p)) => p,
                _ => return (Err(DbError::Busy), StatementStats::default()),
            };
        let pool = self.pool.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let conn = match pool.get() {
                Ok(c) => c,
                Err(e) => return (Err(DbError::from(e)), StatementStats::default()),
            };
            let mut session = Session {
                conn,
                stats: StatementStats::default(),
            };
            let result = work(&mut session);
            if result.is_err() {
                let _ = session.conn.rollback();
            }
            (result, session.stats)
        })
        .await;
        match joined {
            Ok(pair) => pair,
            Err(e) => (Err(DbError::Join(e)), StatementStats::default()),
        }
    }

    /// Applies pending migrations in order; returns how many were applied.
    pub async fn migrate(&self) -> Result<usize, DbError> {
        self.run(migrations::apply).await
    }

    pub fn close(&self) -> Result<(), DbError> {
        self.pool.close(&CloseMode::Default)?;
        Ok(())
    }
}

/// Per-request statement statistics for the single operational log line.
#[derive(Debug, Default, Clone)]
pub struct StatementStats {
    pub count: u32,
    pub slowest: Option<(&'static str, Duration)>,
}

impl StatementStats {
    pub fn merge(&mut self, other: &StatementStats) {
        self.count += other.count;
        if let Some((name, d)) = other.slowest
            && self.slowest.is_none_or(|(_, mine)| d > mine)
        {
            self.slowest = Some((name, d));
        }
    }
}

/// One connection checked out for one unit of work.
pub struct Session {
    conn: Connection,
    pub stats: StatementStats,
}

impl Session {
    fn timed<T>(
        &mut self,
        stmt: &SqlStmt,
        f: impl FnOnce(&Connection) -> oracle::Result<T>,
    ) -> Result<T, DbError> {
        let started = Instant::now();
        let result = f(&self.conn);
        let elapsed = started.elapsed();
        metrics::histogram!("bank_db_statement_duration_seconds", "statement" => stmt.name)
            .record(elapsed.as_secs_f64());
        self.stats.count += 1;
        if self.stats.slowest.is_none_or(|(_, d)| elapsed > d) {
            self.stats.slowest = Some((stmt.name, elapsed));
        }
        tracing::debug!(
            statement = stmt.name,
            sql = stmt.sql,
            elapsed_ms = elapsed.as_millis() as u64,
            "sql"
        );
        result.map_err(|source| DbError::Oracle {
            statement: stmt.name,
            source,
        })
    }

    /// Executes a statement that returns no rows; returns the affected row count.
    pub fn execute(&mut self, stmt: &SqlStmt, params: &[&dyn ToSql]) -> Result<u64, DbError> {
        self.timed(stmt, |c| {
            c.execute(stmt.sql, params).and_then(|s| s.row_count())
        })
    }

    /// Inserts one row and reads its RETURNING ID out bind.
    pub fn insert_returning_id(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<i64, DbError> {
        let ids = self.timed(stmt, |c| {
            let mut statement = c.statement(stmt.sql).build()?;
            statement.bind("id", &None::<i64>)?;
            statement.execute(params)?;
            statement.returned_values::<_, i64>("id")
        })?;
        match ids.as_slice() {
            [id] => Ok(*id),
            _ => Err(DbError::Config(format!(
                "{} returned {} IDs, expected one",
                stmt.name,
                ids.len()
            ))),
        }
    }

    /// Executes a query expected to return exactly one row.
    pub fn query_row<T: oracle::RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<T, DbError> {
        self.timed(stmt, |c| c.query_row_as::<T>(stmt.sql, params))
    }

    /// Executes a query expected to return at most one row.
    pub fn query_opt<T: oracle::RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<Option<T>, DbError> {
        match self.query_row(stmt, params) {
            Ok(row) => Ok(Some(row)),
            Err(DbError::Oracle { source, .. })
                if source.kind() == oracle::ErrorKind::NoDataFound =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Executes a query returning any number of rows.
    pub fn query_all<T: oracle::RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<Vec<T>, DbError> {
        self.timed(stmt, |c| c.query_as::<T>(stmt.sql, params)?.collect())
    }

    /// Executes a statement once per row of binds in one round trip (array DML).
    pub fn execute_batch(
        &mut self,
        stmt: &SqlStmt,
        rows: &[Vec<&dyn ToSql>],
    ) -> Result<(), DbError> {
        if rows.is_empty() {
            return Ok(());
        }
        let size = rows.len();
        self.timed(stmt, |c| {
            let mut batch = c.batch(stmt.sql, size).build()?;
            for row in rows {
                batch.append_row(row)?;
            }
            batch.execute()
        })
    }

    pub fn commit(&mut self) -> Result<(), DbError> {
        self.conn.commit()?;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), DbError> {
        self.conn.rollback()?;
        Ok(())
    }
}
