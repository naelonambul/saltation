use axum::{Extension, extract::State, http::StatusCode, response::Response};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::fx::{self, Validated};
use crate::app::AppState;
use crate::shared::api::{self, ApiError, codes};
use crate::shared::auth::{self, AuthClient};
use crate::shared::context::{BusinessTime, RequestContext};
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::ledger::{self, Kind, LedgerError, Leg, Movement};
use crate::shared::outbox::{self, Event};
use crate::shared::outcome;
use crate::shared::telemetry::RequestStats;
use crate::sql_stmt;

const CHANNEL: &str = "CARD";
const REQUEST_TYPE: &str = "CARD_PAYMENT";
const CLAIM: SqlStmt = sql_stmt!("cards.claim_payment", "sql/claim_payment.sql");
const STORED: SqlStmt = sql_stmt!("cards.stored_payment", "sql/stored_payment.sql");
const LOCK_CARD: SqlStmt = sql_stmt!("cards.lock_card", "sql/lock_card.sql");
const INSERT: SqlStmt = sql_stmt!("cards.insert_authorization", "sql/insert_authorization.sql");
const FINISH: SqlStmt = sql_stmt!("cards.finish_payment", "sql/finish_payment.sql");

struct Payment {
    ctx: RequestContext,
    now: DateTime<FixedOffset>,
    client_id: String,
    scope: String,
    validated: Validated,
}

impl Payment {
    fn text(&self, field: &str) -> &str {
        self.validated.message[field]
            .as_str()
            .expect("validated card message field")
    }

    fn hash(&self) -> String {
        let canonical = json!([
            REQUEST_TYPE,
            self.text("card_no"),
            self.validated.amount,
            self.text("currency"),
            self.text("merchant_name"),
            self.text("mcc"),
            self.text("country"),
            self.text("entry_mode"),
            self.validated.latitude,
            self.validated.longitude,
        ]);
        auth::hex(&Sha256::digest(canonical.to_string()))
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Decline {
    UnknownCard,
    BlockedCard,
    ClosedAccount,
    InsufficientBalance,
}

impl Decline {
    fn bank_result(self) -> (&'static str, &'static str) {
        match self {
            Self::UnknownCard => ("404", "카드를 찾을 수 없습니다"),
            Self::BlockedCard => ("403", "사용 정지된 카드입니다"),
            Self::ClosedAccount => ("403", "계좌 상태 오류"),
            Self::InsufficientBalance => ("405", "잔액 부족"),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Decision {
    krw_amount: u64,
    decline: Option<Decline>,
    account_no: Option<String>,
}

impl Decision {
    fn status(&self) -> StatusCode {
        if self.decline.is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        }
    }

    fn code(&self) -> &'static str {
        if self.decline.is_some() {
            codes::BANK_REJECTED
        } else {
            codes::SUCCESS
        }
    }

    fn bank_result(&self) -> (&'static str, &'static str) {
        self.decline.map_or(("000", ""), Decline::bank_result)
    }

    fn respond(self, ctx: &RequestContext, replayed: bool) -> Response {
        let (bank_code, message) = self.bank_result();
        let body = json!({
            "approved": self.decline.is_none(),
            "decline_reason": self.decline,
            "krw_amount": self.krw_amount.to_string(),
            "bank_rsp_code": bank_code,
            "bank_rsp_message": message,
        });
        let mut response = api::respond(ctx, self.status(), self.code(), message, body);
        outcome::annotate(
            &mut response,
            self.account_no,
            Some(REQUEST_TYPE.into()),
            Some(replayed),
        );
        response
    }
}

enum Attempt {
    Executed(Decision),
    Replay(Decision),
    Conflict,
}

fn previous(s: &mut Session, p: &Payment, key: &str, hash: &str) -> Result<Attempt, DbError> {
    s.rollback()?;
    let saved: Option<(String, String)> = s.query_opt(&STORED, &[&p.scope, &key])?;
    if let Some((saved_hash, decision)) = saved
        && saved_hash == hash
        && !outcome::claimed_elsewhere(s, &p.ctx.request_id, &p.scope, key)?
    {
        let decision = serde_json::from_str(&decision)
            .map_err(|e| DbError::Config(format!("stored card decision: {e}")))?;
        return Ok(Attempt::Replay(decision));
    }

    outcome::conflict(
        s,
        &p.ctx,
        p.now,
        REQUEST_TYPE,
        CHANNEL,
        key,
        (None, None),
        None,
    )?;
    Ok(Attempt::Conflict)
}

fn execute(s: &mut Session, p: &Payment) -> Result<Attempt, DbError> {
    let key = p.text("idempotency_key");
    let hash = p.hash();
    if let Err(error) = s.execute(
        &CLAIM,
        &[
            &p.ctx.request_id,
            &p.ctx.listener.as_str(),
            &p.scope,
            &key,
            &hash,
            &p.ctx.device_id,
            &p.ctx.ip,
            &p.ctx.region,
            &p.now,
        ],
    ) {
        return if error.ora_code() == Some(1) {
            previous(s, p, key, &hash)
        } else {
            Err(error)
        };
    }

    let krw = p.validated.krw_amount;
    let card: Option<(i64, i64, String)> = s.query_opt(&LOCK_CARD, &[&p.text("card_no")])?;
    let card_id = card.as_ref().map(|card| card.0);
    let mut decision = Decision {
        krw_amount: krw,
        decline: Some(Decline::UnknownCard),
        account_no: None,
    };
    if let Some((_, account_id, card_status)) = card {
        let mut locks = ledger::lock_accounts(s, &[account_id])?;
        let account = locks.get(account_id).expect("card account was locked");
        decision.account_no = Some(account.account_no.clone());
        let amount = krw as i64;
        decision.decline = if card_status != "ACTIVE" {
            Some(Decline::BlockedCard)
        } else if !matches!(account.status.as_str(), "ACTIVE" | "DORMANT") {
            Some(Decline::ClosedAccount)
        } else if account.balance < amount {
            Some(Decline::InsufficientBalance)
        } else {
            ledger::post(
                s,
                &mut locks,
                &Movement {
                    kind: Kind::CardPayment,
                    terminal_id: None,
                    payee_registered: None,
                    request_id: Some(&p.ctx.request_id),
                    ctx: Some(&p.ctx),
                    bank_tran_id: None,
                    amount,
                    channel: CHANNEL,
                    occurred_at: p.now,
                    legs: vec![Leg {
                        customer_initiated: true,
                        account_id,
                        amount: -amount,
                        inout_type: "출금",
                        tran_type: "대체",
                        print_content: Some("카드 결제".into()),
                        counterparty_bank_code: None,
                        counterparty_account_no: None,
                        counterparty_name: Some(p.text("merchant_name").to_owned()),
                        transfer_out: 0,
                    }],
                },
            )
            .map_err(|error| match error {
                LedgerError::Db(db) => db,
                other => DbError::Config(other.to_string()),
            })?;
            None
        };
    }
    finish(s, p, card_id, &decision)?;
    Ok(Attempt::Executed(decision))
}

fn finish(
    s: &mut Session,
    p: &Payment,
    card_id: Option<i64>,
    decision: &Decision,
) -> Result<(), DbError> {
    s.execute(
        &INSERT,
        &[
            &p.ctx.request_id,
            &card_id,
            &p.text("card_no"),
            &decision.krw_amount,
            &p.text("currency"),
            &p.validated.amount,
            &p.text("merchant_name"),
            &p.text("mcc"),
            &p.text("country"),
            &p.text("entry_mode"),
            &p.validated.latitude,
            &p.validated.longitude,
        ],
    )?;
    let approved = decision.decline.is_none();
    let status = if approved { "DONE" } else { "REJECTED" };
    let (bank_code, _) = decision.bank_result();
    let saved = serde_json::to_string(decision)
        .map_err(|e| DbError::Config(format!("card decision: {e}")))?;
    s.execute(
        &FINISH,
        &[
            &status,
            &decision.code(),
            &bank_code,
            &saved,
            &p.ctx.request_id,
        ],
    )?;

    let mut event = Event::for_request(
        &p.ctx,
        if approved {
            "card.authorized"
        } else {
            "card.declined"
        },
        p.now,
        CHANNEL,
        if approved { "approved" } else { "rejected" },
    )
    .codes(decision.code(), Some(bank_code))
    .payload(json!({
        "client_id": p.client_id,
        "idempotency_key": p.text("idempotency_key"),
        "card_no": p.text("card_no"),
        "krw_amount": decision.krw_amount,
        "amount": p.text("amount"),
        "currency": p.text("currency"),
        "merchant_name": p.text("merchant_name"),
        "mcc": p.text("mcc"),
        "country": p.text("country"),
        "entry_mode": p.text("entry_mode"),
        "latitude": p.validated.message["latitude"],
        "longitude": p.validated.message["longitude"],
        "decline_reason": decision.decline,
    }));
    event.account_no.clone_from(&decision.account_no);
    outbox::record(s, &event)?;

    let mut completion = Event::for_request(
        &p.ctx,
        "request.completed",
        p.now,
        CHANNEL,
        if approved { "completed" } else { "rejected" },
    )
    .codes(decision.code(), Some(bank_code))
    .payload(json!({"http_status": decision.status().as_u16(), "request_type": REQUEST_TYPE}));
    completion.account_no.clone_from(&decision.account_no);
    outbox::record(s, &completion)?;
    s.commit()
}

pub(super) async fn authorize(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    client: AuthClient,
    body: Result<api::Json<Value>, ApiError>,
) -> Result<Response, ApiError> {
    if !client.has_scope("card") {
        return Err(ApiError::new(
            &ctx,
            StatusCode::FORBIDDEN,
            "O0003",
            "카드 결제 권한이 없습니다",
        ));
    }
    let api::Json(message) = body?;
    let validated = fx::validate(&message).map_err(|field| {
        ApiError::new(
            &ctx,
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMETER,
            format!("요청 항목 오류 ({field})"),
        )
    })?;
    let p = Payment {
        ctx: ctx.clone(),
        now,
        client_id: client.client_id,
        scope: format!("client:{}", client.counterparty_client_id),
        validated,
    };
    let result = state
        .db
        .run_in(&stats, move |s| execute(s, &p))
        .await
        .map_err(|error| {
            tracing::warn!(request_id = %ctx.request_id, error = %error, "card payment database failure");
            if matches!(error, DbError::Busy)
                || matches!(error.ora_code(), Some(60 | 30006 | 1653 | 1654 | 1691))
            {
                ApiError::busy(&ctx)
            } else {
                ApiError::internal(&ctx)
            }
        })?;
    match result {
        Attempt::Executed(result) => Ok(result.respond(&ctx, false)),
        Attempt::Replay(result) => Ok(result.respond(&ctx, true)),
        Attempt::Conflict => Err(ApiError::new(
            &ctx,
            StatusCode::CONFLICT,
            codes::DUPLICATE_REQUEST,
            "같은 멱등성 키 또는 요청 ID로 다른 요청이 이미 처리되었습니다",
        )),
    }
}
