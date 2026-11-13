//! Term deposit products, atomic opening/closure and daily maturity settlement.
use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, FixedOffset, Months, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes, fields};
use crate::shared::auth::{self, AuthCustomer};
use crate::shared::clock::business_date;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{Db, DbError, Session, SqlStmt};
use crate::shared::ledger::{self, Kind, Leg, Movement};
use crate::shared::outbox::{self, Event};
use crate::shared::rollover::{JobFuture, RolloverJob};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
const CANDIDATES: SqlStmt = sql_stmt!("deposits.candidates", "sql/candidates.sql");
const CLOSE_DETAIL: SqlStmt = sql_stmt!("deposits.close_detail", "sql/close_detail.sql");
const CLOSE_INFO: SqlStmt = sql_stmt!("deposits.close_info", "sql/close_info.sql");
const DETAIL: SqlStmt = sql_stmt!("deposits.detail", "sql/detail.sql");
const FINISH_REQUEST: SqlStmt = sql_stmt!("deposits.finish_request", "sql/finish_request.sql");
const INSERT_ACCOUNT: SqlStmt = sql_stmt!("deposits.insert_account", "sql/insert_account.sql");
const INSERT_DETAIL: SqlStmt = sql_stmt!("deposits.insert_detail", "sql/insert_detail.sql");
const INSERT_HISTORY: SqlStmt = sql_stmt!("deposits.insert_history", "sql/insert_history.sql");
const INSERT_FINTECH: SqlStmt = sql_stmt!("deposits.insert_fintech", "sql/insert_fintech.sql");
const INSERT_REQUEST: SqlStmt = sql_stmt!("deposits.insert_request", "sql/insert_request.sql");
const NUMBER: SqlStmt = sql_stmt!("deposits.number", "sql/number.sql");
const PAYER: SqlStmt = sql_stmt!("deposits.payer", "sql/payer.sql");
const PRODUCTS: SqlStmt = sql_stmt!("deposits.products", "sql/products.sql");
const SELECT_STORED: SqlStmt = sql_stmt!("deposits.select_stored", "sql/select_stored.sql");
const TERMS: SqlStmt = sql_stmt!("deposits.terms", "sql/terms.sql");

const ENDPOINT: &str = "/v2.0/ext/deposits";
const MAX_AMOUNT: i64 = 999_999_999_999;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/ext/products", get(products))
        .route(ENDPOINT, post(open))
        .route("/v2.0/ext/deposits/{id}", get(read))
        .route("/v2.0/ext/deposits/{id}/close", post(close))
}

fn invalid(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "요청 항목 오류",
    )
}
fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::warn!(request_id = %ctx.request_id, error = %e, "deposit database failure");
    if matches!(e, DbError::Busy) || matches!(e.ora_code(), Some(30006 | 1653 | 1654 | 1691)) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}
fn ledger_error(e: ledger::LedgerError) -> DbError {
    match e {
        ledger::LedgerError::Db(e) => e,
        e => DbError::Config(e.to_string()),
    }
}
fn numeric(v: &str) -> bool {
    !v.is_empty() && v.len() <= 12 && v.bytes().all(|c| c.is_ascii_digit())
}
fn require_scope(ctx: &RequestContext, who: &AuthCustomer, scope: &str) -> Result<(), ApiError> {
    if who.has_scope(scope) {
        Ok(())
    } else {
        Err(ApiError::new(
            ctx,
            StatusCode::FORBIDDEN,
            "O0003",
            "권한이 없습니다",
        ))
    }
}

type ProductRow = (i64, String, String, i64, Option<i64>, i64, i64, String);
async fn products(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "inquiry")?;
    let rows: Vec<ProductRow> = state
        .db
        .run_in(&stats, |s| s.query_all(&PRODUCTS, &[]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let mut list: Vec<Value> = Vec::new();
    for (id, kind, name, terms, months, min, max, early) in rows {
        if list.last().is_none_or(|v| {
            v["product_id"].as_str().and_then(|x| x.parse::<i64>().ok()) != Some(id)
        }) {
            list.push(json!({"product_id": id.to_string(), "product_type": kind,
                "product_name": name, "terms_list": []}));
        }
        list.last_mut().expect("product")["terms_list"].as_array_mut().expect("terms").push(json!({
            "terms_id": terms.to_string(), "term_months": months.map(|v| v.to_string()).unwrap_or_default(),
            "min_amt": min.to_string(), "max_amt": max.to_string(), "early_close_yn": early
        }));
    }
    Ok(api::ok(
        &ctx,
        json!({"res_cnt": list.len().to_string(), "res_list": list}),
    ))
}

type DetailRow = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);
fn detail(s: &mut Session, id: i64, customer: i64) -> Result<Option<(Value, String)>, DbError> {
    let row: Option<DetailRow> = s.query_opt(&DETAIL, &[&id, &customer])?;
    Ok(row.map(|(id, fin, account, name, principal, balance, opened, maturity, status, linked, close_type, closed)| (json!({
        "deposit_id": id.to_string(), "fintech_use_num": fin, "account_num_masked": fields::mask_account(&account),
        "product_name": name, "principal_amt": principal.to_string(), "balance_amt": balance.to_string(),
        "account_issue_date": opened, "maturity_date": maturity, "account_state": if status == "CLOSED" {"09"} else {"01"},
        "linked_fintech_use_num": linked, "close_type": close_type.unwrap_or_default(), "close_date": closed.unwrap_or_default()
    }), account)))
}
async fn read(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Path(id): crate::shared::api::Path<String>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "inquiry")?;
    let id = id.parse::<i64>().unwrap_or(-1);
    let body = state
        .db
        .run_in(&stats, move |s| detail(s, id, who.customer_id))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    body.map(|(b, account)| {
        let mut response = api::ok(&ctx, b);
        crate::shared::outcome::annotate(&mut response, Some(account), None, None);
        response
    })
    .ok_or_else(|| ApiError::new(&ctx, StatusCode::NOT_FOUND, codes::NOT_FOUND, "대상 없음"))
}

#[derive(Deserialize)]
struct OpenRequest {
    bank_tran_id: String,
    fintech_use_num: String,
    terms_id: String,
    tran_amt: String,
    tran_dtime: String,
}
#[derive(Deserialize)]
struct CloseRequest {
    bank_tran_id: String,
    tran_dtime: String,
}
struct Plan {
    ctx: RequestContext,
    now: DateTime<FixedOffset>,
    customer: i64,
    customer_no: String,
    session: String,
    scope: String,
    key: String,
    hash: String,
    operation: Operation,
}
enum Operation {
    Open {
        fin: String,
        terms: i64,
        amount: i64,
    },
    Close {
        id: i64,
    },
}
impl Plan {
    fn request_type(&self) -> &'static str {
        match self.operation {
            Operation::Open { .. } => "TERM_OPEN",
            Operation::Close { .. } => "TERM_CLOSE",
        }
    }
}
struct Outcome {
    account_no: Option<String>,
    status: StatusCode,
    code: &'static str,
    message: String,
    body: Value,
}
enum Attempt {
    Done(Outcome),
    Stored(Option<(String, String, String, Option<String>)>),
}

fn bank_part(p: &Plan, bank_code: &str, message: &str) -> Value {
    json!({"bank_tran_id": p.key, "bank_tran_date": fields::date(p.now), "bank_code_tran": fields::BANK_CODE,
        "bank_rsp_code": bank_code, "bank_rsp_message": message, "bank_name": fields::BANK_NAME})
}
fn finish(s: &mut Session, p: &Plan, out: Outcome) -> Result<Attempt, DbError> {
    let status = if out.status == StatusCode::OK {
        "DONE"
    } else {
        "REJECTED"
    };
    let bank_code = out.body["bank_rsp_code"].as_str();
    let stored =
        json!({"rsp_message": out.message, "body": out.body, "account_no": out.account_no})
            .to_string();
    s.execute(
        &FINISH_REQUEST,
        &[&status, &out.code, &bank_code, &stored, &p.ctx.request_id],
    )?;
    let endpoint = match p.operation {
        Operation::Open { .. } => ENDPOINT.to_owned(),
        Operation::Close { id } => format!("{ENDPOINT}/{id}/close"),
    };
    let mut event = Event::for_request(
            &p.ctx,
            "request.completed",
            p.now,
            "MOBILE",
            if out.status == StatusCode::OK {
                "completed"
            } else {
                "rejected"
            },
        )
        .customer(p.customer_no.clone())
        .session(p.session.clone())
        .codes(out.code, bank_code)
        .payload(json!({"endpoint": endpoint, "request_type": p.request_type(), "http_status": out.status.as_u16()}));
    event.account_no = out.account_no.clone();
    outbox::record(s, &event)?;
    s.commit()?;
    Ok(Attempt::Done(out))
}
fn reject(
    s: &mut Session,
    p: &Plan,
    bank_code: &str,
    account_no: Option<String>,
) -> Result<Attempt, DbError> {
    finish(
        s,
        p,
        Outcome {
            account_no,
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: codes::BANK_REJECTED,
            message: "상품 거래 거부".into(),
            body: bank_part(p, bank_code, "상품 거래 거부"),
        },
    )
}
fn missing(s: &mut Session, p: &Plan, account_no: Option<String>) -> Result<Attempt, DbError> {
    // As for transfers, persist a missing resource so retries cannot change its outcome.
    finish(
        s,
        p,
        Outcome {
            account_no,
            status: StatusCode::NOT_FOUND,
            code: codes::NOT_FOUND,
            message: "대상 없음".into(),
            body: bank_part(p, "404", "대상 없음"),
        },
    )
}
fn leg(id: i64, amount: i64, initiated: bool, other: &str, name: &str) -> Leg {
    Leg {
        account_id: id,
        amount,
        customer_initiated: initiated,
        inout_type: if amount < 0 { "출금" } else { "입금" },
        tran_type: "대체",
        print_content: None,
        counterparty_bank_code: Some(fields::BANK_CODE.into()),
        counterparty_account_no: Some(other.into()),
        counterparty_name: Some(name.into()),
        transfer_out: 0,
    }
}
fn attempt(s: &mut Session, p: &Plan) -> Result<Attempt, DbError> {
    if let Err(e) = s.execute(
        &INSERT_REQUEST,
        &[
            &p.ctx.request_id,
            &p.request_type(),
            &p.ctx.listener.as_str(),
            &p.scope,
            &p.key,
            &p.hash,
            &p.key,
            &p.session,
            &p.ctx.device_id,
            &p.ctx.ip,
            &p.ctx.region,
            &p.now,
        ],
    ) {
        if e.ora_code() == Some(1) {
            s.rollback()?;
            return Ok(Attempt::Stored(
                s.query_opt(&SELECT_STORED, &[&p.scope, &p.key])?,
            ));
        }
        return Err(e);
    }
    let id = match &p.operation {
        Operation::Open { fin, terms, amount } => {
            let payer: Option<(i64, String, String)> = s.query_opt(&PAYER, &[fin, &p.customer])?;
            let conditions: Option<(i64, i64, i64)> = s.query_opt(&TERMS, &[terms])?;
            let account_no = payer.as_ref().map(|r| r.2.clone());
            let (Some((payer, name, _)), Some((months, min, max))) = (payer, conditions) else {
                return missing(s, p, account_no);
            };
            let payer_locks = ledger::lock_accounts(s, &[payer])?;
            let from = payer_locks.get(payer).expect("locked payer");
            if from.subtype != "DEMAND" || from.status == "CLOSED" {
                return reject(s, p, "403", account_no);
            }
            let rejection = if !(min..=max).contains(amount) {
                Some("407")
            } else if from.balance < *amount {
                Some("405")
            } else {
                None
            };
            if let Some(code) = rejection {
                return reject(s, p, code, account_no);
            }
            // The payer is already locked; the newly allocated account has no other users.
            let placeholder = format!("X{}", &ulid::Ulid::new().to_string()[11..]);
            let id = s.insert_returning_id(
                &INSERT_ACCOUNT,
                &[&placeholder, &p.customer, terms, &p.now],
            )?;
            let number = format!("20{id:012}");
            s.execute(&NUMBER, &[&number, &id])?;
            s.execute(&INSERT_HISTORY, &[&id, &p.now])?;
            let fin = format!("{}{id:021}", fields::BANK_CODE);
            s.execute(
                &INSERT_FINTECH,
                &[&fin, &id, &auth::user_seq_no(p.customer)],
            )?;
            let maturity = business_date(p.now)
                .checked_add_months(Months::new(months as u32))
                .ok_or_else(|| DbError::Config("invalid maturity".into()))?;
            s.execute(&INSERT_DETAIL, &[&id, &payer, amount, &maturity])?;
            let mut locks = ledger::lock_accounts(s, &[payer, id])?;
            ledger::post(
                s,
                &mut locks,
                &Movement {
                    kind: Kind::TermOpen,
                    payee_registered: None,
                    request_id: Some(&p.ctx.request_id),
                    ctx: Some(&p.ctx),
                    bank_tran_id: Some(&p.key),
                    amount: *amount,
                    channel: "MOBILE",
                    occurred_at: p.now,
                    legs: vec![
                        leg(payer, -*amount, true, &number, &name),
                        leg(id, *amount, false, &from.account_no, &name),
                    ],
                },
            )
            .map_err(ledger_error)?;
            id
        }
        Operation::Close { id } => {
            let info: Option<(i64, i64, String, String)> =
                s.query_opt(&CLOSE_INFO, &[id, &p.customer])?;
            let Some((linked, principal, early, name)) = info else {
                return missing(s, p, None);
            };
            let mut locks = ledger::lock_accounts(s, &[*id, linked])?;
            let account_no = Some(locks.get(*id).expect("term").account_no.clone());
            if locks.get(*id).expect("term").status == "CLOSED" {
                return reject(s, p, "403", account_no);
            }
            if early != "Y" {
                return reject(s, p, "407", account_no);
            }
            if locks.get(linked).expect("linked").status == "CLOSED" {
                return reject(s, p, "403", account_no);
            }
            let term_no = locks.get(*id).expect("term").account_no.clone();
            let linked_no = locks.get(linked).expect("linked").account_no.clone();
            ledger::post(
                s,
                &mut locks,
                &Movement {
                    kind: Kind::TermClose,
                    payee_registered: None,
                    request_id: Some(&p.ctx.request_id),
                    ctx: Some(&p.ctx),
                    bank_tran_id: Some(&p.key),
                    amount: principal,
                    channel: "MOBILE",
                    occurred_at: p.now,
                    legs: vec![
                        leg(*id, -principal, false, &linked_no, &name),
                        leg(linked, principal, true, &term_no, &name),
                    ],
                },
            )
            .map_err(ledger_error)?;
            ledger::set_status(
                s,
                &mut locks,
                *id,
                "CLOSED",
                "early_close",
                p.now,
                Some(&p.ctx),
                "MOBILE",
            )
            .map_err(ledger_error)?;
            s.execute(&CLOSE_DETAIL, &[&p.now, &"EARLY", id])?;
            *id
        }
    };
    let (mut body, account_no) =
        detail(s, id, p.customer)?.ok_or_else(|| DbError::Config("new deposit missing".into()))?;
    body.as_object_mut().expect("detail").extend(
        bank_part(p, "000", "")
            .as_object()
            .expect("bank part")
            .clone(),
    );
    finish(
        s,
        p,
        Outcome {
            account_no: Some(account_no),
            status: StatusCode::OK,
            code: codes::SUCCESS,
            message: String::new(),
            body,
        },
    )
}

async fn execute(state: AppState, stats: RequestStats, p: Plan) -> Result<Response, ApiError> {
    let p = std::sync::Arc::new(p);
    let mut retries = 0;
    loop {
        let plan = p.clone();
        match state.db.run_in(&stats, move |s| attempt(s, &plan)).await {
            Ok(Attempt::Done(o)) => {
                return Ok(api::respond(&p.ctx, o.status, o.code, o.message, o.body));
            }
            Ok(Attempt::Stored(row)) => {
                let Some((_, status, code, response)) = row.filter(|r| r.0 == p.hash) else {
                    return Err(ApiError::new(
                        &p.ctx,
                        StatusCode::CONFLICT,
                        codes::DUPLICATE_REQUEST,
                        "중복 요청",
                    ));
                };
                let stored: Value = response
                    .as_deref()
                    .and_then(|r| serde_json::from_str(r).ok())
                    .unwrap_or(Value::Null);
                let code = codes::known(&code).unwrap_or(codes::SYSTEM_ERROR);
                let http = if status == "DONE" {
                    StatusCode::OK
                } else if code == codes::NOT_FOUND {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                };
                let mut response = api::respond(
                    &p.ctx,
                    http,
                    code,
                    stored["rsp_message"].as_str().unwrap_or(""),
                    stored["body"].clone(),
                );
                crate::shared::outcome::annotate(
                    &mut response,
                    stored["account_no"].as_str().map(str::to_owned),
                    Some(p.request_type().into()),
                    Some(true),
                );
                return Ok(response);
            }
            Err(e) if e.ora_code() == Some(60) && retries < 2 => {
                retries += 1;
                metrics::counter!("bank_deadlock_retries_total").increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(10 * retries)).await;
            }
            Err(e) => return Err(db_error(&p.ctx, &e)),
        }
    }
}
fn plan(
    ctx: RequestContext,
    now: DateTime<FixedOffset>,
    who: AuthCustomer,
    key: String,
    canonical: Value,
    operation: Operation,
) -> Plan {
    Plan {
        ctx,
        now,
        customer: who.customer_id,
        customer_no: who.customer_no,
        session: who.session_id,
        scope: format!("C:{}", who.customer_id),
        key,
        hash: auth::hex(&Sha256::digest(canonical.to_string().as_bytes())),
        operation,
    }
}
async fn open(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    body: Result<Json<OpenRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "transfer")?;
    let Json(req) = body.map_err(|_| invalid(&ctx))?;
    let amount = req.tran_amt.parse::<i64>().unwrap_or(0);
    if !fields::valid_bank_tran_id(&req.bank_tran_id)
        || !fields::valid_tran_dtime(&req.tran_dtime)
        || !numeric(&req.terms_id)
        || !numeric(&req.tran_amt)
        || !(1..=MAX_AMOUNT).contains(&amount)
        || req.fintech_use_num.chars().count() != 24
    {
        return Err(invalid(&ctx));
    }
    let terms = req.terms_id.parse::<i64>().map_err(|_| invalid(&ctx))?;
    let canonical = json!(["TERM_OPEN", req.fintech_use_num, req.terms_id, req.tran_amt]);
    execute(
        state,
        stats,
        plan(
            ctx,
            now,
            who,
            req.bank_tran_id,
            canonical,
            Operation::Open {
                fin: req.fintech_use_num,
                terms,
                amount,
            },
        ),
    )
    .await
}
async fn close(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Path(id): crate::shared::api::Path<String>,
    body: Result<Json<CloseRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "transfer")?;
    let Json(req) = body.map_err(|_| invalid(&ctx))?;
    if !fields::valid_bank_tran_id(&req.bank_tran_id) || !fields::valid_tran_dtime(&req.tran_dtime)
    {
        return Err(invalid(&ctx));
    }
    let id = id.parse::<i64>().unwrap_or(-1);
    execute(
        state,
        stats,
        plan(
            ctx,
            now,
            who,
            req.bank_tran_id,
            json!(["TERM_CLOSE", id]),
            Operation::Close { id },
        ),
    )
    .await
}

pub struct MaturityJob;
impl RolloverJob for MaturityJob {
    fn name(&self) -> &'static str {
        "maturity"
    }
    fn run<'a>(&'a self, db: &'a Db, date: NaiveDate, at: DateTime<FixedOffset>) -> JobFuture<'a> {
        Box::pin(async move {
            let rows: Vec<(i64, i64, i64, String)> =
                db.run(move |s| s.query_all(&CANDIDATES, &[&date])).await?;
            for (id, linked, principal, name) in rows {
                db.run(move |s| {
                    let mut locks = ledger::lock_accounts(s, &[id, linked])?;
                    if locks.get(id).expect("term").status != "CLOSED" {
                        let term_no = locks.get(id).expect("term").account_no.clone();
                        let linked_no = locks.get(linked).expect("linked").account_no.clone();
                        ledger::post(
                            s,
                            &mut locks,
                            &Movement {
                                kind: Kind::TermClose,
                                payee_registered: None,
                                request_id: None,
                                ctx: None,
                                bank_tran_id: None,
                                amount: principal,
                                channel: "SYSTEM",
                                occurred_at: at,
                                legs: vec![
                                    leg(id, -principal, false, &linked_no, &name),
                                    leg(linked, principal, false, &term_no, &name),
                                ],
                            },
                        )
                        .map_err(ledger_error)?;
                        ledger::set_status(
                            s, &mut locks, id, "CLOSED", "maturity", at, None, "SYSTEM",
                        )
                        .map_err(ledger_error)?;
                        s.execute(&CLOSE_DETAIL, &[&at, &"MATURITY", &id])?;
                    }
                    s.commit()
                })
                .await?;
            }
            Ok(())
        })
    }
}
