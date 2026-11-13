//! Generator-only client credentials and inbound deposits.
use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes, fields};
use crate::shared::auth::{self, AuthClient};
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::ledger::{self, Kind, LedgerError, Leg, Movement};
use crate::shared::outbox::{self, Event};
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;
use axum::extract::{
    Form, State,
    rejection::{FormRejection, JsonRejection},
};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
const SELECT_CLIENT: SqlStmt = sql_stmt!("inbound.select_client", "sql/select_client.sql");
const INSERT_TOKEN: SqlStmt = sql_stmt!("inbound.insert_token", "sql/insert_token.sql");
const SELECT_SENDER: SqlStmt = sql_stmt!("inbound.select_sender", "sql/select_sender.sql");
const SELECT_RECEIVER: SqlStmt = sql_stmt!("inbound.select_receiver", "sql/select_receiver.sql");
const INSERT_REQUEST: SqlStmt = sql_stmt!("inbound.insert_request", "sql/insert_request.sql");
const SELECT_STORED: SqlStmt = sql_stmt!("inbound.select_stored", "sql/select_stored.sql");
const FINISH_REQUEST: SqlStmt = sql_stmt!("inbound.finish_request", "sql/finish_request.sql");
const ENDPOINT: &str = "/v2.0/transfer/deposit/fin_num";
const FORBIDDEN: &str = "O0003";
// Valid Argon2id hash, fixed for unknown clients to perform the same verification work.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGF0aW9uLWR1bW15IQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/2.0/token", post(token))
        .route(ENDPOINT, post(deposit))
}
fn invalid(ctx: &RequestContext) -> ApiError {
    ApiError::new(
        ctx,
        StatusCode::BAD_REQUEST,
        codes::INVALID_PARAMETER,
        "요청 항목 오류",
    )
}
fn forbidden(ctx: &RequestContext) -> ApiError {
    ApiError::new(ctx, StatusCode::FORBIDDEN, FORBIDDEN, "권한이 없습니다")
}
fn db_error(ctx: &RequestContext, e: &DbError) -> ApiError {
    tracing::warn!(request_id = %ctx.request_id, error = %e, "inbound database failure");
    if matches!(e, DbError::Busy) || matches!(e.ora_code(), Some(30006 | 1653 | 1654 | 1691)) {
        ApiError::busy(ctx)
    } else {
        ApiError::internal(ctx)
    }
}
#[derive(Deserialize)]
struct TokenRequest {
    client_id: String,
    client_secret: String,
    scope: String,
    grant_type: String,
}
async fn token(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    body: Result<Form<TokenRequest>, FormRejection>,
) -> Result<Response, ApiError> {
    let Form(req) = body.map_err(|_| invalid(&ctx))?;
    if req.grant_type != "client_credentials" {
        return Err(invalid(&ctx));
    }
    let client_id = req.client_id.clone();
    let found: Option<(i64, String, String)> = state
        .db
        .run_in(&stats, move |s| s.query_opt(&SELECT_CLIENT, &[&client_id]))
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    let hash = found
        .as_ref()
        .map(|r| r.1.as_str())
        .unwrap_or(DUMMY_HASH)
        .to_owned();
    let verified = auth::verify_secret(hash, req.client_secret)
        .await
        .map_err(|_| ApiError::internal(&ctx))?;
    let Some((id, _, scope)) = found.filter(|_| verified) else {
        return Err(ApiError::new(
            &ctx,
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHORIZED,
            "인증 실패",
        ));
    };
    if req.scope != scope || scope != "oob" {
        return Err(forbidden(&ctx));
    }
    let (token, hash) = auth::new_token();
    let expires = now + chrono::Duration::days(90);
    let c = ctx.clone();
    state
        .db
        .run_in(&stats, move |s| {
            s.execute(&INSERT_TOKEN, &[&hash, &id, &scope, &expires])?;
            outbox::record(
                s,
                &Event::for_request(&c, "request.completed", now, "EXTERNAL", "completed")
                    .codes(codes::SUCCESS, None)
                    .payload(json!({"http_status": 200})),
            )?;
            s.commit()
        })
        .await
        .map_err(|e| db_error(&ctx, &e))?;
    Ok(api::ok(
        &ctx,
        json!({"access_token": token, "token_type": "Bearer", "expires_in": 7776000,
        "scope": "oob", "client_use_code": req.client_id}),
    ))
}
#[derive(Clone, Deserialize, Serialize)]
struct DepositItem {
    tran_no: String,
    bank_tran_id: String,
    fintech_use_num: String,
    print_content: String,
    tran_amt: String,
    req_client_name: String,
    transfer_purpose: String,
}
#[derive(Clone, Deserialize, Serialize)]
struct DepositRequest {
    cntr_account_type: String,
    cntr_account_num: String,
    wd_print_content: String,
    name_check_option: String,
    tran_dtime: String,
    req_cnt: String,
    req_list: Vec<DepositItem>,
}
impl DepositRequest {
    fn amount(&self) -> Option<i64> {
        let i = self.req_list.first()?;
        let digits = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
        };
        if self.cntr_account_type != "N"
            || self.name_check_option != "off"
            || self.req_cnt != "1"
            || self.req_list.len() != 1
            || i.tran_no != "1"
            || i.transfer_purpose != "TR"
            || !digits(&self.cntr_account_num, 16)
            || self.wd_print_content.chars().count() > 20
            || self.tran_dtime.len() != 14
            || !digits(&self.tran_dtime, 14)
            || !fields::valid_bank_tran_id(&i.bank_tran_id)
            || i.fintech_use_num.chars().count() != 24
            || i.print_content.chars().count() > 20
            || i.req_client_name.chars().count() > 20
            || !digits(&i.tran_amt, 12)
        {
            return None;
        }
        i.tran_amt
            .parse::<i64>()
            .ok()
            .filter(|a| (1..=999_999_999_999).contains(a))
    }
    fn hash(&self) -> String {
        auth::hex(&Sha256::digest(
            serde_json::to_vec(self).expect("request serialization"),
        ))
    }
}
struct Plan {
    req: DepositRequest,
    amount: i64,
    hash: String,
    scope: String,
    who: AuthClient,
    ctx: RequestContext,
    now: chrono::DateTime<chrono::FixedOffset>,
}
struct Outcome {
    status: StatusCode,
    code: &'static str,
    message: String,
    body: Value,
}
enum Attempt {
    Done(Outcome),
    Stored(String, String, String, Option<String>),
    InvalidSender,
}
fn finish(
    s: &mut Session,
    p: &Plan,
    body: Value,
    bank_code: &'static str,
    who: Option<(&str, &str)>,
    transfer_id: Option<i64>,
) -> Result<Attempt, DbError> {
    let success = bank_code == "000";
    let status = if success { "DONE" } else { "REJECTED" };
    let code = if success {
        codes::SUCCESS
    } else {
        codes::BANK_REJECTED
    };
    let message = if success { "" } else { "참가기관 거부" };
    let stored = json!({"body": body, "rsp_message": message, "account_no": who.map(|(_, a)| a), "customer_no": who.map(|(c, _)| c)})
        .to_string();
    s.execute(
        &FINISH_REQUEST,
        &[&status, &code, &bank_code, &stored, &p.ctx.request_id],
    )?;
    let mut event = Event::for_request(
        &p.ctx,
        "request.completed",
        p.now,
        "EXTERNAL",
        if success { "completed" } else { "rejected" },
    )
    .codes(code, Some(bank_code))
    .payload(json!({"endpoint": ENDPOINT, "http_status": if success {200} else {422}, "request_type": "INBOUND", "tran_amt": p.amount, "transfer_id": transfer_id}));
    if let Some((customer, account)) = who {
        event = event
            .customer(customer.to_owned())
            .account(account.to_owned());
    }
    outbox::record(s, &event)?;
    s.commit()?;
    Ok(Attempt::Done(Outcome {
        status: if success {
            StatusCode::OK
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        },
        code,
        message: message.into(),
        body,
    }))
}
fn attempt(s: &mut Session, p: &Plan) -> Result<Attempt, DbError> {
    let item = &p.req.req_list[0];
    let listener = p.ctx.listener.as_str();
    if let Err(e) = crate::shared::outcome::claim(
        s,
        &INSERT_REQUEST,
        &[
            &p.ctx.request_id,
            &listener,
            &p.scope,
            &item.bank_tran_id,
            &p.hash,
            &item.bank_tran_id,
            &p.ctx.device_id,
            &p.ctx.ip,
            &p.ctx.region,
            &p.now,
        ],
        &p.ctx.request_id,
        &p.scope,
        &item.bank_tran_id,
    ) {
        if e.ora_code() != Some(1) {
            return Err(e);
        }
        s.rollback()?;
        let row: Option<(String, String, String, Option<String>)> =
            s.query_opt(&SELECT_STORED, &[&p.scope, &item.bank_tran_id])?;
        let row = if crate::shared::outcome::claimed_elsewhere(
            s,
            &p.ctx.request_id,
            &p.scope,
            &item.bank_tran_id,
        )? {
            None
        } else {
            row
        };
        if row.as_ref().is_none_or(|row| row.0 != p.hash) {
            crate::shared::outcome::conflict(
                s,
                &p.ctx,
                p.now,
                "INBOUND",
                "EXTERNAL",
                &item.bank_tran_id,
                (None, None),
            )?;
        }
        let (hash, status, code, body) = row.unwrap_or_default();
        return Ok(Attempt::Stored(hash, status, code, body));
    }
    let sender: Option<(String, String)> = s.query_opt(
        &SELECT_SENDER,
        &[&p.who.institution_code, &p.req.cntr_account_num],
    )?;
    let Some((sender_name, bank_name)) = sender else {
        let stored = json!({"body": {}, "rsp_message": "요청 항목 오류"}).to_string();
        s.execute(
            &FINISH_REQUEST,
            &[
                &"REJECTED",
                &codes::INVALID_PARAMETER,
                &Option::<&str>::None,
                &stored,
                &p.ctx.request_id,
            ],
        )?;
        let event = Event::for_request(&p.ctx, "request.completed", p.now, "EXTERNAL", "rejected")
            .codes(codes::INVALID_PARAMETER, None)
            .payload(json!({"http_status": 400, "request_type": "INBOUND"}));
        outbox::record(s, &event)?;
        s.commit()?;
        return Ok(Attempt::InvalidSender);
    };
    let receiver: Option<(i64, String, String, String)> =
        s.query_opt(&SELECT_RECEIVER, &[&item.fintech_use_num])?;
    let mut result = json!({"tran_no": "1", "bank_tran_id": item.bank_tran_id, "bank_tran_date": fields::date(p.now),
        "bank_code_tran": fields::BANK_CODE, "bank_rsp_code": "000", "bank_rsp_message": "", "fintech_use_num": item.fintech_use_num,
        "print_content": item.print_content, "tran_amt": item.tran_amt});
    let mut locks = if let Some((id, account, _, name)) = &receiver {
        result["account_num_masked"] = json!(fields::mask_account(account));
        result["account_holder_name"] = json!(name.chars().take(50).collect::<String>());
        Some(ledger::lock_accounts(s, &[*id])?)
    } else {
        None
    };
    let bank_code = match (&receiver, &locks) {
        (None, _) => "404",
        (Some((id, _, _, _)), Some(locks))
            if locks
                .get(*id)
                .is_none_or(|a| a.status == "CLOSED" || a.subtype != "DEMAND") =>
        {
            "403"
        }
        _ => "000",
    };
    result["bank_rsp_code"] = json!(bank_code);
    result["bank_rsp_message"] = json!(match bank_code {
        "404" => "계좌 없음",
        "403" => "계좌 상태 오류",
        _ => "",
    });
    let body = json!({"wd_bank_code_std": p.who.institution_code, "wd_bank_name": bank_name.chars().take(20).collect::<String>(),
        "wd_account_num_masked": fields::mask_account(&p.req.cntr_account_num), "wd_print_content": p.req.wd_print_content,
        "wd_account_holder_name": sender_name.chars().take(50).collect::<String>(), "res_cnt": "1", "res_list": [result]});
    let who = receiver
        .as_ref()
        .map(|(_, a, c, _)| (c.as_str(), a.as_str()));
    if bank_code != "000" {
        return finish(s, p, body, bank_code, who, None);
    }
    let id = receiver.as_ref().expect("resolved receiver").0;
    let movement = Movement {
        kind: Kind::Inbound,
        payee_registered: None,
        request_id: Some(&p.ctx.request_id),
        ctx: Some(&p.ctx),
        bank_tran_id: Some(&item.bank_tran_id),
        amount: p.amount,
        channel: "EXTERNAL",
        occurred_at: p.now,
        legs: vec![Leg {
            account_id: id,
            customer_initiated: false,
            amount: p.amount,
            inout_type: "입금",
            tran_type: "대체",
            print_content: Some(item.print_content.clone()),
            counterparty_bank_code: Some(p.who.institution_code.clone()),
            counterparty_account_no: Some(p.req.cntr_account_num.clone()),
            counterparty_name: Some(sender_name),
            transfer_out: 0,
        }],
    };
    let posted = ledger::post(s, locks.as_mut().expect("receiver locked"), &movement).map_err(
        |e| match e {
            LedgerError::Db(e) => e,
            e => DbError::Config(e.to_string()),
        },
    )?;
    finish(s, p, body, "000", who, Some(posted.transfer_id))
}
async fn deposit(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Extension(identity): Extension<crate::shared::outcome::IdentitySlot>,
    who: AuthClient,
    body: Result<Json<DepositRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    if !who.has_scope("oob") {
        return Err(forbidden(&ctx));
    }
    let Json(req) = body.map_err(|_| invalid(&ctx))?;
    let amount = req.amount().ok_or_else(|| invalid(&ctx))?;
    let p = std::sync::Arc::new(Plan {
        hash: req.hash(),
        req,
        amount,
        scope: format!("client:{}", who.counterparty_client_id),
        who,
        ctx: ctx.clone(),
        now,
    });
    let mut retries = 0;
    let result = loop {
        let p = p.clone();
        match state.db.run_in(&stats, move |s| attempt(s, &p)).await {
            Ok(a) => break a,
            Err(e) if e.ora_code() == Some(60) && retries < 2 => {
                retries += 1;
                metrics::counter!("bank_deadlock_retries_total").increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(10 * retries)).await;
            }
            Err(e) => return Err(db_error(&ctx, &e)),
        }
    };
    match result {
        Attempt::InvalidSender => Err(invalid(&ctx)),
        Attempt::Done(o) => Ok(api::respond(&ctx, o.status, o.code, o.message, o.body)),
        Attempt::Stored(hash, status, code, response) => {
            if hash != p.hash {
                return Err(ApiError::new(
                    &ctx,
                    StatusCode::CONFLICT,
                    codes::DUPLICATE_REQUEST,
                    "같은 거래고유번호로 다른 요청이 이미 처리되었습니다",
                ));
            }
            let stored: Value = response
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            identity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .customer_no = stored["customer_no"].as_str().map(str::to_owned);
            let mut response = api::respond(
                &ctx,
                if status == "DONE" {
                    StatusCode::OK
                } else if code == codes::INVALID_PARAMETER {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                },
                codes::known(&code).unwrap_or(codes::SYSTEM_ERROR),
                stored["rsp_message"].as_str().unwrap_or(""),
                stored["body"].clone(),
            );
            crate::shared::outcome::annotate(
                &mut response,
                stored["account_no"].as_str().map(str::to_owned),
                Some("INBOUND".into()),
                Some(true),
            );
            Ok(response)
        }
    }
}
