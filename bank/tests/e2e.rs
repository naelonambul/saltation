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
