//! Additional steps in the single backfill sequence; two date crossings total.
use crate::common::{self, Api, Customer, assert_contract, bank_tran_id, call, new_id};
use axum::http::StatusCode;
use bank::features::daily_close::{CloseReport, close_date};
use bank::shared::clock::{business_date, kst};
use bank::shared::db::SqlStmt;
use bank::shared::rollover::last_instant;
use bank::sql_stmt;
use chrono::{NaiveDate, TimeZone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};
const HISTORY: SqlStmt = sql_stmt!("test.close_history", "../sql/close_history.sql");
const ROWS: SqlStmt = sql_stmt!("test.close_rows", "../sql/close_rows.sql");
const BALANCE: SqlStmt = sql_stmt!("test.close_balance", "../sql/close_balance.sql");
const AMOUNT: SqlStmt = sql_stmt!("test.close_amount", "../sql/close_amount.sql");
const LEG: SqlStmt = sql_stmt!("test.close_leg", "../sql/close_leg.sql");
const BACKUP: SqlStmt = sql_stmt!("test.close_backup", "../sql/close_backup.sql");
const DELETE: SqlStmt = sql_stmt!("test.close_delete_leg", "../sql/close_delete_leg.sql");
const RESTORE: SqlStmt = sql_stmt!("test.close_restore", "../sql/close_restore.sql");
const TRANSFER_AMOUNT: SqlStmt = sql_stmt!(
    "test.close_transfer_amount",
    "../sql/close_transfer_amount.sql"
);
const MOVE_LEG: SqlStmt = sql_stmt!("test.close_move_leg", "../sql/close_move_leg.sql");
const NULL_TRANSFER: SqlStmt =
    sql_stmt!("test.close_null_transfer", "../sql/close_null_transfer.sql");
const DELETE_PREVIOUS: SqlStmt = sql_stmt!(
    "test.close_delete_previous",
    "../sql/close_delete_previous.sql"
);
const RESTORE_PREVIOUS: SqlStmt = sql_stmt!(
    "test.close_restore_previous",
    "../sql/close_restore_previous.sql"
);
const CONSISTENT: SqlStmt = sql_stmt!("test.close_consistent", "../sql/close_consistent.sql");
const FINAL: SqlStmt = sql_stmt!("test.close_final", "../sql/close_final.sql");
type Row = (String, i64, i64, String, Option<String>);
pub type Logs = Arc<Mutex<Vec<Value>>>;
struct Capture(Logs);
struct Fields(serde_json::Map<String, Value>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), json!(format!("{v:?}")));
    }
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        self.0.insert(f.name().into(), json!(v));
    }
    fn record_i64(&mut self, f: &tracing::field::Field, v: i64) {
        self.0.insert(f.name().into(), json!(v));
    }
}
impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut f = Fields(Default::default());
        event.record(&mut f);
        if f.0.get("message").and_then(Value::as_str) == Some("daily close") {
            f.0.insert("level".into(), json!(event.metadata().level().as_str()));
            self.0.lock().unwrap().push(Value::Object(f.0));
        }
    }
}
pub fn capture_logs() -> Logs {
    let logs = Logs::default();
    tracing::subscriber::set_global_default(Registry::default().with(Capture(logs.clone())))
        .expect("daily-close capture subscriber");
    logs
}
async fn date(api: &Api) -> NaiveDate {
    business_date(api.state.clock.state().await.unwrap().business_time)
}
async fn rows(api: &Api, d: NaiveDate) -> Vec<Row> {
    let d = d.to_string();
    api.state
        .db
        .run(move |s| s.query_all(&ROWS, &[&d]))
        .await
        .unwrap()
}
async fn history(api: &Api) {
    let bad: i64 = api
        .state
        .db
        .run(|s| s.query_row(&HISTORY, &[]))
        .await
        .unwrap();
    assert_eq!(
        bad, 0,
        "every crossed date is finished, all eligible accounts present, sums correct, no rows after closure, no mismatches"
    );
}
async fn legs(api: &Api, account: &str, d: NaiveDate) -> Vec<i64> {
    let account = account.to_owned();
    let start = kst()
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap();
    let end = start + chrono::Duration::days(1);
    api.state
        .db
        .run(move |s| s.query_all(&LEG, &[&account, &start, &end]))
        .await
        .unwrap()
}
async fn transfer(api: &Api, from: &Customer, bank: &str, to: &str, amount: i64) {
    let time = api.state.clock.state().await.unwrap().business_time;
    let (status, body) = call(&api.generator, "POST", "/v2.0/transfer/withdraw/fin_num", &new_id(), Some(&from.token), Some(json!({
        "bank_tran_id": bank_tran_id(), "fintech_use_num": from.fin, "tran_amt": amount.to_string(),
        "tran_dtime": time.format("%Y%m%d%H%M%S").to_string(), "transfer_purpose": "TR",
        "recv_client_name": "김수취", "recv_client_bank_code": bank, "recv_client_account_num": to
    }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("TransferResponse", &body);
}
async fn loan_payload(api: &Api, fin: Option<&str>, amount: i64) -> Value {
    let time = api.state.clock.state().await.unwrap().business_time;
    let mut body = json!({"bank_tran_id": bank_tran_id(), "tran_amt": amount.to_string(), "tran_dtime": time.format("%Y%m%d%H%M%S").to_string()});
    if let Some(fin) = fin {
        body["fintech_use_num"] = json!(fin);
        body["terms_id"] = json!("5");
    }
    body
}
async fn inbound_token(api: &Api) -> String {
    let client = format!("close_{}", new_id());
    let secret = "close-inbound-secret-123456";
    let (status, body) = call(&api.generator, "POST", "/internal/v1/seed/counterparty_clients", &new_id(), None,
        Some(json!({"counterparty_clients": [{"client_id": client, "client_secret": secret, "institution_code": "901", "scope": "oob"}]}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_contract("CreatedResponse", &body);
    let (status, body) = common::form_call(
        &api.generator,
        "/oauth/2.0/token",
        format!(
            "client_id={client}&client_secret={secret}&scope=oob&grant_type=client_credentials"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_inbound_contract("TokenResponse", &body);
    body["access_token"].as_str().unwrap().to_owned()
}
async fn inbound(api: &Api, token: &str, to: &Customer, external: &str) {
    let time = api.state.clock.state().await.unwrap().business_time;
    let (status, body) = call(&api.generator, "POST", "/v2.0/transfer/deposit/fin_num", &new_id(), Some(token), Some(json!({
        "cntr_account_type": "N", "cntr_account_num": external, "wd_print_content": "송금", "name_check_option": "off",
        "tran_dtime": time.format("%Y%m%d%H%M%S").to_string(), "req_cnt": "1", "req_list": [{"tran_no": "1", "bank_tran_id": bank_tran_id(),
        "fintech_use_num": to.fin, "print_content": "입금", "tran_amt": "123", "req_client_name": "외부수취인", "transfer_purpose": "TR"}]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    common::assert_inbound_contract("DepositResponse", &body);
}
fn metric(metrics: &metrics_exporter_prometheus::PrometheusHandle, name: &str) -> f64 {
    metrics
        .render()
        .lines()
        .find_map(|l| {
            l.strip_prefix(&format!("{name} "))
                .map(|v| v.parse::<f64>().unwrap())
        })
        .expect("metric")
}
fn logged(logs: &Logs, index: usize, d: NaiveDate, report: &CloseReport) {
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), index + 1, "exactly one log per close");
    let line = &logs[index];
    assert_eq!(line["business_date"], d.to_string());
    assert_eq!(line["accounts"], report.accounts);
    assert_eq!(line["mismatches"], report.mismatches);
    assert_eq!(line["transfer_violations"], report.transfer_violations);
    assert_eq!(
        line["level"],
        if report.mismatches + report.transfer_violations > 0 {
            "WARN"
        } else {
            "INFO"
        }
    );
}
async fn rerun(
    api: &Api,
    d: NaiveDate,
    logs: &Logs,
    metrics: &metrics_exporter_prometheus::PrometheusHandle,
) -> CloseReport {
    let before = logs.lock().unwrap().len();
    let runs = metric(metrics, "bank_daily_close_runs_total");
    let report = close_date(&api.state.db, d, last_instant(d)).await.unwrap();
    logged(logs, before, d, &report);
    assert_eq!(
        metric(metrics, "bank_daily_close_mismatches"),
        (report.mismatches + report.transfer_violations) as f64
    );
    assert_eq!(metric(metrics, "bank_daily_close_runs_total"), runs + 1.0);
    report
}
async fn concurrent(api: &Api, accounts: Arc<Vec<Customer>>) {
    let mut tasks = Vec::new();
    // Same random two-way workload as stage 1, with a few independently owned accounts.
    for worker in 0..32u64 {
        let api = Api {
            public: api.public.clone(),
            generator: api.generator.clone(),
            state: api.state.clone(),
        };
        let accounts = accounts.clone();
        tasks.push(tokio::spawn(async move {
            let mut seed = worker
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let mut next = move || {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (seed >> 33) as usize
            };
            for _ in 0..(1000 / 32 + 1) {
                let from = next() % accounts.len();
                let mut to = next() % accounts.len();
                if from == to {
                    to = (to + 1) % accounts.len();
                }
                transfer(
                    &api,
                    &accounts[from],
                    "998",
                    &accounts[to].account,
                    (next() % 5000 + 1) as i64,
                )
                .await;
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}
pub async fn sequence(
    api: &Api,
    logs: &Logs,
    metrics: &metrics_exporter_prometheus::PrometheusHandle,
) -> usize {
    history(api).await;
    let d = date(api).await;
    let mut accounts = Vec::new();
    for _ in 0..4 {
        accounts.push(common::customer(api, 2_000_000).await);
    }
    let accounts = Arc::new(accounts);
    let idle = common::customer(api, 321).await;
    let altered = common::customer(api, 1000).await;
    let mut sender = common::customer(api, 1000).await;
    let receiver = common::customer(api, 1000).await;
    let empty = common::customer(api, 1000).await;
    let external = format!("93{}", common::unique_digits(10));
    common::seed_counterparty(api, &external).await;
    let inbound_token = inbound_token(api).await;
    let a = &accounts[0];
    transfer(api, a, "998", &accounts[1].account, 100).await;
    transfer(api, a, "901", &external, 10).await;
    inbound(api, &inbound_token, a, &external).await;
    let term = common::deposits::request(
        api,
        &a.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(common::deposits::open_payload(api, &a.fin, 100000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    common::deposits::request(
        api,
        &a.token,
        "POST",
        &format!(
            "/v2.0/ext/deposits/{}/close",
            term["deposit_id"].as_str().unwrap()
        ),
        Some(common::deposits::close_payload(api).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let loan = common::deposits::request(
        api,
        &a.token,
        "POST",
        "/v2.0/ext/loans",
        Some(loan_payload(api, Some(&a.fin), 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    common::deposits::request(
        api,
        &a.token,
        "POST",
        &format!(
            "/v2.0/ext/loans/{}/repay",
            loan["loan_id"].as_str().unwrap()
        ),
        Some(loan_payload(api, None, 1_000_000).await),
        StatusCode::OK,
        "LoanTranResponse",
    )
    .await;
    let mut total_before = 0;
    for a in accounts.iter() {
        total_before += common::deposits::snapshot(api, &a.fin).await.0;
    }
    concurrent(api, accounts.clone()).await;
    let mut total_after = 0;
    for a in accounts.iter() {
        total_after += common::deposits::snapshot(api, &a.fin).await.0;
    }
    assert_eq!(
        total_after, total_before,
        "concurrent transfers preserve total balance"
    );
    for a in accounts.iter() {
        let account = a.account.clone();
        let bad: i64 = api
            .state
            .db
            .run(move |s| s.query_row(&CONSISTENT, &[&account]))
            .await
            .unwrap();
        assert_eq!(
            bad, 0,
            "every transfer offsets, every balance and balance_after equals its rows"
        );
    }
    let n = logs.lock().unwrap().len();
    api.state.clock.advance(86400).await.unwrap();
    let initial = rows(api, d).await;
    let report = CloseReport {
        accounts: initial.len() as i64,
        mismatches: 0,
        transfer_violations: 0,
    };
    logged(logs, n, d, &report);
    for row in &initial {
        assert_eq!(row.3, "Y");
        assert!(row.4.is_none());
    }
    for a in accounts.iter() {
        assert!(initial.iter().any(|r| r.0 == a.account && r.3 == "Y"));
    }
    assert!(initial.iter().any(|r| r.0 == idle.account && r.2 == 321));
    // The next date supplies the idle-account proof and the dedicated corruption witnesses.
    let bad_date = date(api).await;
    // Customer sessions expire across the date advance; log in for the new date.
    let (status, login) = call(
        &api.generator,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({"phone": format!("010{}", &sender.account[2..]), "password": "pw-payees"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_contract("LoginResponse", &login);
    sender.token = login["access_token"].as_str().unwrap().to_owned();
    inbound(api, &inbound_token, &altered, &external).await;
    inbound(api, &inbound_token, &empty, &external).await;
    transfer(api, &sender, "998", &receiver.account, 10).await;
    let altered_id = legs(api, &altered.account, bad_date).await[0];
    let leg_id = legs(api, &sender.account, bad_date).await[0];
    let empty_id = legs(api, &empty.account, bad_date).await[0];
    // Keep one isolated witness through rollover, then repeat it directly.
    let idle_no = idle.account.clone();
    api.state
        .db
        .run(move |s| {
            assert_eq!(s.execute(&BALANCE, &[&1i64, &idle_no])?, 1);
            s.commit()
        })
        .await
        .unwrap();
    let n = logs.lock().unwrap().len();
    api.state.clock.advance(86400).await.unwrap();
    let bad = rows(api, bad_date).await;
    let report = CloseReport {
        accounts: bad.len() as i64,
        mismatches: 1,
        transfer_violations: 0,
    };
    logged(logs, n, bad_date, &report);
    assert_eq!(metric(metrics, "bank_daily_close_mismatches"), 1.0);
    assert_bad_rows(&bad, &[(&idle.account, "balance:")]);
    assert_eq!(rerun(api, bad_date, logs, metrics).await, report);
    assert_eq!(rows(api, bad_date).await, bad, "idempotent replacement");
    let idle_no = idle.account.clone();
    api.state
        .db
        .run(move |s| {
            s.execute(&BALANCE, &[&-1i64, &idle_no])?;
            s.commit()
        })
        .await
        .unwrap();
    assert_clean(api, bad_date, logs, metrics).await;

    // Each corruption starts from, and returns to, an entirely clean close.
    for witness in 0..7 {
        let idle_no = idle.account.clone();
        let sender_no = sender.account.clone();
        let empty_no = empty.account.clone();
        let previous_date = d.to_string();
        let receiver_id = legs(api, &receiver.account, bad_date).await[0];
        let touched_id = match witness {
            1 | 6 => altered_id,
            4 => receiver_id,
            _ => empty_id,
        };
        let saved: String = api
            .state
            .db
            .run(move |s| {
                let saved = s.query_row(&BACKUP, &[&touched_id])?;
                match witness {
                    0 => {
                        assert_eq!(s.execute(&BALANCE, &[&1i64, &idle_no])?, 1);
                    }
                    1 => {
                        assert_eq!(s.execute(&AMOUNT, &[&1i64, &altered_id])?, 1);
                    }
                    2 => {
                        assert_eq!(s.execute(&TRANSFER_AMOUNT, &[&1i64, &leg_id])?, 1);
                    }
                    3 => {
                        assert_eq!(s.execute(&DELETE, &[&empty_id])?, 1);
                        assert_eq!(s.execute(&BALANCE, &[&-123i64, &empty_no])?, 1);
                    }
                    4 => {
                        assert_eq!(s.execute(&MOVE_LEG, &[&sender_no, &receiver_id])?, 1);
                    }
                    5 => {
                        assert_eq!(s.execute(&DELETE_PREVIOUS, &[&idle_no, &previous_date])?, 1);
                    }
                    6 => {
                        assert_eq!(s.execute(&NULL_TRANSFER, &[&altered_id])?, 1);
                    }
                    _ => unreachable!(),
                }
                s.commit()?;
                Ok(saved)
            })
            .await
            .unwrap();
        let (mismatches, violations, expected): (i64, i64, Vec<(&str, &str)>) = match witness {
            0 => (1, 0, vec![(&idle.account, "balance:")]),
            1 => (1, 1, vec![(&altered.account, "balance:")]),
            2 => (
                2,
                1,
                vec![
                    (&sender.account, "violate F-21"),
                    (&receiver.account, "violate F-21"),
                ],
            ),
            3 => (0, 1, vec![]),
            4 => (
                2,
                1,
                vec![
                    (&sender.account, "violate F-21"),
                    (&receiver.account, "balance:"),
                ],
            ),
            5 => (1, 0, vec![(&idle.account, "no previous close")]),
            6 => (1, 1, vec![(&altered.account, "row without transfer")]),
            _ => unreachable!(),
        };
        let report = rerun(api, bad_date, logs, metrics).await;
        assert_eq!(
            report,
            CloseReport {
                accounts: bad.len() as i64,
                mismatches,
                transfer_violations: violations
            },
            "witness {witness}"
        );
        let actual = rows(api, bad_date).await;
        assert_bad_rows(&actual, &expected);
        if witness == 1 {
            assert!(
                actual
                    .iter()
                    .find(|r| r.0 == altered.account)
                    .unwrap()
                    .4
                    .as_ref()
                    .unwrap()
                    .contains("violate F-21")
            );
        }
        let idle_no = idle.account.clone();
        let empty_no = empty.account.clone();
        let previous_date = d.to_string();
        let previous = initial.iter().find(|r| r.0 == idle.account).unwrap();
        let (previous_opening, previous_closing) = (previous.1, previous.2);
        api.state
            .db
            .run(move |s| {
                match witness {
                    0 => {
                        assert_eq!(s.execute(&BALANCE, &[&-1i64, &idle_no])?, 1);
                    }
                    1 | 4 | 6 => {
                        assert_eq!(s.execute(&DELETE, &[&touched_id])?, 1);
                        assert_eq!(s.execute(&RESTORE, &[&saved])?, 1);
                    }
                    2 => {
                        assert_eq!(s.execute(&TRANSFER_AMOUNT, &[&-1i64, &leg_id])?, 1);
                    }
                    3 => {
                        assert_eq!(s.execute(&RESTORE, &[&saved])?, 1);
                        assert_eq!(s.execute(&BALANCE, &[&123i64, &empty_no])?, 1);
                    }
                    5 => {
                        assert_eq!(
                            s.execute(
                                &RESTORE_PREVIOUS,
                                &[
                                    &previous_date,
                                    &previous_opening,
                                    &previous_closing,
                                    &idle_no
                                ]
                            )?,
                            1
                        );
                    }
                    _ => unreachable!(),
                }
                s.commit()
            })
            .await
            .unwrap();
        assert_clean(api, bad_date, logs, metrics).await;
    }
    let restoration = logs.lock().unwrap().len() - 1;
    let restored = rerun(api, bad_date, logs, metrics).await;
    assert_eq!(
        restored,
        CloseReport {
            accounts: bad.len() as i64,
            mismatches: 0,
            transfer_violations: 0
        }
    );
    let good = rows(api, bad_date).await;
    assert!(
        good.iter()
            .any(|r| r.0 == idle.account && r.1 == 321 && r.2 == 321 && r.3 == "Y")
    );
    assert_eq!(rerun(api, bad_date, logs, metrics).await, restored);
    assert_eq!(
        rows(api, bad_date).await,
        good,
        "one row per account after two closes"
    );
    history(api).await;
    restoration
}

fn assert_bad_rows(actual: &[Row], expected: &[(&str, &str)]) {
    for (account, _) in expected {
        assert!(
            actual.iter().any(|r| &r.0 == account),
            "missing account {account}"
        );
    }
    for row in actual {
        let detail = expected.iter().find(|(a, _)| *a == row.0);
        assert_eq!(row.3, if detail.is_some() { "N" } else { "Y" }, "{}", row.0);
        match detail {
            Some((_, text)) => {
                let detail = row.4.as_ref().expect("mismatch detail");
                assert!(detail.contains(text), "{detail}: expected {text}");
                assert!(detail.chars().count() <= 400);
            }
            None => assert!(row.4.is_none()),
        }
    }
}

async fn assert_clean(
    api: &Api,
    d: NaiveDate,
    logs: &Logs,
    metrics: &metrics_exporter_prometheus::PrometheusHandle,
) {
    let report = rerun(api, d, logs, metrics).await;
    let actual = rows(api, d).await;
    assert_eq!(
        report,
        CloseReport {
            accounts: actual.len() as i64,
            mismatches: 0,
            transfer_violations: 0
        }
    );
    assert_bad_rows(&actual, &[]);
}

pub async fn final_check(api: &Api, logs: &Logs, restoration: usize) {
    history(api).await;
    let bad: i64 = api
        .state
        .db
        .run(|s| s.query_row(&FINAL, &[]))
        .await
        .unwrap();
    assert_eq!(
        bad, 0,
        "every close is Y and every date through rolled_through finished"
    );
    let logs = logs.lock().unwrap();
    assert!(logs.len() > restoration);
    for line in &logs[restoration..] {
        assert_eq!(line["mismatches"], 0);
        assert_eq!(line["transfer_violations"], 0);
        assert_eq!(line["level"], "INFO");
    }
}
