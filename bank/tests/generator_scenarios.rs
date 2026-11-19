//! Stage 6 annual A/B/C/D proof. Requires the finished synthetic fixture and
//! read-only access to the bank's JSON app logs (SALTATION_LOG_DIR).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use bank::features::outbox_relay;
use bank::shared::api::fields::mask_account;
use bank::shared::clock::kst;
use bank::shared::db::{Db, DbError, RowValue, SqlStmt};
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset, Utc};
use mongodb::Collection;
use mongodb::bson::{Document, doc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const ACCOUNT: SqlStmt = sql_stmt!(
    "test.scenarios.account",
    "sql/generator_scenarios_account.sql"
);
const ATM_SESSION: SqlStmt = sql_stmt!(
    "test.scenarios.atm_session",
    "sql/generator_scenarios_atm_session.sql"
);
const DAILY: SqlStmt = sql_stmt!("test.scenarios.daily", "sql/generator_scenarios_daily.sql");
const LEGS: SqlStmt = sql_stmt!("test.scenarios.legs", "sql/generator_scenarios_legs.sql");
const LOGIN: SqlStmt = sql_stmt!("test.scenarios.login", "sql/generator_behavior_login.sql");
const NO_MONEY: SqlStmt = sql_stmt!(
    "test.scenarios.no_money",
    "sql/generator_scenarios_no_money.sql"
);
const OUTBOX: SqlStmt = sql_stmt!("test.scenarios.outbox", "sql/count_outbox.sql");
const PAYEE: SqlStmt = sql_stmt!("test.scenarios.payee", "sql/generator_scenarios_payee.sql");
const POSTING: SqlStmt = sql_stmt!(
    "test.scenarios.posting",
    "sql/generator_scenarios_posting.sql"
);
const POSTING_LEGS: SqlStmt = sql_stmt!(
    "test.scenarios.posting_legs",
    "sql/generator_scenarios_posting_legs.sql"
);
const RESET: SqlStmt = sql_stmt!("test.scenarios.reset", "sql/generator_scenarios_reset.sql");
const STATUS: SqlStmt = sql_stmt!(
    "test.scenarios.status",
    "sql/generator_scenarios_status.sql"
);
const TERM: SqlStmt = sql_stmt!("test.scenarios.term", "sql/generator_scenarios_term.sql");

const IDENTITY_ROUTE: &str = "/internal/v1/deposits/{id}/identity";

#[derive(Deserialize)]
struct Fixture {
    run_id: String,
}

#[derive(Deserialize)]
struct Person {
    account_no: String,
    customer_no: String,
    opening_balance: i64,
}

#[derive(Deserialize)]
struct Operation {
    episode_id: String,
    episode_kind: String,
    name: String,
    operation: String,
    actor: String,
    peer: Option<String>,
    window: String,
    state: String,
    result: Value,
    request_id: String,
    business_time: String,
    args: Value,
    dependency: Value,
    labels: Vec<Label>,
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct Label {
    request_id: String,
    subject_type: String,
    subject_id: String,
    label: String,
    run_id: String,
    scenario: String,
    scenario_instance_id: String,
}

impl Operation {
    fn at(&self) -> DateTime<FixedOffset> {
        time(&self.business_time)
    }

    fn date(&self) -> String {
        self.at()
            .with_timezone(&kst())
            .format("%Y-%m-%d")
            .to_string()
    }

    fn label(&self, run_id: &str, role: &str, kind: &str, subject: &str) -> Label {
        let mut instance = format!("{}:{role}", self.episode_id);
        if role == "B" {
            instance.push(':');
            instance.push_str(&self.actor);
        }
        Label {
            request_id: self.request_id.clone(),
            subject_type: kind.into(),
            subject_id: subject.into(),
            label: role.into(),
            run_id: run_id.into(),
            scenario: self.episode_kind.clone(),
            scenario_instance_id: instance,
        }
    }
}

fn time(value: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(value).unwrap_or_else(|_| panic!("invalid business timestamp"))
}

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}"))
}

fn required_env(key: &str) -> String {
    env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| panic!("{key} is required"))
}

async fn mongo_rows<T: DeserializeOwned>(
    collection: &Collection<Document>,
    filter: Document,
) -> Vec<T> {
    let mut cursor = collection
        .find(filter)
        .await
        .unwrap_or_else(|_| panic!("find {}", collection.name()));
    let mut rows = Vec::new();
    while cursor
        .advance()
        .await
        .unwrap_or_else(|_| panic!("read {}", collection.name()))
    {
        let row = cursor
            .deserialize_current()
            .unwrap_or_else(|_| panic!("decode {}", collection.name()));
        rows.push(
            mongodb::bson::from_document(row)
                .unwrap_or_else(|_| panic!("invalid {} row", collection.name())),
        );
    }
    rows
}

async fn sql<T, F>(db: &Db, read: F) -> T
where
    T: Send + 'static,
    F: FnOnce(&mut bank::shared::db::Session) -> Result<T, DbError> + Send + 'static,
{
    db.run(read)
        .await
        .unwrap_or_else(|error| panic!("Oracle scenario query failed: {error}"))
}

async fn rows<T: RowValue + Send + 'static>(db: &Db, stmt: SqlStmt, key: &str) -> Vec<T> {
    let key = key.to_owned();
    sql(db, move |s| s.query_all(&stmt, &[&key])).await
}

async fn no_money(db: &Db, id: &str) {
    let counts: Vec<i64> = rows(db, NO_MONEY, id).await;
    assert_eq!(counts, [0], "unexpected financial records for request {id}");
}

async fn wait_for_outbox(db: &Db) {
    assert!(
        tokio::time::timeout(Duration::from_secs(120), async {
            while sql(db, |s| s.query_row::<i64>(&OUTBOX, &[])).await != 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .is_ok(),
        "bank outbox did not drain within 120 seconds"
    );
}

struct Episode<'a>(BTreeMap<&'a str, &'a Operation>);

impl<'a> Episode<'a> {
    fn step(&mut self, name: &str, operation: &str, actor: &str) -> &'a Operation {
        let op = self
            .0
            .remove(name)
            .unwrap_or_else(|| panic!("missing scenario step {name}"));
        assert!(
            op.operation == operation && op.actor == actor,
            "unexpected operation or actor for {name}"
        );
        op
    }

    fn actor(&self, name: &str) -> &'a str {
        &self
            .0
            .get(name)
            .unwrap_or_else(|| panic!("missing scenario step {name}"))
            .actor
    }
}

struct Proof<'a> {
    db: &'a Db,
    run_id: &'a str,
    people: HashMap<String, Person>,
    events: HashMap<String, Vec<Value>>,
    labels: Vec<Label>,
    sms_request: Option<String>,
    expected_legs: HashMap<String, Vec<ExpectedLeg>>,
    lookups: BTreeMap<String, (u16, &'static str)>,
    lookup_windows: Vec<(DateTime<Utc>, DateTime<Utc>)>,
}

struct ExpectedLeg {
    request_id: String,
    transfer_id: i64,
    at: DateTime<FixedOffset>,
    date: String,
    channel: &'static str,
    amount: i64,
    external_sender: Option<&'static str>,
}

struct Term {
    id: String,
    account: String,
    owner: String,
    linked: String,
}

impl Proof<'_> {
    fn demand(&self, customer: &str) -> &str {
        &self
            .people
            .get(customer)
            .expect("synthetic roster member")
            .account_no
    }

    fn labels(&mut self, op: &Operation, subjects: &[(&str, &str, &str)]) {
        let mut expected: Vec<_> = subjects
            .iter()
            .map(|(role, kind, subject)| op.label(self.run_id, role, kind, subject))
            .collect();
        expected.sort();
        let mut actual: Vec<_> = op.labels.iter().collect();
        actual.sort();
        assert!(
            actual.into_iter().eq(expected.iter()),
            "unexpected truth labels for {}",
            op.name
        );
        self.labels.extend(expected);
    }

    fn event(&self, op: &Operation, event_type: &str) -> &Value {
        let mut events = self
            .events
            .get(&op.request_id)
            .into_iter()
            .flatten()
            .filter(|event| event["event_type"] == event_type);
        let event = events
            .next()
            .unwrap_or_else(|| panic!("missing {event_type} for {}", op.name));
        assert!(
            events.next().is_none(),
            "duplicate {event_type} for {}",
            op.name
        );
        assert!(
            time(string(event, "occurred_at")) == op.at(),
            "bank event time for {}",
            op.name
        );
        event
    }

    async fn login(
        &mut self,
        op: &Operation,
        role: &str,
        device: Option<&str>,
        failed: bool,
    ) -> (String, String) {
        self.labels(op, &[(role, "customer", &op.actor)]);
        let found: Vec<String> = rows(self.db, LOGIN, &op.request_id).await;
        assert_eq!(found.len(), 1, "one Oracle login attempt for {}", op.name);
        let row: Value = serde_json::from_str(&found[0]).expect("Oracle login JSON");
        let expected = if failed { "WRONG_PASSWORD" } else { "SUCCESS" };
        let event = self.event(
            op,
            if failed {
                "login.failed"
            } else {
                "login.succeeded"
            },
        );
        assert!(
            row["customer_no"] == op.actor
                && row["outcome"] == expected
                && row["device_id"] == event["device_id"]
                && event["customer_no"] == op.actor
                && time(string(&row, "occurred_at")) == op.at(),
            "login identity or outcome for {}",
            op.name
        );
        let actual_device = string(&row, "device_id");
        if let Some(device) = device {
            assert!(actual_device == device, "login device for {}", op.name);
        }
        no_money(self.db, &op.request_id).await;
        if failed {
            assert!(
                event["rsp_code"] == "O0001"
                    && op.result["http_status"] == 401
                    && op.result["decision"] == "expected_rejection"
            );
            return (actual_device.into(), String::new());
        }
        let session = string(event, "session_id");
        assert!(
            !session.is_empty() && row["session_id"] == session && event["rsp_code"] == "A0000",
            "successful login session"
        );
        (actual_device.into(), session.into())
    }

    async fn payee(&mut self, op: &Operation, role: &str, peer: &str, session: &str) {
        self.labels(op, &[(role, "customer", &op.actor)]);
        let account = self.demand(peer).to_owned();
        assert!(op.peer.as_deref() == Some(peer), "payee peer");
        let event = self.event(op, "payee.registered");
        assert!(
            event["customer_no"] == op.actor
                && event["session_id"] == session
                && event["payload"]["account_num"] == account
                && event["payload"]["bank_code_std"] == "998",
            "registered payee attributes"
        );
        let id: i64 = string(&event["payload"], "payee_id")
            .parse()
            .expect("numeric payee ID");
        let actor = op.actor.clone();
        let recorded: Vec<(String, String)> = sql(self.db, move |s| {
            s.query_all(&PAYEE, &[&id, &actor, &account])
        })
        .await;
        assert!(
            recorded.len() == 1 && recorded[0].0 == session && time(&recorded[0].1) == op.at(),
            "Oracle payee session and registration"
        );
        no_money(self.db, &op.request_id).await;
    }

    async fn posting(
        &mut self,
        op: &Operation,
        amount: i64,
        legs: &[(&str, i64)],
        session: Option<&str>,
    ) {
        let (kind, request_type, channel, subject) = match op.operation.as_str() {
            "transfer" => ("transfer", "TRANSFER", "MOBILE", legs[0].0),
            "term_open" => ("term_open", "TERM_OPEN", "MOBILE", legs[1].0),
            "term_close" => ("term_close", "TERM_CLOSE", "MOBILE", legs[0].0),
            "atm_withdraw" => ("atm_cash", "ATM_WITHDRAW", "ATM", legs[0].0),
            "atm_deposit" => ("atm_cash", "ATM_DEPOSIT", "ATM", legs[0].0),
            "inbound" => ("inbound", "INBOUND", "EXTERNAL", legs[0].0),
            _ => panic!("unexpected monetary operation"),
        };
        if op.operation != "term_close" {
            assert_eq!(op.args["amount"], amount, "scenario amount for {}", op.name);
        }
        type Posting = (
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
            String,
            i64,
            Option<String>,
            String,
            String,
        );
        let actual: Vec<Posting> = rows(self.db, POSTING, &op.request_id).await;
        let [(id, ty, status, code, sid, transfer, k, amt, payee, request_at, transfer_at)] =
            actual.as_slice()
        else {
            panic!("expected exactly one Oracle posting for {}", op.name);
        };
        assert!(
            id == &op.request_id
                && ty == request_type
                && status == "DONE"
                && code == "A0000"
                && k == kind
                && *amt == amount
                && time(request_at) == op.at()
                && time(transfer_at) == op.at(),
            "Oracle posting attributes for {}",
            op.name
        );
        let event = self.event(op, "request.completed");
        assert!(
            event["account_no"] == subject
                && event["outcome"] == "completed"
                && event["rsp_code"] == "A0000"
                && event["payload"]["request_type"] == request_type
                && event["payload"]["http_status"] == 200
                && event["payload"].get("replayed").is_none(),
            "money completion event for {}",
            op.name
        );
        if kind == "transfer" {
            assert!(
                payee.as_deref() == Some("Y")
                    && event["payload"]["counterparty_bank_code"] == "998"
                    && event["payload"]["counterparty_account_no"] == legs[1].0,
                "payment to the registered in-bank payee"
            );
        }
        if let Some(session) = session {
            assert!(
                event["session_id"] == session && sid.as_deref() == Some(session),
                "posting session for {}",
                op.name
            );
        }
        if matches!(kind, "atm_cash" | "transfer" | "inbound") {
            assert!(
                event["payload"]["transfer_id"] == *transfer
                    && event["payload"]["tran_amt"] == amount,
                "event/Oracle movement mismatch for {}",
                op.name
            );
        }
        let id = op.request_id.clone();
        let transfer = *transfer;
        let mut actual_legs: Vec<(String, i64)> = sql(self.db, move |s| {
            s.query_all(&POSTING_LEGS, &[&id, &transfer])
        })
        .await;
        let mut expected: Vec<_> = legs
            .iter()
            .map(|(account, amount)| (account.to_string(), *amount))
            .collect();
        actual_legs.sort();
        expected.sort();
        assert!(
            actual_legs == expected,
            "signed posting legs for {}",
            op.name
        );
        let sender = match op.name.as_str() {
            "mule_settlement" => Some("9016000000000002"),
            "mule_inbound" => Some("9016000000000003"),
            _ => None,
        };
        for (account, delta) in legs {
            self.expected_legs
                .entry((*account).into())
                .or_default()
                .push(ExpectedLeg {
                    request_id: op.request_id.clone(),
                    transfer_id: transfer,
                    at: op.at(),
                    date: op.date(),
                    channel,
                    amount: *delta,
                    external_sender: sender,
                });
        }
    }

    async fn cash(
        &mut self,
        episode: &mut Episode<'_>,
        name: &str,
        actor: &str,
        role: &str,
        signed: i64,
    ) -> String {
        let start = episode.step(&format!("{name}_start"), "atm_start", actor);
        let operation = if signed > 0 {
            "atm_deposit"
        } else {
            "atm_withdraw"
        };
        let cash = episode.step(name, operation, actor);
        let account = self.demand(actor).to_owned();
        assert!(
            cash.at() >= start.at() && cash.at() - start.at() < chrono::TimeDelta::minutes(3),
            "fresh ATM session for {name}"
        );
        self.labels(start, &[(role, "account", &account)]);
        self.labels(cash, &[(role, "account", &account)]);
        let event = self.event(start, "atm.session_started");
        let session = string(event, "session_id").to_owned();
        assert!(
            event["account_no"] == account && event["payload"]["session_id"] == session,
            "ATM start account and session"
        );
        let sessions: Vec<(String, String, String, String)> =
            rows(self.db, ATM_SESSION, &start.request_id).await;
        let [(oracle_session, oracle_account, started, expires)] = sessions.as_slice() else {
            panic!("one Oracle ATM session from {name}_start");
        };
        assert!(
            oracle_session == &session
                && oracle_account == &account
                && time(started) == start.at()
                && time(expires) == start.at() + chrono::TimeDelta::minutes(3)
                && cash.at() < time(expires),
            "Oracle ATM session and expiry for {name}"
        );
        let operation = self.event(cash, "atm.operation");
        assert!(
            operation["account_no"] == account
                && operation["session_id"] == session
                && operation["outcome"] == "approved"
                && operation["payload"]["amount"] == signed.abs()
                && operation["payload"]["operation"]
                    == if signed > 0 { "deposit" } else { "withdraw" },
            "ATM cash event for {name}"
        );
        self.posting(cash, signed.abs(), &[(&account, signed)], Some(&session))
            .await;
        session
    }

    async fn payment(
        &mut self,
        op: &Operation,
        role: &str,
        peer_role: &str,
        peer: &str,
        session: &str,
        amount: i64,
    ) {
        let from = self.demand(&op.actor).to_owned();
        let to = self.demand(peer).to_owned();
        assert!(
            from != to && op.peer.as_deref() == Some(peer),
            "payment peer"
        );
        self.labels(op, &[(role, "account", &from), (peer_role, "account", &to)]);
        self.posting(
            op,
            amount,
            &[(&from, -amount), (&to, amount)],
            Some(session),
        )
        .await;
    }
}

fn ordered(steps: &[&Operation]) {
    for pair in steps.windows(2) {
        assert!(
            pair[0].at() < pair[1].at(),
            "{} must precede {}",
            pair[0].name,
            pair[1].name
        );
    }
}

async fn takeover(proof: &mut Proof<'_>, episode: &mut Episode<'_>) -> [String; 2] {
    let actor = episode.actor("login");
    let home = episode.step("home_baseline", "login", actor);
    let home_device = format!("dev-{actor}");
    let device = format!("device-{actor}-2");
    let failures = [1, 2, 3].map(|n| episode.step(&format!("bad_login_{n}"), "bad_login", actor));
    assert!(
        home.at() + chrono::TimeDelta::days(1) == failures[0].at()
            && home.date() != failures[0].date(),
        "A home baseline is one business day before the first failure"
    );
    let (baseline_device, _) = proof.login(home, "normal", Some(&home_device), false).await;
    assert!(
        baseline_device != device,
        "A home and takeover devices are distinct"
    );
    for (index, op) in failures.iter().enumerate() {
        proof.login(op, "A", Some(&device), true).await;
        let event = proof.event(op, "login.failed");
        assert!(
            event["region"] == "US-NY"
                && event["ip"] == "198.51.100.17"
                && event["payload"]["failed_count"] == index + 1,
            "A wrong-password context and count"
        );
    }
    let reset = episode.step("reset_request", "reset_request", actor);
    let sms = episode.step("sms", "sms", actor);
    proof.sms_request = Some(sms.request_id.clone());
    let confirm = episode.step("reset_confirm", "reset_confirm", actor);
    let login = episode.step("login", "login", actor);
    let payee = episode.step("payee", "payee", actor);
    let transfer = episode.step("transfer", "transfer", actor);
    ordered(&[
        home,
        failures[0],
        failures[1],
        failures[2],
        reset,
        sms,
        confirm,
        login,
        payee,
        transfer,
    ]);
    for op in [reset, sms, confirm] {
        proof.labels(op, &[("A", "customer", actor)]);
        no_money(proof.db, &op.request_id).await;
    }
    assert!(
        sms.result["decision"] == "retrieved",
        "simulated SMS outcome"
    );
    for (op, kind, reason) in [
        (reset, "password_reset.requested", "issued"),
        (confirm, "password_reset.completed", "completed"),
    ] {
        let event = proof.event(op, kind);
        assert!(
            event["customer_no"] == actor
                && event["device_id"] == device
                && event["outcome"] == "completed"
                && event["payload"]["reason"] == reason,
            "password reset event for {}",
            op.name
        );
    }
    let resets: Vec<(String, String, String, i64)> = rows(proof.db, RESET, actor).await;
    let [(issued, used, expires, attempts)] = resets.as_slice() else {
        panic!("one Oracle password reset")
    };
    assert!(
        time(issued) == reset.at()
            && time(used) == confirm.at()
            && time(expires) == reset.at() + chrono::TimeDelta::minutes(5)
            && confirm.at() < time(expires)
            && *attempts == 0,
        "reset consumed in its five-minute window"
    );
    let (_, session) = proof.login(login, "A", Some(&device), false).await;
    let new_device = proof.event(login, "device.new");
    assert!(
        new_device["customer_no"] == actor
            && new_device["device_id"] == device
            && new_device["session_id"] == session
            && new_device["region"] == "US-NY"
            && new_device["ip"] == "198.51.100.17",
        "first successful use of A device"
    );
    let receiver = transfer.peer.as_deref().expect("A recipient");
    proof.payee(payee, "A", receiver, &session).await;
    proof
        .payment(transfer, "A", "normal", receiver, &session, 1_500_000)
        .await;
    [actor.into(), receiver.into()]
}

async fn victim_and_mule(proof: &mut Proof<'_>, episode: &mut Episode<'_>) -> (Term, String) {
    let victim = episode.actor("victim_1_login");
    let mule = episode.actor("mule_login");
    assert!(victim != mule, "distinct B and C customers");
    let payer = proof.demand(victim).to_owned();
    let mule_account = proof.demand(mule).to_owned();
    let login = episode.step("victim_1_login", "login", victim);
    let open = episode.step("victim_1_open", "term_open", victim);
    let lookup = episode.step("victim_1_identity", "term_identity", victim);
    let close = episode.step("victim_1_close", "term_close", victim);
    let payee = episode.step("victim_1_payee", "payee", victim);
    let transfer = episode.step("victim_1_transfer", "transfer", victim);
    let victim_cash = episode.0["victim_1_withdraw"];
    let mule_login = episode.step("mule_login", "login", mule);
    let settlement = episode.step("mule_settlement", "inbound", mule);
    let inbound = episode.step("mule_inbound", "inbound", mule);
    let mule_cash = episode.0["mule_withdraw"];
    ordered(&[login, open, lookup, close, payee, transfer, victim_cash]);
    ordered(&[mule_login, transfer, settlement, inbound, mule_cash]);
    assert!(
        victim_cash.at() < mule_cash.at()
            && mule_cash.at() - transfer.at() < chrono::TimeDelta::minutes(2),
        "B-to-C cash-out timing"
    );

    let home_device = format!("dev-{victim}");
    let (_, session) = proof
        .login(login, "normal", Some(&home_device), false)
        .await;
    proof
        .login(mule_login, "C", Some("device-180"), false)
        .await;
    let terms: Vec<(i64, String, String, i64, String, String)> =
        rows(proof.db, TERM, &open.request_id).await;
    let [(id, term, linked, principal, method, closed_at)] = terms.as_slice() else {
        panic!("one term account created by the opening request")
    };
    let id = id.to_string();
    assert!(
        linked == &payer
            && term != linked
            && *principal == 1_000_000
            && method == "EARLY"
            && time(closed_at) == close.at(),
        "term ownership linkage, principal and close"
    );
    assert!(
        open.args["terms_id"] == 2
            && open.result["deposit_id"] == id
            && open.result["account_num_masked"] == mask_account(term)
            && open.result["payer_account_no"] == payer,
        "journal opening agrees with Oracle"
    );
    assert!(
        lookup.args["open_step"] == open.name
            && close.args["open_step"] == open.name
            && close.args["identity_step"] == lookup.name,
        "term step dependencies"
    );
    let opening = json!({
        "opening_request_id": open.request_id, "deposit_id": id,
        "account_num_masked": mask_account(term), "payer_account_no": payer
    });
    let mut closing = opening.clone();
    closing
        .as_object_mut()
        .expect("closing dependency")
        .extend([
            ("lookup_request_id".into(), json!(lookup.request_id)),
            ("term_account_no".into(), json!(term)),
            ("linked_account_no".into(), json!(payer)),
        ]);
    assert!(
        lookup.dependency == opening
            && close.dependency == closing
            && lookup.result["deposit_id"] == id
            && lookup.result["term_account_no"] == *term
            && lookup.result["linked_account_no"] == payer,
        "journal identity result and bound close agree with Oracle"
    );

    proof.labels(open, &[("normal", "account", &payer)]);
    proof.labels(lookup, &[("B", "customer", victim)]);
    proof.labels(close, &[("B", "account", &payer), ("B", "account", term)]);
    assert!(
        proof
            .events
            .get(&lookup.request_id)
            .is_none_or(Vec::is_empty),
        "lookup emitted bank events"
    );
    no_money(proof.db, &lookup.request_id).await;
    proof
        .lookups
        .insert(lookup.request_id.clone(), (200, "GENERATOR"));

    proof
        .posting(
            open,
            1_000_000,
            &[(&payer, -1_000_000), (term, 1_000_000)],
            Some(&session),
        )
        .await;
    proof
        .posting(
            close,
            1_000_000,
            &[(term, -1_000_000), (&payer, 1_000_000)],
            Some(&session),
        )
        .await;
    proof.payee(payee, "B", mule, &session).await;
    proof
        .payment(transfer, "B", "C", mule, &session, 300_000)
        .await;
    proof
        .cash(episode, "victim_1_withdraw", victim, "B", -700_000)
        .await;
    for (op, sender, amount) in [
        (settlement, "settlement", 450_000),
        (inbound, "unrelated", 200_000),
    ] {
        assert_eq!(op.args["sender_kind"], sender, "external sender role");
        proof.labels(op, &[("C", "account", &mule_account)]);
        proof
            .posting(op, amount, &[(&mule_account, amount)], None)
            .await;
    }
    proof
        .cash(episode, "mule_withdraw", mule, "C", -900_000)
        .await;

    type History = (Option<String>, String, String, String);
    let history: Vec<History> = rows(proof.db, STATUS, &mule_account).await;
    let [
        (initial, active, opened, opened_at),
        (from, dormant, reason, dormant_at),
        (previous, current, trigger, active_at),
    ] = history.as_slice()
    else {
        panic!("C must have exactly opening, dormancy and reactivation transitions")
    };
    let age = time(dormant_at).with_timezone(&kst()).date_naive()
        - time(opened_at).with_timezone(&kst()).date_naive();
    assert!(
        initial.is_none()
            && active == "ACTIVE"
            && opened == "OPENED"
            && from.as_deref() == Some("ACTIVE")
            && dormant == "DORMANT"
            && reason == "dormancy"
            && age.num_days() == 365
            && time(dormant_at) < mule_login.at()
            && previous.as_deref() == Some("DORMANT")
            && current == "ACTIVE"
            && trigger == "customer_activity"
            && time(active_at) == mule_cash.at(),
        "C dormant before login and incoming funds, reactivated at ATM cash-out"
    );
    let activation = proof.event(mule_cash, "account.status_changed");
    assert!(
        activation["account_no"] == mule_account
            && activation["channel"] == "ATM"
            && activation["payload"]
                == json!({"from": "DORMANT", "to": "ACTIVE", "reason": "customer_activity"}),
        "C activation attributed to cash request"
    );
    (
        Term {
            id,
            account: term.clone(),
            owner: victim.into(),
            linked: payer,
        },
        mule.into(),
    )
}

async fn split_deposits(proof: &mut Proof<'_>, episode: &mut Episode<'_>) -> [String; 2] {
    let actors = [
        episode.actor("deposit_1_1_1"),
        episode.actor("deposit_1_2_1"),
    ];
    assert!(actors[0] != actors[1], "D needs two distinct customers");
    let first = episode.0["deposit_1_1_1"]
        .at()
        .with_timezone(&kst())
        .date_naive();
    let mut sessions = HashSet::new();
    for (index, actor) in actors.iter().enumerate() {
        let mut expected = Vec::new();
        for day in 1..=3 {
            let date = (first + chrono::TimeDelta::days(day - 1)).to_string();
            for deposit in 1..=2 {
                let name = format!("deposit_{day}_{}_{deposit}", index + 1);
                assert_eq!(episode.0[name.as_str()].date(), date, "D posting KST date");
                let session = proof.cash(episode, &name, actor, "D", 4_800_000).await;
                assert!(sessions.insert(session), "D reuses an ATM session");
            }
            expected.push((date, 9_600_000_i64, 0_i64, 0_i64));
        }
        let actual: Vec<(String, i64, i64, i64)> = rows(proof.db, DAILY, proof.demand(actor)).await;
        assert_eq!(
            actual, expected,
            "D exact Oracle cash totals across three dates"
        );
    }
    actors.map(str::to_owned)
}

async fn account_ledger(proof: &Proof<'_>, account: &str, owner: &str, seeded: i64, term: bool) {
    type Account = (String, String, String, i64, i64, String);
    let rows_found: Vec<Account> = rows(proof.db, ACCOUNT, account).await;
    let [(customer, subtype, status, final_balance, terms, opened_at)] = rows_found.as_slice()
    else {
        panic!("one Oracle scenario account")
    };
    let (expected_subtype, expected_status, expected_terms) = if term {
        ("TERM", "CLOSED", 2)
    } else {
        ("DEMAND", "ACTIVE", 1)
    };
    assert!(
        customer == owner
            && subtype == expected_subtype
            && status == expected_status
            && *terms == expected_terms,
        "Oracle scenario account owner, product and final status"
    );

    #[derive(Deserialize)]
    struct Leg {
        account: String,
        request_id: Option<String>,
        transfer_id: Option<i64>,
        amount: i64,
        balance_after: i64,
        channel: String,
        date: String,
        occurred_at: String,
        counterparty_bank: Option<String>,
        counterparty_account: Option<String>,
    }
    let legs = rows::<String>(proof.db, LEGS, account)
        .await
        .into_iter()
        .map(|row| serde_json::from_str::<Leg>(&row).expect("decode Oracle account ledger"))
        .collect::<Vec<_>>();
    let mut expected: HashMap<_, _> = proof.expected_legs[account]
        .iter()
        .map(|leg| (leg.request_id.as_str(), leg))
        .collect();
    assert_eq!(
        legs.len(),
        expected.len() + usize::from(seeded > 0),
        "exact account ledger row count"
    );
    let mut balance = 0;
    let mut last = time(opened_at);
    for (index, actual) in legs.iter().enumerate() {
        let at = time(&actual.occurred_at);
        assert!(
            actual.account == account && at >= last,
            "Oracle ledger account and sequence time"
        );
        last = at;
        if index == 0 && seeded != 0 {
            assert!(
                actual.request_id.is_none()
                    && actual
                        .transfer_id
                        .is_some_and(|transfer_id| transfer_id > 0)
                    && actual.amount == seeded
                    && actual.channel == "SYSTEM"
                    && at == time(opened_at),
                "seeded opening ledger credit"
            );
        } else {
            let id = actual
                .request_id
                .as_deref()
                .expect("scenario ledger request ID");
            let leg = expected
                .remove(id)
                .expect("unexpected request on scenario account ledger");
            assert!(
                actual.transfer_id == Some(leg.transfer_id)
                    && actual.amount == leg.amount
                    && actual.channel == leg.channel
                    && actual.date == leg.date
                    && at == leg.at,
                "Oracle ledger leg for request {id}"
            );
            if let Some(external) = leg.external_sender {
                assert!(
                    actual.counterparty_bank.as_deref() == Some("901")
                        && actual.counterparty_account.as_deref() == Some(external),
                    "external credit counterparty"
                );
            }
        }
        balance += actual.amount;
        assert_eq!(actual.balance_after, balance, "Oracle running balance");
    }
    assert!(expected.is_empty(), "unmatched expected ledger legs");
    assert_eq!(*final_balance, balance, "Oracle closing balance");
}

// The Oracle harness already supplies Python 3. Use its HTTP client without a
// Cargo dependency or credentials in process arguments, diagnostics, or files.
const HTTP_CLIENT: &str = r#"
import json
import sys
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

try:
    call = json.load(sys.stdin)
    request = Request(call["url"], headers=call["headers"],
                      data=None if call["body"] is None else json.dumps(call["body"]).encode())
    client = build_opener(ProxyHandler({}), NoRedirect())
    try:
        response = client.open(request, timeout=10)
    except HTTPError as error:
        response = error
    with response:
        raw = response.read(65537)
        if len(raw) > 65536:
            sys.exit(1)
        body = json.loads(raw) if raw else None
        json.dump({"status": response.status, "request_id": response.headers.get("x-request-id"),
                   "cache_control": response.headers.get("cache-control"), "body": body}, sys.stdout)
except Exception:
    sys.exit(1)
"#;

#[derive(Deserialize)]
struct HttpResponse {
    status: u16,
    request_id: Option<String>,
    cache_control: Option<String>,
    body: Value,
}

fn http(base: &str, path: &str, token: Option<&str>, body: Value) -> (String, HttpResponse) {
    let id = ulid::Ulid::new().to_string();
    let mut headers = json!({"x-request-id": id, "content-type": "application/json"});
    if let Some(token) = token {
        headers["authorization"] = json!(format!("Bearer {token}"));
    }
    let url = format!("{}{path}", base.trim_end_matches('/'));
    assert!(
        url.starts_with("http://") || url.starts_with("https://"),
        "bank URL scheme"
    );
    let request = json!({"url": url, "headers": headers, "body": body});
    let mut child = Command::new("python3")
        .args(["-I", "-c", HTTP_CLIENT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start Python HTTP client");
    let write = child
        .stdin
        .take()
        .expect("HTTP input pipe")
        .write_all(request.to_string().as_bytes());
    let output = child.wait_with_output().expect("wait for HTTP probe");
    assert!(
        write.is_ok() && output.status.success(),
        "HTTP probe transport failed for request {id}"
    );
    let response: HttpResponse = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("invalid HTTP probe response for request {id}"));
    assert!(
        response.request_id.as_deref() == Some(id.as_str()),
        "response header correlation"
    );
    if !response.body.is_null() {
        assert!(
            response.body["request_id"] == id,
            "response envelope correlation"
        );
    }
    (id, response)
}

fn lookup_http(
    proof: &mut Proof<'_>,
    base: &str,
    path: &str,
    token: Option<&str>,
    expected: (u16, &'static str),
) -> HttpResponse {
    let before = Utc::now();
    let (id, response) = http(base, path, token, Value::Null);
    proof.lookup_windows.push((before, Utc::now()));
    assert_eq!(
        response.status, expected.0,
        "lookup response for request {id}"
    );
    proof.lookups.insert(id, expected);
    response
}

fn payload(mut body: Value) -> Value {
    let object = body.as_object_mut().expect("bank response envelope");
    for key in ["api_tran_id", "api_tran_dtm", "request_id"] {
        assert!(
            object.remove(key).is_some_and(|value| value.is_string()),
            "bank envelope field {key}"
        );
    }
    body
}

async fn boundary(proof: &mut Proof<'_>, roster: &Collection<Document>, terms: &[Term]) {
    let public = env::var("SALTATION_PUBLIC_URL").unwrap_or_else(|_| "http://bank:8080".into());
    let generator =
        env::var("SALTATION_GENERATOR_URL").unwrap_or_else(|_| "http://bank:8081".into());
    let mut tokens = Vec::new();
    for term in terms {
        let matched = roster
            .find_one(doc! {
                "customer_no": &term.owner, "account_no": &term.linked, "protected": true
            })
            .projection(doc! {"phone": 1, "password": 1})
            .await
            .unwrap_or_else(|_| panic!("read protected synthetic credentials"))
            .expect("protected synthetic owner");
        let (_, response) = http(
            &public,
            "/v2.0/ext/auth/login",
            None,
            json!({
                "phone": matched.get_str("phone").expect("synthetic phone"),
                "password": matched.get_str("password").expect("synthetic password"),
                "device_id": "stage6-proof"
            }),
        );
        assert!(
            response.status == 200 && response.body["rsp_code"] == "A0000",
            "synthetic proof login"
        );
        tokens.push(string(&response.body, "access_token").to_owned());
    }
    for (index, term) in terms.iter().enumerate() {
        let owner = &tokens[index];
        let other = &tokens[1 - index];
        let path = format!("/internal/v1/deposits/{}/identity", term.id);
        let response = lookup_http(proof, &generator, &path, Some(owner), (200, "GENERATOR"));
        assert!(
            response.status == 200 && response.cache_control.as_deref() == Some("no-store"),
            "owner lookup and no-store"
        );
        assert!(
            payload(response.body)
                == json!({
                    "rsp_code": "A0000", "rsp_message": "", "deposit_id": term.id,
                    "term_account_no": term.account, "linked_account_no": term.linked
                }),
            "owner lookup identity differs from Oracle"
        );

        for token in [None, Some("stage6-invalid-session")] {
            let response = lookup_http(proof, &generator, &path, token, (401, "GENERATOR"));
            assert!(
                response.status == 401 && response.body["rsp_code"] == "O0001",
                "missing or invalid session denied"
            );
            assert!(
                payload(response.body)
                    .as_object()
                    .is_some_and(|fields| fields.len() == 2),
                "authentication denial exposed identity fields"
            );
        }
        let denied = lookup_http(proof, &generator, &path, Some(other), (404, "GENERATOR"));
        assert_eq!(denied.status, 404, "non-owner lookup denied");
        let denied = payload(denied.body);
        assert!(
            denied == json!({"rsp_code": "A0003", "rsp_message": "대상 없음"}),
            "generic lookup error"
        );
        for absent in ["0", "unknown"] {
            let path = format!("/internal/v1/deposits/{absent}/identity");
            let response = lookup_http(proof, &generator, &path, Some(owner), (404, "GENERATOR"));
            assert!(
                response.status == 404 && payload(response.body) == denied,
                "unknown, malformed and non-owner errors agree"
            );
        }

        let response = lookup_http(proof, &public, &path, Some(owner), (404, "PUBLIC"));
        assert!(
            response.status == 404 && response.body.is_null(),
            "identity route absent on public listener"
        );
        let (_, detail) = http(
            &public,
            &format!("/v2.0/ext/deposits/{}", term.id),
            Some(owner),
            Value::Null,
        );
        assert!(
            detail.status == 200
                && detail.body["rsp_code"] == "A0000"
                && detail.body["deposit_id"] == term.id
                && detail.body["account_num_masked"] == mask_account(&term.account),
            "public term detail is masked"
        );
        let serialized = detail.body.to_string();
        assert!(
            !serialized.contains(&term.account)
                && !serialized.contains(&term.linked)
                && detail.body.get("term_account_no").is_none()
                && detail.body.get("linked_account_no").is_none(),
            "public detail exposed account identity"
        );
    }
}

fn verify_application_logs(proof: &Proof<'_>, accounts: &[&str]) {
    let lookups = &proof.lookups;
    let dir = env::var("SALTATION_LOG_DIR").unwrap_or_else(|_| "/var/log/saltation".into());
    let mut access = BTreeSet::new();
    let mut sms_seen = false;
    let sms = proof.sms_request.as_deref().expect("A SMS request ID");
    for entry in fs::read_dir(dir).expect("mount bank app logs read-only in SALTATION_LOG_DIR") {
        let entry = entry.expect("list application logs");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("app") || !name.ends_with(".log") {
            continue;
        }
        let file = fs::File::open(entry.path()).expect("read application log");
        for line in BufReader::new(file).lines() {
            let line = line.expect("read application log line");
            if line.contains(sms) {
                let record: Value = serde_json::from_str(&line).expect("application log JSON");
                if record["target"] == "request" {
                    assert!(
                        !sms_seen
                            && record["request_id"] == sms
                            && record["method"] == "GET"
                            && record["status"] == 200
                            && record["listener"] == "GENERATOR"
                            && record["endpoint"] == "/internal/v1/sms/{phone}",
                        "A retrieved its SMS through the generator listener"
                    );
                    sms_seen = true;
                }
            }
            let redacted = accounts.iter().all(|account| !line.contains(account))
                && !line.contains("\"term_account_no\"")
                && !line.contains("\"linked_account_no\"");
            if !redacted {
                let record: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("invalid application log JSON"));
                let at = time(string(&record, "timestamp")).with_timezone(&Utc);
                assert!(
                    !proof
                        .lookup_windows
                        .iter()
                        .any(|(before, after)| *before <= at && at <= *after),
                    "application log exposed account identity during a lookup probe"
                );
            }
            for (id, &(status, listener)) in
                lookups.iter().filter(|(id, _)| line.contains(id.as_str()))
            {
                assert!(
                    redacted,
                    "lookup log exposed account identity for request {id}"
                );
                let record: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("invalid lookup log record for {id}"));
                if record["target"] == "request" {
                    let expected_path = if listener == "PUBLIC" {
                        "unmatched"
                    } else {
                        IDENTITY_ROUTE
                    };
                    assert!(
                        record["request_id"] == *id
                            && record["method"] == "GET"
                            && record["status"] == status
                            && record["listener"] == listener
                            && record["endpoint"] == expected_path,
                        "lookup access log mismatch for request {id}"
                    );
                    assert!(
                        access.insert(id),
                        "duplicate lookup access log for request {id}"
                    );
                }
            }
        }
    }
    assert!(
        sms_seen,
        "A SMS request is missing from mounted application logs"
    );
    assert_eq!(
        access.len(),
        lookups.len(),
        "lookup request IDs missing from mounted application logs"
    );
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct Credit {
    window: String,
    kind: String,
    identity: String,
    episode_id: String,
}

#[tokio::test]
#[ignore = "requires the Stage 6 annual synthetic fixture, running bank listeners, Oracle, MongoDB and bank app logs"]
async fn generator_scenarios_proof() {
    let fixture: Fixture = serde_json::from_str(&required_env("GENERATOR_SCENARIOS_FIXTURE"))
        .unwrap_or_else(|_| panic!("invalid GENERATOR_SCENARIOS_FIXTURE JSON"));
    assert!(!fixture.run_id.trim().is_empty(), "run_id is required");
    let client = mongodb::Client::with_uri_str(required_env("MONGO_TRUTH_URI"))
        .await
        .unwrap_or_else(|_| panic!("connect to protected Mongo truth"));
    let truth = client
        .default_database()
        .unwrap_or_else(|| client.database("truth"));
    let run = truth
        .collection::<Document>("runs")
        .find_one(doc! {
            "_id": &fixture.run_id, "kind": "generator-stage6", "completed": true
        })
        .await
        .unwrap_or_else(|_| panic!("find completed Stage 6 run"));
    assert!(run.is_some(), "a completed Stage 6 run is required");
    let filter = doc! {"run_id": &fixture.run_id};
    let operations: Vec<Operation> =
        mongo_rows(&truth.collection("stage6_operations"), filter.clone()).await;
    let mut episodes = BTreeMap::new();
    let mut requests = BTreeSet::new();
    let mut actors = BTreeSet::new();
    for op in &operations {
        assert!(
            op.window == "annual" && op.state == "succeeded",
            "every operation must be successful annual evidence"
        );
        let expected = match op.operation.as_str() {
            "bad_login" => "expected_rejection",
            "sms" => "retrieved",
            _ => "approved",
        };
        assert!(
            op.result["decision"] == expected,
            "journal decision for {}",
            op.name
        );
        assert!(
            op.request_id.parse::<ulid::Ulid>().is_ok() && requests.insert(op.request_id.clone()),
            "distinct operation ULID"
        );
        actors.insert(op.actor.clone());
        actors.extend(op.peer.iter().cloned());
        let episode = episodes
            .entry((op.episode_kind.as_str(), op.episode_id.as_str()))
            .or_insert_with(|| Episode(BTreeMap::new()));
        assert!(
            episode.0.insert(op.name.as_str(), op).is_none(),
            "duplicate step in episode"
        );
    }
    let kinds: Vec<_> = episodes.keys().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, ["A", "BC", "BC", "D"], "annual A1 B2 C2 D1 workload");

    let roster = truth.collection("roster");
    let people: Vec<Person> = mongo_rows(
        &roster,
        doc! {
            "customer_no": {"$in": actors.iter().cloned().collect::<Vec<_>>()}, "protected": true
        },
    )
    .await;
    assert_eq!(
        people.len(),
        actors.len(),
        "all scenario roles have protected roster entries"
    );
    let db = Db::connect(
        &bank::shared::db::DbConfig::from_env().unwrap_or_else(|_| panic!("Oracle config")),
    )
    .unwrap_or_else(|_| panic!("connect to Oracle"));
    wait_for_outbox(&db).await;
    let events_collection = outbox_relay::events_collection(&required_env("MONGO_URI"))
        .await
        .unwrap_or_else(|_| panic!("connect to authoritative bank events"));
    let mut events: HashMap<String, Vec<Value>> = HashMap::new();
    for event in mongo_rows::<Value>(
        &events_collection,
        doc! {"request_id": {"$in": requests.into_iter().collect::<Vec<_>>()}},
    )
    .await
    {
        events
            .entry(string(&event, "request_id").to_owned())
            .or_default()
            .push(event);
    }
    let mut proof = Proof {
        db: &db,
        run_id: &fixture.run_id,
        people: people
            .into_iter()
            .map(|p| (p.customer_no.clone(), p))
            .collect(),
        events,
        labels: Vec::new(),
        sms_request: None,
        expected_legs: HashMap::new(),
        lookups: BTreeMap::new(),
        lookup_windows: Vec::new(),
    };
    let mut expected_credit = Vec::new();
    let mut terms = Vec::new();
    let mut mules = BTreeSet::new();
    let mut used = BTreeSet::new();
    for ((kind, id), mut episode) in episodes {
        let mut credit = |kind: &str, identity: &str| {
            expected_credit.push(Credit {
                window: "annual".into(),
                kind: kind.into(),
                identity: identity.into(),
                episode_id: id.into(),
            })
        };
        let customers = match kind {
            "A" => {
                credit("A", id);
                takeover(&mut proof, &mut episode).await
            }
            "BC" => {
                let (term, mule) = victim_and_mule(&mut proof, &mut episode).await;
                credit("B", &term.owner);
                credit("C", proof.demand(&mule));
                mules.insert(mule.clone());
                let victim = term.owner.clone();
                terms.push(term);
                [victim, mule]
            }
            "D" => {
                credit("D", id);
                split_deposits(&mut proof, &mut episode).await
            }
            _ => unreachable!("validated annual episode kinds"),
        };
        assert!(
            episode.0.is_empty(),
            "unexpected operation in {kind} episode"
        );
        for customer in customers {
            assert!(
                used.insert(customer),
                "annual scenario roles must be disjoint"
            );
        }
    }
    let mut credits: Vec<Credit> =
        mongo_rows(&truth.collection("stage6_credit"), filter.clone()).await;
    credits.sort();
    expected_credit.sort();
    assert!(
        credits == expected_credit,
        "exact annual A1 B2 C2 D1 quota credit"
    );
    let mut labels: Vec<Label> = mongo_rows(&truth.collection("labels"), filter).await;
    labels.sort();
    proof.labels.sort();
    assert!(
        labels == proof.labels,
        "exact persisted request labels for the run"
    );

    for person in proof.people.values() {
        let seeded = if mules.contains(&person.customer_no) {
            0
        } else {
            3_000_000
        };
        assert_eq!(person.opening_balance, seeded, "synthetic opening balance");
        account_ledger(
            &proof,
            &person.account_no,
            &person.customer_no,
            seeded,
            false,
        )
        .await;
    }
    for term in &terms {
        account_ledger(&proof, &term.account, &term.owner, 0, true).await;
    }

    boundary(&mut proof, &roster, &terms).await;
    wait_for_outbox(&db).await;
    let ids: Vec<_> = proof.lookups.keys().cloned().collect();
    assert!(
        mongo_rows::<Value>(&events_collection, doc! {"request_id": {"$in": ids}})
            .await
            .is_empty(),
        "lookup requests must have no bank event records"
    );
    for id in proof.lookups.keys() {
        no_money(&db, id).await;
    }
    let accounts: Vec<_> = proof
        .people
        .values()
        .map(|person| person.account_no.as_str())
        .chain(terms.iter().map(|term| term.account.as_str()))
        .collect();
    verify_application_logs(&proof, &accounts);
}
