//! Saltation bank server: three HTTP listeners over one Oracle pool.

use std::net::SocketAddr;

use anyhow::Context;
use bank::app::{self, AppState};
use bank::shared::clock::Clock;
use bank::shared::db::{Db, DbConfig};
use bank::shared::rollover::Registry;
use bank::shared::telemetry;
use tokio::net::TcpListener;

fn addr(var: &str, default: &str) -> anyhow::Result<SocketAddr> {
    std::env::var(var)
        .unwrap_or_else(|_| default.to_owned())
        .parse()
        .with_context(|| format!("{var} is not an address"))
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}

async fn serve(name: &'static str, addr: SocketAddr, router: axum::Router) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {name} listener on {addr}"))?;
    tracing::info!(listener = name, %addr, "listening");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .with_context(|| format!("{name} listener"))
}

/// MongoDB connection string: MONGO_URI, or built from MONGO_HOST, MONGO_USER,
/// MONGO_PASSWORD and MONGO_DB so no credential sits inside a configured URI.
fn mongo_uri() -> Result<String, std::env::VarError> {
    if let Ok(uri) = std::env::var("MONGO_URI") {
        return Ok(uri);
    }
    let host = std::env::var("MONGO_HOST")?;
    let user = std::env::var("MONGO_USER")?;
    let password = std::env::var("MONGO_PASSWORD")?;
    let db = std::env::var("MONGO_DB")?;
    let enc = |v: &str| -> String {
        v.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    Ok(format!(
        "mongodb://{}:{}@{host}/{db}",
        enc(&user),
        enc(&password)
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let limits = bank::shared::limits::Limits::from_env()?;
    let log_dir =
        std::env::var("SALTATION_LOG_DIR").unwrap_or_else(|_| "/var/log/saltation".to_owned());
    let _log_guard = telemetry::init_logging(&log_dir).context("app.log")?;
    let metrics = telemetry::metrics_handle();

    let db = Db::connect(&DbConfig::from_env().context("database configuration")?)
        .context("connect to Oracle")?;
    let applied = db.migrate().await.context("apply migrations")?;
    tracing::info!(applied, "migrations up to date");
    let mut registry = Registry::default();
    registry.register(std::sync::Arc::new(bank::features::deposits::MaturityJob));
    for job in bank::features::accounts_lifecycle::jobs() {
        registry.register(job);
    }
    registry.register(std::sync::Arc::new(
        bank::features::daily_close::DailyCloseJob,
    ));
    let clock = Clock::init(db.clone(), registry)
        .await
        .context("initialise the clock")?;
    clock.catch_up().await.context("catch up rollover")?;
    clock.spawn_live_scheduler();
    let state = AppState {
        db: db.clone(),
        clock,
        limits,
    };
    match mongo_uri() {
        Ok(uri) => {
            tokio::spawn(bank::features::outbox_relay::run(db, uri));
        }
        Err(_) => tracing::warn!("MONGO_URI is not set; the outbox relay is not running"),
    }

    let public = serve(
        "public",
        addr("SALTATION_PUBLIC_ADDR", "0.0.0.0:8080")?,
        app::public_router(state.clone()),
    );
    let generator = serve(
        "generator",
        addr("SALTATION_GENERATOR_ADDR", "0.0.0.0:8081")?,
        app::generator_router(state.clone()),
    );
    let metrics = serve(
        "metrics",
        addr("SALTATION_METRICS_ADDR", "0.0.0.0:9100")?,
        app::metrics_router(metrics),
    );
    tokio::try_join!(public, generator, metrics)?;
    Ok(())
}
