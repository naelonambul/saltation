//! Card foundation proofs; no authorization or posting endpoint.
mod common;
use axum::{
    Router,
    http::StatusCode,
    middleware::{from_fn, from_fn_with_state},
};
use bank::{
    features::cards,
    shared::{
        context::{self, Listener},
        db::SqlStmt,
        telemetry,
    },
    sql_stmt,
};
use common::*;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
const ENDPOINT: &str = "/internal/v1/seed/cards";
const COUNT: SqlStmt = sql_stmt!("test.card_count", "sql/card_count.sql");
const SAVED: SqlStmt = sql_stmt!("test.card_saved", "sql/card_saved.sql");
const OUTBOX: SqlStmt = sql_stmt!("test.card_outbox", "sql/card_outbox.sql");
async fn count(api: &Api, account: &str) -> i64 {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&COUNT, &[&account]))
        .await
        .unwrap()
}
async fn seed(router: &Router, rows: Vec<Value>) -> (StatusCode, Value) {
    call(
        router,
        "POST",
        ENDPOINT,
        &new_id(),
        None,
        Some(json!({"cards":rows})),
    )
    .await
}
fn row(account: &str, status: &str) -> Value {
    json!({"account_no":account,"status":status})
}
fn valid(number: &str) -> bool {
    number.len() == 16
        && number.starts_with("0000")
        && number.bytes().all(|b| b.is_ascii_digit())
        && number
            .bytes()
            .rev()
            .enumerate()
            .map(|(i, b)| {
                let n = u32::from(b - b'0') * if i % 2 == 0 { 1 } else { 2 };
                if n > 9 { n - 9 } else { n }
            })
            .sum::<u32>()
            .is_multiple_of(10)
}
fn fixed(api: &Api, sequence: Vec<String>) -> (Router, Arc<AtomicUsize>) {
    let sequence = Mutex::new(VecDeque::from(sequence));
    let calls = Arc::new(AtomicUsize::new(0));
    let source_calls = calls.clone();
    let router = cards::seed_router(move || {
        source_calls.fetch_add(1, Ordering::SeqCst);
        Ok(sequence
            .lock()
            .unwrap()
            .pop_front()
            .expect("number sequence exhausted"))
    })
    .layer(from_fn_with_state(api.state.clone(), context::operation))
    .layer(from_fn(|req, next| {
        telemetry::request_log(Listener::Generator, req, next)
    }))
    .layer(from_fn(|req, next| {
        context::attach(Listener::Generator, req, next)
    }))
    .with_state(api.state.clone());
    (router, calls)
}
#[test]
fn country_codes_mirror_contract_and_numbers_pass_luhn() {
    let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".into());
    let contract: Vec<String> = serde_json::from_str(
        &std::fs::read_to_string(format!("{dir}/codes/countries.json")).unwrap(),
    )
    .unwrap();
    let embedded: Vec<String> =
        serde_json::from_str(include_str!("../src/features/cards/countries.json")).unwrap();
    assert_eq!(contract, embedded);
    assert_eq!(contract.len(), 249);
    assert_eq!(contract.iter().collect::<HashSet<_>>().len(), 249);
    for _ in 0..1000 {
        assert!(valid(&cards::random_number().unwrap()));
    }
}
#[tokio::test]
async fn thousand_unique_cards_blocked_status_order_and_no_events() {
    let api = api().await;
    let a = customer(&api, 0).await;
    let b = customer(&api, 0).await;
    let mut numbers = HashSet::new();
    for _ in 0..2 {
        let rows: Vec<_> = (0..500)
            .map(|i| {
                row(
                    if i % 2 == 0 { &a.account } else { &b.account },
                    if i % 2 == 0 { "ACTIVE" } else { "BLOCKED" },
                )
            })
            .collect();
        let id = new_id();
        let (http, body) = call(
            &api.generator,
            "POST",
            ENDPOINT,
            &id,
            None,
            Some(json!({"cards":rows})),
        )
        .await;
        assert_eq!(http, StatusCode::OK, "{body}");
        assert_contract("SeedCardsResponse", &body);
        for (i, card) in body["cards"].as_array().unwrap().iter().enumerate() {
            assert_eq!(card["account_no"], rows[i]["account_no"]);
            assert_eq!(card["status"], rows[i]["status"]);
            let no = card["card_no"].as_str().unwrap();
            assert!(valid(no));
            assert!(numbers.insert(no.to_owned()));
            let no = no.to_owned();
            let stored: (String, String) = api
                .state
                .db
                .run(move |s| s.query_row(&SAVED, &[&no]))
                .await
                .unwrap();
            assert_eq!(
                stored,
                (
                    card["account_no"].as_str().unwrap().into(),
                    card["status"].as_str().unwrap().into()
                )
            );
        }
        let events: i64 = api
            .state
            .db
            .run(move |s| s.query_row(&OUTBOX, &[&id]))
            .await
            .unwrap();
        assert_eq!(events, 0);
    }
    assert_eq!(numbers.len(), 1000);
    assert_eq!(count(&api, &a.account).await, 500);
    assert_eq!(count(&api, &b.account).await, 500);
    assert_eq!(
        seed(&api.public, vec![row(&a.account, "ACTIVE")]).await.0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn invalid_batches_write_nothing() {
    let api = api().await;
    let good = customer(&api, 0).await;
    let other = customer(&api, 0).await;
    for subtype in ["TERM", "LOAN"] {
        let account = other.account.clone();
        api.state
            .db
            .run(move |s| {
                s.execute(
                    &sql_stmt!("test.card_subtype", "sql/card_subtype.sql"),
                    &[&subtype, &account],
                )?;
                s.commit()
            })
            .await
            .unwrap();
        let (http, body) = seed(
            &api.generator,
            vec![row(&good.account, "ACTIVE"), row(&other.account, "ACTIVE")],
        )
        .await;
        assert_eq!(http, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(count(&api, &good.account).await, 0);
        assert_eq!(count(&api, &other.account).await, 0);
    }
    for rows in [
        vec![row(&good.account, "ACTIVE"), row("1", "ACTIVE")],
        vec![row(&good.account, "ACTIVE"), row(&good.account, "OTHER")],
        vec![],
        vec![row(&good.account, "ACTIVE"); 501],
    ] {
        let (http, body) = seed(&api.generator, rows).await;
        assert_eq!(http, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(count(&api, &good.account).await, 0);
    }
}
#[tokio::test]
async fn collisions_retry_and_sixth_collision_rolls_back_the_batch() {
    let api = api().await;
    let a = customer(&api, 0).await;
    let existing = cards::random_number().unwrap();
    let next = cards::random_number().unwrap();
    let earlier = cards::random_number().unwrap();
    let (router, initial_calls) = fixed(&api, vec![existing.clone()]);
    assert_eq!(
        seed(&router, vec![row(&a.account, "ACTIVE")]).await.0,
        StatusCode::OK
    );
    assert_eq!(initial_calls.load(Ordering::SeqCst), 1);
    let mut sequence = vec![existing.clone(); 5];
    sequence.push(next.clone());
    let (router, retry_calls) = fixed(&api, sequence);
    let (http, body) = seed(&router, vec![row(&a.account, "BLOCKED")]).await;
    assert_eq!(http, StatusCode::OK, "{body}");
    assert_eq!(body["cards"][0]["card_no"], next);
    assert_eq!(retry_calls.load(Ordering::SeqCst), 6);
    let mut sequence = vec![earlier.clone()];
    sequence.extend(vec![existing; 6]);
    let (router, failed_calls) = fixed(&api, sequence);
    let (http, body) = seed(
        &router,
        vec![row(&a.account, "ACTIVE"), row(&a.account, "ACTIVE")],
    )
    .await;
    assert_eq!(http, StatusCode::CONFLICT, "{body}");
    // One call inserted the first card; exactly six collided for the second.
    assert_eq!(failed_calls.load(Ordering::SeqCst), 7);
    assert_eq!(failed_calls.load(Ordering::SeqCst) - 1, 6);
    assert_eq!(count(&api, &a.account).await, 2);
    let (router, reuse_calls) = fixed(&api, vec![earlier.clone()]);
    let (http, body) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        seed(&router, vec![row(&a.account, "ACTIVE")]),
    )
    .await
    .expect("failed batch must release its unique-key lock");
    assert_eq!(http, StatusCode::OK, "{body}");
    assert_eq!(body["cards"][0]["card_no"], earlier);
    assert_eq!(reuse_calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&api, &a.account).await, 3);
}
