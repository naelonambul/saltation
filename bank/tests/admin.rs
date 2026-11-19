//! Analyst auth, read contracts, Oracle audit, and keyset witnesses.
mod common;

use std::sync::OnceLock;

use axum::{Router, http::StatusCode};
use bank::shared::auth::token_hash;
use bank::shared::clock::{business_date, kst};
use bank::shared::db::SqlStmt;
use bank::sql_stmt;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use common::card_network;
use common::{Api, api, call, customer, new_id};
use serde_json::{Value, json};

const LOGIN: &str = "/admin/v1/login";
const SEED: &str = "/internal/v1/seed/analysts";
const PASSWORD: &str = "synthetic-analyst-secret";
const PROBE: &str = "/admin/v1/customers?name=not-a-real-customer";

const EVENTS: SqlStmt = sql_stmt!("test.admin_read_events", "sql/admin_read_events.sql");
const HASH: SqlStmt = sql_stmt!("test.admin_analyst_hash", "sql/admin_analyst_hash.sql");
const TOKEN: SqlStmt = sql_stmt!("test.admin_token", "sql/admin_token.sql");
const COUNT: SqlStmt = sql_stmt!("test.admin_token_count", "sql/admin_token_count.sql");
const SCOPE: SqlStmt = sql_stmt!("test.admin_token_scope", "sql/admin_token_scope.sql");
const EXPIRY: SqlStmt = sql_stmt!("test.admin_token_expiry", "sql/admin_token_expiry.sql");
const REVOKE: SqlStmt = sql_stmt!("test.admin_token_revoke", "sql/admin_token_revoke.sql");
const CLOCK_STATE: SqlStmt = sql_stmt!("test.admin_clock_state", "sql/admin_clock_state.sql");
const CLOCK_SET: SqlStmt = sql_stmt!("test.admin_clock_set", "sql/admin_clock_set.sql");
const FAULT_ON: SqlStmt = sql_stmt!("test.admin_fault_on", "sql/admin_fault_on.sql");
const FAULT_OFF: SqlStmt = sql_stmt!("test.admin_fault_off", "sql/admin_fault_off.sql");
const CUSTOMER: SqlStmt = sql_stmt!("test.admin_test_customer", "sql/admin_test_customer.sql");
const NAME: SqlStmt = sql_stmt!("test.admin_test_name", "sql/admin_test_name.sql");
const ATTEMPT: SqlStmt = sql_stmt!("test.admin_test_attempt", "sql/admin_test_attempt.sql");
const CLOSE: SqlStmt = sql_stmt!("test.admin_test_close", "sql/admin_test_close.sql");
const LEG: SqlStmt = sql_stmt!("test.admin_test_leg", "sql/admin_test_leg.sql");

type Wall = DateTime<FixedOffset>;
type ClockRow = (String, Wall, NaiveDate);
type TokenRow = (String, String, String, Option<String>, Wall);

fn wall() -> Wall {
    Utc::now().with_timezone(&kst())
}

fn document() -> &'static Value {
    static DOC: OnceLock<Value> = OnceLock::new();
    DOC.get_or_init(|| {
        let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".into());
        serde_yaml::from_str(&std::fs::read_to_string(format!("{dir}/openapi/admin.yaml")).unwrap())
            .unwrap()
    })
}

fn schema(name: &str, value: &Value) {
    let doc = json!({
        "$defs":document()["components"]["schemas"],
        "$ref":format!("#/$defs/{name}"),
    })
    .to_string()
    .replace("#/components/schemas/", "#/$defs/");
    let doc: Value = serde_json::from_str(&doc).unwrap();
    let validator = jsonschema::draft202012::new(&doc).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{name}: {errors:?}");
}

fn check(response: (StatusCode, Value), expected: u16, contract: &str) -> Value {
    let (status, body) = response;
    assert_eq!(status.as_u16(), expected, "{body}");
    schema(contract, &body);
    body
}

async fn read(router: &Router, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    call(router, "GET", path, &new_id(), bearer, None).await
}

async fn seed(router: &Router, analyst: &str) -> (StatusCode, Value) {
    call(
        router,
        "POST",
        SEED,
        &new_id(),
        None,
        Some(
            json!({"analysts":[{"analyst_no":analyst,"name":"테스트 분석가","password":PASSWORD}]}),
        ),
    )
    .await
}

async fn login(router: &Router, id: &str, analyst: &str, password: &str) -> (StatusCode, Value) {
    call(
        router,
        "POST",
        LOGIN,
        id,
        None,
        Some(json!({"analyst_no":analyst,"password":password})),
    )
    .await
}

async fn issue(api: &Api) -> (String, String) {
    let analyst = format!("analyst_{}", new_id());
    assert_eq!(seed(&api.generator, &analyst).await.0, StatusCode::OK);
    let body = check(
        login(&api.public, &new_id(), &analyst, PASSWORD).await,
        200,
        "LoginResponse",
    );
    (analyst, body["access_token"].as_str().unwrap().to_owned())
}

async fn stored_token(api: &Api, token: &str) -> TokenRow {
    let hash = token_hash(token);
    api.state
        .db
        .run(move |s| s.query_row(&TOKEN, &[&hash]))
        .await
        .unwrap()
}

async fn token_count(api: &Api, analyst: &str) -> i64 {
    let analyst = analyst.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&COUNT, &[&analyst]))
        .await
        .unwrap()
}

async fn events(api: &Api, id: &str) -> Vec<Value> {
    let id = id.to_owned();
    let rows: Vec<String> = api
        .state
        .db
        .run(move |s| s.query_all(&EVENTS, &[&id]))
        .await
        .unwrap();
    rows.iter()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect()
}

fn audit_types(rows: &[Value], expected: &[&str], status: u16, endpoint: &str) {
    let kinds: Vec<_> = rows
        .iter()
        .map(|row| row["event_type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, expected);
    assert!(rows.iter().all(|row| row["channel"] == "ADMIN"));
    let completion = rows.last().unwrap();
    assert_eq!(completion["payload"]["http_status"], status);
    assert_eq!(completion["payload"]["endpoint"], endpoint);
}

async fn set_scope(api: &Api, token: &str, scope: &'static str) {
    let digest = token_hash(token);
    api.state
        .db
        .run(move |s| {
            s.execute(&SCOPE, &[&scope, &digest])?;
            s.commit()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn seed_login_and_auth_boundaries() {
    let api = api().await;
    let analyst = format!("analyst_{}", new_id());
    let half = format!("rollback_{}", new_id());
    assert_eq!(seed(&api.public, &analyst).await.0, StatusCode::NOT_FOUND);
    assert_eq!(seed(&api.generator, &analyst).await.0, StatusCode::OK);
    let duplicate = call(
        &api.generator,
        "POST",
        SEED,
        &new_id(),
        None,
        Some(json!({"analysts":[
            {"analyst_no":half,"name":"rollback","password":PASSWORD},
            {"analyst_no":analyst,"name":"duplicate","password":PASSWORD}
        ]})),
    )
    .await;
    assert_eq!(duplicate.0, StatusCode::CONFLICT);
    assert_eq!(seed(&api.generator, &half).await.0, StatusCode::OK);
    let seeded = analyst.clone();
    let hash: String = api
        .state
        .db
        .run(move |s| s.query_row(&HASH, &[&seeded]))
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2id$") && hash != PASSWORD);

    let missing = format!("missing_{}", new_id());
    let first = new_id();
    let second = new_id();
    let bad = check(
        login(&api.public, &first, &analyst, "wrong-secret").await,
        401,
        "Envelope",
    );
    let unknown = check(
        login(&api.generator, &second, &missing, PASSWORD).await,
        401,
        "Envelope",
    );
    assert_eq!(bad["rsp_code"], unknown["rsp_code"]);
    assert_eq!(bad["rsp_message"], unknown["rsp_message"]);
    for id in [&first, &second] {
        let rows = events(&api, id).await;
        audit_types(
            &rows,
            &["analyst.login.failed", "request.completed"],
            401,
            LOGIN,
        );
        let data = serde_json::to_string(&rows).unwrap();
        assert!(!data.contains(&analyst) && !data.contains(&missing) && !data.contains(PASSWORD));
    }

    let id = new_id();
    let body = check(
        login(&api.generator, &id, &analyst, PASSWORD).await,
        200,
        "LoginResponse",
    );
    let bearer = body["access_token"].as_str().unwrap();
    let stored = stored_token(&api, bearer).await;
    assert!(stored.0 == token_hash(bearer) && stored.0 != bearer);
    assert_eq!(
        (stored.1.as_str(), stored.2.as_str(), stored.3),
        ("ANALYST", "analyst:read", None)
    );
    let rows = events(&api, &id).await;
    audit_types(
        &rows,
        &["analyst.login.succeeded", "request.completed"],
        200,
        LOGIN,
    );
    let data = serde_json::to_string(&rows).unwrap();
    assert!(!data.contains(bearer) && !data.contains(PASSWORD) && !data.contains(&analyst));

    check(
        read(&api.public, PROBE, Some(bearer)).await,
        200,
        "CustomerPage",
    );
    let customer = customer(&api, 100).await;
    let card_token = card_network::token(&api, "card").await;
    for token in [
        None,
        Some(customer.token.as_str()),
        Some(card_token.as_str()),
    ] {
        check(read(&api.public, PROBE, token).await, 401, "Envelope");
    }
    set_scope(&api, &card_token, "analyst:read").await;
    check(
        read(&api.public, PROBE, Some(&card_token)).await,
        401,
        "Envelope",
    );

    let time = api
        .state
        .clock
        .state()
        .await
        .unwrap()
        .business_time
        .format("%Y%m%d%H%M%S")
        .to_string();
    let card_no = card_network::card(&api, &customer.account, "ACTIVE").await;
    let cases = [
        (
            "/v2.0/transfer/withdraw/fin_num",
            json!({"bank_tran_id":common::bank_tran_id(),
            "fintech_use_num":customer.fin,"tran_amt":"1","tran_dtime":time,"transfer_purpose":"TR",
            "recv_client_name":"김수취","recv_client_bank_code":"998","recv_client_account_num":customer.account}),
        ),
        (
            "/v2.0/transfer/deposit/fin_num",
            json!({"cntr_account_type":"N","cntr_account_num":"930000000000",
            "wd_print_content":"송금","name_check_option":"off","tran_dtime":time,"req_cnt":"1",
            "req_list":[{"tran_no":"1","bank_tran_id":common::bank_tran_id(),"fintech_use_num":customer.fin,
            "print_content":"입금","tran_amt":"1","req_client_name":"외부수취인","transfer_purpose":"TR"}]}),
        ),
        (
            "/atm/v1/cash/withdraw",
            json!({"idempotency_key":"test-key","amount":"1"}),
        ),
        (
            card_network::ENDPOINT,
            card_network::message(&card_no, "admin-token-denial", "1"),
        ),
    ];
    for (path, body) in cases {
        assert_eq!(
            call(
                &api.generator,
                "POST",
                path,
                &new_id(),
                Some(bearer),
                Some(body)
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    set_scope(&api, bearer, "wrong").await;
    check(
        read(&api.public, PROBE, Some(bearer)).await,
        401,
        "Envelope",
    );
}

async fn clock_set(api: &Api, mode: String, time: Wall, rolled: NaiveDate) {
    api.state
        .db
        .run(move |s| {
            s.execute(&CLOCK_SET, &[&mode, &time, &rolled])?;
            s.commit()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn analyst_expiry_uses_wall_time_with_independent_revocation() {
    let api = api().await;
    let stored: ClockRow = api
        .state
        .db
        .run(|s| s.query_row(&CLOCK_STATE, &[]))
        .await
        .unwrap();
    let test_api = Api {
        public: api.public.clone(),
        generator: api.generator.clone(),
        state: api.state.clone(),
    };
    let witness = tokio::spawn(async move {
        let past = wall() - Duration::minutes(2);
        clock_set(
            &test_api,
            "BACKFILL".into(),
            past,
            business_date(past).pred_opt().unwrap(),
        )
        .await;
        let before = wall();
        let (_, token) = issue(&test_api).await;
        let after = wall();
        let expiry = stored_token(&test_api, &token).await.4;
        assert!(
            expiry >= before + Duration::minutes(30) && expiry <= after + Duration::minutes(30)
        );
        let future = expiry + Duration::minutes(5);
        clock_set(
            &test_api,
            "BACKFILL".into(),
            future,
            business_date(future).pred_opt().unwrap(),
        )
        .await;
        check(
            read(&test_api.public, PROBE, Some(&token)).await,
            200,
            "CustomerPage",
        );
        let digest = token_hash(&token);
        test_api
            .state
            .db
            .run(move |s| {
                s.execute(&EXPIRY, &[&(wall() - Duration::seconds(1)), &digest])?;
                s.commit()
            })
            .await
            .unwrap();
        check(
            read(&test_api.public, PROBE, Some(&token)).await,
            401,
            "Envelope",
        );
        let (_, revoked) = issue(&test_api).await;
        let digest = token_hash(&revoked);
        test_api
            .state
            .db
            .run(move |s| {
                s.execute(&REVOKE, &[&wall(), &digest])?;
                s.commit()
            })
            .await
            .unwrap();
        check(
            read(&test_api.public, PROBE, Some(&revoked)).await,
            401,
            "Envelope",
        );
    })
    .await;
    clock_set(&api, stored.0, stored.1, stored.2).await;
    witness.unwrap();
}

#[tokio::test]
async fn audit_failures_roll_back_logins_and_hide_read_data() {
    let api = api().await;
    let (analyst, bearer) = issue(&api).await;
    let before = token_count(&api, &analyst).await;
    let correct = "000000000000000000000000A1";
    let wrong = "000000000000000000000000A2";
    let read_id = "000000000000000000000000A3";
    let customer = customer(&api, 0).await;
    let (no, _) = customer_ids(&api, &customer.account).await;
    let profile = format!("/admin/v1/customers/{no}");
    let router = api.public.clone();
    let name = analyst.clone();
    // Fixed request IDs may be reused in serial test runs. Compare only newly
    // persisted events against the baseline to scope each fault witness.
    let old = [
        events(&api, correct).await.len(),
        events(&api, wrong).await.len(),
        events(&api, read_id).await.len(),
    ];
    api.state
        .db
        .run(|s| s.execute(&FAULT_ON, &[]))
        .await
        .unwrap();
    let result = tokio::spawn(async move {
        let correct = login(&router, correct, &name, PASSWORD).await;
        let wrong = login(&router, wrong, &name, "wrong-secret").await;
        let profile = call(&router, "GET", &profile, read_id, Some(&bearer), None).await;
        (correct, wrong, profile)
    })
    .await;
    api.state
        .db
        .run(|s| s.execute(&FAULT_OFF, &[]))
        .await
        .unwrap();
    let (success, failure, protected) = result.unwrap();
    check(success, 503, "Envelope");
    check(failure, 503, "Envelope");
    assert!(
        check(protected, 503, "Envelope")
            .get("customer_no")
            .is_none()
    );
    assert_eq!(token_count(&api, &analyst).await, before);
    for (i, id) in [correct, wrong, read_id].iter().enumerate() {
        let rows = events(&api, id).await;
        let rows = &rows[old[i]..];
        if *id == read_id {
            assert!(rows.is_empty());
        } else {
            audit_types(rows, &["request.completed"], 503, LOGIN);
        }
    }
}

async fn customer_ids(api: &Api, account: &str) -> (String, String) {
    let account = account.to_owned();
    api.state
        .db
        .run(move |s| s.query_row(&CUSTOMER, &[&account]))
        .await
        .unwrap()
}

#[tokio::test]
async fn customer_collections_are_private_bounded_and_attributed() {
    let api = api().await;
    let (_, token) = issue(&api).await;
    let a = customer(&api, 500_000).await;
    let b = customer(&api, 0).await;
    let c = customer(&api, 0).await;
    let (no, phone) = customer_ids(&api, &a.account).await;
    let (b_no, _) = customer_ids(&api, &b.account).await;
    let (c_no, _) = customer_ids(&api, &c.account).await;
    common::register_payee(&api, &a.token, "998", &b.account).await;
    let seed = format!("P{}", common::unique_digits(8));
    let padding = "\u{0001}".repeat(48);
    let name = format!("{seed}%_\\{padding}");
    let records = [no.clone(), b_no.clone(), c_no.clone()];
    let display_name = name.clone();
    api.state
        .db
        .run(move |s| {
            for no in records {
                s.execute(&NAME, &[&display_name, &no])?;
            }
            s.commit()
        })
        .await
        .unwrap();

    let search = format!("/admin/v1/customers?name={seed}%25_%5C&limit=1");
    let first = check(
        read(&api.public, &search, Some(&token)).await,
        200,
        "CustomerPage",
    );
    assert_eq!(first["items"][0]["name"], name);
    let cursor = first["next_cursor"].as_str().unwrap();
    assert!(
        cursor.len() > 512,
        "fixture exercises escaped 60-character names"
    );
    for path in [
        format!("/admin/v1/customers?name=other&cursor={cursor}"),
        format!("/admin/v1/customers/{no}/accounts?cursor={cursor}"),
        format!("/admin/v1/customers?phone={phone}&name=x"),
        "/admin/v1/customers?name=x&limit=0".into(),
        "/admin/v1/customers?name=x&limit=101".into(),
        "/admin/v1/customers?name=x&cursor=%%%".into(),
    ] {
        check(
            read(&api.public, &path, Some(&token)).await,
            400,
            "Envelope",
        );
    }
    let second = check(
        read(
            &api.public,
            &format!("{search}&cursor={cursor}"),
            Some(&token),
        )
        .await,
        200,
        "CustomerPage",
    );
    let cursor = second["next_cursor"].as_str().unwrap();
    let third = check(
        read(
            &api.public,
            &format!("{search}&cursor={cursor}"),
            Some(&token),
        )
        .await,
        200,
        "CustomerPage",
    );
    assert!(third["next_cursor"].is_null());
    let found: Vec<_> = [first, second, third]
        .iter()
        .map(|p| p["items"][0]["customer_no"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(found, [no.clone(), b_no.clone(), c_no]);

    for (field, value) in [("phone", &phone), ("customer_no", &no)] {
        let body = check(
            read(
                &api.public,
                &format!("/admin/v1/customers?{field}={value}"),
                Some(&token),
            )
            .await,
            200,
            "CustomerPage",
        );
        assert_eq!(body["items"][0]["customer_no"], no);
    }
    let path = format!("/admin/v1/customers/{no}");
    for router in [&api.public, &api.generator] {
        check(read(router, &search, None).await, 401, "Envelope");
        check(read(router, &path, None).await, 401, "Envelope");
    }
    let details = check(
        read(&api.public, &path, Some(&token)).await,
        200,
        "CustomerResponse",
    );
    assert_eq!(details["customer_no"], no);
    let wrong = call(
        &api.public,
        "POST",
        "/v2.0/ext/auth/login",
        &new_id(),
        None,
        Some(json!({"phone":phone,"password":"wrong-customer-secret","device_id":"ignored"})),
    )
    .await;
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    let phantom = new_id();
    let foreign = new_id();
    let now = api.state.clock.state().await.unwrap().business_time;
    let phone_insert = phone.clone();
    let excluded = [phantom.clone(), foreign.clone()];
    api.state
        .db
        .run(move |s| {
            s.execute(
                &ATTEMPT,
                &[&phone_insert, &Option::<String>::None, &excluded[0], &now],
            )?;
            s.execute(&ATTEMPT, &[&phone_insert, &Some(b_no), &excluded[1], &now])?;
            s.commit()
        })
        .await
        .unwrap();

    let card = card_network::card(&api, &a.account, "ACTIVE").await;
    common::deposits::request(
        &api,
        &a.token,
        "POST",
        "/v2.0/ext/deposits",
        Some(common::deposits::open_payload(&api, &a.fin, 100_000).await),
        StatusCode::OK,
        "DepositTranResponse",
    )
    .await;
    let loan = call(
        &api.generator,
        "POST",
        "/v2.0/ext/loans",
        &new_id(),
        Some(&a.token),
        Some(
            json!({"bank_tran_id":common::bank_tran_id(), "fintech_use_num":a.fin, "terms_id":"5",
            "tran_amt":"1000000","tran_dtime":now.format("%Y%m%d%H%M%S").to_string()}),
        ),
    )
    .await;
    assert_eq!(loan.0, StatusCode::OK, "{}", loan.1);

    for (collection, shape) in [
        ("accounts", "AccountPage"),
        ("cards", "CardPage"),
        ("deposits", "DepositPage"),
        ("loans", "LoanPage"),
        ("payees", "PayeePage"),
        ("devices", "DevicePage"),
        ("sessions", "SessionPage"),
        ("login-attempts", "LoginAttemptPage"),
    ] {
        let endpoint = format!("{path}/{collection}");
        let body = check(read(&api.public, &endpoint, Some(&token)).await, 200, shape);
        assert!(
            !body["items"].as_array().unwrap().is_empty(),
            "{collection}"
        );
        match collection {
            "cards" => assert_eq!(body["items"][0]["card_no"], card),
            "login-attempts" => {
                let rows = body["items"].as_array().unwrap();
                assert!(rows.iter().any(|a| a["outcome"] == "SUCCESS"));
                assert!(rows.iter().any(|a| a["outcome"] == "WRONG_PASSWORD"));
                assert!(
                    rows.iter()
                        .all(|a| a["request_id"] != phantom && a["request_id"] != foreign)
                );
            }
            _ => {}
        }
        let serialized = body.to_string();
        for private in [
            "password_hash",
            "ci_hash",
            "token_hash",
            "pin_hash",
            "response_json",
        ] {
            assert!(!serialized.contains(private));
        }
        for router in [&api.public, &api.generator] {
            check(read(router, &endpoint, None).await, 401, "Envelope");
        }
    }

    let missing = "/admin/v1/customers/does-not-exist/cards";
    check(
        read(&api.public, missing, Some(&token)).await,
        404,
        "Envelope",
    );
    let (other, _) = customer_ids(&api, &b.account).await;
    let empty = check(
        read(
            &api.public,
            &format!("/admin/v1/customers/{other}/cards"),
            Some(&token),
        )
        .await,
        200,
        "CardPage",
    );
    assert_eq!(empty["items"], json!([]));

    for (router, endpoint, template) in [
        (
            &api.public,
            format!("{path}?secret={seed}"),
            "/admin/v1/customers/{customer_no}",
        ),
        (
            &api.generator,
            format!("/admin/v1/customers?phone={phone}"),
            "/admin/v1/customers",
        ),
    ] {
        let id = new_id();
        assert_eq!(
            call(router, "GET", &endpoint, &id, Some(&token), None)
                .await
                .0,
            StatusCode::OK
        );
        let rows = events(&api, &id).await;
        audit_types(&rows, &["request.completed"], 200, template);
        let row = serde_json::to_string(&rows).unwrap();
        for value in [&no, &phone, &seed, &token] {
            assert!(!row.contains(value));
        }
    }
    let id = new_id();
    assert_eq!(
        call(
            &api.generator,
            "POST",
            LOGIN,
            &id,
            None,
            Some(json!({"unexpected":seed}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let rows = events(&api, &id).await;
    audit_types(&rows, &["request.completed"], 400, LOGIN);
    assert!(!serde_json::to_string(&rows).unwrap().contains(&seed));
}

#[tokio::test]
async fn ledger_card_atm_and_close_views_use_their_own_source_rows() {
    let api = api().await;
    let (_, bearer) = issue(&api).await;
    let a = customer(&api, 100_000).await;
    let b = customer(&api, 3_000).await;
    let card_token = card_network::token(&api, "card").await;
    let first = card_network::card(&api, &a.account, "ACTIVE").await;
    let second = card_network::card(&api, &a.account, "BLOCKED").await;
    let missing = bank::features::cards::random_number().unwrap();
    let ids = [new_id(), new_id(), new_id(), new_id()];
    for (i, (card, code)) in [
        (&first, 200),
        (&second, 422),
        (&first, 200),
        (&missing, 422),
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = card_network::message(card, &ids[i], "25");
        payload["currency"] = json!("USD");
        payload["entry_mode"] = json!("in_person");
        payload["latitude"] = json!("-89.999999");
        payload["longitude"] = json!("180.000000");
        assert_eq!(
            card_network::send(&api, Some(&card_token), &ids[i], payload)
                .await
                .0
                .as_u16(),
            code
        );
    }
    const REQUEST_TIME: SqlStmt = sql_stmt!(
        "test.admin_test_request_time",
        "sql/admin_test_request_time.sql"
    );
    let keys = ids.clone();
    let now = api.state.clock.state().await.unwrap().business_time;
    api.state
        .db
        .run(move |s| {
            s.execute(&REQUEST_TIME, &[&now, &keys[0]])?;
            s.execute(&REQUEST_TIME, &[&now, &keys[1]])?;
            s.execute(
                &REQUEST_TIME,
                &[&(now + Duration::microseconds(1)), &keys[2]],
            )?;
            s.commit()
        })
        .await
        .unwrap();

    let account_path = format!("/admin/v1/accounts/{}", a.account);
    let card_path = format!("{account_path}/card-payments");
    let mut current = format!("{card_path}?limit=1");
    let mut seen = Vec::new();
    for index in 0..3 {
        let result = check(
            read(&api.public, &current, Some(&bearer)).await,
            200,
            "CardPaymentPage",
        );
        let item = &result["items"][0];
        let id = item["request_id"].as_str().unwrap();
        seen.push(id.to_owned());
        assert_eq!(item["original_amount"], "25");
        assert_eq!(item["krw_amount"], "32500");
        assert_eq!(item["merchant_name"], " 테스트 가게 ");
        assert_eq!(item["latitude"], "-89.999999");
        assert_eq!(item["longitude"], "180.000000");
        assert_eq!(
            item["status"],
            if id == ids[1] { "REJECTED" } else { "DONE" }
        );
        if index < 2 {
            let cursor = result["next_cursor"].as_str().unwrap();
            let unrelated = format!(
                "/admin/v1/accounts/{}/card-payments?cursor={cursor}",
                b.account
            );
            check(
                read(&api.public, &unrelated, Some(&bearer)).await,
                400,
                "Envelope",
            );
            current = format!("{card_path}?limit=1&cursor={cursor}");
        } else {
            assert!(result["next_cursor"].is_null());
        }
    }
    let mut ordered = [ids[0].clone(), ids[1].clone()];
    ordered.sort_by(|left, right| right.cmp(left));
    assert_eq!(
        seen,
        [ids[2].clone(), ordered[0].clone(), ordered[1].clone()]
    );
    assert!(!seen.contains(&ids[3]));

    let day = business_date(now);
    let ledger = format!(
        "{account_path}/transactions?from_date={day}&to_date={day}&channel=CARD&direction=debit&min_amount=32500&max_amount=32500"
    );
    let tx = check(
        read(&api.public, &ledger, Some(&bearer)).await,
        200,
        "TransactionPage",
    );
    let legs = tx["items"].as_array().unwrap();
    assert_eq!(legs.len(), 2);
    assert!(
        legs.iter()
            .all(|row| row["amount"] == "-32500" && row["business_date"] == day.to_string())
    );
    let limited = format!("{ledger}&limit=1");
    let first_page = check(
        read(&api.public, &limited, Some(&bearer)).await,
        200,
        "TransactionPage",
    );
    let cursor = first_page["next_cursor"].as_str().unwrap();
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut invalid_key: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).unwrap()).unwrap();
    invalid_key["first"] = json!("invalid-business-date");
    let invalid_key = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invalid_key).unwrap());
    check(
        read(
            &api.public,
            &format!("{limited}&cursor={invalid_key}"),
            Some(&bearer),
        )
        .await,
        400,
        "Envelope",
    );
    let second_page = check(
        read(
            &api.public,
            &format!("{limited}&cursor={cursor}"),
            Some(&bearer),
        )
        .await,
        200,
        "TransactionPage",
    );
    assert_eq!(first_page["items"][0], legs[0]);
    assert_eq!(second_page["items"][0], legs[1]);
    assert!(second_page["next_cursor"].is_null());
    let transfer_path = format!(
        "/admin/v1/transfers/{}",
        legs[0]["transfer_id"].as_str().unwrap()
    );
    let transfer = check(
        read(&api.public, &transfer_path, Some(&bearer)).await,
        200,
        "TransferResponse",
    );
    assert_eq!(transfer["kind"], "card_payment");
    assert_eq!(transfer["request"]["request_id"], legs[0]["request_id"]);
    assert_eq!(transfer["legs"].as_array().unwrap().len(), 1);
    let not_atm = format!(
        "/admin/v1/transactions/{}/terminal",
        legs[0]["tran_id"].as_str().unwrap()
    );
    check(
        read(&api.public, &not_atm, Some(&bearer)).await,
        404,
        "Envelope",
    );

    let terminal = format!("T{}", common::unique_digits(10));
    let (http, body) = call(
        &api.generator,
        "POST",
        "/internal/v1/seed/atm_terminals",
        &new_id(),
        None,
        Some(
            json!({"atm_terminals":[{"terminal_id":terminal,"region":"KR-11","location":"서울",
            "latitude":"37.566500","longitude":"126.978000"}]}),
        ),
    )
    .await;
    assert_eq!(http, StatusCode::OK, "{body}");
    let (http, body) = call(
        &api.generator,
        "POST",
        "/atm/v1/session",
        &new_id(),
        None,
        Some(json!({"terminal_id":terminal,"account_no":b.account,"pin":common::TEST_ATM_PIN})),
    )
    .await;
    assert_eq!(http, StatusCode::OK, "{body}");
    let atm_bearer = body["atm_session_token"].as_str().unwrap();
    let atm_id = new_id();
    let (http, body) = call(
        &api.generator,
        "POST",
        "/atm/v1/cash/deposit",
        &atm_id,
        Some(atm_bearer),
        Some(json!({"idempotency_key":"admin-atm-fixture","amount":"10"})),
    )
    .await;
    assert_eq!(http, StatusCode::OK, "{body}");
    let leg: i64 = api
        .state
        .db
        .run(move |s| s.query_row(&LEG, &[&atm_id]))
        .await
        .unwrap();
    let terminal_path = format!("/admin/v1/transactions/{leg}/terminal");
    let item = check(
        read(&api.public, &terminal_path, Some(&bearer)).await,
        200,
        "TerminalResponse",
    );
    assert_eq!(item["latitude"], "37.566500");
    assert_eq!(item["longitude"], "126.978000");
    assert_eq!(item["terminal_id"], terminal);

    let fixture = [
        (a.account.clone(), "Y", None),
        (b.account.clone(), "N", Some("test mismatch")),
    ];
    api.state
        .db
        .run(move |s| {
            for (account, label, detail) in fixture {
                s.execute(&CLOSE, &[&label, &detail, &account])?;
            }
            s.commit()
        })
        .await
        .unwrap();
    let close_path = "/admin/v1/daily-close?business_date=2001-11-19&limit=1";
    let first = check(
        read(&api.public, close_path, Some(&bearer)).await,
        200,
        "ClosePage",
    );
    assert!(first["items"][0]["verified"].is_boolean());
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = check(
        read(
            &api.public,
            &format!("{close_path}&cursor={cursor}"),
            Some(&bearer),
        )
        .await,
        200,
        "ClosePage",
    );
    assert_ne!(
        first["items"][0]["account_no"],
        second["items"][0]["account_no"]
    );
    let mismatch = check(
        read(
            &api.public,
            &format!("{close_path}&account_no={}", b.account),
            Some(&bearer),
        )
        .await,
        200,
        "ClosePage",
    );
    assert_eq!(mismatch["items"][0]["verified"], false);
    assert_eq!(mismatch["items"][0]["mismatch_detail"], "test mismatch");
    let empty = check(
        read(
            &api.public,
            &format!("{close_path}&account_no=missing"),
            Some(&bearer),
        )
        .await,
        200,
        "ClosePage",
    );
    assert_eq!(empty["items"], json!([]));

    for endpoint in [
        &ledger,
        &transfer_path,
        &card_path,
        &terminal_path,
        &close_path.to_owned(),
    ] {
        for router in [&api.public, &api.generator] {
            check(read(router, endpoint, None).await, 401, "Envelope");
        }
    }
}
