//! Step 10 proof, run after the generator by scripts/dev/it-oracle.sh e2e
//! (E2E_SINCE is the run's start, RFC 3339). Checks, over the whole run:
//! labels for every generator request and subject, an empty outbox, a
//! business event in bank_logs for every transfer request, and consistent books.

mod common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bank::features::outbox_relay;
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset};
use mongodb::bson::{Document, doc};

const TRANSFERS: SqlStmt = sql_stmt!(
    "test.generator_transfer_requests",
    "sql/generator_transfer_requests.sql"
);
const LOGINS: SqlStmt = sql_stmt!(
    "test.generator_login_attempts",
    "sql/generator_login_attempts.sql"
);
const COUNT_OUTBOX: SqlStmt = sql_stmt!("test.count_outbox", "sql/count_outbox.sql");
const BAD_TRANSFERS: SqlStmt = sql_stmt!("test.bad_transfers", "sql/bad_transfers.sql");
const BAD_BALANCES: SqlStmt = sql_stmt!("test.bad_balances", "sql/bad_balances.sql");
const BAD_RUNNING: SqlStmt = sql_stmt!("test.bad_running_balances", "sql/bad_running_balances.sql");

async fn labels_by_request(
    truth: &mongodb::Collection<Document>,
    ids: &[String],
) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for chunk in ids.chunks(1000) {
        let mut cursor = truth
            .find(doc! { "request_id": { "$in": chunk } })
            .await
            .expect("find labels");
        while cursor.advance().await.expect("cursor") {
            let d = cursor.deserialize_current().expect("label");
            let rid = d.get_str("request_id").expect("request_id").to_owned();
            let st = d.get_str("subject_type").expect("subject_type").to_owned();
            assert_eq!(
                d.get_str("label").ok(),
                Some("normal"),
                "stage 1 labels are normal"
            );
            out.entry(rid).or_default().push(st);
        }
    }
    out
}

#[tokio::test]
async fn generator_run_is_labelled_logged_and_consistent() {
    let Ok(since) = std::env::var("E2E_SINCE") else {
        eprintln!("skipped: run through scripts/dev/it-oracle.sh e2e");
        return;
    };
    let since: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&since).expect("E2E_SINCE");
    let state = common::state().await;
    let db = state.db.clone();

    // The bank container's relay drains the outbox.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let n: i64 = db
            .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
            .await
            .expect("outbox");
        if n == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "outbox still has {n} events");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let transfers: Vec<String> = db
        .run(move |s| s.query_all(&TRANSFERS, &[&since]))
        .await
        .expect("transfers");
    let logins: Vec<(String, String)> = db
        .run(move |s| s.query_all(&LOGINS, &[&since]))
        .await
        .expect("logins");
    assert!(
        transfers.len() >= 2_000,
        "the 10-minute run sent at least 2,000 transfers, got {}",
        transfers.len()
    );
    eprintln!(
        "checked {} transfers and {} logins",
        transfers.len(),
        logins.len()
    );

    let truth_client =
        mongodb::Client::with_uri_str(std::env::var("MONGO_TRUTH_URI").expect("MONGO_TRUTH_URI"))
            .await
            .expect("truth");
    let truth = truth_client
        .database("truth")
        .collection::<Document>("labels");
    let by_request = labels_by_request(&truth, &transfers).await;
    for id in &transfers {
        let subjects = by_request.get(id).map(Vec::as_slice).unwrap_or_default();
        assert_eq!(
            subjects,
            ["account", "account"],
            "transfer {id} has one label per account"
        );
    }
    let login_ids: Vec<String> = logins.iter().map(|(r, _)| r.clone()).collect();
    let by_login = labels_by_request(&truth, &login_ids).await;
    for (id, _) in &logins {
        assert_eq!(
            by_login.get(id).map(Vec::as_slice).unwrap_or_default(),
            ["customer"],
            "login {id} is labelled"
        );
    }

    let events = outbox_relay::events_collection(&std::env::var("MONGO_URI").expect("MONGO_URI"))
        .await
        .expect("events");
    let mut delivered = 0u64;
    for chunk in transfers.chunks(1000) {
        delivered += events
            .count_documents(
                doc! { "request_id": { "$in": chunk }, "event_type": "request.completed" },
            )
            .await
            .expect("count");
    }
    assert_eq!(
        delivered as usize,
        transfers.len(),
        "every transfer request has exactly one business event"
    );

    for (name, stmt) in [
        ("transfers", &BAD_TRANSFERS),
        ("balances", &BAD_BALANCES),
        ("running balances", &BAD_RUNNING),
    ] {
        let bad: i64 = db
            .run(move |s| s.query_row(stmt, &[]))
            .await
            .expect("check");
        assert_eq!(bad, 0, "inconsistent {name}");
    }
}

const ATM_SESSION: SqlStmt = sql_stmt!("test.e2e_atm_session", "sql/e2e_atm_session.sql");
const ATM_TRANSFER: SqlStmt = sql_stmt!("test.e2e_atm_transfer", "sql/e2e_atm_transfer.sql");
const ATM_CASH: SqlStmt = sql_stmt!("test.atm_cash_rows", "sql/atm_cash_rows.sql");

async fn atm_rows(db: &bank::shared::db::Db, stmt: SqlStmt, id: &str) -> Vec<serde_json::Value> {
    let id = id.to_owned();
    let rows: Vec<String> = db
        .run(move |s| s.query_all(&stmt, &[&id]))
        .await
        .expect("ATM rows");
    rows.iter()
        .map(|row| serde_json::from_str(row).expect("ATM row JSON"))
        .collect()
}

fn check_log_keys(value: &serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "pin" | "atm_pin" | "pin_hash" | "atm_session_token"
                ) {
                    return Err("secret key in application log".into());
                }
                check_log_keys(value)?;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                check_log_keys(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `pin` is checked only on lines of the smoke requests: a random 4-digit PIN
/// would sooner or later equal an unrelated number among a long run's lines.
fn check_log_line(line: &str, pin: Option<&str>) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| "invalid JSON log line".to_owned())?;
    check_log_keys(&value)?;
    if line.contains("$argon2id$") {
        return Err("PIN hash in application log".into());
    }
    if line.contains("Bearer ") {
        return Err("Bearer token in application log".into());
    }
    if pin.is_some_and(|pin| common::find_pin(&value, pin).is_some()) {
        return Err("PIN in application log".into());
    }
    Ok(())
}

#[test]
fn log_line_rejects_pin_and_bearer_but_allows_ulid() {
    let pin = Some("7391");
    assert!(check_log_line(r#"{"message":"entered PIN 7391 here"}"#, pin).is_err());
    assert!(check_log_line(r#"{"message":"Bearer x"}"#, pin).is_err());
    assert!(check_log_line(r#"{"message":"Bearer x"}"#, None).is_err());
    assert!(check_log_line(r#"{"atm_pin":"x"}"#, None).is_err());
    assert!(check_log_line(r#"{"request_id":"01ARZ3NDEKTSV4RRFFQ69G7391"}"#, pin).is_ok());
}

#[tokio::test]
async fn atm_smoke_is_posted_located_and_secret_free() {
    let Ok(smoke) = std::env::var("E2E_ATM") else {
        eprintln!("skipped: run through scripts/dev/it-oracle.sh e2e");
        return;
    };
    let smoke: serde_json::Value = serde_json::from_str(&smoke).expect("E2E_ATM JSON");
    let requests = smoke["requests"].as_object().expect("requests");
    assert_eq!(requests.len(), 6);
    let state = common::state().await;
    let db = state.db.clone();
    for (step, amount) in [("withdraw", -10000_i64), ("deposit", 20000_i64)] {
        let id = requests[step].as_str().expect("request ID");
        let rows = atm_rows(&db, ATM_CASH, id).await;
        assert_eq!(rows.len(), 1, "one cash posting");
        assert_eq!(rows[0]["kind"], "atm_cash");
        assert_eq!(rows[0]["amount"], amount);
        assert_eq!(rows[0]["request_id"], id);
        assert_eq!(rows[0]["channel"], "ATM");
        assert_eq!(rows[0]["terminal_id"], smoke["terminal_id"]);
        let account = smoke["account_no"].as_str().expect("account").to_owned();
        let id = id.to_owned();
        let transfers: Vec<String> = db
            .run(move |s| s.query_all(&ATM_TRANSFER, &[&account, &id]))
            .await
            .expect("ATM transfer");
        assert_eq!(transfers.len(), 1);
        let transfer: serde_json::Value = serde_json::from_str(&transfers[0]).unwrap();
        assert_eq!(transfer["kind"], "atm_cash");
        assert_eq!(transfer["amount"], amount.abs());
        assert_eq!(transfer["rows"], 1);
        assert_eq!(transfer["account_rows"], 1);
    }
    let mut session_ids = Vec::new();
    for (step, ended) in [("start", true), ("restart", false)] {
        let rows = atm_rows(&db, ATM_SESSION, requests[step].as_str().unwrap()).await;
        assert_eq!(rows.len(), 1);
        let session = &rows[0];
        assert_eq!(session["account_no"], smoke["account_no"]);
        assert_eq!(session["terminal_id"], smoke["terminal_id"]);
        assert_eq!(session["ended"], i32::from(ended));
        if ended {
            assert_eq!(session["end_reason"], "replaced");
        } else {
            assert!(session["end_reason"].is_null());
        }
        session_ids.push(session["session_id"].as_str().unwrap().to_owned());
    }
    assert_ne!(session_ids[0], session_ids[1]);

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let n: i64 = db.run(|s| s.query_row(&COUNT_OUTBOX, &[])).await.unwrap();
        if n == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "outbox still has {n} events");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let events = outbox_relay::events_collection(&std::env::var("MONGO_URI").expect("MONGO_URI"))
        .await
        .expect("events");
    for (step, expected) in [
        ("wrong_pin", vec!["atm.session_rejected"]),
        ("start", vec!["atm.session_started"]),
        ("withdraw", vec!["atm.operation"]),
        ("deposit", vec!["atm.operation"]),
        ("balance", vec!["atm.operation"]),
        ("restart", vec!["atm.session_ended", "atm.session_started"]),
    ] {
        let id = requests[step].as_str().unwrap();
        let mut cursor = events.find(doc! {"request_id": id}).await.unwrap();
        let mut completed = 0;
        let mut actual = Vec::new();
        while cursor.advance().await.unwrap() {
            let document = cursor.deserialize_current().unwrap();
            let event = serde_json::to_value(document).unwrap();
            assert_eq!(event["request_id"], id);
            assert!(common::envelope_errors(&event).is_empty());
            let kind = event["event_type"].as_str().unwrap();
            if kind == "request.completed" {
                completed += 1;
            }
            if kind.starts_with("atm.") {
                actual.push(kind.to_owned());
                assert_eq!(event["channel"], "ATM");
                assert_eq!(event["account_no"], smoke["account_no"]);
                for key in [
                    "terminal_id",
                    "region",
                    "latitude",
                    "longitude",
                    "account_no",
                ] {
                    assert_eq!(event["payload"][key], smoke[key], "{step}: {key}");
                }
                assert_eq!(event["payload"]["request_id"], id);
                if kind == "atm.operation" {
                    assert_eq!(event["payload"]["operation"], step);
                }
                if kind == "atm.session_ended" {
                    assert_eq!(event["payload"]["session_id"], session_ids[0]);
                    assert_eq!(event["payload"]["reason"], "replaced");
                }
            }
        }
        actual.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(actual, expected, "ATM events for {step}");
        assert_eq!(completed, 1, "one completion for {step}");
    }
    assert_eq!(
        events
            .count_documents(doc! {
                "event_type": "atm.session_ended", "payload.session_id": &session_ids[0]
            })
            .await
            .unwrap(),
        1
    );

    let truth_client =
        mongodb::Client::with_uri_str(std::env::var("MONGO_TRUTH_URI").expect("MONGO_TRUTH_URI"))
            .await
            .expect("truth");
    let roster = truth_client
        .database("truth")
        .collection::<Document>("roster")
        .find_one(doc! { "account_no": smoke["account_no"].as_str().expect("account") })
        .await
        .expect("find smoke roster entry")
        .expect("smoke roster entry");
    let pin = roster.get_str("atm_pin").expect("smoke roster atm_pin");

    let mut lines = 0;
    let mut found_request = false;
    for entry in std::fs::read_dir("/var/log/saltation").expect("bank log directory") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if !name.starts_with("app") || !name.ends_with(".log") {
            continue;
        }
        let contents = std::fs::read_to_string(path).expect("read bank log");
        for line in contents.lines() {
            lines += 1;
            let smoke_line = requests
                .values()
                .any(|id| line.contains(id.as_str().unwrap()));
            check_log_line(line, smoke_line.then_some(pin)).expect("secret-free JSON log line");
            found_request |= smoke_line;
        }
    }
    assert!(lines > 0, "bank app*.log files missing or empty");
    assert!(found_request, "no smoke request in bank logs");
}
