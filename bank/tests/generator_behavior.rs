//! Read-only Oracle and MongoDB proof of the two-day behavior fixture.

use std::{
    collections::{BTreeMap, HashSet},
    env,
    time::Duration,
};

use bank::{
    shared::db::{Db, DbConfig, SqlStmt},
    sql_stmt,
};
use chrono::{DateTime, Days, FixedOffset, NaiveDate, TimeZone, Timelike};
use mongodb::{
    Client, Collection,
    bson::{DateTime as BsonDateTime, Document, doc},
};
use serde::Deserialize;
use serde_json::{Value, json};

const LOGIN: SqlStmt = sql_stmt!(
    "test.generator_behavior_login",
    "sql/generator_behavior_login.sql"
);
const TRANSFER: SqlStmt = sql_stmt!(
    "test.generator_behavior_transfer",
    "sql/generator_behavior_transfer.sql"
);
const LEGS: SqlStmt = sql_stmt!(
    "test.generator_behavior_legs",
    "sql/generator_behavior_legs.sql"
);
const ACCOUNT: SqlStmt = sql_stmt!(
    "test.generator_behavior_account",
    "sql/generator_behavior_account.sql"
);
const DEVICES: SqlStmt = sql_stmt!(
    "test.generator_behavior_devices",
    "sql/generator_behavior_devices.sql"
);
const ACTIVITY: SqlStmt = sql_stmt!(
    "test.generator_behavior_activity",
    "sql/generator_behavior_activity.sql"
);
const COUNT_OUTBOX: SqlStmt = sql_stmt!("test.count_outbox", "sql/count_outbox.sql");

const ROLES: [&str; 8] = [
    "e", "f", "travel", "phone", "buyer", "house1", "house2", "receiver",
];
const OPENING: i64 = 1_000_000;
const FINAL_BALANCES: [i64; 8] = [
    640_000, OPENING, OPENING, OPENING, 760_000, OPENING, OPENING, 1_600_000,
];
const HOME_REGION: &str = "KR-11";
const NEW_PHONE: &str = "device-114";

type At = DateTime<FixedOffset>;
type OracleRow = (String, String, String);
type Activity = (&'static str, String, At);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    run_id: String,
    date: String,
    requests: BTreeMap<String, String>,
    recovery_logins: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    E,
    F,
    Travel,
    Phone,
    Buyer,
    House1,
    House2,
    Receiver,
}

impl Role {
    fn index(self) -> usize {
        self as usize
    }
}

struct Subject {
    customer: String,
    account: String,
    home_device: String,
    home_ip: String,
}

fn subject(index: usize) -> Subject {
    Subject {
        customer: format!("C{:08}", 97_200_000 + index),
        account: format!("{:011}", 99_897_200_000_u64 + index as u64),
        home_device: format!("device-{}", 101 + index),
        home_ip: match ROLES[index] {
            "house1" | "house2" => "192.0.2.55".into(),
            _ => format!("192.0.2.{}", 11 + index),
        },
    }
}

#[derive(Clone, Copy)]
enum Motif {
    Background,
    Velocity,
    LateHour,
    Travel,
    NewPhone,
    Purchase,
    Household,
}

#[derive(Clone, Copy)]
enum Operation {
    Login { new_device: bool },
    Transfer { amount: i64, login: &'static str },
}

struct Action {
    name: &'static str,
    role: Role,
    at: At,
    operation: Operation,
    motif: Motif,
}

impl Action {
    fn context<'a>(&self, subject: &'a Subject) -> (&'a str, &'a str, &'static str) {
        match self.motif {
            Motif::Travel => (&subject.home_device, "198.51.100.17", "US"),
            Motif::NewPhone => (NEW_PHONE, &subject.home_ip, HOME_REGION),
            _ => (&subject.home_device, &subject.home_ip, HOME_REGION),
        }
    }
}

fn kst(date: NaiveDate, hour: u32, min: u32, sec: u32) -> At {
    FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .from_local_datetime(&date.and_hms_opt(hour, min, sec).unwrap())
        .single()
        .unwrap()
}

fn plan(day1: NaiveDate, day2: NaiveDate) -> [Action; 18] {
    use Motif::{Background, Household, LateHour, NewPhone, Purchase, Velocity};
    use Role::{Buyer, E, F, House1, House2, Phone, Travel};

    let login = |name, role, at, new_device, motif| Action {
        name,
        role,
        at,
        operation: Operation::Login { new_device },
        motif,
    };
    let transfer = |name, role, at, amount, login, motif| Action {
        name,
        role,
        at,
        operation: Operation::Transfer { amount, login },
        motif,
    };
    let d1 = |hour, min| kst(day1, hour, min, 0);
    let d2 = |hour, min| kst(day2, hour, min, 0);
    let burst =
        |name, sec, amount| transfer(name, E, kst(day2, 10, 0, sec), amount, "e_setup", Velocity);
    [
        login("e_baseline_login", E, d1(9, 59), true, Background),
        transfer(
            "e_baseline_transfer",
            E,
            d1(10, 0),
            30_000,
            "e_baseline_login",
            Background,
        ),
        login("f_baseline", F, d1(10, 5), true, Background),
        login("travel_baseline", Travel, d1(10, 6), true, Background),
        login("phone_baseline", Phone, d1(10, 7), true, Background),
        login("buyer_baseline", Buyer, d1(10, 8), true, Background),
        login("f_anomaly", F, d2(3, 0), false, LateHour),
        login("e_setup", E, d2(9, 59), false, Background),
        burst("e_1", 0, 30_000),
        burst("e_2", 15, 30_000),
        burst("e_3", 30, 30_000),
        burst("e_4", 45, 240_000),
        login("buyer_setup", Buyer, d2(10, 9), false, Background),
        transfer(
            "purchase",
            Buyer,
            d2(10, 10),
            240_000,
            "buyer_setup",
            Purchase,
        ),
        login("travel_login", Travel, d2(11, 0), false, Motif::Travel),
        login("phone_login", Phone, d2(12, 0), true, NewPhone),
        login("household_1", House1, d2(13, 0), true, Household),
        login("household_2", House2, d2(13, 1), true, Household),
    ]
}

fn required_env(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is required"))
}

fn fields(actual: &Value, expected: Value, context: &str) {
    for (field, value) in expected.as_object().unwrap() {
        assert!(actual.get(field) == Some(value), "{context}: wrong {field}");
    }
}

fn timestamp(value: &str, context: &str) -> At {
    DateTime::parse_from_rfc3339(value).unwrap_or_else(|_| panic!("{context}: invalid timestamp"))
}

fn assert_time(row: &Value, field: &str, at: At, context: &str) {
    let actual = row[field]
        .as_str()
        .unwrap_or_else(|| panic!("{context}: missing {field}"));
    assert_eq!(timestamp(actual, context), at, "{context}: wrong {field}");
}

fn decode(row: &str, context: &str) -> Value {
    serde_json::from_str(row).unwrap_or_else(|_| panic!("{context}: invalid Oracle row"))
}

async fn oracle_row(db: &Db, stmt: SqlStmt, key: &str, context: &str) -> Value {
    let key = key.to_owned();
    let rows: Vec<String> = db
        .run(move |s| s.query_all(&stmt, &[&key]))
        .await
        .unwrap_or_else(|_| panic!("{context}: Oracle query failed"));
    assert_eq!(
        rows.len(),
        1,
        "{context}: Oracle row count for {}",
        stmt.name
    );
    decode(&rows[0], context)
}

async fn assert_login(db: &Db, id: &str, action: &Action, subject: &Subject) -> String {
    let row = oracle_row(db, LOGIN, id, action.name).await;
    let (device, ip, region) = action.context(subject);
    fields(
        &row,
        json!({
            "customer_no": subject.customer, "outcome": "SUCCESS",
            "device_id": device, "ip": ip, "region": region,
            "session_device": device, "session_ip": ip, "session_region": region,
            "session_listener": "GENERATOR", "session_human": "N",
        }),
        action.name,
    );
    assert_time(&row, "occurred_at", action.at, action.name);
    row["session_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| panic!("{}: missing Oracle session", action.name))
        .to_owned()
}

struct TransferProof<'a> {
    session: &'a str,
    sender: &'a Subject,
    receiver: &'a Subject,
    amount: i64,
    balances: [i64; 2],
}

async fn assert_transfer(db: &Db, id: &str, action: &Action, proof: TransferProof<'_>) -> i64 {
    let row = oracle_row(db, TRANSFER, id, action.name).await;
    let (device, ip, region) = action.context(proof.sender);
    fields(
        &row,
        json!({
            "request_id": id, "request_type": "TRANSFER", "status": "DONE", "channel": "MOBILE",
            "listener": "GENERATOR", "rsp_code": "A0000", "bank_rsp_code": "000",
            "session_id": proof.session, "device_id": device, "ip": ip, "region": region,
            "kind": "transfer", "amount": proof.amount,
        }),
        action.name,
    );
    for field in ["occurred_at", "transfer_at"] {
        assert_time(&row, field, action.at, action.name);
    }
    let transfer_id = row["transfer_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("{}: missing Oracle transfer", action.name));

    let request = id.to_owned();
    let legs: Vec<String> = db
        .run(move |s| s.query_all(&LEGS, &[&request, &transfer_id]))
        .await
        .unwrap_or_else(|_| panic!("{}: Oracle leg query failed", action.name));
    assert_eq!(legs.len(), 2, "{}: ledger leg count", action.name);
    let expected = [
        (
            &proof.sender.account,
            &proof.receiver.account,
            -proof.amount,
            "출금",
        ),
        (
            &proof.receiver.account,
            &proof.sender.account,
            proof.amount,
            "입금",
        ),
    ];
    for ((raw, (account, counterparty, signed, direction)), balance) in
        legs.iter().zip(expected).zip(proof.balances)
    {
        let leg = decode(raw, action.name);
        fields(
            &leg,
            json!({
                "account_no": account, "request_id": id, "transfer_id": transfer_id,
                "amount": signed, "balance_after": balance, "channel": "MOBILE",
                "inout_type": direction, "counterparty_account_no": counterparty,
                "counterparty_bank_code": "998", "business_date": action.at.date_naive().to_string(),
            }),
            action.name,
        );
        assert_time(&leg, "occurred_at", action.at, action.name);
    }
    transfer_id
}

struct Verified {
    session: String,
    transfer_id: Option<i64>,
}

type Window = (At, At);

async fn assert_subject_activity(
    db: &Db,
    subject: &Subject,
    role: usize,
    actions: &[Action],
    fixture: &Fixture,
    verified: &BTreeMap<&str, Verified>,
    (start, end): Window,
) {
    let context = ROLES[role];
    let account = oracle_row(db, ACCOUNT, &subject.account, context).await;
    fields(
        &account,
        json!({
            "customer_no": subject.customer, "balance": FINAL_BALANCES[role],
            "opening_channel": "SYSTEM", "opening_kind": "inbound",
            "opening_amount": OPENING, "opening_balance": OPENING,
            "opening_request_id": null, "opening_transfer_request_id": null,
            "opening_transfer_amount": OPENING,
        }),
        context,
    );
    let opening = account["opening_leg_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("{context}: missing Oracle opening leg"));

    let mut expected: Vec<Activity> = Vec::new();
    let mut devices = BTreeMap::<&str, (At, At)>::new();
    for action in actions {
        let id = &fixture.requests[action.name];
        let owner = action.role.index() == role;
        match action.operation {
            Operation::Login { .. } if owner => {
                expected.push(("login", id.clone(), action.at));
                expected.push(("session", verified[action.name].session.clone(), action.at));
                let device = action.context(subject).0;
                devices
                    .entry(device)
                    .and_modify(|last| last.1 = action.at)
                    .or_insert((action.at, action.at));
            }
            Operation::Transfer { .. } if owner => {
                expected.push(("request", id.clone(), action.at));
                expected.push(("leg", id.clone(), action.at));
            }
            Operation::Transfer { .. } if role == Role::Receiver.index() => {
                expected.push(("leg", id.clone(), action.at));
            }
            _ => {}
        }
    }

    let customer = subject.customer.clone();
    let account_no = subject.account.clone();
    let home_device = subject.home_device.clone();
    let other_device = if role == Role::Phone.index() {
        NEW_PHONE
    } else {
        ""
    };
    let (rows, device_rows): (Vec<OracleRow>, Vec<OracleRow>) = db
        .run(move |s| {
            let activity = s.query_all(
                &ACTIVITY,
                &[
                    &opening,
                    &home_device,
                    &other_device,
                    &start,
                    &end,
                    &customer,
                    &account_no,
                ],
            )?;
            let devices = s.query_all(&DEVICES, &[&customer])?;
            Ok((activity, devices))
        })
        .await
        .unwrap_or_else(|_| panic!("{context}: Oracle activity query failed"));
    let mut actual: Vec<_> = rows
        .iter()
        .map(|(kind, id, at)| (kind.as_str(), id.clone(), timestamp(at, context)))
        .collect();
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(
        actual.len(),
        expected.len(),
        "{context}: Oracle interval activity count"
    );
    for (row, expected) in actual.iter().zip(&expected) {
        let (kind, _, at) = expected;
        assert!(
            row == expected,
            "{context}: wrong Oracle activity for {kind} at {at}"
        );
    }
    assert_eq!(
        device_rows.len(),
        devices.len(),
        "{context}: Oracle device count"
    );
    for (device, first, last) in device_rows {
        let (first_expected, last_expected) = devices
            .remove(device.as_str())
            .unwrap_or_else(|| panic!("{context}: unexpected Oracle device"));
        assert_eq!(
            timestamp(&first, context),
            first_expected,
            "{context}: device first seen"
        );
        assert_eq!(
            timestamp(&last, context),
            last_expected,
            "{context}: device last seen"
        );
    }
}

async fn collection(uri: &str, database: &str, name: &str) -> Collection<Document> {
    let client = Client::with_uri_str(uri)
        .await
        .unwrap_or_else(|_| panic!("{name}: invalid Mongo connection settings"));
    client
        .default_database()
        .unwrap_or_else(|| client.database(database))
        .collection(name)
}

async fn wait_for_outbox(db: &Db) {
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let pending: i64 = db
                .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
                .await
                .unwrap_or_else(|_| panic!("Oracle outbox count query failed"));
            if pending == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "Oracle outbox did not drain within 120 seconds"
    );
}

fn label(id: &str, role: Role, subject: &str, kind: &str, motif: Motif, run: &str) -> [String; 6] {
    let (code, scenario, instance) = match motif {
        Motif::Background => (
            "normal",
            "background",
            format!("{run}:normal:{}", ROLES[role.index()]),
        ),
        Motif::Velocity => ("E", "velocity", format!("{run}:velocity")),
        Motif::LateHour => ("F", "late_hour", format!("{run}:late_hour")),
        Motif::Travel => ("lookalike", "travel", format!("{run}:travel")),
        Motif::NewPhone => ("lookalike", "new_phone", format!("{run}:new_phone")),
        Motif::Purchase => ("lookalike", "purchase", format!("{run}:purchase")),
        Motif::Household => ("lookalike", "household", format!("{run}:household")),
    };
    [
        id.into(),
        kind.into(),
        subject.into(),
        code.into(),
        scenario.into(),
        instance,
    ]
}

async fn assert_labels(
    labels: &Collection<Document>,
    fixture: &Fixture,
    subjects: &[Subject; 8],
    actions: &[Action],
) {
    let mut expected = Vec::new();
    for action in actions {
        let id = &fixture.requests[action.name];
        let subject = &subjects[action.role.index()];
        let run = &fixture.run_id;
        match action.operation {
            Operation::Login { .. } => {
                expected.push(label(
                    id,
                    action.role,
                    &subject.customer,
                    "customer",
                    action.motif,
                    run,
                ));
            }
            Operation::Transfer { .. } => {
                expected.push(label(
                    id,
                    action.role,
                    &subject.account,
                    "account",
                    action.motif,
                    run,
                ));
                let receiver = Role::Receiver;
                expected.push(label(
                    id,
                    receiver,
                    &subjects[receiver.index()].account,
                    "account",
                    Motif::Background,
                    run,
                ));
            }
        }
    }
    assert_eq!(expected.len(), 24, "expected fixture label count");
    let mut cursor = labels
        .find(doc! {"$or": [
            {"run_id": &fixture.run_id},
            {"request_id": {"$in": fixture.requests.values().cloned().collect::<Vec<_>>()}},
        ]})
        .await
        .unwrap_or_else(|_| panic!("find fixture truth labels"));
    let mut actual = Vec::new();
    while cursor
        .advance()
        .await
        .unwrap_or_else(|_| panic!("read fixture truth labels"))
    {
        let row = cursor
            .deserialize_current()
            .unwrap_or_else(|_| panic!("decode fixture truth label"));
        assert!(
            row.get_str("run_id").ok() == Some(fixture.run_id.as_str()),
            "truth label belongs to a different run"
        );
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
                row.get_str(field)
                    .unwrap_or_else(|_| panic!("invalid truth label field {field}"))
                    .to_owned()
            }),
        );
    }
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual.len(), expected.len(), "fixture truth label count");
    assert!(actual == expected, "wrong fixture truth label tuples");
}

async fn assert_events(
    events: &Collection<Document>,
    fixture: &Fixture,
    subjects: &[Subject; 8],
    actions: &[Action],
    verified: &BTreeMap<&str, Verified>,
    (start, end): Window,
) {
    let mut devices = HashSet::new();
    let mut ips = HashSet::new();
    for action in actions {
        let (device, ip, _) = action.context(&subjects[action.role.index()]);
        devices.insert(device);
        ips.insert(ip);
    }
    let mut cursor = events.find(doc! {"$or": [
        {"request_id": {"$in": fixture.requests.values().cloned().collect::<Vec<_>>()}},
        {
            "$or": [
                {"customer_no": {"$in": subjects.iter().map(|s| s.customer.as_str()).collect::<Vec<_>>()}},
                {"account_no": {"$in": subjects.iter().map(|s| s.account.as_str()).collect::<Vec<_>>()}},
                {"session_id": {"$in": verified.values().map(|proof| proof.session.as_str()).collect::<Vec<_>>()}},
                {"device_id": {"$in": devices.into_iter().collect::<Vec<_>>()}},
                {"ip": {"$in": ips.into_iter().collect::<Vec<_>>()}},
            ],
            "$expr": {"$let": {
                "vars": {"at": {"$convert": {"input": "$occurred_at", "to": "date", "onError": null, "onNull": null}}},
                "in": {"$and": [
                    {"$gte": ["$$at", BsonDateTime::from_millis(start.timestamp_millis())]},
                    {"$lt": ["$$at", BsonDateTime::from_millis(end.timestamp_millis())]},
                ]},
            }},
        },
    ]}).await.unwrap_or_else(|_| panic!("find fixture bank_logs events"));
    let mut by_request = BTreeMap::<String, Vec<Value>>::new();
    let mut event_ids = HashSet::new();
    let mut expired_sessions = HashSet::new();
    let subject_customers: HashSet<&str> = subjects.iter().map(|s| s.customer.as_str()).collect();
    while cursor
        .advance()
        .await
        .unwrap_or_else(|_| panic!("read fixture bank_logs events"))
    {
        let row = cursor
            .deserialize_current()
            .unwrap_or_else(|_| panic!("decode fixture bank_logs event"));
        let event_id = row
            .get_str("event_id")
            .unwrap_or_else(|_| panic!("missing bank_logs event ID"));
        assert!(
            event_ids.insert(event_id.to_owned()),
            "duplicate bank_logs event ID"
        );
        let value =
            serde_json::to_value(&row).unwrap_or_else(|_| panic!("invalid bank_logs event"));
        let Ok(id) = row.get_str("request_id") else {
            assert_eq!(
                value["event_type"], "session.expired",
                "unexpected requestless bank_logs activity in fixture interval"
            );
            assert_eq!(value["channel"], "SYSTEM");
            assert_eq!(value["outcome"], "completed");
            assert!(
                value["customer_no"]
                    .as_str()
                    .is_some_and(|c| subject_customers.contains(c)),
                "unexpected customer on rollover session.expired"
            );
            assert_eq!(value["payload"]["end_reason"], "expired");
            let session_id = value["session_id"]
                .as_str()
                .unwrap_or_else(|| panic!("missing session_id on rollover session.expired"));
            assert_eq!(value["payload"]["session_id"], session_id);
            assert!(
                expired_sessions.insert(session_id.to_owned()),
                "duplicate rollover session.expired for session {session_id}"
            );
            continue;
        };
        by_request.entry(id.to_owned()).or_default().push(value);
    }

    let receiver = &subjects[Role::Receiver.index()].account;
    for action in actions {
        let id = &fixture.requests[action.name];
        let subject = &subjects[action.role.index()];
        let proof = &verified[action.name];
        let logs = by_request.remove(id).unwrap_or_default();
        assert_request_events(&logs, id, action, subject, receiver, proof);
    }
    let extras: usize = by_request.values().map(Vec::len).sum();
    assert_eq!(
        extras, 0,
        "unexpected bank_logs activity in fixture interval"
    );
}

fn assert_request_events(
    logs: &[Value],
    id: &str,
    action: &Action,
    subject: &Subject,
    receiver: &str,
    proof: &Verified,
) {
    let context = action.name;
    let (device, ip, region) = action.context(subject);
    let (expected, account, bank_code): (&[&str], Option<&str>, Option<&str>) = match action
        .operation
    {
        Operation::Login { new_device: true } => (
            &["device.new", "login.succeeded", "request.completed"],
            None,
            None,
        ),
        Operation::Login { new_device: false } => {
            (&["login.succeeded", "request.completed"], None, None)
        }
        Operation::Transfer { .. } => (&["request.completed"], Some(&subject.account), Some("000")),
    };
    assert_eq!(
        logs.len(),
        expected.len(),
        "{context}: bank_logs event count"
    );
    for &kind in expected {
        let mut matching = logs.iter().filter(|row| row["event_type"] == kind);
        let row = matching
            .next()
            .unwrap_or_else(|| panic!("{context}: missing {kind}"));
        assert!(matching.next().is_none(), "{context}: duplicate {kind}");
        assert_time(row, "occurred_at", action.at, context);
        let response = if kind == "device.new" {
            None
        } else {
            Some("A0000")
        };
        fields(
            row,
            json!({
                "schema": "saltation.log/1", "request_id": id, "session_id": proof.session,
                "device_id": device, "ip": ip, "region": region, "customer_no": subject.customer,
                "account_no": account, "channel": "MOBILE", "listener": "generator",
                "human": false, "outcome": "completed", "rsp_code": response, "bank_rsp_code": bank_code,
            }),
            context,
        );
        let payload = &row["payload"];
        match (action.operation, kind) {
            (Operation::Login { .. }, "login.succeeded") => {
                fields(
                    payload,
                    json!({"outcome": "SUCCESS", "failed_count": 0}),
                    context,
                );
            }
            (Operation::Login { .. }, "request.completed") => {
                fields(payload, json!({"http_status": 200}), context);
            }
            (Operation::Transfer { amount, .. }, "request.completed") => {
                fields(
                    payload,
                    json!({
                        "http_status": 200, "request_type": "TRANSFER", "tran_amt": amount,
                        "transfer_id": proof.transfer_id, "counterparty_account_no": receiver,
                        "counterparty_bank_code": "998",
                    }),
                    context,
                );
                assert!(
                    payload.get("replayed").is_none(),
                    "{context}: transfer replay"
                );
            }
            _ => {}
        }
    }
}

#[tokio::test]
#[ignore = "requires the behavior fixture and running Oracle and MongoDB"]
async fn generator_behavior_proof() {
    let fixture: Fixture = serde_json::from_str(&required_env("GENERATOR_BEHAVIOR_FIXTURE"))
        .unwrap_or_else(|_| panic!("invalid GENERATOR_BEHAVIOR_FIXTURE JSON"));
    assert!(!fixture.run_id.trim().is_empty(), "fixture run_id is empty");
    assert_eq!(fixture.recovery_logins, 0, "fixture recovery login count");
    let day1 = NaiveDate::parse_from_str(&fixture.date, "%Y-%m-%d")
        .unwrap_or_else(|_| panic!("invalid fixture date"));
    assert!(
        fixture.date.len() == 10 && fixture.date == day1.to_string(),
        "fixture date must be YYYY-MM-DD"
    );
    let day2 = day1.checked_add_days(Days::new(1)).expect("fixture day 2");
    let day3 = day2
        .checked_add_days(Days::new(1))
        .expect("fixture interval end");
    let window = (kst(day1, 0, 0, 0), kst(day3, 0, 0, 0));
    let actions = plan(day1, day2);
    assert_eq!(
        fixture.requests.len(),
        actions.len(),
        "fixture request name count"
    );
    let mut ids = HashSet::new();
    for action in &actions {
        let id = fixture
            .requests
            .get(action.name)
            .unwrap_or_else(|| panic!("missing fixture request {}", action.name));
        assert!(
            ulid::Ulid::from_string(id).is_ok_and(|uid| uid.to_string() == *id),
            "{}: invalid fixture request ID",
            action.name
        );
        assert!(
            ids.insert(id),
            "{}: duplicate fixture request ID",
            action.name
        );
    }

    let events_uri = required_env("MONGO_URI");
    let truth_uri = required_env("MONGO_TRUTH_URI");
    let config =
        DbConfig::from_env().unwrap_or_else(|_| panic!("invalid Oracle connection settings"));
    let db = Db::connect(&config).unwrap_or_else(|_| panic!("connect to Oracle"));
    let subjects = std::array::from_fn(subject);
    let receiver = Role::Receiver.index();
    let mut balances = [OPENING; 8];
    let mut verified = BTreeMap::<&str, Verified>::new();
    for action in &actions {
        if action.name == "f_baseline" || action.name == "f_anomaly" {
            assert_eq!(
                (8..22).contains(&action.at.hour()),
                action.name == "f_baseline"
            );
        }
        let id = &fixture.requests[action.name];
        let index = action.role.index();
        let sender = &subjects[index];
        let (session, transfer_id) = match action.operation {
            Operation::Login { .. } => (assert_login(&db, id, action, sender).await, None),
            Operation::Transfer { amount, login } => {
                let session = verified[login].session.clone();
                balances[index] -= amount;
                balances[receiver] += amount;
                let proof = TransferProof {
                    session: &session,
                    sender,
                    receiver: &subjects[receiver],
                    amount,
                    balances: [balances[index], balances[receiver]],
                };
                let transfer_id = assert_transfer(&db, id, action, proof).await;
                (session, Some(transfer_id))
            }
        };
        verified.insert(
            action.name,
            Verified {
                session,
                transfer_id,
            },
        );
    }
    assert_eq!(balances, FINAL_BALANCES, "fixture final balance schedule");
    for (role, subject) in subjects.iter().enumerate() {
        assert_subject_activity(&db, subject, role, &actions, &fixture, &verified, window).await;
    }
    let labels = collection(&truth_uri, "truth", "labels").await;
    assert_labels(&labels, &fixture, &subjects, &actions).await;
    wait_for_outbox(&db).await;
    let events = collection(&events_uri, "bank_logs", "events").await;
    assert_events(&events, &fixture, &subjects, &actions, &verified, window).await;
}
