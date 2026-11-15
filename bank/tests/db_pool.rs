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
