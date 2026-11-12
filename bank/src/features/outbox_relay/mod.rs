//! Outbox relay (spec F-36, F-36a): reads events in event-ID order in batches
//! of 500, inserts them into MongoDB `bank_logs.events` with journaled writes,
//! then deletes exactly those rows from `OUTBOX`. Delivery is at least once;
//! `_id = event_id` makes a repeated insert a no-op (duplicate keys ignored).

use std::time::Duration;

use mongodb::bson::{self, Document};
use mongodb::options::{InsertManyOptions, WriteConcern};
use mongodb::{Client, Collection};
use oracle::sql_type::ToSql;
use serde_json::Value;

use crate::shared::db::{Db, DbError, SqlStmt};
use crate::shared::outbox;
use crate::sql_stmt;

const SELECT_BATCH: SqlStmt = sql_stmt!("outbox_relay.select_batch", "sql/select_batch.sql");
const DELETE_DELIVERED: SqlStmt =
    sql_stmt!("outbox_relay.delete_delivered", "sql/delete_delivered.sql");

pub const BATCH_SIZE: i64 = 500;
const IDLE_POLL: Duration = Duration::from_millis(200);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// MongoDB duplicate key error code.
const DUPLICATE_KEY: i32 = 11000;

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("mongodb: {0}")]
    Mongo(#[from] mongodb::error::Error),
    #[error("event JSON: {0}")]
    Json(String),
}

/// The events collection with journaled, majority-free acknowledgement.
pub async fn events_collection(uri: &str) -> Result<Collection<Document>, mongodb::error::Error> {
    let client = Client::with_uri_str(uri).await?;
    let db = client
        .default_database()
        .unwrap_or_else(|| client.database("bank_logs"));
    Ok(db.collection::<Document>("events"))
}

fn to_document(event_id: &str, json: &str) -> Result<Document, RelayError> {
    let value: Value = serde_json::from_str(json).map_err(|e| RelayError::Json(e.to_string()))?;
    let mut doc = bson::to_document(&value).map_err(|e| RelayError::Json(e.to_string()))?;
    doc.insert("_id", event_id);
    Ok(doc)
}

fn only_duplicates(e: &mongodb::error::Error) -> bool {
    match e.kind.as_ref() {
        mongodb::error::ErrorKind::InsertMany(failure) => {
            failure.write_concern_error.is_none()
                && failure
                    .write_errors
                    .as_ref()
                    .is_some_and(|errs| errs.iter().all(|w| w.code == DUPLICATE_KEY))
        }
        _ => false,
    }
}

/// Delivers one batch. Returns how many events were delivered and deleted.
/// With `delete == false` it stops after inserting, simulating a relay that
/// crashes between the MongoDB write and the Oracle delete (tests only).
pub async fn relay_once(
    db: &Db,
    events: &Collection<Document>,
    delete: bool,
) -> Result<usize, RelayError> {
    let rows: Vec<(String, String)> = db
        .run(|s| s.query_all(&SELECT_BATCH, &[&BATCH_SIZE]))
        .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let docs = rows
        .iter()
        .map(|(id, json)| to_document(id, json))
        .collect::<Result<Vec<_>, _>>()?;
    let options = InsertManyOptions::builder()
        .ordered(false)
        .write_concern(WriteConcern::builder().journal(true).build())
        .build();
    match events.insert_many(docs).with_options(options).await {
        Ok(_) => {}
        Err(e) if only_duplicates(&e) => {}
        Err(e) => return Err(e.into()),
    }
    if !delete {
        return Ok(0);
    }
    let ids: Vec<String> = rows.into_iter().map(|(id, _)| id).collect();
    let count = ids.len();
    db.run(move |s| {
        let binds: Vec<Vec<&dyn ToSql>> = ids.iter().map(|id| vec![id as &dyn ToSql]).collect();
        s.execute_batch(&DELETE_DELIVERED, &binds)?;
        s.commit()
    })
    .await?;
    metrics::counter!("bank_outbox_delivered_total").increment(count as u64);
    Ok(count)
}

/// Runs the relay until the process stops, backing off while MongoDB or
/// Oracle is unavailable, and exporting the backlog gauges.
pub async fn run(db: Db, mongo_uri: String) {
    let mut backoff = Duration::from_millis(500);
    let mut events: Option<Collection<Document>> = None;
    loop {
        if let Ok((count, age)) = outbox::backlog(&db).await {
            metrics::gauge!("bank_outbox_undelivered").set(count as f64);
            metrics::gauge!("bank_outbox_oldest_age_seconds").set(age);
        }
        if events.is_none() {
            match events_collection(&mongo_uri).await {
                Ok(c) => events = Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "outbox relay: MongoDB client not ready");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            }
        }
        let Some(collection) = events.as_ref() else {
            continue;
        };
        match relay_once(&db, collection, true).await {
            Ok(0) => {
                backoff = Duration::from_millis(500);
                tokio::time::sleep(IDLE_POLL).await;
            }
            Ok(_) => backoff = Duration::from_millis(500),
            Err(e) => {
                metrics::counter!("bank_outbox_relay_errors_total").increment(1);
                tracing::warn!(error = %e, backoff_ms = backoff.as_millis() as u64, "outbox relay: delivery failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}
