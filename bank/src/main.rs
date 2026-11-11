//! Saltation bank server entry point.

use anyhow::Context;
use bank::shared::db::{Db, DbConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let config = DbConfig::from_env().context("database configuration")?;
    let db = Db::connect(&config).context("connect to Oracle")?;
    let applied = db.migrate().await.context("apply migrations")?;
    tracing::info!(applied, "migrations up to date");
    Ok(())
}
