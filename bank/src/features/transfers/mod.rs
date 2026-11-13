//! In-bank and external transfers and transfer results (spec F-14, F-16,
//! F-18, F-19; Money operation processing).
//!
//! One Oracle transaction per attempt: claim the idempotency key, lock
//! affected accounts in ascending ID order, check status, balance and the daily limit,
//! post through the ledger, record the outcome and its outbox event, commit.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Extension, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::shared::api::fields::{self, BANK_CODE, BANK_NAME};
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth::{self, AuthCustomer};
use crate::shared::clock::business_date;
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::ledger::{self, Kind, LedgerError, Leg, Movement};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const INSERT_REQUEST: SqlStmt = sql_stmt!("transfers.insert_request", "sql/insert_request.sql");
const FINISH_REQUEST: SqlStmt = sql_stmt!("transfers.finish_request", "sql/finish_request.sql");
const SELECT_STORED: SqlStmt = sql_stmt!("transfers.select_stored", "sql/select_stored.sql");
const SELECT_SENDER: SqlStmt = sql_stmt!("transfers.select_sender", "sql/select_sender.sql");
const SELECT_RECEIVER: SqlStmt = sql_stmt!("transfers.select_receiver", "sql/select_receiver.sql");
const SELECT_EXTERNAL: SqlStmt = sql_stmt!("transfers.select_external", "sql/select_external.sql");
const REGISTERED: SqlStmt = sql_stmt!("transfers.registered", "sql/registered.sql");
const SELECT_RESULT: SqlStmt = sql_stmt!("transfers.select_result", "sql/select_result.sql");

const MAX_AMOUNT: i64 = 999_999_999_999;
const ENDPOINT: &str = "/v2.0/transfer/withdraw/fin_num";
const DEADLOCK_RETRIES: usize = 2;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.0/transfer/withdraw/fin_num", post(withdraw))
        .route("/v2.0/transfer/result", post(result))
}

#[derive(Debug, Clone, Deserialize)]
struct WithdrawRequest {
    bank_tran_id: String,
    fintech_use_num: String,
    tran_amt: String,
    tran_dtime: String,
    #[serde(default)]
    wd_print_content: String,
    #[serde(default)]
    dps_print_content: String,
    transfer_purpose: String,
    recv_client_name: String,
    recv_client_bank_code: String,
    recv_client_account_num: String,
}

impl WithdrawRequest {
    /// SHA-256 of the fields that define the movement (spec F-18).
    fn hash(&self) -> String {
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            self.fintech_use_num,
            self.tran_amt,
            self.recv_client_bank_code,
            self.recv_client_account_num,
            self.recv_client_name,
            self.wd_print_content,
            self.dps_print_content,
            self.transfer_purpose
        );
        auth::hex(&Sha256::digest(canonical.as_bytes()))
    }
}

/// The outcome of one attempt, as stored for replay.
#[derive(Debug, Clone)]
struct Outcome {
    status: StatusCode,
    rsp_code: &'static str,
    rsp_message: String,
    body: Value,
}

enum Attempt {
    Done(Outcome),
    Replayed {
        hash: String,
        status: String,
        rsp_code: String,
        response: Option<String>,
    },
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn invalid(ctx: &RequestContext, message: &str) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        message,
    )
}

fn map_db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::warn!(request_id = %ctx.request_id, error = %e, "transfer database failure");
    match e.ora_code() {
        // Lock wait expired, or the outbox tablespace is full (spec F-36a).
        Some(30006) | Some(1653) | Some(1654) | Some(1691) => {
            if e.ora_code() != Some(30006) {
                metrics::counter!("bank_outbox_full_total").increment(1);
                tracing::error!(request_id = %ctx.request_id, error = %e, "outbox tablespace full");
            }
            ApiError::busy(ctx)
        }
        _ if matches!(e, DbError::Busy) => ApiError::busy(ctx),
        _ => {
            tracing::error!(request_id = %ctx.request_id, error = %e, "transfer database error");
            ApiError::internal(ctx)
        }
    }
}

struct Plan {
    req: WithdrawRequest,
    amount: i64,
    daily_limit: i64,
    hash: String,
    scope: String,
    customer_id: i64,
    customer_no: String,
    session_id: String,
    ctx: RequestContext,
    now: chrono::DateTime<chrono::FixedOffset>,
}

fn bank_part(
    req: &WithdrawRequest,
    plan: &Plan,
    bank_rsp_code: &str,
    bank_rsp_message: &str,
) -> Value {
    json!({
        "bank_tran_id": req.bank_tran_id,
        "bank_tran_date": fields::date(plan.now),
        "bank_code_tran": BANK_CODE,
        "bank_rsp_code": bank_rsp_code,
        "bank_rsp_message": bank_rsp_message,
        "bank_name": BANK_NAME,
        "fintech_use_num": req.fintech_use_num,
        "tran_amt": req.tran_amt,
    })
}

/// Records a bank-level rejection with no ledger effect (spec F-19).
/// A bank-level rejection reason.
struct Rejection {
    status: StatusCode,
    rsp_code: &'static str,
    bank_rsp_code: &'static str,
    message: &'static str,
}

const fn rejection(
    status: StatusCode,
    rsp_code: &'static str,
    bank_rsp_code: &'static str,
    message: &'static str,
) -> Rejection {
    Rejection {
        status,
        rsp_code,
        bank_rsp_code,
        message,
    }
}

fn reject(
    s: &mut Session,
    plan: &Plan,
    who: (Option<&str>, Option<&str>),
    r: Rejection,
) -> Result<Outcome, DbError> {
    let (customer_no, account_no) = who;
    let Rejection {
        status,
        rsp_code,
        bank_rsp_code,
        message,
    } = r;
    let body = bank_part(&plan.req, plan, bank_rsp_code, message);
    finish(
        s,
        plan,
        "REJECTED",
        rsp_code,
        Some(bank_rsp_code),
        (&body, account_no),
        message,
    )?;
    let mut event = Event::for_request(
        &plan.ctx,
        "request.completed",
        plan.now,
        "MOBILE",
        "rejected",
    )
    .customer(plan.customer_no.clone())
    .session(plan.session_id.clone())
    .codes(rsp_code, Some(bank_rsp_code))
    .payload(json!({ "endpoint": ENDPOINT, "http_status": status.as_u16(), "request_type": "TRANSFER", "tran_amt": plan.amount, "reason": message }));
    if let Some(c) = customer_no {
        event = event.customer(c);
    }
    if let Some(a) = account_no {
        event = event.account(a);
    }
    outbox::record(s, &event)?;
    s.commit()?;
    Ok(Outcome {
        status,
        rsp_code,
        rsp_message: message.to_owned(),
        body,
    })
}

fn finish(
    s: &mut Session,
    plan: &Plan,
    status: &str,
    rsp_code: &str,
    bank_rsp_code: Option<&str>,
    result: (&Value, Option<&str>),
    message: &str,
) -> Result<(), DbError> {
    let (body, account_no) = result;
    let stored =
        json!({ "rsp_message": message, "body": body, "account_no": account_no }).to_string();
    s.execute(
        &FINISH_REQUEST,
        &[
            &status,
            &rsp_code,
            &bank_rsp_code,
            &stored,
            &plan.ctx.request_id,
        ],
    )?;
    Ok(())
}

fn attempt(s: &mut Session, plan: &Plan) -> Result<Attempt, DbError> {
    let ctx = &plan.ctx;
    let req = &plan.req;
    let human = ctx.listener.as_str();
    if let Err(e) = s.execute(
        &INSERT_REQUEST,
        &[
            &ctx.request_id,
            &human,
            &plan.scope,
            &req.bank_tran_id,
            &plan.hash,
            &req.bank_tran_id,
            &plan.session_id,
            &ctx.device_id,
            &ctx.ip,
            &ctx.region,
            &plan.now,
        ],
    ) {
        if e.ora_code() == Some(1) {
            s.rollback()?;
            let stored: Option<(String, String, String, Option<String>)> =
                s.query_opt(&SELECT_STORED, &[&plan.scope, &req.bank_tran_id])?;
            return match stored {
                Some((hash, status, rsp_code, response)) => Ok(Attempt::Replayed {
                    hash,
                    status,
                    rsp_code,
                    response,
                }),
                // The conflict was on request_id (a reused X-Request-Id with a different key).
                None => Ok(Attempt::Replayed {
                    hash: String::new(),
                    status: String::new(),
                    rsp_code: String::new(),
                    response: None,
                }),
            };
        }
        return Err(e);
    }

    let sender: Option<(i64, String, String, String)> =
        s.query_opt(&SELECT_SENDER, &[&req.fintech_use_num, &plan.customer_id])?;
    let Some((sender_id, sender_no, customer_no, sender_name)) = sender else {
        return reject(
            s,
            plan,
            (None, None),
            rejection(
                StatusCode::NOT_FOUND,
                codes::NOT_FOUND,
                "404",
                "출금계좌를 찾을 수 없습니다",
            ),
        )
        .map(Attempt::Done);
    };
    let cust = Some(customer_no.as_str());
    let acct = Some(sender_no.as_str());
    let external = req.recv_client_bank_code != BANK_CODE;
    let receiver: Option<(i64, String, String, String)> = if external {
        let row: Option<(String, String)> = s.query_opt(
            &SELECT_EXTERNAL,
            &[&req.recv_client_bank_code, &req.recv_client_account_num],
        )?;
        row.map(|(name, bank)| (0, String::new(), name, bank))
    } else {
        let row: Option<(i64, String, String)> =
            s.query_opt(&SELECT_RECEIVER, &[&req.recv_client_account_num])?;
        row.map(|(id, customer, name)| (id, customer, name, BANK_NAME.to_owned()))
    };
    let Some((receiver_id, receiver_customer, receiver_name, receiver_bank_name)) = receiver else {
        return reject(
            s,
            plan,
            (cust, acct),
            rejection(
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "404",
                "입금계좌를 찾을 수 없습니다",
            ),
        )
        .map(Attempt::Done);
    };
    if receiver_id == sender_id {
        return reject(
            s,
            plan,
            (cust, acct),
            rejection(
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "403",
                "같은 계좌로는 이체할 수 없습니다",
            ),
        )
        .map(Attempt::Done);
    }

    let ids = if external {
        vec![sender_id]
    } else {
        vec![sender_id, receiver_id]
    };
    let mut locks = ledger::lock_accounts(s, &ids)?;
    let from = locks
        .get(sender_id)
        .cloned()
        .ok_or_else(|| DbError::Config("locked sender missing".into()))?;
    let receiver_invalid = !external
        && locks
            .get(receiver_id)
            .is_none_or(|to| to.status == "CLOSED" || to.subtype != "DEMAND");
    if from.status == "CLOSED" || from.subtype != "DEMAND" || receiver_invalid {
        return reject(
            s,
            plan,
            (cust, acct),
            rejection(
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "403",
                "이체할 수 없는 계좌 상태입니다",
            ),
        )
        .map(Attempt::Done);
    }
    let daily = ledger::lock_daily(s, sender_id, business_date(plan.now))?;
    if daily.transfer_out + plan.amount > plan.daily_limit {
        return reject(
            s,
            plan,
            (cust, acct),
            rejection(
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "406",
                "1일 이체한도를 초과했습니다",
            ),
        )
        .map(Attempt::Done);
    }
    if from.balance < plan.amount {
        return reject(
            s,
            plan,
            (cust, acct),
            rejection(
                StatusCode::UNPROCESSABLE_ENTITY,
                codes::BANK_REJECTED,
                "405",
                "잔액이 부족합니다",
            ),
        )
        .map(Attempt::Done);
    }

    let wd_print = if req.wd_print_content.is_empty() {
        truncate(&receiver_name, 14)
    } else {
        truncate(&req.wd_print_content, 14)
    };
    let dps_print = if req.dps_print_content.is_empty() {
        truncate(&sender_name, 20)
    } else {
        truncate(&req.dps_print_content, 20)
    };
    let registered: i64 = s.query_row(
        &REGISTERED,
        &[
            &plan.customer_id,
            &req.recv_client_bank_code,
            &req.recv_client_account_num,
        ],
    )?;
    let mut movement = Movement {
        kind: Kind::Transfer,
        payee_registered: Some(registered > 0),
        request_id: Some(&ctx.request_id),
        ctx: Some(ctx),
        bank_tran_id: Some(&req.bank_tran_id),
        amount: plan.amount,
        channel: "MOBILE",
        occurred_at: plan.now,
        legs: vec![
            Leg {
                account_id: sender_id,
                customer_initiated: true,
                amount: -plan.amount,
                inout_type: "출금",
                tran_type: "대체",
                print_content: Some(wd_print.clone()),
                counterparty_bank_code: Some(req.recv_client_bank_code.clone()),
                counterparty_account_no: Some(req.recv_client_account_num.clone()),
                counterparty_name: Some(receiver_name.clone()),
                transfer_out: plan.amount,
            },
            Leg {
                account_id: receiver_id,
                customer_initiated: receiver_customer == customer_no,
                amount: plan.amount,
                inout_type: "입금",
                tran_type: "대체",
                print_content: Some(dps_print.clone()),
                counterparty_bank_code: Some(BANK_CODE.into()),
                counterparty_account_no: Some(sender_no.clone()),
                counterparty_name: Some(sender_name.clone()),
                transfer_out: 0,
            },
        ],
    };
    if external {
        movement.legs.truncate(1);
    }
    let posted = match ledger::post(s, &mut locks, &movement) {
        Ok(p) => p,
        Err(LedgerError::Db(e)) => return Err(e),
        Err(e) => return Err(DbError::Config(e.to_string())),
    };

    let mut body = bank_part(req, plan, "000", "");
    if let Some(obj) = body.as_object_mut() {
        obj.extend(
            json!({
                "account_num_masked": fields::mask_account(&sender_no),
                "print_content": wd_print,
                "wd_limit_remain_amt": (plan.daily_limit - daily.transfer_out - plan.amount).min(MAX_AMOUNT).to_string(),
                "dps_bank_code_std": req.recv_client_bank_code,
                "dps_bank_name": receiver_bank_name,
                "dps_account_num_masked": fields::mask_account(&req.recv_client_account_num),
                "dps_print_content": dps_print,
                "dps_account_holder_name": receiver_name,
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
    }
    finish(
        s,
        plan,
        "DONE",
        codes::SUCCESS,
        Some("000"),
        (&body, Some(&sender_no)),
        "",
    )?;
    let event = Event::for_request(ctx, "request.completed", plan.now, "MOBILE", "completed")
        .customer(customer_no.clone())
        .account(sender_no.clone())
        .session(plan.session_id.clone())
        .codes(codes::SUCCESS, Some("000"))
        .payload(json!({
            "endpoint": ENDPOINT,
            "http_status": 200,
            "request_type": "TRANSFER",
            "tran_amt": plan.amount,
            "transfer_id": posted.transfer_id,
            "counterparty_bank_code": req.recv_client_bank_code,
            "counterparty_account_no": req.recv_client_account_num,
        }));
    outbox::record(s, &event)?;
    s.commit()?;
    Ok(Attempt::Done(Outcome {
        status: StatusCode::OK,
        rsp_code: codes::SUCCESS,
        rsp_message: String::new(),
        body,
    }))
}

fn replay(ctx: &RequestContext, plan_hash: &str, a: Attempt) -> Result<Response, ApiError> {
    match a {
        Attempt::Done(o) => Ok(api::respond(
            ctx,
            o.status,
            o.rsp_code,
            o.rsp_message,
            o.body,
        )),
        Attempt::Replayed {
            hash,
            status,
            rsp_code,
            response,
        } => {
            if hash != plan_hash {
                return Err(ApiError::new(
                    ctx,
                    StatusCode::CONFLICT,
                    codes::DUPLICATE_REQUEST,
                    "같은 거래고유번호로 다른 요청이 이미 처리되었습니다",
                ));
            }
            let stored: Value = response
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok())
                .unwrap_or(Value::Null);
            let code = codes::known(&rsp_code).unwrap_or(codes::SYSTEM_ERROR);
            let http = if status == "DONE" {
                StatusCode::OK
            } else if stored["body"]["bank_rsp_code"] == "404" && code == codes::NOT_FOUND {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            let message = stored["rsp_message"].as_str().unwrap_or("").to_owned();
            let mut response = api::respond(ctx, http, code, message, stored["body"].clone());
            crate::shared::outcome::annotate(
                &mut response,
                stored["account_no"].as_str().map(str::to_owned),
                Some("TRANSFER".into()),
                Some(true),
            );
            Ok(response)
        }
    }
}

async fn withdraw(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Json(req): crate::shared::api::Json<WithdrawRequest>,
) -> Result<Response, ApiError> {
    if !who.has_scope("transfer") {
        return Err(ApiError::new(
            &ctx,
            StatusCode::FORBIDDEN,
            codes::UNAUTHORIZED,
            "권한이 없습니다",
        ));
    }
    if !fields::valid_bank_tran_id(&req.bank_tran_id) || !fields::valid_tran_dtime(&req.tran_dtime)
    {
        return Err(invalid(
            &ctx,
            "bank_tran_id 또는 tran_dtime 형식이 올바르지 않습니다",
        ));
    }
    if req.transfer_purpose != "TR" {
        return Err(invalid(&ctx, "transfer_purpose는 TR이어야 합니다"));
    }
    let amount = match req.tran_amt.parse::<i64>() {
        Ok(a)
            if (1..=MAX_AMOUNT).contains(&a)
                && req.tran_amt.chars().all(|c| c.is_ascii_digit()) =>
        {
            a
        }
        _ => {
            return Err(invalid(
                &ctx,
                "tran_amt는 1 이상 12자리 이하의 숫자여야 합니다",
            ));
        }
    };

    let plan = std::sync::Arc::new(Plan {
        hash: req.hash(),
        req,
        amount,
        daily_limit: state.limits.transfer_daily,
        scope: format!("C:{}", who.customer_id),
        customer_id: who.customer_id,
        customer_no: who.customer_no.clone(),
        session_id: who.session_id.clone(),
        ctx: ctx.clone(),
        now,
    });
    let mut retries = 0;
    loop {
        let p = plan.clone();
        match state.db.run_in(&stats, move |s| attempt(s, &p)).await {
            Ok(a) => return replay(&ctx, &plan.hash, a),
            Err(e) if e.ora_code() == Some(60) && retries < DEADLOCK_RETRIES => {
                retries += 1;
                metrics::counter!("bank_deadlock_retries_total").increment(1);
                tracing::warn!(request_id = %ctx.request_id, retries, "deadlock, retrying");
                tokio::time::sleep(std::time::Duration::from_millis(10 * retries as u64)).await;
            }
            Err(e) => return Err(map_db_error(&ctx, &e)),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ResultItem {
    tran_no: String,
    org_bank_tran_id: String,
}

#[derive(Debug, Deserialize)]
struct ResultRequest {
    req_list: Vec<ResultItem>,
}

async fn result(
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    who: AuthCustomer,
    crate::shared::api::Json(req): crate::shared::api::Json<ResultRequest>,
) -> Result<Response, ApiError> {
    if req.req_list.is_empty() || req.req_list.len() > 25 {
        return Err(invalid(&ctx, "req_list는 1건 이상 25건 이하여야 합니다"));
    }
    let scope = format!("C:{}", who.customer_id);
    let items = req.req_list;
    let rows = state
        .db
        .run_in(&stats, move |s| {
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                let found: Option<(String, String, Option<String>)> =
                    s.query_opt(&SELECT_RESULT, &[&scope, &item.org_bank_tran_id])?;
                out.push((item.tran_no.clone(), item.org_bank_tran_id.clone(), found));
            }
            Ok(out)
        })
        .await
        .map_err(|e| map_db_error(&ctx, &e))?;
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(tran_no, btid, found)| match found {
            Some((_, _, response)) => {
                let stored: Value = response.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
                let mut item = stored["body"].clone();
                if let Some(o) = item.as_object_mut() {
                    o.insert("tran_no".into(), json!(tran_no));
                }
                item
            }
            None => json!({ "tran_no": tran_no, "bank_tran_id": btid, "bank_rsp_code": "404", "bank_rsp_message": "거래를 찾을 수 없습니다" }),
        })
        .collect();
    Ok(api::ok(
        &ctx,
        json!({ "res_cnt": list.len().to_string(), "res_list": list }),
    ))
}
