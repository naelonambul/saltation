//! Bounded acquisition and cleanup against standalone Oracle connections.
use bank::shared::db::{Db, DbConfig, DbError, SqlStmt};
use bank::sql_stmt;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const PING: SqlStmt = sql_stmt!("db_pool.ping", "sql/db_pool_ping.sql");
const SLEEP: SqlStmt = sql_stmt!("db_pool.sleep", "sql/db_pool_sleep.sql");
const SETUP: SqlStmt = sql_stmt!("db_pool.setup", "sql/db_pool_setup.sql");
const UPDATE: SqlStmt = sql_stmt!("db_pool.update", "sql/db_pool_update.sql");
const READ: SqlStmt = sql_stmt!("db_pool.read", "sql/db_pool_read.sql");

fn config() -> DbConfig {
    let mut config = DbConfig::from_env().unwrap();
    config.min_sessions = 1;
    config.max_sessions = 2;
    config.acquire_timeout = Duration::from_secs(2);
    config.call_timeout = Duration::from_secs(2);
    config.warm_up_wait = Duration::from_secs(5);
    config
}

#[tokio::test]
async fn db_pool_panic_retires_and_releases_transaction() {
    let config = config();
    let db = Db::connect(&config).unwrap();
    db.run(|s| {
        s.execute(&SETUP, &[])?;
        s.commit()
    })
    .await
    .unwrap();
    let error = db
        .run::<(), _>(|s| {
            s.execute(&UPDATE, &[&999_i64])?;
            panic!("panic after uncommitted update");
        })
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::Join(ref error) if error.is_panic()));
    let observer = Db::connect(&config).unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        observer.run(|s| {
            assert_eq!(s.query_row::<i64>(&READ, &[])?, 7);
            s.execute(&UPDATE, &[&8_i64])?;
            s.rollback()
        }),
    )
    .await
    .expect("retired connection releases its row lock")
    .unwrap();
    observer.close().unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_call_timeout_recovers_full_capacity() {
    let mut config = config();
    config.call_timeout = Duration::from_millis(500);
    let db = Db::connect(&config).unwrap();
    assert!(db.run(|s| s.execute(&SLEEP, &[])).await.is_err());
    let barrier = Arc::new(Barrier::new(config.max_sessions as usize));
    let mut tasks = Vec::new();
    for _ in 0..config.max_sessions {
        let db = db.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            db.run(move |s| {
                let value = s.query_row::<i64>(&PING, &[])?;
                barrier.wait();
                Ok(value)
            })
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), 1);
        }
    })
    .await
    .expect("all slots serve concurrent work after timeout");
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_busy_has_one_budget() {
    let mut config = config();
    config.max_sessions = 1;
    config.acquire_timeout = Duration::from_millis(100);
    let db = Db::connect(&config).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let first = {
        let db = db.clone();
        tokio::spawn(async move {
            db.run(move |s| {
                let value = s.query_row::<i64>(&PING, &[])?;
                tx.send(()).unwrap();
                held.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(value)
            })
            .await
        })
    };
    rx.await.unwrap();
    let start = Instant::now();
    assert!(matches!(
        db.run(|s| s.query_row::<i64>(&PING, &[])).await,
        Err(DbError::Busy)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_silent_listener_bounds_startup_and_run() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    // Detached, with accepted sockets held until process exit: the test must
    // finish while driver opener threads are still stalled.
    std::thread::spawn(move || {
        let mut sockets = Vec::new();
        loop {
            match listener.accept() {
                Ok((socket, _)) => sockets.push(socket),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => panic!("listener: {error}"),
            }
        }
    });
    let mut config = config();
    config.connect_string = format!("127.0.0.1:{}/x", address.port());
    config.warm_up_wait = Duration::from_millis(100);
    config.acquire_timeout = Duration::from_millis(100);
    let start = Instant::now();
    assert!(Db::connect(&config).is_err());
    assert!(start.elapsed() < config.warm_up_wait + Duration::from_secs(5));
    config.min_sessions = 0;
    let db = Db::connect(&config).unwrap();
    let start = Instant::now();
    assert!(matches!(
        db.run(|s| s.query_row::<i64>(&PING, &[])).await,
        Err(DbError::Busy)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    db.close().unwrap();
}

#[tokio::test]
#[ignore = "run by it-oracle with ORA_SDTZ=+00:00"]
async fn db_pool_rejects_other_time_zones() {
    let mut config = config();
    config.min_sessions = 0;
    let db = Db::connect(&config).unwrap();
    assert!(
        matches!(db.run(|s| s.query_row::<i64>(&PING, &[])).await, Err(DbError::TimeZone(zone)) if zone == "+00:00")
    );
    assert!(matches!(db.migrate().await, Err(DbError::TimeZone(zone)) if zone == "+00:00"));
    db.close().unwrap();
}

#[test]
#[ignore = "run as a separately timed process by it-oracle"]
fn db_pool_close_exits() {
    let mut config = config();
    config.min_sessions = 4;
    config.max_sessions = 4;
    let db = Db::connect(&config).unwrap();
    let start = Instant::now();
    db.close().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
}

const LOCK_NOWAIT: SqlStmt = sql_stmt!("db_pool.lock_nowait", "sql/db_pool_lock_nowait.sql");
const LOCK_WAIT: SqlStmt = sql_stmt!("db_pool.lock_wait", "sql/db_pool_lock_wait.sql");
const SESSION_STATE: SqlStmt =
    sql_stmt!("db_pool.session_state", "sql/db_pool_session_identity.sql");
const ALTERNATIVE_QUOTE_LOCK: SqlStmt = sql_stmt!(
    "db_pool.alternative_quote_lock",
    "sql/db_pool_alternative_quote_lock.sql"
);

#[tokio::test]
async fn db_pool_alternative_quote_for_update_releases_lock() {
    let config = config();
    let db = Db::connect(&config).unwrap();
    db.run(|s| {
        s.execute(&SETUP, &[])?;
        s.commit()
    })
    .await
    .unwrap();
    assert_eq!(
        db.run(|s| s.query_row::<String>(&ALTERNATIVE_QUOTE_LOCK, &[]))
            .await
            .unwrap(),
        "it's"
    );
    let observer = Db::connect(&config).unwrap();
    observer
        .run(|s| {
            assert_eq!(s.query_row::<i64>(&LOCK_NOWAIT, &[])?, 7);
            s.rollback()
        })
        .await
        .unwrap();
    observer.close().unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_for_update_without_commit_releases_lock() {
    let config = config();
    let db = Db::connect(&config).unwrap();
    db.run(|s| {
        s.execute(&SETUP, &[])?;
        s.commit()
    })
    .await
    .unwrap();
    assert_eq!(
        db.run(|s| s.query_row::<i64>(&LOCK_NOWAIT, &[]))
            .await
            .unwrap(),
        7
    );
    let observer = Db::connect(&config).unwrap();
    observer
        .run(|s| {
            assert_eq!(s.query_row::<i64>(&LOCK_NOWAIT, &[])?, 7);
            s.rollback()
        })
        .await
        .unwrap();
    observer.close().unwrap();
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_lock_timeout_reuses_clean_session() {
    let mut config = config();
    config.max_sessions = 1;
    config.call_timeout = Duration::from_secs(5);
    let db = Db::connect(&config).unwrap();
    let (sid, logon, transaction) = db
        .run(|s| {
            s.execute(&SETUP, &[])?;
            s.execute(&SETUP_ROW_TWO, &[])?;
            s.commit()?;
            s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[])
        })
        .await
        .unwrap();
    assert!(transaction.is_none());

    let observer = Db::connect(&config).unwrap();
    let (locked, ready) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let holder = {
        let observer = observer.clone();
        tokio::spawn(async move {
            observer
                .run(move |s| {
                    assert_eq!(s.query_row::<i64>(&LOCK_NOWAIT, &[])?, 7);
                    locked.send(()).unwrap();
                    held.recv_timeout(Duration::from_secs(15)).unwrap();
                    s.rollback()
                })
                .await
        })
    };
    ready.await.unwrap();
    let error = db
        .run(|s| {
            s.execute(&UPDATE_ROW_TWO, &[])?;
            s.query_row::<i64>(&LOCK_WAIT, &[])
        })
        .await
        .unwrap_err();
    assert_eq!(error.ora_code(), Some(30006));
    let independent = Db::connect(&config).unwrap();
    independent
        .run(|s| {
            assert_eq!(s.query_row::<i64>(&READ_ROW_TWO, &[])?, 7);
            assert_eq!(s.query_row::<i64>(&LOCK_ROW_TWO, &[])?, 7);
            s.rollback()
        })
        .await
        .unwrap();
    independent.close().unwrap();
    let (next_sid, next_logon, transaction) = db
        .run(|s| s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[]))
        .await
        .unwrap();
    assert_eq!(next_sid, sid, "recoverable lock timeout keeps the session");
    assert_eq!(
        next_logon, logon,
        "recoverable lock timeout keeps the logon"
    );
    assert!(transaction.is_none(), "rollback leaves no transaction open");
    release.send(()).unwrap();
    holder.await.unwrap().unwrap();
    observer.close().unwrap();
    db.close().unwrap();
}

const SETUP_ROW_TWO: SqlStmt = sql_stmt!("db_pool.setup_row_two", "sql/db_pool_setup_row_two.sql");
const UPDATE_ROW_TWO: SqlStmt =
    sql_stmt!("db_pool.update_row_two", "sql/db_pool_update_row_two.sql");
const READ_ROW_TWO: SqlStmt = sql_stmt!("db_pool.read_row_two", "sql/db_pool_read_row_two.sql");
const LOCK_ROW_TWO: SqlStmt = sql_stmt!("db_pool.lock_row_two", "sql/db_pool_lock_row_two.sql");
const DEFERRED_SETUP: SqlStmt =
    sql_stmt!("db_pool.deferred_setup", "sql/db_pool_deferred_setup.sql");
const DEFERRED_INSERT: SqlStmt =
    sql_stmt!("db_pool.deferred_insert", "sql/db_pool_deferred_insert.sql");
const DEFERRED_COUNT: SqlStmt =
    sql_stmt!("db_pool.deferred_count", "sql/db_pool_deferred_count.sql");
const FAIL: SqlStmt = sql_stmt!("db_pool.fail", "sql/db_pool_fail.sql");

#[tokio::test]
async fn db_pool_call_timeout_retires_session_even_when_remapped() {
    let mut config = config();
    config.max_sessions = 1;
    // The two-second sleep also finishes within the driver's recovery timeout.
    config.call_timeout = Duration::from_millis(1500);
    let db = Db::connect(&config).unwrap();
    let (_, logon, _) = db
        .run(|s| s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[]))
        .await
        .unwrap();
    let error = db
        .run(|s| {
            let error = s.execute(&SLEEP, &[]).unwrap_err();
            assert!(
                matches!(error, DbError::Oracle { ref source, .. }
                if *source.kind() == oracledb::ErrorKind::CallTimeoutExceeded),
                "{error:?}"
            );
            Err::<(), _>(DbError::Busy)
        })
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::Busy));
    let (_, next_logon, transaction) = db
        .run(|s| s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[]))
        .await
        .unwrap();
    assert_ne!(next_logon, logon, "call timeout retires the logon");
    assert!(transaction.is_none());
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_failed_commit_retires_session() {
    let mut config = config();
    config.max_sessions = 1;
    let db = Db::connect(&config).unwrap();
    let (_, logon, _) = db
        .run(|s| {
            s.execute(&DEFERRED_SETUP, &[])?;
            s.commit()?;
            s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[])
        })
        .await
        .unwrap();
    let error = db
        .run(|s| {
            s.execute(&DEFERRED_INSERT, &[])?;
            s.commit()
        })
        .await
        .unwrap_err();
    assert_eq!(error.ora_code(), Some(2091));
    let (_, next_logon, transaction) = db
        .run(|s| {
            assert_eq!(s.query_row::<i64>(&DEFERRED_COUNT, &[])?, 0);
            s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[])
        })
        .await
        .unwrap();
    assert_ne!(next_logon, logon, "failed commit retires the logon");
    assert!(transaction.is_none());
    db.close().unwrap();
}

#[tokio::test]
async fn db_pool_swallowed_error_rolls_back() {
    let mut config = config();
    config.max_sessions = 1;
    let db = Db::connect(&config).unwrap();
    let (sid, logon, _) = db
        .run(|s| {
            s.execute(&SETUP, &[])?;
            s.commit()?;
            s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[])
        })
        .await
        .unwrap();
    db.run(|s| {
        s.execute(&UPDATE, &[&999_i64])?;
        let error = s.query_row::<i64>(&FAIL, &[]).unwrap_err();
        assert_eq!(error.ora_code(), Some(1476));
        Ok(())
    })
    .await
    .unwrap();
    let observer = Db::connect(&config).unwrap();
    observer
        .run(|s| {
            assert_eq!(s.query_row::<i64>(&READ, &[])?, 7);
            assert_eq!(s.query_row::<i64>(&LOCK_NOWAIT, &[])?, 7);
            s.rollback()
        })
        .await
        .unwrap();
    let (next_sid, next_logon, transaction) = db
        .run(|s| s.query_row::<(String, String, Option<String>)>(&SESSION_STATE, &[]))
        .await
        .unwrap();
    assert_eq!(next_sid, sid);
    assert_eq!(next_logon, logon);
    assert!(transaction.is_none());
    observer.close().unwrap();
    db.close().unwrap();
}
