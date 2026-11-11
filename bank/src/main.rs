//! Saltation bank server: three HTTP listeners over one Oracle pool.

use std::net::SocketAddr;

use anyhow::Context;
use bank::app::{self, AppState};
use bank::shared::clock::Clock;
use bank::shared::db::{Db, DbConfig};
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let log_dir =
        std::env::var("SALTATION_LOG_DIR").unwrap_or_else(|_| "/var/log/saltation".to_owned());
    let _log_guard = telemetry::init_logging(&log_dir).context("app.log")?;
    let metrics = telemetry::metrics_handle();

    let db = Db::connect(&DbConfig::from_env().context("database configuration")?)
        .context("connect to Oracle")?;
    let applied = db.migrate().await.context("apply migrations")?;
    tracing::info!(applied, "migrations up to date");
    let clock = Clock::init(db.clone())
        .await
        .context("initialise the clock")?;
    let state = AppState { db, clock };

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
