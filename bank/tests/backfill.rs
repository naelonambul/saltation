//! One ordered sequence over a freshly reset schema, including crash recovery.
mod common;
#[path = "common/daily_close.rs"]
mod daily_close_tests;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use bank::app::{self, AppState};
use bank::shared::auth::token_hash;
use bank::shared::clock::{Clock, ClockError, ClockMode, business_date, kst};
use bank::shared::context::{self, BusinessTime, Listener};
use bank::shared::db::{Db, DbConfig, DbError, SqlStmt};
use bank::shared::rollover::{JobFuture, Registry, RolloverJob, mark_finished};
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use common::{Api, call, envelope_errors, new_id};
use serde_json::{Value, json};

const FINISHED_ROWS: SqlStmt = sql_stmt!("test.rollover_finished", "sql/rollover_finished.sql");
const EVENTS: SqlStmt = sql_stmt!("test.rollover_events", "sql/rollover_events.sql");
const ROLLOVER_ROWS: SqlStmt = sql_stmt!("test.rollover_rows", "sql/rollover_rows.sql");
const EXPIRY: SqlStmt = sql_stmt!("test.token_expiry", "sql/token_expiry.sql");

#[derive(Default)]
struct Recording {
    calls: Vec<(NaiveDate, &'static str)>,
    attempts: Vec<(NaiveDate, &'static str)>,
    fail_once: Option<NaiveDate>,
    sleep_on: Option<NaiveDate>,
}

struct Job {
    name: &'static str,
    recording: Arc<Mutex<Recording>>,
}

impl RolloverJob for Job {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run<'a>(&'a self, db: &'a Db, date: NaiveDate, at: DateTime<FixedOffset>) -> JobFuture<'a> {
        Box::pin(async move {
            assert_eq!(at, bank::shared::rollover::last_instant(date));
            let sleep = {
                let mut r = self.recording.lock().expect("recording");
                r.attempts.push((date, self.name));
                if self.name == "second" && r.fail_once == Some(date) {
                    r.fail_once = None;
                    return Err(DbError::Config("injected failure".into()));
                }
                self.name == "second" && r.sleep_on == Some(date)
            };
            if sleep {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            // Exercise finishing inside the job's own last transaction and
            // repeated marking: roll_date's later mark must emit no duplicate.
            if self.name == "first" {
                let name = self.name;
                db.run(move |s| {
                    mark_finished(s, date, name, at)?;
                    mark_finished(s, date, name, at)?;
                    s.commit()
                })
                .await?;
            }
            self.recording
                .lock()
                .expect("recording")
                .calls
                .push((date, self.name));
            Ok(())
        })
    }
}

async fn post(api: &Api, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    call(&api.generator, "POST", path, &new_id(), None, body).await
}

async fn read(api: &Api) -> Value {
    let (status, body) = call(
        &api.generator,
        "GET",
        "/internal/v1/clock",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("pending_rollover").is_none());
    body
}

async fn set(api: &Api, at: DateTime<FixedOffset>) {
    let (status, body) = post(
        api,
        "/internal/v1/clock/set",
        Some(json!({"business_time": at.to_rfc3339()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        DateTime::parse_from_rfc3339(body["business_time"].as_str().expect("time")).expect("time"),
        at
    );
}

async fn now(api: &Api) -> DateTime<FixedOffset> {
    api.state.clock.state().await.expect("clock").business_time
}

fn expected_dates(start: NaiveDate, end: NaiveDate) -> Vec<(NaiveDate, &'static str)> {
    let mut date = start;
    let mut expected = Vec::new();
    while date < end {
        expected.extend([(date, "first"), (date, "second")]);
        date = date.succ_opt().expect("date");
    }
    expected
}

async fn starts_in_backfill(api: &Api) {
    assert_eq!(read(api).await["mode"], "BACKFILL");
}

async fn set_moves_back_without_customers(api: &Api) -> NaiveDate {
    let date = Utc::now().with_timezone(&kst()).date_naive() - chrono::Duration::days(400);
    let at = kst()
        .from_local_datetime(&date.and_hms_opt(12, 0, 0).expect("time"))
        .single()
        .expect("KST");
    set(api, at).await;
    let body = read(api).await;
    assert_eq!(
        body["rolled_through"],
        date.pred_opt().expect("date").to_string()
    );
    date
}

async fn rewind_forgets_rollover_runs(api: &Api, recording: &Arc<Mutex<Recording>>) -> NaiveDate {
    let before = now(api).await;
    let date = business_date(before);
    api.state.clock.advance(86400).await.expect("first advance");
    set(api, before).await;
    let date_text = date.to_string();
    let rows: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&ROLLOVER_ROWS, &[&date_text]))
        .await
        .expect("rollover rows");
    assert_eq!(rows, 0);
    api.state
        .clock
        .advance(86400)
        .await
        .expect("repeat advance");
    {
        let mut r = recording.lock().expect("recording");
        for name in ["first", "second"] {
            assert_eq!(
                r.calls.iter().filter(|call| **call == (date, name)).count(),
                2
            );
        }
        *r = Recording::default();
    }
    business_date(now(api).await)
}

async fn jobs_run_once_per_crossed_date_in_order(
    api: &Api,
    recording: &Arc<Mutex<Recording>>,
    start: NaiveDate,
) {
    // `recorded_at` has millisecond precision, so compare from the start of this millisecond.
    let wall_before = Utc::now() - chrono::Duration::milliseconds(1);
    let (status, body) = post(
        api,
        "/internal/v1/clock/advance",
        Some(json!({"seconds": 3 * 86400 + 3600})),
    )
    .await;
    let wall_after = Utc::now();
    assert_eq!(status, StatusCode::OK, "{body}");
    let d3 = start + chrono::Duration::days(3);
    assert_eq!(
        recording.lock().expect("recording").calls,
        expected_dates(start, d3)
    );
    assert_eq!(
        body["rolled_through"],
        d3.pred_opt().expect("date").to_string()
    );
    let start_text = start.to_string();
    let count: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&FINISHED_ROWS, &[&start_text]))
        .await
        .expect("rows");
    assert_eq!(count, 6);
    let start_text = start.to_string();
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENTS, &[&start_text]))
        .await
        .expect("events");
    assert_eq!(events.len(), 6);
    for (event, (date, job)) in events.iter().zip(expected_dates(start, d3)) {
        let event: Value = serde_json::from_str(event).expect("event JSON");
        let errors = envelope_errors(&event);
        assert!(errors.is_empty(), "{errors:?}: {event}");
        assert!(event.get("request_id").is_some());
        assert!(event.get("listener").is_some());
        let recorded =
            DateTime::parse_from_rfc3339(event["recorded_at"].as_str().expect("recorded_at"))
                .expect("wall timestamp");
        assert!(recorded >= wall_before && recorded <= wall_after);
        assert_eq!(event["event_type"], "rollover.job_completed");
        assert_eq!(event["channel"], "SYSTEM");
        assert!(event["request_id"].is_null());
        assert!(event["listener"].is_null());
        assert_eq!(event["human"], false);
        assert_eq!(event["outcome"], "completed");
        assert_eq!(
            event["payload"],
            json!({"business_date": date.to_string(), "job": job})
        );
        assert_eq!(
            event["occurred_at"],
            bank::shared::rollover::last_instant(date).to_rfc3339()
        );
    }
}

async fn failed_job_leaves_the_clock_and_retry_skips_finished(
    api: &Api,
    recording: &Arc<Mutex<Recording>>,
) {
    let before = api.state.clock.state().await.expect("clock");
    let date = business_date(before.business_time);
    recording.lock().expect("recording").fail_once = Some(date);
    assert!(matches!(
        api.state.clock.advance(86400).await,
        Err(ClockError::Db(_))
    ));
    let failed = api.state.clock.state().await.expect("clock");
    assert_eq!(failed.business_time, before.business_time);
    assert_eq!(failed.rolled_through, before.rolled_through);
    let retry = api.state.clock.advance(86400).await.expect("retry");
    assert_eq!(
        retry.business_time,
        before.business_time + chrono::Duration::days(1)
    );
    assert_eq!(retry.rolled_through, date);
    let r = recording.lock().expect("recording");
    for name in ["first", "second"] {
        assert_eq!(
            r.calls.iter().filter(|call| **call == (date, name)).count(),
            1
        );
    }
    assert_eq!(
        r.attempts
            .iter()
            .filter(|call| **call == (date, "first"))
            .count(),
        1
    );
    assert_eq!(
        r.attempts
            .iter()
            .filter(|call| **call == (date, "second"))
            .count(),
        2
    );
}

async fn closed_date_refuses_operations(api: &Api, recording: &Arc<Mutex<Recording>>) {
    let before = api.state.clock.state().await.expect("clock");
    let first = business_date(before.business_time);
    recording.lock().expect("recording").fail_once = first.succ_opt();
    assert!(matches!(
        api.state.clock.advance(2 * 86400).await,
        Err(ClockError::Db(_))
    ));
    let failed = api.state.clock.state().await.expect("clock");
    assert_eq!(failed.business_time, before.business_time);
    assert_eq!(failed.rolled_through, first);
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["rsp_code"], "A0013");
    let retry = api.state.clock.advance(2 * 86400).await.expect("retry");
    assert_eq!(
        retry.business_time,
        before.business_time + chrono::Duration::days(2)
    );
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    // Move forward to noon so expiry tests retain their original alignment.
    let noon = kst()
        .from_local_datetime(
            &business_date(retry.business_time)
                .succ_opt()
                .expect("date")
                .and_hms_opt(12, 0, 0)
                .expect("noon"),
        )
        .single()
        .expect("KST");
    set(api, noon).await;
}

async fn failed_rollover_refuses_operations(api: &Api, recording: &Arc<Mutex<Recording>>) {
    let before = api.state.clock.state().await.expect("clock");
    let date = business_date(before.business_time);
    recording.lock().expect("recording").fail_once = Some(date);
    assert!(matches!(
        api.state.clock.advance(86400).await,
        Err(ClockError::Db(_))
    ));
    let failed = api.state.clock.state().await.expect("clock");
    assert_eq!(failed.business_time, before.business_time);
    assert_eq!(failed.rolled_through, before.rolled_through);
    assert!(failed.pending_rollover);
    {
        let r = recording.lock().expect("recording");
        assert!(r.calls.contains(&(date, "first")));
        assert!(!r.calls.contains(&(date, "second")));
    }
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["rsp_code"], "A0013");
    let retry = api.state.clock.advance(86400).await.expect("retry");
    assert_eq!(
        retry.business_time,
        before.business_time + chrono::Duration::days(1)
    );
    assert_eq!(retry.rolled_through, date);
    assert!(!retry.pending_rollover);
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

async fn set_backward_is_rejected_with_customers(api: &Api) {
    let (status, body) = post(
        api,
        "/internal/v1/seed/customers",
        Some(json!({"customers": [{
            "customer_no": "C_BACKFILL", "name": "시계", "phone": "01000004000", "address": "서울",
            "customer_type": "INDIVIDUAL", "password": "pw-backfill"
        }]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = now(api).await;
    let (status, body) = post(
        api,
        "/internal/v1/clock/set",
        Some(json!({"business_time": (before - chrono::Duration::seconds(1)).to_rfc3339()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["rsp_code"], "A0002");
    assert_eq!(now(api).await, before);
    set(api, before + chrono::Duration::seconds(1)).await;
}

async fn advance_waits_for_an_operation_then_times_out(api: &Api) {
    let clock = api
        .state
        .clock
        .clone()
        .with_barrier_wait(Duration::from_secs(1));
    let ticket = clock.enter().await.expect("ticket");
    let start = Instant::now();
    assert!(matches!(clock.advance(1).await, Err(ClockError::Busy)));
    assert!(start.elapsed() >= Duration::from_secs(1));
    drop(ticket);
    clock.advance(1).await.expect("advance after operation");
}

async fn operations_wait_during_an_advance(api: &Api, recording: &Arc<Mutex<Recording>>) {
    let date = business_date(now(api).await);
    recording.lock().expect("recording").sleep_on = Some(date);
    let clock = api.state.clock.clone();
    let advance = tokio::spawn(async move {
        clock.advance(86400).await.expect("slow advance");
        Instant::now()
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let (status, _) = call(
        &api.generator,
        "GET",
        "/v2.0/account/list",
        &new_id(),
        Some("any"),
        None,
    )
    .await;
    let completed = Instant::now();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(completed >= advance.await.expect("advance task"));
    assert!(started.elapsed() >= Duration::from_millis(1000));
    recording.lock().expect("recording").sleep_on = None;
}

async fn slow_operation(
    State(state): State<AppState>,
    BusinessTime(_): BusinessTime,
) -> StatusCode {
    state
        .db
        .run(|_| {
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        })
        .await
        .expect("slow db");
    StatusCode::OK
}

async fn cancelled_request_keeps_the_barrier(api: &Api) {
    let router = Router::new()
        .route("/slow", get(slow_operation))
        .layer(from_fn_with_state(api.state.clone(), context::operation))
        .layer(from_fn(|req, next| {
            context::attach(Listener::Generator, req, next)
        }))
        .with_state(api.state.clone());
    let started = Instant::now();
    {
        let request = call(&router, "GET", "/slow", "cancelled", None, None);
        tokio::pin!(request);
        tokio::select! {
            _ = &mut request => panic!("request finished before cancellation"),
            () = tokio::time::sleep(Duration::from_millis(300)) => {}
        }
    }
    api.state
        .clock
        .advance(1)
        .await
        .expect("advance waits for detached operation");
    assert!(started.elapsed() >= Duration::from_millis(1500));
}

async fn cancelled_advance_keeps_the_barrier(api: &Api, recording: &Arc<Mutex<Recording>>) {
    let before = now(api).await;
    recording.lock().expect("recording").sleep_on = Some(business_date(before));
    let started = Instant::now();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), api.state.clock.advance(86400))
            .await
            .is_err()
    );
    let ticket = api
        .state
        .clock
        .enter()
        .await
        .expect("ticket after cancellation");
    assert!(started.elapsed() >= Duration::from_millis(1200));
    assert_eq!(ticket.now, before + chrono::Duration::days(1));
    assert_eq!(now(api).await, before + chrono::Duration::days(1));
    recording.lock().expect("recording").sleep_on = None;
}

async fn token_check_and_handler_share_one_business_time(api: &Api) {
    let (status, login) = post(
        api,
        "/v2.0/ext/auth/login",
        Some(json!({"phone": "01000004000", "password": "pw-backfill"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let token = login["access_token"].as_str().expect("token");
    let hash = token_hash(token);
    let hash1 = hash.clone();
    let e1: DateTime<FixedOffset> = api
        .state
        .db
        .run(move |s| s.query_row(&EXPIRY, &[&hash1]))
        .await
        .expect("expiry");
    set(api, e1 - chrono::Duration::seconds(1)).await;
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/user/me",
        &new_id(),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e2: DateTime<FixedOffset> = api
        .state
        .db
        .run(move |s| s.query_row(&EXPIRY, &[&hash]))
        .await
        .expect("extended expiry");
    assert!(e2 > e1);
    set(api, e2 + chrono::Duration::seconds(1)).await;
    let (status, body) = call(
        &api.generator,
        "GET",
        "/v2.0/user/me",
        &new_id(),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

const DORMANCY_ACCOUNT: SqlStmt = sql_stmt!("test.dormancy_account", "sql/dormancy_account.sql");
const DORMANCY_HISTORY: SqlStmt = sql_stmt!("test.dormancy_history", "sql/dormancy_history.sql");
const DORMANCY_EVENTS: SqlStmt = sql_stmt!("test.dormancy_events", "sql/dormancy_events.sql");

type AccountState = (String, i64, Option<DateTime<FixedOffset>>);
type StatusChange = (String, String, String, DateTime<FixedOffset>);

async fn dormancy_account(api: &Api, account: &str) -> AccountState {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&DORMANCY_ACCOUNT, &[&account]))
        .await
        .expect("account")
}

async fn dormancy_history(api: &Api, account: &str) -> Vec<StatusChange> {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_all(&DORMANCY_HISTORY, &[&account]))
        .await
        .expect("history")
}

async fn dormancy_events(api: &Api, account: &str) -> Vec<Value> {
    let account = account.to_owned();
    let events: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&DORMANCY_EVENTS, &[&account]))
        .await
        .expect("events");
    events
        .iter()
        .map(|event| {
            let event: Value = serde_json::from_str(event).expect("JSON");
            let errors = envelope_errors(&event);
            assert!(errors.is_empty(), "{errors:?}: {event}");
            event
        })
        .collect()
}

async fn dormancy_login(api: &Api, phone: &str) -> String {
    let (status, body) = post(
        api,
        "/v2.0/ext/auth/login",
        Some(json!({"phone": phone, "password": "pw-dormancy"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().expect("token").to_owned()
}

async fn dormancy_transfer(
    api: &Api,
    token: &str,
    from: &str,
    to: &str,
) -> (StatusCode, Value, String) {
    let request = new_id();
    let (status, body) = call(
        &api.generator,
        "POST",
        "/v2.0/transfer/withdraw/fin_num",
        &request,
        Some(token),
        Some(json!({
            "bank_tran_id": common::bank_tran_id(), "fintech_use_num": from, "tran_amt": "1000",
            "tran_dtime": now(api).await.format("%Y%m%d%H%M%S").to_string(),
            "transfer_purpose": "TR", "recv_client_name": "휴면",
            "recv_client_bank_code": "998", "recv_client_account_num": to
        })),
    )
    .await;
    (status, body, request)
}

async fn dormancy_and_reactivation(api: &Api) {
    let d0 = business_date(now(api).await);
    let (status, body) = post(api, "/internal/v1/seed/customers", Some(json!({"customers": [
        {"customer_no": "C_DORM_A", "name": "휴면A", "phone": "01000004001", "address": "서울", "customer_type": "INDIVIDUAL", "password": "pw-dormancy"},
        {"customer_no": "C_DORM_B", "name": "휴면B", "phone": "01000004002", "address": "서울", "customer_type": "INDIVIDUAL", "password": "pw-dormancy"}
    ]})))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (x, y, z) = ("970000040001", "970000040002", "970000040003");
    let (status, body) = post(
        api,
        "/internal/v1/seed/accounts",
        Some(json!({"accounts": [
            {"customer_no": "C_DORM_A", "atm_pin": common::TEST_ATM_PIN, "account_no": x, "opening_balance": 50000},
            {"customer_no": "C_DORM_A", "atm_pin": common::TEST_ATM_PIN, "account_no": y},
            {"customer_no": "C_DORM_B", "atm_pin": common::TEST_ATM_PIN, "account_no": z, "opening_balance": 50000}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fins: Vec<String> = body["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .map(|a| a["fintech_use_num"].as_str().expect("fin").to_owned())
        .collect();
    assert_eq!(dormancy_account(api, x).await.1, 50_000);
    assert_eq!(dormancy_account(api, y).await.1, 0);
    assert_eq!(dormancy_account(api, z).await.1, 50_000);
    let original_activity = dormancy_account(api, x).await.2;
    assert!(
        original_activity.is_none(),
        "opening balance is not customer activity"
    );

    api.state
        .clock
        .advance(365 * 86400)
        .await
        .expect("365 days");
    for account in [x, y, z] {
        assert_eq!(dormancy_account(api, account).await.0, "ACTIVE");
    }
    api.state.clock.advance(86400).await.expect("366th day");
    for account in [x, y, z] {
        assert_eq!(dormancy_account(api, account).await.0, "DORMANT");
    }
    let dormant_at = bank::shared::rollover::last_instant(d0 + chrono::Duration::days(365));
    let first = (
        "ACTIVE".into(),
        "DORMANT".into(),
        "dormancy".into(),
        dormant_at,
    );
    assert_eq!(dormancy_history(api, x).await, vec![first.clone()]);
    let events = dormancy_events(api, x).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["channel"], "SYSTEM");
    assert!(events[0]["request_id"].is_null());
    assert_eq!(events[0]["account_no"], x);
    assert_eq!(events[0]["payload"]["to"], "DORMANT");
    assert_eq!(events[0]["occurred_at"], dormant_at.to_rfc3339());

    let date = d0 + chrono::Duration::days(365);
    for _ in 0..2 {
        bank::features::accounts_lifecycle::DormancyJob
            .run(
                &api.state.db,
                date,
                bank::shared::rollover::last_instant(date),
            )
            .await
            .expect("direct dormancy rerun");
        assert_eq!(dormancy_history(api, x).await, vec![first.clone()]);
        assert_eq!(dormancy_events(api, x).await.len(), 1);
    }

    let b = dormancy_login(api, "01000004002").await;
    let (status, body, _) = dormancy_transfer(api, &b, &fins[2], x).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(dormancy_account(api, z).await.0, "ACTIVE");
    let state = dormancy_account(api, x).await;
    assert_eq!(state.0, "DORMANT");
    assert_eq!(state.1, 51_000);
    assert_eq!(state.2, original_activity);

    let a = dormancy_login(api, "01000004001").await;
    let y_history = dormancy_history(api, y).await;
    let (status, body, _) = dormancy_transfer(api, &a, &fins[1], z).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["bank_rsp_code"], "405");
    assert_eq!(dormancy_account(api, y).await.0, "DORMANT");
    assert_eq!(dormancy_history(api, y).await, y_history);

    let active_at = now(api).await;
    let (status, body, request) = dormancy_transfer(api, &a, &fins[0], y).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for account in [x, y] {
        let state = dormancy_account(api, account).await;
        assert_eq!(state.0, "ACTIVE");
        assert_eq!(state.2, Some(active_at));
        let history = dormancy_history(api, account).await;
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1],
            (
                "DORMANT".into(),
                "ACTIVE".into(),
                "customer_activity".into(),
                active_at
            )
        );
    }
    assert_eq!(dormancy_account(api, x).await.1, 50_000);
    assert_eq!(dormancy_account(api, y).await.1, 1_000);
    let (status, body, _) = dormancy_transfer(api, &a, &fins[0], z).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(dormancy_account(api, x).await.0, "ACTIVE");
    assert_eq!(dormancy_account(api, x).await.2, Some(active_at));
    let expected = vec![
        first,
        (
            "DORMANT".into(),
            "ACTIVE".into(),
            "customer_activity".into(),
            active_at,
        ),
    ];
    assert_eq!(dormancy_history(api, x).await, expected);
    let events = dormancy_events(api, x).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["account_no"], x);
    assert_eq!(events[1]["channel"], "MOBILE");
    assert_eq!(events[1]["request_id"], request);
    assert_eq!(events[1]["payload"]["to"], "ACTIVE");

    api.state.clock.advance(86400).await.expect("one more day");
    assert_eq!(dormancy_history(api, x).await, expected);
    assert_eq!(dormancy_events(api, x).await.len(), 2);
}

async fn term_deposit_open(api: &Api) -> (String, String, i64, i64, i64, String) {
    use common::deposits::*;
    let phone = format!("010{}", common::unique_digits(10));
    let no = format!("D{}", common::unique_digits(10));
    let account = format!("94{}", common::unique_digits(10));
    let (status, body) = post(api, "/internal/v1/seed/customers", Some(json!({"customers": [{
        "customer_no": no, "phone": phone, "name": "예금", "customer_type": "INDIVIDUAL", "password": "deposit-pass"
    }]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_contract("CreatedResponse", &body);
    let (status, body) = post(
        api,
        "/internal/v1/seed/accounts",
        Some(json!({"accounts": [{
            "customer_no": no, "atm_pin": common::TEST_ATM_PIN, "account_no": account, "opening_balance": 1000000
        }]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_contract("SeedAccountsResponse", &body);
    let fin = body["accounts"][0]["fintech_use_num"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, login) = post(
        api,
        "/v2.0/ext/auth/login",
        Some(json!({"phone": phone, "password": "deposit-pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    common::assert_contract("LoginResponse", &login);
    let token = login["access_token"].as_str().unwrap();
    let start = snapshot(api, &fin).await.0;
    let first = request(
        api,
        token,
        "POST",
        "/v2.0/ext/deposits",
        Some(open_payload(api, &fin, 100000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let second = request(
        api,
        token,
        "POST",
        "/v2.0/ext/deposits",
        Some(open_payload(api, &fin, 100000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let first_id = first["deposit_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let second_id = second["deposit_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let maturity = first["maturity_date"].as_str().unwrap().to_owned();
    assert_eq!(
        maturity,
        business_date(now(api).await)
            .checked_add_months(chrono::Months::new(6))
            .unwrap()
            .format("%Y%m%d")
            .to_string()
    );
    let closed = request(
        api,
        token,
        "POST",
        &format!("/v2.0/ext/deposits/{second_id}/close"),
        Some(close_payload(api).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    assert_eq!(closed["close_type"], "EARLY");
    assert_eq!(snapshot(api, &fin).await.0, start - 100000);
    (phone, fin, start, first_id, second_id, maturity)
}

async fn term_deposit_matured(api: &Api, saved: (String, String, i64, i64, i64, String)) {
    use common::deposits::*;
    let (phone, fin, start, first, second, maturity) = saved;
    assert!(business_date(now(api).await).format("%Y%m%d").to_string() > maturity);
    let (status, login) = post(
        api,
        "/v2.0/ext/auth/login",
        Some(json!({"phone": phone, "password": "deposit-pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    common::assert_contract("LoginResponse", &login);
    let token = login["access_token"].as_str().unwrap();
    let path = format!("/v2.0/ext/deposits/{first}");
    let body = request(
        api,
        token,
        "GET",
        &path,
        None,
        StatusCode::OK,
        "DepositResponse",
    )
    .await;
    assert_eq!(body["account_state"], "09");
    assert_eq!(body["balance_amt"], "0");
    assert_eq!(body["close_type"], "MATURITY");
    assert_eq!(body["close_date"], maturity);
    assert_eq!(snapshot(api, &fin).await.0, start);
    let rows = movements(api, first).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[1],
        (
            "term_close".into(),
            2,
            0,
            maturity.clone(),
            maturity.clone(),
            0
        )
    );
    let date = maturity.clone();
    let finished: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&ROLLOVER, &[&date]))
        .await
        .unwrap();
    assert_eq!(finished, 1);
    // The maturity close has its status history row and status event.
    let closure: (String, i64, i64) = api
        .state
        .db
        .run(move |s| s.query_row(&CLOSURE, &[&first]))
        .await
        .unwrap();
    assert_eq!(closure, ("MATURITY".into(), 1, 1));
    let body = request(
        api,
        token,
        "GET",
        &format!("/v2.0/ext/deposits/{second}"),
        None,
        StatusCode::OK,
        "DepositResponse",
    )
    .await;
    assert_eq!(body["close_type"], "EARLY");
    let rows = movements(api, second).await;
    assert_eq!(rows.iter().filter(|r| r.0 == "term_close").count(), 1);
    let body = request(
        api,
        token,
        "POST",
        &format!("{path}/close"),
        Some(close_payload(api).await),
        StatusCode::UNPROCESSABLE_ENTITY,
        "BankRejected",
    )
    .await;
    assert_eq!(body["bank_rsp_code"], "403");
    // Run the job directly again: a repeated settlement cannot add money or rows.
    let date = NaiveDate::parse_from_str(&maturity, "%Y%m%d").unwrap();
    bank::features::deposits::MaturityJob
        .run(
            &api.state.db,
            date,
            bank::shared::rollover::last_instant(date),
        )
        .await
        .unwrap();
    assert_eq!(movements(api, first).await.len(), 2);
    assert_eq!(snapshot(api, &fin).await.0, start);
}

async fn password_reset_code_expires(api: &Api) {
    let phone = format!("010{}", common::unique_digits(10));
    let no = format!("R{}", common::unique_digits(10));
    let (status, body) = post(api, "/internal/v1/seed/customers", Some(json!({"customers": [{
        "customer_no": no, "phone": phone, "name": "만료", "customer_type": "INDIVIDUAL", "password": "reset-before"
    }]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(
        api,
        "/v2.0/ext/auth/password_reset/request",
        Some(json!({"phone": phone})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &api.generator,
        "GET",
        &format!("/internal/v1/sms/{phone}"),
        &new_id(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_contract("SmsResponse", &body);
    let code = body["code"].as_str().unwrap().to_owned();
    api.state.clock.advance(300).await.expect("five minutes");
    let (status, body) = post(
        api,
        "/v2.0/ext/auth/password_reset/confirm",
        Some(json!({"phone": phone, "code": code, "new_password": "reset-after"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["rsp_code"], "O0002");
    common::assert_contract("Error", &body);
}

async fn daily_maintenance_window(api: &Api) {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut maint_state = api.state.clone();
    maint_state.maintenance_minutes = 30;
    let maint_public = app::public_router(maint_state.clone());
    let maint_gen = app::generator_router(maint_state);

    let now = api.state.clock.state().await.expect("clock").business_time;
    let next_day = business_date(now).succ_opt().expect("next day");
    let midnight = kst()
        .from_local_datetime(&next_day.and_hms_opt(0, 0, 0).expect("midnight"))
        .single()
        .expect("kst midnight");
    api.state
        .clock
        .set(midnight)
        .await
        .expect("advance to kst midnight");

    let sender_no = format!("M{}", common::unique_digits(10));
    let recv_no = format!("M{}", common::unique_digits(10));
    let sender_phone = format!("010{}", common::unique_digits(8));
    let recv_phone = format!("010{}", common::unique_digits(8));
    let (status, body) = call(
        &maint_gen,
        "POST",
        "/internal/v1/seed/customers",
        &new_id(),
        None,
        Some(json!({"customers": [
            {"customer_no": sender_no, "phone": sender_phone, "name": "점검송신", "customer_type": "INDIVIDUAL", "password": "maint-password"},
            {"customer_no": recv_no, "phone": recv_phone, "name": "점검수신", "customer_type": "INDIVIDUAL", "password": "maint-password"},
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sender_acct = format!("998{}", common::unique_digits(11));
    let recv_acct = format!("998{}", common::unique_digits(11));
    let (status, body) = call(
        &maint_gen,
        "POST",
        "/internal/v1/seed/accounts",
        &new_id(),
        None,
        Some(json!({"accounts": [
            {"customer_no": sender_no, "atm_pin": common::TEST_ATM_PIN, "account_no": sender_acct, "opening_balance": 1_000_000},
            {"customer_no": recv_no, "atm_pin": common::TEST_ATM_PIN, "account_no": recv_acct, "opening_balance": 0},
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sender_fin = body["accounts"][0]["fintech_use_num"]
        .as_str()
        .expect("sender fin")
        .to_owned();

    // Seed an analyst to prove /admin/* remains available during maintenance.
    let analyst_no = format!("A{}", common::unique_digits(8));
    let (status, body) = call(
        &maint_gen,
        "POST",
        "/internal/v1/seed/analysts",
        &new_id(),
        None,
        Some(json!({"analysts": [
            {"analyst_no": analyst_no, "name": "점검분석가", "password": "maint-admin-password"},
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let tran_id = format!("M000000001U{}", common::unique_digits(9));
    let tran_dtime = midnight.format("%Y%m%d%H%M%S").to_string();
    let transfer_body = json!({
        "bank_tran_id": tran_id,
        "fintech_use_num": sender_fin,
        "dps_print_content": "정산점검테스트",
        "wd_print_content": "정산점검테스트",
        "tran_amt": "10000",
        "tran_dtime": tran_dtime,
        "req_client_name": "테스트",
        "req_client_num": "1",
        "transfer_purpose": "TR",
        "recv_client_name": "수취인",
        "recv_client_bank_code": "998",
        "recv_client_account_num": recv_acct,
    });

    // 00:00:00 KST -> customer login, balance inquiry, and transfer are ALL blocked with 503 A0013 + Retry-After: 1800.
    let (login_status, login_blocked) = call(
        &maint_public,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({"phone": sender_phone, "password": "maint-password"})),
    )
    .await;
    assert_eq!(
        login_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{login_blocked}"
    );
    assert_eq!(login_blocked["rsp_code"], "A0013");

    let balance_uri = format!(
        "/v2.0/account/balance/fin_num?bank_tran_id=M000000001U{}&fintech_use_num={}&tran_dtime={}",
        common::unique_digits(9),
        sender_fin,
        tran_dtime
    );
    let (bal_status, bal_body) = call(
        &maint_public,
        "GET",
        &balance_uri,
        &new_id(),
        Some("dummy-token"),
        None,
    )
    .await;
    assert_eq!(bal_status, StatusCode::SERVICE_UNAVAILABLE, "{bal_body}");
    assert_eq!(bal_body["rsp_code"], "A0013");

    let blocked_id = new_id();
    let req = Request::builder()
        .method("POST")
        .uri("/v2.0/transfer/withdraw/fin_num")
        .header("content-type", "application/json")
        .header("authorization", "Bearer dummy-token")
        .header("x-request-id", &blocked_id)
        .body(Body::from(transfer_body.to_string()))
        .expect("request");
    let res = maint_public.clone().oneshot(req).await.expect("response");
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        res.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("1800")
    );
    let body_bytes = res.into_body().collect().await.expect("body").to_bytes();
    let body: Value = serde_json::from_slice(&body_bytes).expect("json");
    assert_eq!(body["rsp_code"], "A0013");
    assert_eq!(body["rsp_message"], "일일 정산 점검 중입니다");

    // Verify request.completed was recorded in OUTBOX for the maintenance rejection.
    let events = atm_rows(api, ATM_REQUEST_EVENTS, &blocked_id).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["event_type"], "request.completed");
    assert_eq!(events[0]["payload"]["http_status"], 503);
    assert_eq!(events[0]["payload"]["request_type"], "TRANSFER");

    // 00:15:00 KST -> /admin/* (operational monitoring) remains available during maintenance.
    api.state.clock.advance(900).await.expect("advance 900s");
    let (admin_login_status, admin_login_body) = call(
        &maint_public,
        "POST",
        "/admin/v1/login",
        &new_id(),
        None,
        Some(json!({"analyst_no": analyst_no, "password": "maint-admin-password"})),
    )
    .await;
    assert_eq!(admin_login_status, StatusCode::OK, "{admin_login_body}");
    let admin_token = admin_login_body["access_token"]
        .as_str()
        .expect("admin token");
    let (admin_read_status, admin_read_body) = call(
        &maint_public,
        "GET",
        &format!("/admin/v1/customers/{sender_no}"),
        &new_id(),
        Some(admin_token),
        None,
    )
    .await;
    assert_eq!(admin_read_status, StatusCode::OK, "{admin_read_body}");

    // 00:29:59 KST -> still blocked with Retry-After: 1
    api.state.clock.advance(899).await.expect("advance 899s");
    let req2 = Request::builder()
        .method("POST")
        .uri("/v2.0/transfer/withdraw/fin_num")
        .header("content-type", "application/json")
        .header("authorization", "Bearer dummy-token")
        .header("x-request-id", &new_id())
        .body(Body::from(transfer_body.to_string()))
        .expect("request");
    let res2 = maint_public.clone().oneshot(req2).await.expect("response");
    assert_eq!(res2.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        res2.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );

    // 00:30:00 KST -> customer login and same bank_tran_id transfer succeed (idempotency key was not claimed).
    api.state.clock.advance(1).await.expect("advance 1s");
    let (status, login_body) = call(
        &maint_public,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({"phone": sender_phone, "password": "maint-password"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login_body}");
    let token = login_body["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let (ok_status, ok_body) = call(
        &maint_public,
        "POST",
        "/v2.0/transfer/withdraw/fin_num",
        &new_id(),
        Some(&token),
        Some(transfer_body),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK, "{ok_body}");
    assert_eq!(ok_body["rsp_code"], "A0000");
}

async fn live_switch_is_one_way(api: &Api, recording: &Arc<Mutex<Recording>>) {
    let state = api.state.clock.state().await.expect("clock");
    let before = recording.lock().expect("recording").calls.clone();
    let wall_date = business_date(Utc::now().with_timezone(&kst()));
    let (status, body) = post(api, "/internal/v1/mode/live", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "LIVE");
    assert_eq!(
        body["rolled_through"],
        wall_date.pred_opt().expect("date").to_string()
    );
    let mut expected = before;
    expected.extend(expected_dates(
        state.rolled_through.succ_opt().expect("date"),
        wall_date,
    ));
    assert_eq!(recording.lock().expect("recording").calls, expected);
    let (status, again) = post(api, "/internal/v1/mode/live", None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["mode"], body["mode"]);
    assert_eq!(again["rolled_through"], body["rolled_through"]);
    assert_eq!(recording.lock().expect("recording").calls, expected);
    assert_eq!(
        api.state.clock.state().await.expect("clock").mode,
        ClockMode::Live
    );
    for (path, request) in [
        (
            "/internal/v1/clock/set",
            json!({"business_time": Utc::now().to_rfc3339()}),
        ),
        ("/internal/v1/clock/advance", json!({"seconds": 1})),
    ] {
        let (status, body) = post(api, path, Some(request)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["rsp_code"], "A0002");
    }
}

#[tokio::test]
async fn backfill_sequence() {
    let close_logs = daily_close_tests::capture_logs();
    let close_metrics = bank::shared::telemetry::metrics_handle();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");
    db.migrate().await.expect("migrations");
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut registry = Registry::default();
    for name in ["first", "second"] {
        registry.register(Arc::new(Job {
            name,
            recording: recording.clone(),
        }));
    }
    registry.register(Arc::new(bank::features::atm::AtmSessionExpiryJob));
    registry.register(Arc::new(bank::features::deposits::MaturityJob));
    registry.register(Arc::new(bank::features::accounts_lifecycle::DormancyJob));
    registry.register(Arc::new(bank::features::daily_close::DailyCloseJob));
    let clock = Clock::init(db.clone(), registry).await.expect("clock");
    let state = AppState {
        db,
        clock,
        limits: Default::default(),
        maintenance_minutes: 0,
    };
    let api = Api {
        public: app::public_router(state.clone()),
        generator: app::generator_router(state.clone()),
        state,
    };
    starts_in_backfill(&api).await;
    set_moves_back_without_customers(&api).await;
    let start = rewind_forgets_rollover_runs(&api, &recording).await;
    jobs_run_once_per_crossed_date_in_order(&api, &recording, start).await;
    failed_job_leaves_the_clock_and_retry_skips_finished(&api, &recording).await;
    closed_date_refuses_operations(&api, &recording).await;
    failed_rollover_refuses_operations(&api, &recording).await;
    set_backward_is_rejected_with_customers(&api).await;
    advance_waits_for_an_operation_then_times_out(&api).await;
    operations_wait_during_an_advance(&api, &recording).await;
    cancelled_request_keeps_the_barrier(&api).await;
    cancelled_advance_keeps_the_barrier(&api, &recording).await;
    token_check_and_handler_share_one_business_time(&api).await;
    {
        let r = recording.lock().expect("recording");
        assert!(r.sleep_on.is_none());
        assert!(r.fail_once.is_none());
    }
    let deposits = term_deposit_open(&api).await;
    dormancy_and_reactivation(&api).await;
    term_deposit_matured(&api, deposits).await;
    password_reset_code_expires(&api).await;
    atm_sessions_in_business_time(&api).await;
    daily_maintenance_window(&api).await;
    let restoration = daily_close_tests::sequence(&api, &close_logs, &close_metrics).await;
    live_switch_is_one_way(&api, &recording).await;
    daily_close_tests::final_check(&api, &close_logs, restoration).await;
    card_payment_reconciliation(&api).await;
}

const ATM_SESSIONS: SqlStmt = sql_stmt!(
    "test.backfill_atm_sessions",
    "sql/backfill_atm_sessions.sql"
);
const ATM_CREDENTIAL: SqlStmt = sql_stmt!(
    "test.backfill_atm_credential",
    "sql/backfill_atm_credential.sql"
);
const ATM_END_EVENTS: SqlStmt = sql_stmt!("test.atm_session_events", "sql/atm_session_events.sql");
const ATM_REQUEST_EVENTS: SqlStmt =
    sql_stmt!("test.atm_request_events", "sql/atm_request_events.sql");

async fn atm_rows(api: &Api, stmt: SqlStmt, key: &str) -> Vec<Value> {
    let key = key.to_owned();
    let rows: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&stmt, &[&key]))
        .await
        .unwrap();
    rows.iter()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect()
}

fn atm_at(value: &Value) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(value.as_str().unwrap()).unwrap()
}

async fn atm_advance(api: &Api, seconds: i64) {
    let (status, _) = post(
        api,
        "/internal/v1/clock/advance",
        Some(json!({"seconds": seconds})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

async fn atm_start(
    api: &Api,
    terminal: &str,
    account: &str,
    pin: &str,
    request: &str,
) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        "/atm/v1/session",
        request,
        None,
        Some(json!({"terminal_id": terminal, "account_no": account, "pin": pin})),
    )
    .await
}

async fn atm_finish(api: &Api, token: &str, request: &str) -> (StatusCode, Value) {
    call(
        &api.generator,
        "POST",
        "/atm/v1/session/end",
        request,
        Some(token),
        None,
    )
    .await
}

async fn atm_assert_expired(
    api: &Api,
    terminal: &str,
    index: usize,
    request: Option<&str>,
) -> Value {
    let rows = atm_rows(api, ATM_SESSIONS, terminal).await;
    let row = &rows[index];
    assert_eq!(row["end_reason"], "expired");
    assert_eq!(atm_at(&row["ended_at"]), atm_at(&row["expires_at"]));
    assert_eq!(
        atm_at(&row["ended_at"]) - atm_at(&row["started_at"]),
        chrono::Duration::seconds(180)
    );
    let events = atm_rows(api, ATM_END_EVENTS, row["session_id"].as_str().unwrap()).await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["request_id"], json!(request));
    assert_eq!(event["payload"]["request_id"], json!(request));
    assert_eq!(event["payload"]["session_id"], row["session_id"]);
    assert_eq!(
        atm_at(&event["payload"]["ended_at"]),
        atm_at(&row["expires_at"])
    );
    assert_eq!(event["payload"]["reason"], "expired");
    assert_eq!(
        event["channel"],
        if request.is_none() { "SYSTEM" } else { "ATM" }
    );
    assert_eq!(event["payload"]["terminal_id"], terminal);
    assert_eq!(event["payload"]["region"], "KR-11");
    assert_eq!(event["payload"]["latitude"], "37.566500");
    assert_eq!(event["payload"]["longitude"], "126.978000");
    event.clone()
}

async fn atm_sessions_in_business_time(api: &Api) {
    let customer = common::customer(api, 10000).await;
    let terminals: Vec<String> = (0..3)
        .map(|_| format!("T{}", common::unique_digits(15)))
        .collect();
    let (status, _) = post(api, "/internal/v1/seed/atm_terminals", Some(json!({"atm_terminals": terminals.iter().map(|t|
        json!({"terminal_id": t, "region": "KR-11", "location": "서울", "latitude": "37.566500", "longitude": "126.978000"})
    ).collect::<Vec<_>>()}))).await;
    assert_eq!(status, StatusCode::OK);
    let day = business_date(now(api).await).succ_opt().unwrap();
    let time = |date: NaiveDate, h, m, s| {
        kst()
            .from_local_datetime(&date.and_hms_opt(h, m, s).unwrap())
            .single()
            .unwrap()
    };
    set(api, time(day, 12, 0, 0)).await;
    let t = &terminals[0];
    let start_id = new_id();
    let (status, body) =
        atm_start(api, t, &customer.account, common::TEST_ATM_PIN, &start_id).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["atm_session_token"].as_str().unwrap();
    atm_advance(api, 180).await;
    let request = new_id();
    let (status, body) = atm_finish(api, token, &request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["rsp_code"], "O0001");
    let event = atm_assert_expired(api, t, 0, Some(&request)).await;
    assert_eq!(event["payload"]["start_request_id"], start_id);
    let events = atm_rows(api, ATM_REQUEST_EVENTS, &request).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["event_type"] == "request.completed")
            .count(),
        1
    );
    let (status, body) = atm_finish(api, token, &new_id()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["rsp_code"], "O0001");
    atm_assert_expired(api, t, 0, Some(&request)).await;

    let first_request = new_id();
    assert_eq!(
        atm_start(
            api,
            t,
            &customer.account,
            common::TEST_ATM_PIN,
            &first_request
        )
        .await
        .0,
        StatusCode::OK
    );
    atm_advance(api, 240).await;
    let replacement = new_id();
    assert_eq!(
        atm_start(
            api,
            t,
            &customer.account,
            common::TEST_ATM_PIN,
            &replacement
        )
        .await
        .0,
        StatusCode::OK
    );
    let event = atm_assert_expired(api, t, 1, Some(&replacement)).await;
    assert_eq!(event["payload"]["start_request_id"], first_request);

    for pin in [
        common::WRONG_ATM_PIN,
        common::WRONG_ATM_PIN,
        common::WRONG_ATM_PIN,
        common::TEST_ATM_PIN,
    ] {
        let (status, body) = atm_start(api, t, &customer.account, pin, &new_id()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body["bank_rsp_code"],
            if pin == common::TEST_ATM_PIN {
                "409"
            } else {
                "410"
            }
        );
    }
    set(api, time(day, 23, 59, 0)).await;
    let (status, body) =
        atm_start(api, t, &customer.account, common::TEST_ATM_PIN, &new_id()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["bank_rsp_code"], "409");
    atm_advance(api, 90).await;
    let (status, body) =
        atm_start(api, t, &customer.account, common::WRONG_ATM_PIN, &new_id()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["bank_rsp_code"], "410");
    let account = customer.account.clone();
    let credential: (i64, Option<String>) = api
        .state
        .db
        .run(move |s| s.query_row(&ATM_CREDENTIAL, &[&account]))
        .await
        .unwrap();
    assert_eq!(credential, (1, None));
    assert_eq!(
        atm_start(api, t, &customer.account, common::TEST_ATM_PIN, &new_id())
            .await
            .0,
        StatusCode::OK
    );

    let account = customer.account.clone();
    let credential: (i64, Option<String>) = api
        .state
        .db
        .run(move |s| s.query_row(&ATM_CREDENTIAL, &[&account]))
        .await
        .unwrap();
    assert_eq!(credential, (0, None));
    for _ in 0..2 {
        let (status, body) =
            atm_start(api, t, &customer.account, common::WRONG_ATM_PIN, &new_id()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["bank_rsp_code"], "410");
    }
    let account = customer.account.clone();
    let credential: (i64, Option<String>) = api
        .state
        .db
        .run(move |s| s.query_row(&ATM_CREDENTIAL, &[&account]))
        .await
        .unwrap();
    assert_eq!(credential, (2, None));
    assert_eq!(
        atm_start(api, t, &customer.account, common::TEST_ATM_PIN, &new_id())
            .await
            .0,
        StatusCode::OK
    );

    let d = day.succ_opt().unwrap();
    set(api, time(d, 22, 0, 0)).await;
    let a_request = new_id();
    assert_eq!(
        atm_start(
            api,
            &terminals[1],
            &customer.account,
            common::TEST_ATM_PIN,
            &a_request
        )
        .await
        .0,
        StatusCode::OK
    );
    set(api, time(d, 23, 59, 0)).await;
    let (status, b) = atm_start(
        api,
        &terminals[2],
        &customer.account,
        common::TEST_ATM_PIN,
        &new_id(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    atm_advance(api, 120).await;
    let event = atm_assert_expired(api, &terminals[1], 0, None).await;
    assert_eq!(event["payload"]["start_request_id"], a_request);
    assert_eq!(
        atm_at(&event["occurred_at"]),
        bank::shared::rollover::last_instant(d)
    );
    assert_eq!(event["account_no"], customer.account);
    assert_eq!(event["payload"]["account_no"], customer.account);
    let b_rows = atm_rows(api, ATM_SESSIONS, &terminals[2]).await;
    assert!(b_rows[0]["ended_at"].is_null());
    // A direct retry of the date job must also leave B open and A unchanged.
    bank::features::atm::AtmSessionExpiryJob
        .run(&api.state.db, d, bank::shared::rollover::last_instant(d))
        .await
        .unwrap();
    atm_assert_expired(api, &terminals[1], 0, None).await;
    assert!(atm_rows(api, ATM_SESSIONS, &terminals[2]).await[0]["ended_at"].is_null());
    assert_eq!(
        atm_finish(api, b["atm_session_token"].as_str().unwrap(), &new_id())
            .await
            .0,
        StatusCode::OK
    );
    let b_rows = atm_rows(api, ATM_SESSIONS, &terminals[2]).await;
    assert_eq!(b_rows[0]["end_reason"], "ended");
    assert_eq!(atm_at(&b_rows[0]["ended_at"]), now(api).await);
    let b_id = b_rows[0]["session_id"].as_str().unwrap();
    assert_eq!(atm_rows(api, ATM_END_EVENTS, b_id).await.len(), 1);
    atm_advance(api, 86400).await;
    atm_assert_expired(api, &terminals[1], 0, None).await;
    assert_eq!(atm_rows(api, ATM_END_EVENTS, b_id).await.len(), 1);
}

async fn card_payment_reconciliation(api: &Api) {
    use common::card_network::{card, message, send, token};

    const AUDIT: SqlStmt = sql_stmt!(
        "test.card_payment_close_audit",
        "../src/features/daily_close/sql/transfers.sql"
    );
    const SELECT_ID: SqlStmt = sql_stmt!(
        "test.card_payment_transfer_id",
        "sql/card_payment_transfer_id.sql"
    );
    const BAD_LEG: SqlStmt = sql_stmt!("test.card_payment_bad_leg", "sql/card_payment_bad_leg.sql");

    let account = common::customer(api, 100).await.account;
    let card = card(api, &account, "ACTIVE").await;
    let token = token(api, "card").await;
    let id = new_id();
    let (status, response) = send(api, Some(&token), &id, message(&card, &id, "25")).await;
    assert_eq!(status, StatusCode::OK, "{response}");

    let now = api.state.clock.state().await.unwrap().business_time;
    let date = business_date(now);
    let midnight = kst()
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .unwrap();
    let end = midnight + chrono::Duration::days(1);
    api.state
        .db
        .run(move |s| {
            let transfer: i64 = s.query_row(&SELECT_ID, &[&id])?;
            let valid: Vec<(i64, String, Option<i64>)> = s.query_all(&AUDIT, &[&midnight, &end])?;
            assert!(valid.iter().all(|row| row.0 != transfer));
            assert_eq!(s.execute(&BAD_LEG, &[&id])?, 1);
            let invalid: Vec<(i64, String, Option<i64>)> =
                s.query_all(&AUDIT, &[&midnight, &end])?;
            let invalid = invalid.iter().find(|row| row.0 == transfer).unwrap();
            assert_eq!(invalid.1, "card_payment");
            assert!(invalid.2.is_some());
            s.rollback()
        })
        .await
        .unwrap();
}
