//! Customer loans with atomic disbursement and repayment.
use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, FixedOffset, Months};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes, fields};
use crate::shared::auth::{self, AuthCustomer};
use crate::shared::clock::business_date;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::ledger::{self, Kind, Leg, Movement};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
const REPAY_INFO: SqlStmt = sql_stmt!("loans.repay_info", "sql/repay_info.sql");
const DETAIL: SqlStmt = sql_stmt!("loans.detail", "sql/detail.sql");
const FINISH_REQUEST: SqlStmt = sql_stmt!("loans.finish_request", "sql/finish_request.sql");
const INSERT_ACCOUNT: SqlStmt = sql_stmt!("loans.insert_account", "sql/insert_account.sql");
const INSERT_DETAIL: SqlStmt = sql_stmt!("loans.insert_detail", "sql/insert_detail.sql");
const INSERT_HISTORY: SqlStmt = sql_stmt!("loans.insert_history", "sql/insert_history.sql");
const INSERT_FINTECH: SqlStmt = sql_stmt!("loans.insert_fintech", "sql/insert_fintech.sql");
const INSERT_REQUEST: SqlStmt = sql_stmt!("loans.insert_request", "sql/insert_request.sql");
const NUMBER: SqlStmt = sql_stmt!("loans.number", "sql/number.sql");
const PAYER: SqlStmt = sql_stmt!("loans.payer", "sql/payer.sql");
const SELECT_STORED: SqlStmt = sql_stmt!("loans.select_stored", "sql/select_stored.sql");
const TERMS: SqlStmt = sql_stmt!("loans.terms", "sql/terms.sql");

const ENDPOINT: &str = "/v2.0/ext/loans";
const MAX_AMOUNT: i64 = 999_999_999_999;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(ENDPOINT, post(open))
        .route("/v2.0/ext/loans/{id}", get(read))
        .route("/v2.0/ext/loans/{id}/repay", post(repay))
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
    tracing::warn!(request_id = %ctx.request_id, error = %e, "loan database failure");
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
);
fn detail(s: &mut Session, id: i64, customer: i64) -> Result<Option<(Value, String)>, DbError> {
    let row: Option<DetailRow> = s.query_opt(&DETAIL, &[&id, &customer])?;
    Ok(row.map(|(id, fin, account, name, principal, balance, opened, maturity, status, linked)| (json!({
        "loan_id": id.to_string(), "fintech_use_num": fin, "account_num_masked": fields::mask_account(&account),
        "product_name": name, "principal_amt": principal.to_string(), "balance_amt": balance.to_string(),
        "account_issue_date": opened, "maturity_date": maturity, "account_state": if status == "CLOSED" {"09"} else {"01"},
        "linked_fintech_use_num": linked
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
struct RepayRequest {
    bank_tran_id: String,
    tran_amt: String,
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
    Repay {
        id: i64,
        amount: i64,
    },
}
impl Plan {
    fn request_type(&self) -> &'static str {
        match self.operation {
            Operation::Open { .. } => "LOAN_TAKE",
            Operation::Repay { .. } => "LOAN_REPAY",
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
        Operation::Repay { id, .. } => format!("{ENDPOINT}/{id}/repay"),
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
            let stored: Option<(String, String, String, Option<String>)> =
                s.query_opt(&SELECT_STORED, &[&p.scope, &p.key])?;
            if stored.as_ref().is_none_or(|row| row.0 != p.hash) {
                crate::shared::outcome::conflict(
                    s,
                    &p.ctx,
                    p.now,
                    p.request_type(),
                    "MOBILE",
                    &p.key,
                    (Some(&p.customer_no), Some(&p.session)),
                )?;
            }
            return Ok(Attempt::Stored(stored));
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
            if !(min..=max).contains(amount) {
                return reject(s, p, "407", account_no);
            }
            // The payer is already locked; the newly allocated account has no other users.
            let placeholder = format!("X{}", &ulid::Ulid::new().to_string()[11..]);
            let id = s.insert_returning_id(
                &INSERT_ACCOUNT,
                &[&placeholder, &p.customer, terms, &p.now],
            )?;
            let number = format!("30{id:012}");
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
            s.execute(&INSERT_DETAIL, &[&id, &payer, amount, &p.now, &maturity])?;
            let mut locks = ledger::lock_accounts(s, &[payer, id])?;
            ledger::post(
                s,
                &mut locks,
                &Movement {
                    kind: Kind::LoanDisburse,
                    payee_registered: None,
                    request_id: Some(&p.ctx.request_id),
                    ctx: Some(&p.ctx),
                    bank_tran_id: Some(&p.key),
                    amount: *amount,
                    channel: "MOBILE",
                    occurred_at: p.now,
                    legs: vec![
                        leg(payer, *amount, true, &number, &name),
                        leg(id, *amount, false, &from.account_no, &name),
                    ],
                },
            )
            .map_err(ledger_error)?;
            id
        }
        Operation::Repay { id, amount } => {
            let linked: Option<(i64, String)> = s.query_opt(&REPAY_INFO, &[id, &p.customer])?;
            let Some((linked, name)) = linked else {
                return missing(s, p, None);
            };
            let mut locks = ledger::lock_accounts(s, &[*id, linked])?;
            let owed = locks.get(*id).expect("loan");
            let account_no = Some(owed.account_no.clone());
            let payer = locks.get(linked).expect("linked");
            if owed.status == "CLOSED" || payer.status == "CLOSED" {
                return reject(s, p, "403", account_no);
            }
            if *amount > owed.balance {
                return reject(s, p, "408", account_no);
            }
            if payer.balance < *amount {
                return reject(s, p, "405", account_no);
            }
            let loan_no = owed.account_no.clone();
            let payer_no = payer.account_no.clone();
            ledger::post(
                s,
                &mut locks,
                &Movement {
                    kind: Kind::LoanRepay,
                    payee_registered: None,
                    request_id: Some(&p.ctx.request_id),
                    ctx: Some(&p.ctx),
                    bank_tran_id: Some(&p.key),
                    amount: *amount,
                    channel: "MOBILE",
                    occurred_at: p.now,
                    legs: vec![
                        leg(linked, -*amount, true, &loan_no, &name),
                        leg(*id, -*amount, false, &payer_no, &name),
                    ],
                },
            )
            .map_err(ledger_error)?;
            if locks.get(*id).expect("loan").balance == 0 {
                ledger::set_status(
                    s,
                    &mut locks,
                    *id,
                    "CLOSED",
                    "repaid",
                    p.now,
                    Some(&p.ctx),
                    "MOBILE",
                )
                .map_err(ledger_error)?;
            }
            *id
        }
    };
    let (mut body, account_no) =
        detail(s, id, p.customer)?.ok_or_else(|| DbError::Config("new loan missing".into()))?;
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
    let canonical = json!(["LOAN_TAKE", req.fintech_use_num, req.terms_id, req.tran_amt]);
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
async fn repay(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Path(id): crate::shared::api::Path<String>,
    body: Result<Json<RepayRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    require_scope(&ctx, &who, "transfer")?;
    let Json(req) = body.map_err(|_| invalid(&ctx))?;
    let amount = req.tran_amt.parse::<i64>().unwrap_or(0);
    if !fields::valid_bank_tran_id(&req.bank_tran_id)
        || !fields::valid_tran_dtime(&req.tran_dtime)
        || !numeric(&req.tran_amt)
        || !(1..=MAX_AMOUNT).contains(&amount)
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
            json!(["LOAN_REPAY", id, req.tran_amt]),
            Operation::Repay { id, amount },
        ),
    )
    .await
}
