//! Business events (spec F-35, F-37). Every event is inserted into `OUTBOX`
//! inside the same transaction as the state it describes; the relay
//! (`features::outbox_relay`) delivers it to MongoDB `bank_logs`.

use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, FixedOffset, SecondsFormat};
use serde::Serialize;
use serde_json::Value;
use ulid::Generator;

use crate::shared::context::RequestContext;
use crate::shared::db::{Db, DbError, Session, SqlStmt};
use crate::sql_stmt;

const INSERT_EVENT: SqlStmt = sql_stmt!("outbox.insert_event", "sql/insert_event.sql");
const SELECT_BACKLOG: SqlStmt = sql_stmt!("outbox.select_backlog", "sql/select_backlog.sql");
const SCHEMA: &str = "saltation.log/1";

/// Monotonic ULIDs, so event IDs sort in creation order within the process.
fn next_event_id() -> String {
    static GENERATOR: OnceLock<Mutex<Generator>> = OnceLock::new();
    let mut g = GENERATOR
        .get_or_init(|| Mutex::new(Generator::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match g.generate() {
        Ok(id) => id.to_string(),
        Err(_) => ulid::Ulid::new().to_string(),
    }
}

/// The F-37 envelope. Only strings, numbers, booleans and nulls.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub schema: &'static str,
    pub event_id: String,
    pub event_type: &'static str,
    pub occurred_at: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub device_id: Option<String>,
    pub customer_no: Option<String>,
    pub account_no: Option<String>,
    pub channel: &'static str,
    pub listener: Option<&'static str>,
    pub ip: Option<String>,
    pub region: Option<String>,
    pub outcome: &'static str,
    pub rsp_code: Option<String>,
    pub bank_rsp_code: Option<String>,
    pub human: bool,
    pub payload: Value,
}

impl Event {
    /// An event caused by a request.
    pub fn for_request(
        ctx: &RequestContext,
        event_type: &'static str,
        occurred_at: DateTime<FixedOffset>,
        channel: &'static str,
        outcome: &'static str,
    ) -> Self {
        Self {
            schema: SCHEMA,
            event_id: next_event_id(),
            event_type,
            occurred_at: occurred_at.to_rfc3339_opts(SecondsFormat::Millis, false),
            request_id: Some(ctx.request_id.clone()),
            session_id: None,
            device_id: ctx.device_id.clone(),
            customer_no: None,
            account_no: None,
            channel,
            listener: Some(ctx.listener.as_str()),
            ip: ctx.ip.clone(),
            region: ctx.region.clone(),
            outcome,
            rsp_code: None,
            bank_rsp_code: None,
            human: ctx.human,
            payload: Value::Object(Default::default()),
        }
    }

    pub fn customer(mut self, customer_no: impl Into<String>) -> Self {
        self.customer_no = Some(customer_no.into());
        self
    }

    pub fn account(mut self, account_no: impl Into<String>) -> Self {
        self.account_no = Some(account_no.into());
        self
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn device(mut self, device_id: Option<String>) -> Self {
        if device_id.is_some() {
            self.device_id = device_id;
        }
        self
    }

    pub fn codes(mut self, rsp_code: &str, bank_rsp_code: Option<&str>) -> Self {
        self.rsp_code = Some(rsp_code.to_owned());
        self.bank_rsp_code = bank_rsp_code.map(str::to_owned);
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Inserts the event into `OUTBOX` in the caller's transaction.
pub fn record(session: &mut Session, event: &Event) -> Result<(), DbError> {
    let json =
        serde_json::to_string(event).map_err(|e| DbError::Config(format!("event JSON: {e}")))?;
    session.execute(&INSERT_EVENT, &[&event.event_id, &json])?;
    Ok(())
}

/// Undelivered event count and the oldest event's age in seconds (spec F-36a).
pub async fn backlog(db: &Db) -> Result<(i64, f64), DbError> {
    db.run(|s| s.query_row(&SELECT_BACKLOG, &[])).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ids_increase_monotonically() {
        let a = next_event_id();
        let b = next_event_id();
        assert!(b > a, "{b} should sort after {a}");
    }
}
