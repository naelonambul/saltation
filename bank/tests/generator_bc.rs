//! Read-only proof of generator B-to-C recovery against Oracle and MongoDB.

use std::env;
use std::time::Duration;

use bank::features::outbox_relay;
use bank::shared::db::{Db, DbConfig, SqlStmt};
use bank::sql_stmt;
use mongodb::Collection;
use mongodb::bson::{Document, doc};
use serde::Deserialize;
use serde_json::Value;

const REQUEST: SqlStmt = sql_stmt!("test.generator_bc_request", "sql/generator_bc_request.sql");
const TRANSFER: SqlStmt = sql_stmt!(
    "test.generator_bc_transfer",
    "sql/generator_bc_transfer.sql"
);
const LEGS: SqlStmt = sql_stmt!("test.generator_bc_legs", "sql/generator_bc_legs.sql");
const BALANCE: SqlStmt = sql_stmt!("test.generator_bc_balance", "sql/generator_bc_balance.sql");
const COUNT_OUTBOX: SqlStmt = sql_stmt!("test.count_outbox", "sql/count_outbox.sql");

#[derive(Deserialize)]
struct Fixture {
    run_id: String,
    sender_account: String,
    receiver_account: String,
    sender_customer: String,
    receiver_customer: String,
    sender_opening: i64,
    receiver_opening: i64,
    transfer_amount: i64,
    withdraw_amount: i64,
    original_login_id: String,
    recovery_login_id: String,
    transfer_id: String,
    atm_start_id: String,
    withdraw_id: String,
}

fn required_env(name: &str) -> String {
    let value = env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(!value.trim().is_empty(), "{name} is empty");
    value
}

async fn assert_posting(
    db: &Db,
    request_id: &str,
    request_type: &str,
    kind: &str,
    amount: i64,
    legs: &[(&str, i64)],
) -> i64 {
    let id = request_id.to_owned();
    let (requests, transfers) = db
        .run(move |s| {
            let requests: Vec<(String, String, String)> = s.query_all(&REQUEST, &[&id])?;
            let transfers: Vec<(i64, String, i64)> = s.query_all(&TRANSFER, &[&id])?;
            Ok((requests, transfers))
        })
        .await
        .expect("read Oracle request and transfers");
    assert_eq!(
        requests,
        [(
            request_id.to_owned(),
            request_type.to_owned(),
            "DONE".into()
        )],
        "transaction_request for {request_id}"
    );
    assert_eq!(
        transfers.len(),
        1,
        "one Oracle transfer for {request_id}: {transfers:?}"
    );
    let (oracle_id, actual_kind, actual_amount) = &transfers[0];
    assert_eq!(actual_kind, kind, "transfer kind for {request_id}");
    assert_eq!(*actual_amount, amount, "transfer amount for {request_id}");
    let oracle_id = *oracle_id;
    let mut actual: Vec<(String, i64)> = db
        .run(move |s| s.query_all(&LEGS, &[&oracle_id]))
        .await
        .expect("read Oracle account_transaction legs");
    let mut expected: Vec<_> = legs
        .iter()
        .map(|(account, amount)| (account.to_string(), *amount))
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "signed ledger legs for {request_id}");
    oracle_id
}

async fn assert_balance(db: &Db, account: &str, customer: &str, balance: i64) {
    let account_no = account.to_owned();
    let actual: (String, i64) = db
        .run(move |s| s.query_row(&BALANCE, &[&account_no]))
        .await
        .expect("read Oracle account balance and owner");
    assert_eq!(actual, (customer.to_owned(), balance), "account {account}");
}

async fn assert_labels(labels: &Collection<Document>, fixture: &Fixture) {
    let b_instance = format!("{}:B", fixture.run_id);
    let c_instance = format!("{}:C", fixture.run_id);
    let mut expected = [
        (
            &fixture.original_login_id,
            "customer",
            &fixture.sender_customer,
            "B",
            &b_instance,
        ),
        (
            &fixture.recovery_login_id,
            "customer",
            &fixture.sender_customer,
            "B",
            &b_instance,
        ),
        (
            &fixture.transfer_id,
            "account",
            &fixture.sender_account,
            "B",
            &b_instance,
        ),
        (
            &fixture.transfer_id,
            "account",
            &fixture.receiver_account,
            "C",
            &c_instance,
        ),
        (
            &fixture.atm_start_id,
            "account",
            &fixture.receiver_account,
            "C",
            &c_instance,
        ),
        (
            &fixture.withdraw_id,
            "account",
            &fixture.receiver_account,
            "C",
            &c_instance,
        ),
    ]
    .map(|(request, subject_type, subject, scenario, instance)| {
        [
            request.to_owned(),
            subject_type.to_owned(),
            subject.to_owned(),
            scenario.to_owned(),
            scenario.to_owned(),
            instance.to_owned(),
        ]
    });
    let mut cursor = labels
        .find(doc! { "run_id": &fixture.run_id })
        .await
        .expect("find truth labels for the run");
    let mut actual = Vec::new();
    while cursor.advance().await.expect("read truth cursor") {
        let label = cursor.deserialize_current().expect("truth label");
        actual.push(
            [
                "request_id",
                "subject_type",
                "subject_id",
                "label",
                "scenario",
                "scenario_instance_id",
            ]
            .map(|field| {
                label
                    .get_str(field)
                    .unwrap_or_else(|_| panic!("truth label requires string {field}"))
                    .to_owned()
            }),
        );
    }
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "exact truth labels for run {}",
        fixture.run_id
    );
}

async fn wait_for_outbox(db: &Db) {
    let mut remaining = None;
    let drained = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let count: i64 = db
                .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
                .await
                .expect("read outbox count");
            if count == 0 {
                return;
            }
            remaining = Some(count);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await;
    assert!(
        drained.is_ok(),
        "outbox did not drain within 120s, last count: {remaining:?}"
    );
}

async fn events_for(
    events: &Collection<Document>,
    request_id: &str,
    event_type: &str,
) -> Vec<Value> {
    let mut cursor = events
        .find(doc! { "request_id": request_id, "event_type": event_type })
        .await
        .expect("find business events");
    let mut found = Vec::new();
    while cursor.advance().await.expect("read business event cursor") {
        let event = cursor.deserialize_current().expect("business event");
        found.push(serde_json::to_value(event).expect("business event JSON"));
    }
    found
}

fn assert_success(event: &Value, account: &str, request_type: &str) {
    assert_eq!(event["outcome"], "completed");
    assert_eq!(event["account_no"], account);
    assert_eq!(event["payload"]["http_status"], 200);
    assert_eq!(event["payload"]["request_type"], request_type);
}

async fn assert_events(
    events: &Collection<Document>,
    fixture: &Fixture,
    transfer: i64,
    withdrawal: i64,
) {
    let completions = events_for(events, &fixture.transfer_id, "request.completed").await;
    assert_eq!(
        completions.len(),
        2,
        "transfer must have one original and one replay completion"
    );
    let mut original = None;
    let mut replay = None;
    for event in &completions {
        assert_success(event, &fixture.sender_account, "TRANSFER");
        let slot = match event["payload"].get("replayed") {
            Some(Value::Bool(true)) => &mut replay,
            None => &mut original,
            flag => panic!("unexpected transfer replay flag: {flag:?}"),
        };
        assert!(
            slot.replace(event).is_none(),
            "duplicate transfer completion role"
        );
    }
    let original = original.expect("original transfer completion");
    let replay = replay.expect("replayed transfer completion");
    assert_eq!(original["payload"]["transfer_id"], transfer);
    assert_eq!(original["payload"]["tran_amt"], fixture.transfer_amount);
    assert_ne!(
        original["event_id"].as_str().expect("original event ID"),
        replay["event_id"].as_str().expect("replay event ID")
    );

    let cash = events_for(events, &fixture.withdraw_id, "request.completed").await;
    assert_eq!(cash.len(), 1, "one ATM withdrawal completion");
    let cash = &cash[0];
    assert_success(cash, &fixture.receiver_account, "ATM_WITHDRAW");
    assert!(cash["payload"].get("replayed").is_none());
    assert_eq!(cash["payload"]["transfer_id"], withdrawal);
    assert_eq!(cash["payload"]["tran_amt"], fixture.withdraw_amount);

    let starts = events_for(events, &fixture.atm_start_id, "atm.session_started").await;
    assert_eq!(starts.len(), 1, "one ATM session start");
    let start = &starts[0];
    assert_eq!(start["account_no"], fixture.receiver_account);
    assert_eq!(start["payload"]["account_no"], fixture.receiver_account);
    assert_eq!(start["payload"]["request_id"], fixture.atm_start_id);
    let session = start["payload"]["session_id"]
        .as_str()
        .expect("ATM start session ID");
    assert_eq!(start["session_id"], session);
    assert_eq!(
        cash["session_id"], session,
        "withdrawal used the started ATM session"
    );
}

#[tokio::test]
#[ignore = "requires the generator B-to-C recovery fixture and running Oracle and MongoDB"]
async fn generator_bc_recovery_proof() {
    let fixture: Fixture = serde_json::from_str(&required_env("GENERATOR_BC_FIXTURE"))
        .expect("GENERATOR_BC_FIXTURE JSON");
    let truth_uri = required_env("MONGO_TRUTH_URI");
    let events_uri = required_env("MONGO_URI");

    // Connect to the existing schema without running migrations or starting a relay.
    let db = Db::connect(&DbConfig::from_env().expect("Oracle config")).expect("connect to Oracle");
    let transfer = assert_posting(
        &db,
        &fixture.transfer_id,
        "TRANSFER",
        "transfer",
        fixture.transfer_amount,
        &[
            (&fixture.sender_account, -fixture.transfer_amount),
            (&fixture.receiver_account, fixture.transfer_amount),
        ],
    )
    .await;
    let withdrawal = assert_posting(
        &db,
        &fixture.withdraw_id,
        "ATM_WITHDRAW",
        "atm_cash",
        fixture.withdraw_amount,
        &[(&fixture.receiver_account, -fixture.withdraw_amount)],
    )
    .await;
    assert_balance(
        &db,
        &fixture.sender_account,
        &fixture.sender_customer,
        fixture.sender_opening - fixture.transfer_amount,
    )
    .await;
    assert_balance(
        &db,
        &fixture.receiver_account,
        &fixture.receiver_customer,
        fixture.receiver_opening + fixture.transfer_amount - fixture.withdraw_amount,
    )
    .await;

    let truth = mongodb::Client::with_uri_str(&truth_uri)
        .await
        .expect("connect to Mongo truth");
    let truth_db = truth
        .default_database()
        .unwrap_or_else(|| truth.database("truth"));
    assert_labels(&truth_db.collection("labels"), &fixture).await;

    wait_for_outbox(&db).await;
    let events = outbox_relay::events_collection(&events_uri)
        .await
        .expect("connect to Mongo events");
    assert_events(&events, &fixture, transfer, withdrawal).await;
}
