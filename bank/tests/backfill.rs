//! One ordered sequence over a freshly reset schema, including crash recovery.
mod common;

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
    let wall_before = Utc::now();
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
            {"customer_no": "C_DORM_A", "account_no": x, "opening_balance": 50000},
            {"customer_no": "C_DORM_A", "account_no": y},
            {"customer_no": "C_DORM_B", "account_no": z, "opening_balance": 50000}
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
    registry.register(Arc::new(bank::features::accounts_lifecycle::DormancyJob));
    let clock = Clock::init(db.clone(), registry).await.expect("clock");
    let state = AppState { db, clock };
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
    dormancy_and_reactivation(&api).await;
    live_switch_is_one_way(&api, &recording).await;
}
