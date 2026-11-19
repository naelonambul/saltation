//! Read-only proof of the scaled Stage 6 fixture against truth, Oracle and bank logs.

use std::collections::{BTreeMap, BTreeSet};
use std::{env, fs};

use bank::features::outbox_relay;
use bank::shared::db::{Db, DbConfig, SqlStmt};
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset, NaiveDate, TimeDelta, Timelike};
use mongodb::bson::{Document, doc};
use mongodb::{Collection, Database};
use serde::Deserialize;
use serde_json::{Value, json};

const CLOCK: SqlStmt = sql_stmt!(
    "test.generator_lifecycle_clock",
    "sql/generator_lifecycle_clock.sql"
);
const REQUEST: SqlStmt = sql_stmt!(
    "test.generator_lifecycle_request",
    "sql/generator_lifecycle_request.sql"
);
const DORMANCY: SqlStmt = sql_stmt!(
    "test.generator_lifecycle_dormancy",
    "sql/generator_lifecycle_dormancy.sql"
);
const COUNT_OUTBOX: SqlStmt = sql_stmt!("test.count_outbox", "sql/count_outbox.sql");
const MODE: &str = "/internal/v1/mode/live";

type Stamp = DateTime<FixedOffset>;
type Counts = BTreeMap<String, usize>;
type Labels = BTreeSet<(String, String, String)>;

#[derive(Deserialize)]
struct Fixture {
    run_id: String,
    started_at: Stamp,
    finished_at: Stamp,
    mode_request_id: String,
    mode_repeat_request_id: String,
    dispatch_count: usize,
    profiles: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Operation {
    request_id: String,
    episode_id: String,
    episode_kind: String,
    name: String,

    operation: String,
    window: String,
    carry_in: bool,
    prior_to_live: bool,
    persona: String,
    context: Value,
    actor: String,
    peer: Option<String>,
    business_time: Stamp,
    state: String,
    result: Value,
    args: Value,
}

struct Cutover {
    planned: Stamp,
    committed: Stamp,
    observed: Stamp,
    end: Stamp,
}

fn time(value: &Value) -> Stamp {
    DateTime::parse_from_rfc3339(value.as_str().expect("timestamp string"))
        .expect("RFC3339 timestamp")
}

async fn documents(collection: &Collection<Document>, filter: Document) -> Vec<Value> {
    let mut cursor = collection
        .find(filter)
        .await
        .expect("read proof collection");
    let mut rows = Vec::new();
    while cursor.advance().await.expect("advance proof cursor") {
        let row = cursor.deserialize_current().expect("decode proof document");
        rows.push(serde_json::to_value(row).expect("proof document JSON"));
    }
    rows
}

async fn truth_rows(truth: &Database, name: &str, run_id: &str) -> Vec<Value> {
    documents(&truth.collection(name), doc! {"run_id": run_id}).await
}

fn expected_credit() -> Value {
    json!({
        "annual": {"A":1, "B":2, "C":2, "E":1, "F":1, "G":1, "lookalike":6},
        "live": {"B":2, "C":2, "D":2}
    })
}

async fn assert_manifest(db: &Db, manifest: &Value, fixture: &Fixture) -> Cutover {
    let expected = expected_credit();
    let config = &manifest["inputs"]["config"];
    let run = &manifest["inputs"]["run"];
    let progress = &manifest["progress"];
    let summary = &manifest["summary"];
    assert_eq!(manifest["kind"], "generator-stage6");
    assert_eq!(manifest["completed"], true);
    assert!(
        manifest.get("owner").is_none(),
        "the finished run released its owner"
    );
    assert_eq!(config["population"], 40);
    assert_eq!(config["normal"], true);
    assert_eq!(config["annual"], expected["annual"]);
    assert_eq!(config["live"], expected["live"]);
    assert_eq!(run["start"], 73_000_000);
    assert_eq!(run["days"], 7);
    assert_eq!(run["live_seconds"], 420);
    assert_eq!(summary["run_id"], fixture.run_id);
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["progress"], *progress);
    assert_eq!(summary["credit"], expected);
    assert_eq!(summary["scenario_quotas"], expected);
    assert_eq!(
        summary["planned_scenarios"],
        json!({
            "A":1, "BC":4, "D":2, "E":1, "F":1, "G":1,
            "travel":1, "trip":1, "online":1, "new_phone":1, "purchase":1, "household":1,
        })
    );
    assert_eq!(progress["backfill_finished"], true);
    assert_eq!(progress["live_intended"], true);
    assert_eq!(progress["seed_state"], "seeded");
    let (mode, backfill, switched): (String, String, String) = db
        .run(|s| s.query_row(&CLOCK, &[]))
        .await
        .expect("read Oracle clock");
    let planned = time(&run["live_at"]);
    let committed = switched.parse::<Stamp>().expect("Oracle switch timestamp");
    let observed = time(&progress["live_started"]);
    let end = observed + TimeDelta::seconds(420);
    assert_eq!(mode, "LIVE");
    assert_eq!(
        backfill.parse::<Stamp>().expect("Oracle BACKFILL boundary"),
        planned
    );
    assert!(planned > fixture.started_at && planned < fixture.started_at + TimeDelta::seconds(65));
    assert!(planned <= committed);
    assert_eq!(
        committed, observed,
        "recovery uses the committed transition, not the later read"
    );
    assert_eq!(planned.date_naive(), observed.date_naive());
    assert!(observed <= time(&progress["live_observed_at"]));
    assert!(
        fixture.finished_at >= end,
        "generator ran through the live measurement window"
    );
    let undelivered: i64 = db
        .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
        .await
        .expect("read Oracle outbox count");
    assert_eq!(undelivered, 0, "outbox drained before completion");
    Cutover {
        planned,
        committed,
        observed,
        end,
    }
}

fn access_logs(fixture: &Fixture, cutover: &Cutover) -> BTreeMap<String, Value> {
    let directory = env::var("SALTATION_LOG_DIR").unwrap_or_else(|_| "/var/log/saltation".into());
    let mut requests = BTreeMap::new();
    let mut switches = Vec::new();
    for entry in fs::read_dir(directory).expect("read the mounted bank application log directory") {
        let path = entry.expect("bank application log entry").path();
        let name = path.file_name().expect("log file name").to_string_lossy();
        if !name.starts_with("app") || !name.ends_with(".log") {
            continue;
        }
        for line in fs::read_to_string(path)
            .expect("read bank application log")
            .lines()
        {
            let row: Value = serde_json::from_str(line).expect("bank application log JSON");
            if row["target"] != "request" || row["listener"] != "GENERATOR" {
                continue;
            }
            let at = time(&row["timestamp"]);
            if at < fixture.started_at {
                continue;
            }
            let id = row["request_id"]
                .as_str()
                .expect("access request ID")
                .to_owned();
            if row["endpoint"] == MODE {
                assert_eq!(row["status"], 200);
                assert_eq!(row["method"], "POST");
                assert!(at >= cutover.committed);
                switches.push(id.clone());
            }
            if matches!(
                row["endpoint"].as_str(),
                Some("/internal/v1/clock/set" | "/internal/v1/clock/advance")
            ) {
                assert!(
                    at <= cutover.committed,
                    "clock write after the mode transition"
                );
            }
            assert!(
                requests.insert(id, row).is_none(),
                "duplicate HTTP dispatch"
            );
        }
    }
    assert_eq!(
        switches,
        [
            fixture.mode_request_id.clone(),
            fixture.mode_repeat_request_id.clone()
        ],
        "the generator sends one switch; the proof separately tests idempotent reentry"
    );
    requests
}

fn expected_http(op: &Operation) -> u64 {
    let (decision, status) = match (op.operation.as_str(), op.state.as_str()) {
        ("bad_login", "succeeded") => ("expected_rejection", 401),
        ("sms", "succeeded") => ("retrieved", 200),
        ("term_identity", "succeeded") => {
            // Identity validation is checked by its correlation with the close and opening legs.
            assert_eq!(op.result["http_status"], 200, "{}", op.request_id);
            return 200;
        }
        (_, "succeeded") => ("approved", 200),
        ("card" | "atm_withdraw" | "transfer", "declined")
            if op.episode_kind.starts_with("normal_") =>
        {
            ("rejected", 422)
        }
        _ => panic!("{}: unexpected operation state {}", op.request_id, op.state),
    };
    assert_eq!(op.result["decision"], decision, "{}", op.request_id);
    if let Some(saved) = op.result.get("http_status") {
        assert_eq!(*saved, status, "{}", op.request_id);
    }
    status
}

fn money_type(op: &Operation) -> Option<(&'static str, &'static str)> {
    match op.operation.as_str() {
        "transfer" => Some(("TRANSFER", "transfer")),
        "inbound" => Some(("INBOUND", "inbound")),
        "card" => Some(("CARD_PAYMENT", "card_payment")),
        "atm_deposit" => Some(("ATM_DEPOSIT", "atm_cash")),
        "atm_withdraw" => Some(("ATM_WITHDRAW", "atm_cash")),
        "term_open" => Some(("TERM_OPEN", "term_open")),
        "term_close" => Some(("TERM_CLOSE", "term_close")),
        _ => None,
    }
}

fn assert_completion<'a>(
    op: &Operation,
    http: u64,
    access: &Value,
    events: &'a [Value],
) -> Option<&'a Value> {
    let id = &op.request_id;
    assert_eq!(access["status"], http, "access status for {id}");
    let mut terminal = events
        .iter()
        .filter(|event| event["event_type"] == "request.completed");
    if matches!(op.operation.as_str(), "sms" | "term_identity") {
        let expected = if op.operation == "sms" {
            "/internal/v1/sms/{phone}"
        } else {
            "/internal/v1/deposits/{id}/identity"
        };
        assert_eq!(access["endpoint"], expected, "internal read {id}");
        assert_eq!(access["method"], "GET");
        assert!(
            terminal.next().is_none(),
            "internal read has an outbox completion"
        );
        return None;
    }
    let completion = terminal
        .next()
        .unwrap_or_else(|| panic!("{id}: missing bank terminal event"));
    assert!(terminal.next().is_none(), "{id}: duplicate bank completion");
    assert_eq!(completion["listener"], "generator", "{id}");
    assert_eq!(completion["human"], false, "{id}");
    assert_eq!(completion["payload"]["http_status"], http, "{id}");
    assert_eq!(
        completion["outcome"],
        if http == 200 { "completed" } else { "rejected" },
        "{id}"
    );
    assert!(
        completion["payload"].get("replayed").is_none(),
        "{id}: replay"
    );
    if let Some((kind, _)) = money_type(op) {
        assert_eq!(completion["payload"]["request_type"], kind, "{id}");
    }
    let domain = match op.operation.as_str() {
        "login" => Some("login.succeeded"),
        "bad_login" => Some("login.failed"),
        "reset_request" => Some("password_reset.requested"),
        "reset_confirm" => Some("password_reset.completed"),
        "atm_start" => Some("atm.session_started"),
        "card" if http == 200 => Some("card.authorized"),
        "card" => Some("card.declined"),
        _ => None,
    };
    if let Some(kind) = domain {
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_type"] == kind)
                .count(),
            1,
            "{id}: {kind}"
        );
    }
    Some(completion)
}

async fn posting(db: &Db, op: &Operation, event: &Value, http: u64) -> Vec<Value> {
    let id = op.request_id.clone();
    let (request_kind, movement_kind) = money_type(op).expect("monetary operation");
    let raw: Vec<String> = db
        .run(move |s| s.query_all(&REQUEST, &[&id]))
        .await
        .expect("read Oracle request and posting");
    let rows: Vec<Value> = raw
        .iter()
        .map(|row| serde_json::from_str(row).expect("posting JSON"))
        .collect();
    let id = &op.request_id;
    let expected_rows = if http == 200
        && matches!(
            op.operation.as_str(),
            "transfer" | "term_open" | "term_close"
        ) {
        2
    } else {
        1
    };
    assert_eq!(rows.len(), expected_rows, "{id}: request and leg count");
    for row in &rows {
        assert_eq!(row["id"], *id, "{id}: unexpected replay request");
        assert_eq!(row["type"], request_kind, "{id}");
        assert_eq!(row["listener"], "GENERATOR", "{id}");
        assert_eq!(
            row["status"],
            if http == 200 { "DONE" } else { "REJECTED" },
            "{id}"
        );
        assert_eq!(row["bank_code"], event["bank_rsp_code"], "{id}");
        if http == 200 {
            assert_eq!(row["kind"], movement_kind, "{id}");
            assert_eq!(row["posting_id"], rows[0]["posting_id"], "{id}");
            if let Some(transfer) = event["payload"].get("transfer_id") {
                assert_eq!(row["posting_id"], *transfer, "{id}");
            }
            let at = time(&row["at"]);
            assert_eq!(at.date_naive().to_string(), row["date"], "{id}");
            assert_eq!(
                at.timestamp_millis(),
                time(&event["occurred_at"]).timestamp_millis(),
                "{id}"
            );
        } else {
            assert!(
                row["posting_id"].is_null(),
                "{id}: a rejection posted money"
            );
        }
    }
    rows
}

fn demand(customer: &str) -> String {
    let number = customer
        .strip_prefix('C')
        .expect("synthetic customer number");
    assert_eq!(number.len(), 8);
    assert!((73_000_000..73_000_040).contains(&number.parse::<u32>().expect("synthetic slot")));
    format!("998{number}")
}

fn assert_money(op: &Operation, rows: &[Value], events: &[Value]) -> Option<String> {
    let id = &op.request_id;
    let account = demand(&op.actor);
    let mut legs: BTreeMap<&str, i64> = rows
        .iter()
        .map(|row| {
            (
                row["account"].as_str().expect("leg account"),
                row["leg"].as_i64().expect("signed amount"),
            )
        })
        .collect();
    assert_eq!(legs.len(), rows.len(), "{id}: duplicate account leg");
    let principal = op.args["amount"].as_i64();
    let primary = legs
        .remove(account.as_str())
        .unwrap_or_else(|| panic!("{id}: missing demand leg"));
    let mut term = None;
    match op.operation.as_str() {
        "transfer" => {
            let peer = demand(op.peer.as_deref().expect("transfer peer"));
            let amount = principal.expect("transfer amount");
            assert_eq!(primary, -amount, "{id}");
            assert_eq!(legs.remove(peer.as_str()), Some(amount), "{id}");
        }
        "term_open" | "term_close" => {
            assert_eq!(
                primary,
                if op.operation == "term_open" {
                    -1_000_000
                } else {
                    1_000_000
                },
                "{id}"
            );
            let (number, amount) = legs.pop_first().expect("term leg");
            assert_eq!(amount, -primary, "{id}");
            assert!(number.starts_with("20"), "{id}: term account number");
            term = Some(number.to_owned());
        }
        "inbound" | "atm_deposit" => {
            assert_eq!(Some(primary), principal, "{id}");
            if op.operation == "inbound" {
                let sender = match op.args["sender_kind"].as_str().expect("inbound sender") {
                    "salary" => "9016000000000001",
                    "settlement" => "9016000000000002",
                    "unrelated" => "9016000000000003",
                    _ => panic!("{id}: unknown sender"),
                };
                assert_eq!(rows[0]["counterparty"], sender, "{id}");
            }
        }
        "atm_withdraw" => assert_eq!(Some(-primary), principal, "{id}"),
        "card" => {
            let event = events
                .iter()
                .find(|row| row["event_type"] == "card.authorized")
                .expect("card domain event");
            assert_eq!(event["account_no"], account, "{id}");
            assert_eq!(event["payload"]["krw_amount"], -primary, "{id}");
            assert_eq!(
                event["payload"]["amount"],
                principal.expect("card amount").to_string(),
                "{id}"
            );
            assert_eq!(event["payload"]["currency"], op.args["currency"], "{id}");
        }
        _ => unreachable!("monetary operation"),
    }
    assert!(legs.is_empty(), "{id}: unexpected posting leg");
    assert_eq!(rows[0]["amount"], primary.abs(), "{id}: transfer amount");
    term
}

fn primary_label(op: &Operation) -> &str {
    if matches!(
        op.name.as_str(),
        "home_baseline" | "baseline_transfer" | "victim_1_login"
    ) || op.name.starts_with("baseline_transfer__")
        || (matches!(op.episode_kind.as_str(), "E" | "purchase") && op.name == "login")
        || op.operation == "term_open"
    {
        return "normal";
    }
    match op.episode_kind.as_str() {
        "A" | "D" | "E" | "F" | "G" => &op.episode_kind,
        "BC" if op.name.starts_with("mule_") => "C",
        "BC" => "B",
        kind if kind.starts_with("normal_") => "normal",
        _ => "lookalike",
    }
}

fn assert_labels(op: &Operation, term: Option<&str>, actual: Labels) {
    let label = primary_label(op);
    let subject_type = match op.operation.as_str() {
        "login" | "bad_login" | "sms" | "reset_request" | "reset_confirm" | "payee"
        | "term_identity" => "customer",
        _ => "account",
    };
    let subject = if subject_type == "customer" {
        op.actor.clone()
    } else {
        demand(&op.actor)
    };
    let mut expected = Labels::from([(subject_type.into(), subject, label.into())]);
    if let Some(number) = term.filter(|_| op.operation == "term_close") {
        expected.insert(("account".into(), number.into(), "B".into()));
    }
    if op.operation == "transfer" {
        let peer_label = if op.episode_kind == "BC" {
            "C"
        } else {
            "normal"
        };
        expected.insert((
            "account".into(),
            demand(op.peer.as_deref().expect("transfer peer")),
            peer_label.into(),
        ));
    }
    assert_eq!(actual, expected, "exact labels for {}", op.request_id);
}

fn count(counts: &mut Counts, key: &str) {
    *counts.entry(key.to_owned()).or_default() += 1;
}

fn assert_episode_shapes(episodes: &BTreeMap<&str, Vec<&Operation>>, fixture: &Fixture) {
    let mut scenario_counts = Counts::new();
    let mut profile_counts = Counts::new();
    let mut normal_profiles = BTreeSet::new();
    let mut normal_channels = BTreeSet::new();
    for (customer, profile) in &fixture.profiles {
        demand(customer);
        count(&mut profile_counts, profile);
    }
    assert_eq!(
        json!(profile_counts),
        json!({"working":10, "student":10, "sole_trader":10, "older":10})
    );
    for ops in episodes.values() {
        let kind = ops[0].episode_kind.as_str();
        assert!(
            ops.iter()
                .all(|op| op.episode_kind == kind && op.window == ops[0].window)
        );
        if kind.starts_with("normal_") {
            for op in ops.iter().filter(|op| op.state == "succeeded") {
                normal_profiles.insert(fixture.profiles[&op.actor].as_str());
                normal_channels.insert(op.operation.as_str());
            }
            continue;
        }
        count(&mut scenario_counts, kind);
        let expected: &[(&str, usize)] = match kind {
            "A" => &[
                ("bad_login", 3),
                ("reset_request", 1),
                ("sms", 1),
                ("reset_confirm", 1),
                ("login", 2),
                ("payee", 1),
                ("transfer", 1),
            ],
            "BC" => &[
                ("login", 2),
                ("term_open", 1),
                ("term_identity", 1),
                ("term_close", 1),
                ("payee", 1),
                ("transfer", 1),
                ("atm_start", 2),
                ("atm_withdraw", 2),
                ("inbound", 2),
            ],
            "D" => &[("atm_start", 12), ("atm_deposit", 12)],
            "E" => &[("login", 2), ("transfer", 5)],
            "F" | "travel" | "new_phone" => &[("login", 2)],
            "G" => &[
                ("login", 1),
                ("atm_start", 1),
                ("atm_withdraw", 1),
                ("card", 3),
            ],
            "trip" => &[("login", 2), ("card", 1)],
            "online" => &[("card", 1)],
            "purchase" => &[("login", 2), ("transfer", 1)],
            "household" => &[("login", 2)],
            _ => panic!("unexpected scenario {kind}"),
        };
        let mut actual = BTreeMap::new();
        for op in ops.iter().filter(|op| !op.name.contains("__")) {
            *actual.entry(op.operation.as_str()).or_default() += 1;
        }
        assert_eq!(
            actual,
            expected.iter().copied().collect(),
            "{}",
            ops[0].episode_id
        );
        assert!(ops.iter().all(|op| op.carry_in == (kind == "D")));
        if kind == "E" {
            assert_eq!(named(ops, "baseline_transfer").args["amount"], 30_000);
        }
        if kind == "F" {
            let home = named(ops, "home_baseline");
            let unusual = named(ops, "login");
            assert_eq!(home.actor, unusual.actor);
            assert_eq!(home.context["device"], unusual.context["device"]);
            assert_eq!(home.context["region"], unusual.context["region"]);
            assert_eq!(home.business_time.time().hour(), 10);
            assert_eq!(unusual.business_time.time().hour(), 3);
            assert_eq!(
                unusual.business_time - home.business_time,
                TimeDelta::hours(17)
            );
        }
        if !matches!(kind, "BC" | "D") {
            assert_eq!(ops[0].window, "annual");
        }
    }
    assert_eq!(
        json!(scenario_counts),
        json!({
            "A":1, "BC":4, "D":2, "E":1, "F":1, "G":1,
            "travel":1, "trip":1, "online":1, "new_phone":1, "purchase":1, "household":1
        })
    );
    assert_eq!(
        normal_profiles,
        BTreeSet::from(["older", "sole_trader", "student", "working"])
    );
    for channel in ["card", "atm_withdraw", "inbound"] {
        assert!(
            normal_channels.contains(channel),
            "missing normal {channel}"
        );
    }
}

fn assert_d(ops: &[&Operation], postings: &BTreeMap<String, Vec<Value>>, cutover: &Cutover) {
    let cutover_date = cutover.planned.date_naive();
    let expected_days: BTreeSet<_> = (0..=2)
        .map(|delta| cutover_date - TimeDelta::days(delta))
        .collect();
    let mut subjects: BTreeMap<&str, BTreeMap<NaiveDate, usize>> = BTreeMap::new();
    for op in ops.iter().filter(|op| op.operation == "atm_deposit") {
        let row = &postings[&op.request_id][0];
        let at = time(&row["at"]);
        let recorded = time(&row["recorded_at"]);
        let day = at.date_naive();
        assert_eq!(op.prior_to_live, day != cutover_date);
        if day == cutover_date {
            assert!(
                at >= cutover.observed && at < cutover.end,
                "D settlement was outside LIVE"
            );
            assert!(
                recorded >= cutover.committed,
                "D settlement posted before switch"
            );
        } else {
            assert!(
                at < cutover.planned && recorded < cutover.committed,
                "D preparation posted after switch"
            );
        }
        assert_eq!(row["leg"], 4_800_000);
        *subjects
            .entry(&op.actor)
            .or_default()
            .entry(day)
            .or_default() += 1;
    }
    assert_eq!(subjects.len(), 2, "two cash depositors per D instance");
    for dates in subjects.values() {
        assert_eq!(
            dates.keys().copied().collect::<BTreeSet<_>>(),
            expected_days
        );
        assert!(
            dates.values().all(|deposits| *deposits == 2),
            "two deposits per subject per date"
        );
    }
}

fn named<'a>(ops: &[&'a Operation], name: &str) -> &'a Operation {
    ops.iter()
        .copied()
        .find(|op| op.name == name)
        .unwrap_or_else(|| panic!("missing fixture step {name}"))
}

async fn assert_bc(
    db: &Db,
    ops: &[&Operation],
    postings: &BTreeMap<String, Vec<Value>>,
    cutover: &Cutover,
) {
    let open = named(ops, "victim_1_open");
    let identity = named(ops, "victim_1_identity");
    let close = named(ops, "victim_1_close");
    let transfer = named(ops, "victim_1_transfer");
    let settlement = named(ops, "mule_settlement");
    let withdrawal = named(ops, "mule_withdraw");
    assert_eq!(open.actor, identity.actor);
    assert_eq!(open.actor, close.actor);
    assert_eq!(open.actor, transfer.actor);
    assert_eq!(transfer.peer.as_ref(), Some(&withdrawal.actor));
    assert_eq!(settlement.actor, withdrawal.actor);
    assert_eq!(settlement.args["sender_kind"], "settlement");
    assert_eq!(named(ops, "mule_inbound").args["sender_kind"], "unrelated");
    assert!(
        open.business_time < identity.business_time && identity.business_time < close.business_time
    );
    assert_eq!(identity.result["deposit_id"], open.result["deposit_id"]);
    let payer = demand(&open.actor);
    let term = postings[&open.request_id]
        .iter()
        .find(|row| row["account"] != payer)
        .expect("opening term leg");
    assert_eq!(identity.result["term_account_no"], term["account"]);
    assert_eq!(identity.result["linked_account_no"], payer);
    assert!(
        postings[&close.request_id]
            .iter()
            .any(|row| row["account"] == term["account"])
    );
    assert_eq!(postings[&transfer.request_id][0]["amount"], 300_000);
    assert_eq!(postings[&settlement.request_id][0]["amount"], 450_000);
    assert_eq!(postings[&withdrawal.request_id][0]["amount"], 900_000);
    let mule = demand(&withdrawal.actor);
    let (opened, dormant, prior_legs): (String, String, i64) = db
        .run(move |s| s.query_row(&DORMANCY, &[&mule]))
        .await
        .expect("dormant C demand account in Oracle");
    let opened = opened.parse::<NaiveDate>().expect("opening date");
    let dormant = dormant.parse::<NaiveDate>().expect("dormancy date");
    assert!(dormant - opened >= TimeDelta::days(364));
    assert_eq!(prior_legs, 0);
    assert!(dormant < time(&postings[&transfer.request_id][0]["at"]).date_naive());
    for op in ops {
        if let Some(rows) = postings.get(&op.request_id) {
            let at = time(&rows[0]["at"]);
            let recorded = time(&rows[0]["recorded_at"]);
            if op.window == "live" {
                assert!(
                    at >= cutover.observed && at < cutover.end && recorded >= cutover.committed
                );
            } else {
                assert_eq!(op.window, "annual");
                assert!(at < cutover.planned && recorded < cutover.committed);
            }
        }
    }
}

async fn assert_credit(
    db: &Db,
    truth: &Database,
    fixture: &Fixture,
    episodes: &BTreeMap<&str, Vec<&Operation>>,
    postings: &BTreeMap<String, Vec<Value>>,
    cutover: &Cutover,
) {
    let mut expected = BTreeSet::new();
    let mut mules = BTreeSet::new();
    let mut victims = BTreeSet::new();
    for (&id, ops) in episodes {
        let window = &ops[0].window;
        let kind = ops[0].episode_kind.as_str();
        match kind {
            "BC" => {
                assert_bc(db, ops, postings, cutover).await;
                let transfer = named(ops, "victim_1_transfer");
                let mule = demand(transfer.peer.as_deref().expect("mule"));
                assert!(
                    mules.insert(mule.clone()),
                    "each C has a separate dormant account"
                );
                assert!(
                    victims.insert(&transfer.actor),
                    "each B is a distinct victim"
                );
                expected.insert(json!([window, "B", transfer.actor, id]).to_string());
                expected.insert(json!([window, "C", mule, id]).to_string());
            }
            "D" => {
                assert_eq!(window, "live");
                assert_d(ops, postings, cutover);
                expected.insert(json!([window, "D", id, id]).to_string());
            }
            kind if kind.starts_with("normal_") => {}
            "A" | "E" | "F" | "G" => {
                expected.insert(json!([window, kind, id, id]).to_string());
            }
            _ => {
                expected.insert(json!([window, "lookalike", id, id]).to_string());
            }
        }
    }
    let mut actual = BTreeSet::new();
    let mut totals = BTreeMap::<String, Counts>::new();
    for row in truth_rows(truth, "stage6_credit", &fixture.run_id).await {
        let entry = json!([
            row["window"],
            row["kind"],
            row["identity"],
            row["episode_id"]
        ]);
        assert!(actual.insert(entry.to_string()), "duplicate quota credit");
        let window = row["window"].as_str().expect("credit window");
        let kind = row["kind"].as_str().expect("credited scenario");
        count(totals.entry(window.to_owned()).or_default(), kind);
    }
    assert_eq!(mules.len(), 4);
    assert_eq!(victims.len(), 4);
    assert_eq!(actual.len(), 20);
    assert_eq!(
        json!(totals),
        expected_credit(),
        "actual distinct credit per window"
    );
    assert_eq!(
        actual, expected,
        "credits independently reconciled against bank operations"
    );
}

async fn assert_operations(
    db: &Db,
    truth: &Database,
    events: &Collection<Document>,
    fixture: &Fixture,
    manifest: &Value,
    cutover: &Cutover,
) {
    let ops: Vec<Operation> = truth_rows(truth, "stage6_operations", &fixture.run_id)
        .await
        .into_iter()
        .map(|row| serde_json::from_value(row).expect("operation journal"))
        .collect();
    let ids: BTreeSet<_> = ops.iter().map(|op| op.request_id.as_str()).collect();
    assert_eq!(ids.len(), ops.len(), "distinct journal request IDs");
    assert_eq!(
        ids.len(),
        fixture.dispatch_count,
        "transport and journal dispatch count"
    );
    let mut by_request = BTreeMap::<String, Vec<Value>>::new();
    let mut seen_events = BTreeSet::new();
    for row in documents(
        events,
        doc! {"request_id": {"$in": ids.into_iter().collect::<Vec<_>>()}},
    )
    .await
    {
        assert!(seen_events.insert(row["event_id"].as_str().expect("event ID").to_owned()));
        by_request
            .entry(
                row["request_id"]
                    .as_str()
                    .expect("event request ID")
                    .to_owned(),
            )
            .or_default()
            .push(row);
    }
    let mut labels_by_request = BTreeMap::<String, Vec<Value>>::new();
    for row in truth_rows(truth, "labels", &fixture.run_id).await {
        labels_by_request
            .entry(
                row["request_id"]
                    .as_str()
                    .expect("label request ID")
                    .to_owned(),
            )
            .or_default()
            .push(row);
    }
    let access = access_logs(fixture, cutover);
    let mut states = Counts::new();
    let mut breakdown = BTreeMap::<&str, BTreeMap<&str, BTreeMap<&str, usize>>>::new();
    let mut episodes = BTreeMap::<&str, Vec<&Operation>>::new();
    let mut postings = BTreeMap::new();
    for op in &ops {
        let id = &op.request_id;
        count(&mut states, &op.state);
        assert_eq!(
            op.persona, fixture.profiles[&op.actor],
            "{id}: actor persona"
        );
        let counts = breakdown
            .entry(primary_label(op))
            .or_default()
            .entry(&op.persona)
            .or_insert_with(|| {
                BTreeMap::from([
                    ("prepared", 0),
                    ("attempted", 0),
                    ("completed", 0),
                    ("rejected", 0),
                    ("unresolved", 0),
                    ("carried_in", 0),
                ])
            });
        *counts.get_mut("attempted").expect("count attempted") += 1;
        *counts
            .get_mut(if op.state == "succeeded" {
                "completed"
            } else {
                "rejected"
            })
            .expect("count outcome") += 1;
        *counts.get_mut("carried_in").expect("count carry-in") += usize::from(op.prior_to_live);
        episodes.entry(&op.episode_id).or_default().push(op);
        let http = expected_http(op);
        let logs = by_request.remove(id).unwrap_or_default();
        let access = access
            .get(id)
            .unwrap_or_else(|| panic!("{id}: missing bank access log"));
        let event = assert_completion(op, http, access, &logs);
        let mut term = None;
        if money_type(op).is_some() {
            let rows = posting(db, op, event.expect("money completion event"), http).await;
            if http == 200 {
                term = assert_money(op, &rows, &logs);
                postings.insert(id.clone(), rows);
            }
        }
        let mut actual = Labels::new();
        for row in labels_by_request.remove(id).unwrap_or_default() {
            assert_eq!(row["scenario"], op.episode_kind, "{id}");
            let [category, subject, label] = ["subject_type", "subject_id", "label"].map(|key| {
                row[key]
                    .as_str()
                    .unwrap_or_else(|| panic!("{id}: label missing {key}"))
            });
            let suffix = if label == "B" {
                format!(":{0}", op.actor)
            } else {
                String::new()
            };
            assert_eq!(
                row["scenario_instance_id"],
                format!("{}:{label}{suffix}", op.episode_id),
                "{id}"
            );
            assert!(
                actual.insert((category.into(), subject.into(), label.into())),
                "{id}: duplicate label"
            );
        }
        assert_labels(op, term.as_deref(), actual);
    }
    assert!(
        labels_by_request.is_empty(),
        "truth labels without corresponding operations"
    );
    assert_eq!(manifest["summary"]["states"], json!(states));
    assert_eq!(
        manifest["summary"]["operations_by_class_and_persona"],
        json!(breakdown)
    );
    let d_schedule = manifest["summary"]["d_carry_in_schedule"]
        .as_object()
        .expect("D schedule");
    let live_day = cutover.planned.date_naive();
    assert_eq!(d_schedule.len(), 2);
    for (id, schedule) in d_schedule {
        assert_eq!(episodes[id.as_str()][0].episode_kind, "D");
        let instants = schedule.as_array().expect("D carry-in instants");
        assert_eq!(instants.len(), 3);
        for (i, at) in instants.iter().enumerate() {
            assert_eq!(
                time(at).date_naive(),
                live_day - TimeDelta::days(2 - i as i64)
            );
        }
    }
    assert_episode_shapes(&episodes, fixture);
    assert_credit(db, truth, fixture, &episodes, &postings, cutover).await;
}

#[tokio::test]
#[ignore = "requires the Stage 6 lifecycle fixture, Oracle, MongoDB and read-only bank application logs"]
async fn generator_lifecycle_proof() {
    let fixture: Fixture = serde_json::from_str(
        &env::var("GENERATOR_LIFECYCLE_FIXTURE").expect("GENERATOR_LIFECYCLE_FIXTURE"),
    )
    .expect("lifecycle fixture JSON");
    let db = Db::connect(&DbConfig::from_env().expect("Oracle config")).expect("connect to Oracle");
    let truth_client =
        mongodb::Client::with_uri_str(env::var("MONGO_TRUTH_URI").expect("MONGO_TRUTH_URI"))
            .await
            .expect("connect to truth MongoDB");
    let truth = truth_client
        .default_database()
        .unwrap_or_else(|| truth_client.database("truth"));
    let manifest = documents(&truth.collection("runs"), doc! {"_id": &fixture.run_id})
        .await
        .pop()
        .expect("lifecycle manifest");
    let cutover = assert_manifest(&db, &manifest, &fixture).await;
    let events = outbox_relay::events_collection(&env::var("MONGO_URI").expect("MONGO_URI"))
        .await
        .expect("connect to bank_logs");
    assert_operations(&db, &truth, &events, &fixture, &manifest, &cutover).await;
}
