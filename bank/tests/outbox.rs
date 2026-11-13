//! Step 8 proof: the relay delivers every outbox event to MongoDB exactly once
//! and empties the outbox, including after a crash between the MongoDB write
//! and the Oracle delete; and (with OUTBOX_OUTAGE_TEST=1, driven by
//! scripts/dev/it-oracle.sh outbox-outage) events accumulate under the cap
//! while MongoDB is stopped and are fully delivered after it returns.

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use bank::features::outbox_relay;
use bank::shared::db::SqlStmt;
use bank::shared::outbox;
use bank::sql_stmt;
use common::{Api, api, bank_tran_id, call, new_id, unique_digits};
use mongodb::bson::{Document, doc};
use serde_json::json;

const COUNT_OUTBOX: SqlStmt = sql_stmt!("test.count_outbox", "sql/count_outbox.sql");
const REQUEST_IDS: SqlStmt = sql_stmt!("test.request_ids_since", "sql/request_ids_since.sql");
const CUSTOMER_ID: SqlStmt = sql_stmt!("test.customer_id_by_phone", "sql/customer_id_by_phone.sql");

async fn collection() -> mongodb::Collection<Document> {
    outbox_relay::events_collection(&std::env::var("MONGO_URI").expect("MONGO_URI"))
        .await
        .expect("mongo")
}

async fn drain(api: &Api, events: &mongodb::Collection<Document>) {
    while outbox_relay::relay_once(&api.state.db, events, true)
        .await
        .expect("relay")
        > 0
    {}
}

async fn outbox_count(api: &Api) -> i64 {
    api.state
        .db
        .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
        .await
        .expect("count")
}

async fn login_attempts(api: &Api, n: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for _ in 0..n {
        let r = new_id();
        let (status, _) = call(
            &api.generator,
            "POST",
            "/v2.0/ext/auth/login",
            &r,
            None,
            Some(json!({ "phone": format!("018{}", unique_digits(8)), "password": "x" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        ids.push(r);
    }
    ids
}

async fn mongo_count(events: &mongodb::Collection<Document>, request_ids: &[String]) -> u64 {
    events
        .count_documents(doc! { "request_id": { "$in": request_ids } })
        .await
        .expect("count")
}

#[tokio::test]
async fn relay_delivers_once_even_after_a_crash() {
    let api = api().await;
    let events = collection().await;

    let first = login_attempts(&api, 30).await;
    drain(&api, &events).await;
    assert_eq!(outbox_count(&api).await, 0, "delivered rows are deleted");
    assert_eq!(
        mongo_count(&events, &first).await,
        30,
        "one event per attempt"
    );

    // Crash between the MongoDB insert and the Oracle delete, then recover.
    let second = login_attempts(&api, 30).await;
    outbox_relay::relay_once(&api.state.db, &events, false)
        .await
        .expect("insert only");
    assert!(
        outbox_count(&api).await >= 30,
        "rows remain after the simulated crash"
    );
    drain(&api, &events).await;
    assert_eq!(outbox_count(&api).await, 0);
    assert_eq!(
        mongo_count(&events, &second).await,
        30,
        "redelivery adds no duplicates"
    );

    let sample = events
        .find_one(doc! { "request_id": &second[0] })
        .await
        .expect("find")
        .expect("event");
    assert_eq!(sample.get_str("schema").ok(), Some("saltation.log/1"));
    assert!(
        sample.get_str("occurred_at").is_ok(),
        "timestamps stay ISO-8601 strings, not BSON dates"
    );
    assert!(sample.get_str("recorded_at").is_ok());
    assert_eq!(sample.get_str("_id").ok(), sample.get_str("event_id").ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn outage_then_full_delivery() {
    if std::env::var("OUTBOX_OUTAGE_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: run through scripts/dev/it-oracle.sh outbox-outage");
        return;
    }
    let api = api().await;
    let events = collection().await;
    drain(&api, &events).await;
    let relay = tokio::spawn(outbox_relay::run(
        api.state.db.clone(),
        std::env::var("MONGO_URI").expect("uri"),
    ));

    // One customer with two funded accounts sending small transfers back and forth.
    let n = unique_digits(8);
    let phone = format!("010{n}");
    let customer_no = format!("C{n}");
    let accounts = [format!("96{n}01"), format!("96{n}02")];
    call(
        &api.generator,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(json!({ "customers": [{
        "customer_no": customer_no, "name": "최장애", "phone": phone, "address": "광주",
        "customer_type": "INDIVIDUAL", "password": "pw-outage" }] })),
    )
    .await;
    let (_, seeded) = call(&api.generator, "POST", "/internal/v1/seed/accounts", &new_id(), None, Some(json!({ "accounts": [
        { "customer_no": customer_no, "account_no": accounts[0], "opening_balance": 10_000_000 },
        { "customer_no": customer_no, "account_no": accounts[1], "opening_balance": 10_000_000 } ] }))).await;
    let fins: Vec<String> = seeded["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .map(|a| a["fintech_use_num"].as_str().expect("fin").to_owned())
        .collect();
    let (_, login) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({ "phone": phone, "password": "pw-outage" })),
    )
    .await;
    let token = login["access_token"].as_str().expect("token").to_owned();

    // ~150 seconds of load; the script stops MongoDB at about 10 s for 120 s.
    let started = Instant::now();
    let mut max_backlog = 0i64;
    let mut i = 0usize;
    while started.elapsed() < Duration::from_secs(150) {
        let body = json!({
            "bank_tran_id": bank_tran_id(), "fintech_use_num": fins[i % 2], "tran_amt": "100",
            "tran_dtime": api.state.clock.state().await.expect("clock").business_time.format("%Y%m%d%H%M%S").to_string(), "transfer_purpose": "TR", "recv_client_name": "최장애",
            "recv_client_bank_code": "998", "recv_client_account_num": accounts[(i + 1) % 2],
        });
        let (status, body) = call(
            &api.generator,
            "POST",
            "/v2.0/transfer/withdraw/fin_num",
            &new_id(),
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "transfers continue while MongoDB is down: {body}"
        );
        let (backlog, _) = outbox::backlog(&api.state.db).await.expect("backlog");
        max_backlog = max_backlog.max(backlog);
        i += 1;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    eprintln!("sent {i} transfers; max undelivered {max_backlog}");
    assert!(
        max_backlog > 100,
        "events accumulated while MongoDB was down (max {max_backlog})"
    );

    // MongoDB is back; wait for the relay to drain.
    let deadline = Instant::now() + Duration::from_secs(300);
    while outbox_count(&api).await > 0 {
        assert!(Instant::now() < deadline, "outbox did not drain");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    relay.abort();

    let phone2 = phone.clone();
    let customer_id: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&CUSTOMER_ID, &[&phone2]))
        .await
        .expect("customer");
    let scope = format!("C:{customer_id}");
    let ids: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&REQUEST_IDS, &[&scope]))
        .await
        .expect("ids");
    assert_eq!(ids.len(), i, "every transfer request is in Oracle");
    let delivered = events
        .count_documents(doc! { "request_id": { "$in": &ids }, "event_type": "request.completed" })
        .await
        .expect("count");
    assert_eq!(
        delivered as usize, i,
        "every transfer's event reached MongoDB exactly once"
    );
}
