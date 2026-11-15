//! Oracle access: the connection pool, the blocking-work gate, the statement
//! runner and the migration runner. Every SQL statement the bank runs is a
//! named [`SqlStmt`] embedded from a `.sql` file; nothing assembles SQL text.

mod migrations;
mod oracle_conn;
mod pool;
mod values;

use std::sync::Arc;
use std::time::{Duration, Instant};

use oracle_conn::OracleConnector;
use oracledb::Connection;
use pool::{Connector, Lease, Pool, PoolError};
use values::with_statement_binds;
pub use values::{RowValue, ToSql};

use crate::shared::telemetry::RequestStats;

pub use migrations::{MIGRATIONS, Migration};

/// A named SQL statement loaded from a file at compile time.
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
    pub call_timeout: Duration,
    pub warm_up_wait: Duration,
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
            call_timeout: Duration::from_millis(u64::from(num(
                "SALTATION_DB_CALL_TIMEOUT_MS",
                30000,
            )?)),
            warm_up_wait: Duration::from_millis(u64::from(num("SALTATION_DB_WARM_UP_MS", 30000)?)),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("configuration: {0}")]
    Config(String),
    #[error("database busy: no session within the acquire timeout")]
    Busy,
    #[error("database connector panicked")]
    Panicked,
    #[error("database pool is closed")]
    Closed,
    #[error("unexpected session time zone: {0}")]
    TimeZone(String),
    #[error("database worker spawn failed: {0}")]
    Spawn(std::io::Error),
    #[error("oracle error in {statement}: {source}")]
    Oracle {
        statement: &'static str,
        source: oracledb::Error,
    },
    #[error("oracle error: {0}")]
    Pool(#[from] oracledb::Error),
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
        source
            .db_error_code()
            .and_then(|code| i32::try_from(code).ok())
    }
}

/// The bank's handle on Oracle. Cheap to clone.
#[derive(Clone)]
pub struct Db {
    pool: Arc<Pool<OracleConnector>>,
    connector: OracleConnector,
    acquire_timeout: Duration,
}

impl Db {
    pub fn connect(config: &DbConfig) -> Result<Self, DbError> {
        let connector = OracleConnector(config.clone());
        let pool = Pool::new(connector.clone(), config.max_sessions as usize);
        if let Err(error) = pool.warm(config.min_sessions as usize, config.warm_up_wait) {
            pool.close();
            return Err(map_pool_error(error));
        }
        Ok(Self {
            pool: Arc::new(pool),
            connector,
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
        let metadata = stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (result, collected) = self
            .run_collect(move |session| {
                session.outcome_endpoint = metadata.outcome_endpoint;
                session.outcome_method = metadata.outcome_method;
                session.outcome_arrival = metadata.outcome_arrival;
                work(session)
            })
            .await;
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
        let lease = match self.pool.acquire(self.acquire_timeout).await {
            Ok(lease) => lease,
            Err(error) => return (Err(map_pool_error(error)), StatementStats::default()),
        };
        let joined = tokio::task::spawn_blocking(move || {
            let mut session = Session::new(SessionConnection::Pooled(lease));
            if let SessionConnection::Pooled(lease) = &mut session.conn {
                lease.mark_blocking();
            }
            let result = work_and_rollback(&mut session, work, Session::rollback);
            let healthy = result.as_ref().is_ok_and(|(_, healthy)| *healthy);
            if let SessionConnection::Pooled(lease) = session.conn {
                lease.release(healthy);
            }
            match result {
                Ok((result, _)) => (result, session.stats),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        })
        .await;
        match joined {
            Ok(pair) => pair,
            Err(e) => (Err(DbError::Join(e)), StatementStats::default()),
        }
    }

    /// Applies pending migrations in order; returns how many were applied.
    pub async fn migrate(&self) -> Result<usize, DbError> {
        let connector = Arc::new(self.connector.clone());
        let conn = pool::open_detached(connector.clone())
            .map_err(DbError::Spawn)?
            .await
            .map_err(|_| DbError::Panicked)?
            .map_err(map_pool_error)?;
        tokio::task::spawn_blocking(move || {
            let mut session = Session::new(SessionConnection::Standalone(conn));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                session.conn.as_ref().set_call_timeout(None)?;
                migrations::apply(&mut session)
            }));
            if let SessionConnection::Standalone(conn) = session.conn {
                pool::close_detached(connector, conn);
            }
            match result {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        })
        .await?
    }

    pub fn close(&self) -> Result<(), DbError> {
        self.pool.close();
        Ok(())
    }
}

/// Per-request statement statistics for the single operational log line.
#[derive(Debug, Default, Clone)]
pub struct StatementStats {
    pub count: u32,
    pub completion_committed: bool,
    pub outcome_endpoint: Option<String>,
    pub outcome_method: Option<String>,
    pub outcome_arrival: Option<Instant>,
    pub slowest: Option<(&'static str, Duration)>,
}

impl StatementStats {
    pub fn merge(&mut self, other: &StatementStats) {
        self.count += other.count;
        self.completion_committed |= other.completion_committed;
        if let Some((name, d)) = other.slowest
            && self.slowest.is_none_or(|(_, mine)| d > mine)
        {
            self.slowest = Some((name, d));
        }
    }
}

/// One connection checked out for one unit of work.
pub struct Session {
    conn: SessionConnection,
    pub stats: StatementStats,
    pending_completion: bool,
    pub(crate) outcome_endpoint: Option<String>,
    pub(crate) outcome_method: Option<String>,
    pub(crate) outcome_arrival: Option<Instant>,
}

enum SessionConnection {
    Pooled(Lease<OracleConnector>),
    Standalone(Connection),
}

impl AsRef<Connection> for SessionConnection {
    fn as_ref(&self) -> &Connection {
        match self {
            Self::Pooled(lease) => lease.conn(),
            Self::Standalone(conn) => conn,
        }
    }
}

impl SessionConnection {
    fn as_mut(&mut self) -> &mut Connection {
        match self {
            Self::Pooled(lease) => lease.conn_mut(),
            Self::Standalone(conn) => conn,
        }
    }
}

impl Session {
    fn new(conn: SessionConnection) -> Self {
        Self {
            conn,
            stats: StatementStats::default(),
            pending_completion: false,
            outcome_endpoint: None,
            outcome_method: None,
            outcome_arrival: None,
        }
    }

    fn timed<T>(
        &mut self,
        stmt: &SqlStmt,
        f: impl FnOnce(&Connection) -> Result<T, oracledb::Error>,
    ) -> Result<T, DbError> {
        let started = Instant::now();
        let result = f(self.conn.as_ref());
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
            with_statement_binds(
                c,
                stmt.sql,
                params,
                |mut s, p| s.execute(p),
                |mut s, p| s.execute_named(p),
            )
            .map(|s| s.rows_affected())
        })
    }

    /// Inserts one row and reads its RETURNING ID out bind.
    pub fn insert_returning_id(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<i64, DbError> {
        let ids = self.timed(stmt, |c| {
            let null_id = None::<i64>;
            let mut binds = params.to_vec();
            binds.push(&null_id);
            with_statement_binds(
                c,
                stmt.sql,
                &binds,
                |mut s, p| s.execute(p),
                |mut s, p| s.execute_named(p),
            )?
            .returned_data()
            .iter()
            .map(|row| row.get::<i64>(0))
            .collect::<Result<Vec<_>, _>>()
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
    pub fn query_row<T: RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<T, DbError> {
        self.timed(stmt, |c| {
            with_statement_binds(
                c,
                stmt.sql,
                params,
                |s, p| s.query_row(p),
                |s, p| s.query_row_named(p),
            )
            .and_then(|row| T::from_row(&row))
        })
    }

    /// Executes a query expected to return at most one row.
    pub fn query_opt<T: RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<Option<T>, DbError> {
        match self.query_row(stmt, params) {
            Ok(row) => Ok(Some(row)),
            Err(DbError::Oracle { source, .. })
                if *source.kind() == oracledb::ErrorKind::NoDataFound =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Executes a query returning any number of rows.
    pub fn query_all<T: RowValue>(
        &mut self,
        stmt: &SqlStmt,
        params: &[&dyn ToSql],
    ) -> Result<Vec<T>, DbError> {
        self.timed(stmt, |c| {
            with_statement_binds(
                c,
                stmt.sql,
                params,
                |s, p| s.query(p),
                |s, p| s.query_named(p),
            )?
            .map(|row| T::from_row(&row?))
            .collect()
        })
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
        self.timed(stmt, |c| {
            let values: Vec<_> = rows
                .iter()
                .map(|row| row.iter().map(|v| v.bind_value()).collect::<Vec<_>>())
                .collect();
            let refs: Vec<Vec<&dyn oracledb::ToDbValue>> = values
                .iter()
                .map(|row| row.iter().map(|v| v.as_ref()).collect())
                .collect();
            let slices: Vec<&[&dyn oracledb::ToDbValue]> = refs.iter().map(Vec::as_slice).collect();
            c.execute_batch(stmt.sql, slices.as_slice()).map(|_| ())
        })
    }

    pub fn note_completion(&mut self) {
        self.pending_completion = true;
    }

    pub fn commit(&mut self) -> Result<(), DbError> {
        self.conn.as_mut().commit()?;
        self.stats.completion_committed |= self.pending_completion;
        self.pending_completion = false;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), DbError> {
        self.pending_completion = false;
        self.conn.as_mut().rollback()?;
        Ok(())
    }
}

fn map_pool_error(error: PoolError<DbError>) -> DbError {
    match error {
        PoolError::Panicked => DbError::Panicked,
        PoolError::Busy => DbError::Busy,
        PoolError::Closed => DbError::Closed,
        PoolError::Open(error) => error,
        PoolError::Spawn(error) => DbError::Spawn(error),
    }
}

// Keep cleanup inside the unwind boundary, including a rollback that panics.
fn work_and_rollback<S, T, E>(
    session: &mut S,
    work: impl FnOnce(&mut S) -> Result<T, E>,
    rollback: impl FnOnce(&mut S) -> Result<(), E>,
) -> std::thread::Result<(Result<T, E>, bool)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = work(session);
        let healthy = result.is_ok() && rollback(session).is_ok();
        (result, healthy)
    }))
}
