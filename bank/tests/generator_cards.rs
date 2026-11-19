//! Read-only proof of the generator's short card fixture in Oracle and MongoDB.

use std::{collections::HashSet, env, time::Duration};

use bank::{
    shared::db::{Db, DbConfig, SqlStmt},
    sql_stmt,
};
use chrono::{DateTime, FixedOffset, NaiveDate, TimeDelta, TimeZone};
use mongodb::{
    Client, Collection,
    bson::{Bson, Document, doc},
};
use serde::Deserialize;
use serde_json::{Value, json};

const AUTHORIZATION: SqlStmt = sql_stmt!(
    "test.generator_cards_authorization",
    "sql/generator_cards_authorization.sql"
);
const POSTING: SqlStmt = sql_stmt!(
    "test.generator_cards_posting",
    "sql/generator_cards_posting.sql"
);
const LOGIN: SqlStmt = sql_stmt!(
    "test.generator_cards_login",
    "sql/generator_cards_login.sql"
);
const ATM: SqlStmt = sql_stmt!("test.generator_cards_atm", "sql/generator_cards_atm.sql");
const ONLINE_HISTORY: SqlStmt = sql_stmt!(
    "test.generator_cards_online_history",
    "sql/generator_cards_online_history.sql"
);
const BALANCE: SqlStmt = sql_stmt!(
    "test.generator_cards_balance",
    "sql/generator_bc_balance.sql"
);
const COUNT_OUTBOX: SqlStmt =
    sql_stmt!("test.generator_cards_count_outbox", "sql/count_outbox.sql");

type At = DateTime<FixedOffset>;

#[derive(Deserialize)]
struct Fixture {
    run_id: String,
    subjects: Subjects,
    requests: Requests,
    normal_amount: i64,
    date: NaiveDate,
}

#[derive(Deserialize)]
struct Subjects {
    g: Subject,
    trip: Subject,
    online: Subject,
    normal: Subject,
}

#[derive(Deserialize)]
struct Subject {
    account_no: String,
    customer_no: String,
}

#[derive(Deserialize)]
struct Requests {
    g_login: String,
    g_atm_start: String,
    g_atm_withdraw: String,
    g_small: String,
    g_large: String,
    g_decline: String,
    trip_login: String,
    trip_card: String,
    online_card: String,
    normal_card: String,
    network_token: String,
}

fn required_env(name: &str) -> String {
    let value = env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(!value.trim().is_empty(), "{name} is empty");
    value
}

fn kst(date: NaiveDate, hour: u32, minute: u32) -> At {
    FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
        .single()
        .unwrap()
}

fn time(row: &Value, field: &str, context: &str) -> At {
    row[field]
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .unwrap_or_else(|| panic!("{context}: invalid {field}"))
}

// Report the failed field only. Oracle and Mongo rows may contain card numbers.
fn fields(actual: &Value, expected: Value, context: &str) {
    for (field, value) in expected.as_object().unwrap() {
        assert!(actual.get(field) == Some(value), "{context}: wrong {field}");
    }
}

async fn oracle_row(db: &Db, stmt: SqlStmt, id: &str, context: &str) -> Value {
    let id = id.to_owned();
    let rows: Vec<String> = db
        .run(move |s| s.query_all(&stmt, &[&id]))
        .await
        .unwrap_or_else(|_| panic!("{context}: Oracle query failed"));
    assert_eq!(rows.len(), 1, "{context}: Oracle row count");
    serde_json::from_str(&rows[0]).unwrap_or_else(|_| panic!("{context}: invalid Oracle row"))
}

async fn mongo_collection(uri: &str, default_db: &str, name: &str) -> Collection<Document> {
    let client = Client::with_uri_str(uri)
        .await
        .unwrap_or_else(|_| panic!("{name}: invalid Mongo connection settings"));
    client
        .default_database()
        .unwrap_or_else(|| client.database(default_db))
        .collection(name)
}

async fn event(
    events: &Collection<Document>,
    id: &str,
    kind: impl Into<Bson>,
    context: &str,
) -> Value {
    let mut cursor = events
        .find(doc! { "request_id": id, "event_type": kind.into() })
        .await
        .unwrap_or_else(|_| panic!("{context}: find Mongo events"));
    assert!(
        cursor
            .advance()
            .await
            .unwrap_or_else(|_| panic!("{context}: read Mongo events")),
        "{context}: missing Mongo event"
    );
    let row = serde_json::to_value(
        cursor
            .deserialize_current()
            .unwrap_or_else(|_| panic!("{context}: decode Mongo event")),
    )
    .unwrap_or_else(|_| panic!("{context}: invalid Mongo event"));
    assert!(
        !cursor
            .advance()
            .await
            .unwrap_or_else(|_| panic!("{context}: read Mongo events")),
        "{context}: duplicate Mongo event"
    );
    fields(
        &row,
        json!({"listener": "generator", "human": false}),
        context,
    );
    row
}

async fn wait_for_outbox(db: &Db) {
    let drained = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let count: i64 = db
                .run(|s| s.query_row(&COUNT_OUTBOX, &[]))
                .await
                .unwrap_or_else(|_| panic!("read Oracle outbox count"));
            if count == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
    .await;
    assert!(
        drained.is_ok(),
        "Oracle outbox did not drain within 120 seconds"
    );
}

async fn assert_labels(labels: &Collection<Document>, fixture: &Fixture) {
    let r = &fixture.requests;
    let s = &fixture.subjects;
    let entries = [
        (&r.g_login, "customer", &s.g.customer_no, "G", "g"),
        (&r.g_atm_start, "account", &s.g.account_no, "G", "g"),
        (&r.g_atm_withdraw, "account", &s.g.account_no, "G", "g"),
        (&r.g_small, "account", &s.g.account_no, "G", "g"),
        (&r.g_large, "account", &s.g.account_no, "G", "g"),
        (&r.g_decline, "account", &s.g.account_no, "G", "g"),
        (
            &r.trip_login,
            "customer",
            &s.trip.customer_no,
            "lookalike",
            "trip",
        ),
        (
            &r.trip_card,
            "account",
            &s.trip.account_no,
            "lookalike",
            "trip",
        ),
        (
            &r.online_card,
            "account",
            &s.online.account_no,
            "lookalike",
            "online",
        ),
        (
            &r.normal_card,
            "account",
            &s.normal.account_no,
            "normal",
            "normal",
        ),
    ];
    let request_ids: HashSet<_> = entries
        .iter()
        .map(|entry| entry.0)
        .chain([&r.network_token])
        .collect();
    assert_eq!(
        request_ids.len(),
        11,
        "fixture request IDs must be distinct"
    );
    let mut expected = entries.map(|(id, subject_type, subject_id, label, key)| {
        [
            id.clone(),
            subject_type.into(),
            subject_id.clone(),
            label.into(),
            label.into(),
            format!("{}:{key}", fixture.run_id),
        ]
    });
    let mut cursor = labels
        .find(doc! { "$or": [
            { "run_id": &fixture.run_id },
            { "request_id": { "$in": request_ids.into_iter().cloned().collect::<Vec<_>>() } },
        ] })
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
            .unwrap_or_else(|_| panic!("decode truth label"));
        assert!(
            row.get_str("run_id").ok() == Some(fixture.run_id.as_str()),
            "label belongs to a different run"
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
                    .unwrap_or_else(|_| panic!("missing truth label field {field}"))
                    .to_owned()
            }),
        );
    }
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(actual.len(), expected.len(), "fixture truth label count");
    assert!(
        actual == expected,
        "fixture truth labels differ from the expected six-field set"
    );
}

fn assert_debit(posting: &Value, account: &str, kind: &str, krw: i64, at: At, context: &str) {
    fields(
        posting,
        json!({
            "account_no": account,
            "kind": kind,
            "amount": krw,
            "leg_amount": -krw,
            "transfer_count": 1,
            "leg_count": 1,
            "inout_type": "출금",
            "business_date": at.date_naive().to_string(),
            "counterparty_bank_code": null,
            "counterparty_account_no": null,
        }),
        context,
    );
    for field in ["transfer_at", "leg_at"] {
        assert_eq!(time(posting, field, context), at, "{context}: {field}");
    }
}

fn is_country(region: &Value, country: &str) -> bool {
    region.as_str().is_some_and(|region| {
        region == country
            || region
                .split_once('-')
                .is_some_and(|(prefix, _)| prefix == country)
    })
}

async fn assert_login(
    db: &Db,
    events: &Collection<Document>,
    id: &str,
    holder: &Subject,
    country: &str,
    at: At,
) -> At {
    let context = match country {
        "KR" => "G login",
        _ => "trip login",
    };
    let login = oracle_row(db, LOGIN, id, context).await;
    fields(
        &login,
        json!({"customer_no": holder.customer_no, "outcome": "SUCCESS"}),
        context,
    );
    assert!(
        is_country(&login["region"], country),
        "{context}: wrong login country"
    );
    let at_oracle = time(&login, "occurred_at", context);
    assert_eq!(at_oracle, at, "{context}: business time");
    for kind in ["login.succeeded", "request.completed"] {
        let log = event(events, id, kind, context).await;
        assert_eq!(
            time(&log, "occurred_at", context),
            at,
            "{context}: event time"
        );
        fields(
            &log,
            json!({
                "channel": "MOBILE", "customer_no": holder.customer_no, "account_no": null,
                "session_id": login["session_id"], "region": login["region"],
                "outcome": "completed", "rsp_code": "A0000",
            }),
            context,
        );
        if kind == "request.completed" {
            fields(&log["payload"], json!({"http_status": 200}), context);
        } else {
            fields(&log["payload"], json!({"outcome": "SUCCESS"}), context);
        }
    }
    at_oracle
}

async fn assert_atm(db: &Db, events: &Collection<Document>, fixture: &Fixture, at: At) -> At {
    let r = &fixture.requests;
    let account = &fixture.subjects.g.account_no;
    let context = "G ATM";
    let session = oracle_row(db, ATM, &r.g_atm_start, context).await;
    fields(&session, json!({"account_no": account}), context);
    assert!(
        is_country(&session["region"], "KR"),
        "{context}: terminal country"
    );
    let started_at = time(&session, "started_at", context);
    assert_eq!(started_at, at, "{context}: session start time");
    let posting = oracle_row(db, POSTING, &r.g_atm_withdraw, context).await;
    fields(
        &posting,
        json!({
            "request_id": r.g_atm_withdraw, "request_type": "ATM_WITHDRAW", "status": "DONE",
            "channel": "ATM", "leg_channel": "ATM", "listener": "GENERATOR",
            "session_id": session["session_id"], "terminal_id": session["terminal_id"],
            "leg_terminal_id": session["terminal_id"], "rsp_code": "A0000", "bank_rsp_code": "000",
            "counterparty_name": null, "balance_after": 90_000,
        }),
        context,
    );
    assert_eq!(
        time(&posting, "occurred_at", context),
        at,
        "{context}: request time"
    );
    assert_debit(&posting, account, "atm_cash", 10_000, at, context);

    for (id, kind, outcome) in [
        (&r.g_atm_start, "atm.session_started", "completed"),
        (&r.g_atm_withdraw, "atm.operation", "approved"),
    ] {
        let log = event(events, id, kind, context).await;
        assert_eq!(
            time(&log, "occurred_at", context),
            at,
            "{context}: event time"
        );
        fields(
            &log,
            json!({
                "channel": "ATM", "account_no": account, "session_id": session["session_id"],
                "outcome": outcome,
            }),
            context,
        );
        fields(
            &log["payload"],
            json!({
                "request_id": id, "account_no": account, "terminal_id": session["terminal_id"],
                "region": session["region"], "latitude": session["latitude"], "longitude": session["longitude"],
            }),
            context,
        );
        let completion = event(events, id, "request.completed", context).await;
        assert_eq!(
            time(&completion, "occurred_at", context),
            at,
            "{context}: completion time"
        );
        fields(
            &completion,
            json!({
                "channel": "ATM", "account_no": account, "session_id": session["session_id"],
                "outcome": "completed", "rsp_code": "A0000",
            }),
            context,
        );
        if kind == "atm.operation" {
            fields(
                &log["payload"],
                json!({"operation": "withdraw", "outcome": "approved", "amount": 10_000}),
                context,
            );
            fields(
                &completion["payload"],
                json!({
                    "http_status": 200, "request_type": "ATM_WITHDRAW",
                    "tran_amt": 10_000, "transfer_id": posting["transfer_id"],
                }),
                context,
            );
        } else {
            fields(
                &log["payload"],
                json!({"session_id": session["session_id"]}),
                context,
            );
            fields(
                &completion["payload"],
                json!({"http_status": 200, "request_type": "ATM_SESSION"}),
                context,
            );
        }
    }
    started_at
}

type Coordinate = (&'static str, i64);

struct Location {
    country: &'static str,
    currency: &'static str,
    coords: Option<(Coordinate, Coordinate)>,
}

const NEW_YORK: Location = Location {
    country: "US",
    currency: "USD",
    coords: Some((("40.712800", 40_712_800), ("-74.006000", -74_006_000))),
};
const ONLINE: Location = Location {
    country: "US",
    currency: "USD",
    coords: None,
};
const SEOUL: Location = Location {
    country: "KR",
    currency: "KRW",
    coords: Some((("37.566500", 37_566_500), ("126.978000", 126_978_000))),
};

struct CardCase<'a> {
    request: &'a str,
    holder: &'a Subject,
    at: At,
    merchant: &'static str,
    location: Location,
    amount: i64,
    g_balance: Option<i64>,
    decline: bool,
}

async fn assert_card(db: &Db, events: &Collection<Document>, case: &CardCase<'_>) -> At {
    let context = case.merchant;
    let account = &case.holder.account_no;
    let krw = if case.location.currency == "USD" {
        case.amount * 1300
    } else {
        case.amount
    };
    let (latitude, longitude) = case.location.coords.unzip();
    let entry = if latitude.is_some() {
        "in_person"
    } else {
        "online"
    };
    let (oracle_status, rsp_code, bank_code, reason, domain, outcome, http, completion) =
        if case.decline {
            (
                "REJECTED",
                "A0004",
                "405",
                Some("insufficient_balance"),
                "card.declined",
                "rejected",
                422,
                "rejected",
            )
        } else {
            (
                "DONE",
                "A0000",
                "000",
                None,
                "card.authorized",
                "approved",
                200,
                "completed",
            )
        };

    let auth = oracle_row(db, AUTHORIZATION, case.request, context).await;
    let attrs = json!({
        "merchant_name": case.merchant, "country": case.location.country,
        "currency": case.location.currency, "entry_mode": entry, "krw_amount": krw,
        "decline_reason": reason, "mcc": "5411",
    });
    fields(&auth, attrs.clone(), context);
    fields(
        &auth,
        json!({
            "account_no": account, "customer_no": case.holder.customer_no, "amount": case.amount,
            "latitude_micro": latitude.map(|coord| coord.1),
            "longitude_micro": longitude.map(|coord| coord.1),
        }),
        context,
    );
    let is_g = case.g_balance.is_some() || case.decline;
    if is_g {
        fields(&auth, json!({"prior_payments": 0}), context);
    }

    let posting = oracle_row(db, POSTING, case.request, context).await;
    let at = time(&posting, "occurred_at", context);
    assert_eq!(at, case.at, "{context}: request time");
    fields(
        &posting,
        json!({
            "request_id": case.request, "request_type": "CARD_PAYMENT", "status": oracle_status,
            "channel": "CARD", "listener": "GENERATOR", "session_id": null, "terminal_id": null,
            "rsp_code": rsp_code, "bank_rsp_code": bank_code,
        }),
        context,
    );
    if case.decline {
        fields(
            &posting,
            json!({"transfer_count": 0, "leg_count": 0}),
            context,
        );
    } else {
        assert_debit(&posting, account, "card_payment", krw, at, context);
        fields(
            &posting,
            json!({
                "leg_channel": "CARD", "counterparty_name": case.merchant, "leg_terminal_id": null,
            }),
            context,
        );
        if let Some(balance) = case.g_balance {
            fields(&posting, json!({"balance_after": balance}), context);
        }
    }

    let decision = event(
        events,
        case.request,
        doc! {"$in": ["card.authorized", "card.declined"]},
        context,
    )
    .await;
    let completed = event(events, case.request, "request.completed", context).await;
    for log in [&decision, &completed] {
        assert_eq!(
            time(log, "occurred_at", context),
            at,
            "{context}: event time"
        );
        fields(
            log,
            json!({
                "channel": "CARD", "account_no": account, "customer_no": null, "session_id": null,
                "rsp_code": rsp_code, "bank_rsp_code": bank_code,
            }),
            context,
        );
    }
    fields(
        &decision,
        json!({"event_type": domain, "outcome": outcome}),
        context,
    );
    fields(&completed, json!({"outcome": completion}), context);
    fields(
        &completed["payload"],
        json!({"http_status": http, "request_type": "CARD_PAYMENT"}),
        context,
    );
    fields(&decision["payload"], attrs, context);
    fields(
        &decision["payload"],
        json!({
            "card_no": auth["card_no"], "idempotency_key": posting["idempotency_key"],
            "latitude": latitude.map(|coord| coord.0), "longitude": longitude.map(|coord| coord.0),
        }),
        context,
    );
    let logged_amount = decision["payload"]["amount"]
        .as_str()
        .and_then(|amount| amount.parse::<i64>().ok());
    assert!(
        logged_amount == Some(case.amount),
        "{context}: wrong event amount"
    );
    at
}

#[tokio::test]
#[ignore = "requires the generator card fixture and running Oracle and MongoDB"]
async fn generator_cards_proof() {
    let fixture: Fixture = serde_json::from_str(&required_env("GENERATOR_CARDS_FIXTURE"))
        .unwrap_or_else(|_| panic!("invalid GENERATOR_CARDS_FIXTURE JSON"));
    let truth_uri = required_env("MONGO_TRUTH_URI");
    let events_uri = required_env("MONGO_URI");
    assert!(!fixture.run_id.trim().is_empty(), "fixture run_id is empty");
    assert!(
        (1..=999_999_999_999).contains(&fixture.normal_amount),
        "normal amount outside the ledger range"
    );

    let r = &fixture.requests;
    let s = &fixture.subjects;
    let holders = [&s.g, &s.trip, &s.online, &s.normal];
    let accounts: HashSet<_> = holders.iter().map(|holder| &holder.account_no).collect();
    let customers: HashSet<_> = holders.iter().map(|holder| &holder.customer_no).collect();
    assert_eq!(accounts.len(), 4, "fixture accounts must be distinct");
    assert_eq!(customers.len(), 4, "fixture customers must be distinct");

    // The producer owns the fixture, clock and relay. This proof only reads them.
    let config = DbConfig::from_env().unwrap_or_else(|_| panic!("invalid Oracle config"));
    let db = Db::connect(&config).unwrap_or_else(|_| panic!("connect to Oracle"));
    let labels = mongo_collection(&truth_uri, "truth", "labels").await;
    assert_labels(&labels, &fixture).await;
    wait_for_outbox(&db).await;
    let events = mongo_collection(&events_uri, "bank_logs", "events").await;

    let at = |hour, minute| kst(fixture.date, hour, minute);
    let g_login = assert_login(&db, &events, &r.g_login, &s.g, "KR", at(9, 0)).await;
    let g_atm = assert_atm(&db, &events, &fixture, at(9, 1)).await;
    assert_login(&db, &events, &r.trip_login, &s.trip, "US", at(10, 0)).await;
    let online_customer = s.online.customer_no.clone();
    let foreign_logins: i64 = db
        .run(move |session| session.query_row(&ONLINE_HISTORY, &[&online_customer]))
        .await
        .unwrap_or_else(|_| panic!("read online holder's Oracle login history"));
    assert_eq!(
        foreign_logins, 0,
        "online holder has foreign login evidence"
    );

    let cases = [
        CardCase {
            request: &r.g_small,
            holder: &s.g,
            at: at(10, 0),
            merchant: "Harbor Market",
            location: NEW_YORK,
            amount: 5,
            g_balance: Some(83_500),
            decline: false,
        },
        CardCase {
            request: &r.g_large,
            holder: &s.g,
            at: at(10, 1),
            merchant: "Cedar Grocery",
            location: NEW_YORK,
            amount: 20,
            g_balance: Some(57_500),
            decline: false,
        },
        CardCase {
            request: &r.g_decline,
            holder: &s.g,
            at: at(10, 2),
            merchant: "Union Food Hall",
            location: NEW_YORK,
            amount: 100,
            g_balance: None,
            decline: true,
        },
        CardCase {
            request: &r.trip_card,
            holder: &s.trip,
            at: at(10, 3),
            merchant: "Maple Corner",
            location: NEW_YORK,
            amount: 20,
            g_balance: None,
            decline: false,
        },
        CardCase {
            request: &r.online_card,
            holder: &s.online,
            at: at(10, 4),
            merchant: "Northside Foods",
            location: ONLINE,
            amount: 20,
            g_balance: None,
            decline: false,
        },
        CardCase {
            request: &r.normal_card,
            holder: &s.normal,
            at: at(10, 5),
            merchant: "River Daily Market",
            location: SEOUL,
            amount: fixture.normal_amount,
            g_balance: None,
            decline: false,
        },
    ];
    // The large G payment and both lookalikes must each match US/USD/20 at hour 10 KST.
    for (index, case) in cases.iter().enumerate() {
        let paid_at = assert_card(&db, &events, case).await;
        if index < 3 {
            for activity in [g_login, g_atm] {
                let age = paid_at - activity;
                assert!(
                    (TimeDelta::zero()..=TimeDelta::hours(24)).contains(&age),
                    "{}: missing recent Korean holder activity",
                    case.merchant
                );
            }
        }
    }

    let g_account = s.g.account_no.clone();
    let (owner, balance): (String, i64) = db
        .run(move |session| session.query_row(&BALANCE, &[&g_account]))
        .await
        .unwrap_or_else(|_| panic!("read G Oracle account balance"));
    assert!(owner == s.g.customer_no, "wrong G account holder");
    assert_eq!(balance, 57_500, "G balance after declined payment");

    let token = event(
        &events,
        &r.network_token,
        "request.completed",
        "network token",
    )
    .await;
    fields(
        &token,
        json!({
            "channel": "EXTERNAL", "outcome": "completed", "rsp_code": "A0000",
            "customer_no": null, "account_no": null, "session_id": null,
        }),
        "network token",
    );
    fields(
        &token["payload"],
        json!({"http_status": 200}),
        "network token",
    );
}
