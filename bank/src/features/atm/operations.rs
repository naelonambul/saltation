//! Cash and balance operations, with one transaction per attempt.
use super::*;
use crate::shared::clock::business_date;
use crate::shared::ledger::{self, Kind, LedgerError, Leg, Movement};
use crate::shared::limits::Limits;
use axum::body::Bytes;
use serde_json::Value;
use sha2::{Digest, Sha256};

const INSERT: SqlStmt = sql_stmt!("atm.cash_insert_request", "sql/cash_insert_request.sql");
const STORED: SqlStmt = sql_stmt!("atm.cash_stored", "sql/cash_stored.sql");
const FINISH: SqlStmt = sql_stmt!("atm.cash_finish", "sql/cash_finish.sql");
const ACCOUNT: SqlStmt = sql_stmt!("atm.operation_account", "sql/operation_account.sql");
type Stored = (String, String, String, Option<String>);

#[derive(Clone, Copy)]
enum Operation {
    Withdraw,
    Deposit,
    Balance,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Withdraw => "withdraw",
            Self::Deposit => "deposit",
            Self::Balance => "balance",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Withdraw => "ATM_WITHDRAW",
            Self::Deposit => "ATM_DEPOSIT",
            Self::Balance => "ATM_BALANCE",
        }
    }
    fn cash(self) -> bool {
        !matches!(self, Self::Balance)
    }
    fn endpoint(self) -> &'static str {
        match self {
            Self::Withdraw => "/atm/v1/cash/withdraw",
            Self::Deposit => "/atm/v1/cash/deposit",
            Self::Balance => "/atm/v1/balance",
        }
    }
}
#[derive(Deserialize)]
struct Cash {
    idempotency_key: String,
    amount: String,
}
struct Plan {
    ctx: RequestContext,
    now: At,
    hash: Option<String>,
    identity: IdentitySlot,
    op: Operation,
    body: Bytes,
    limits: Limits,
}
struct ResultRow {
    status: StatusCode,
    code: &'static str,
    body: Value,
    replayed: bool,
}
impl ResultRow {
    fn error(status: StatusCode, code: &'static str) -> Self {
        Self {
            status,
            code,
            body: json!({}),
            replayed: false,
        }
    }
}
fn completed(p: &Plan, row: &SessionRow, status: u16, code: &str, bank: Option<&str>) -> Event {
    Event::for_request(&p.ctx, "request.completed", p.now, "ATM", if status < 400 { "completed" } else { "rejected" })
        .session(&row.0).account(&row.1).codes(code, bank)
        .payload(json!({"endpoint": p.op.endpoint(), "method": if p.op.cash() { "POST" } else { "GET" }, "http_status": status, "request_type": p.op.kind()}))
}
fn operation(
    s: &mut Session,
    p: &Plan,
    row: &SessionRow,
    t: &Terminal,
    amount: Option<i64>,
    bank: Option<&str>,
) -> Result<(), DbError> {
    let approved = bank.is_none();
    let code = if approved {
        codes::SUCCESS
    } else {
        codes::BANK_REJECTED
    };
    let bank = bank.or(Some("000"));
    let mut e = event(&p.ctx, p.now, t, &row.1, "atm.operation")
        .session(&row.0)
        .codes(code, bank);
    e.outcome = if approved { "approved" } else { "rejected" };
    e.payload["operation"] = p.op.name().into();
    e.payload["outcome"] = e.outcome.into();
    e.payload["amount"] = json!(amount);
    e.payload["rsp_code"] = code.into();
    e.payload["bank_rsp_code"] = json!(bank);
    outbox::record(s, &e)
}
fn store(s: &mut Session, p: &Plan, body: &Value, bank: Option<&str>) -> Result<(), DbError> {
    let code = if bank.is_some() {
        codes::BANK_REJECTED
    } else {
        codes::SUCCESS
    };
    let response = json!({"body": body}).to_string();
    s.execute(
        &FINISH,
        &[
            &if bank.is_some() { "REJECTED" } else { "DONE" },
            &code,
            &bank.or(Some("000")),
            &response,
            &p.ctx.request_id,
        ],
    )?;
    Ok(())
}
fn bank_message(code: &str) -> &'static str {
    match code {
        "403" => "계좌 상태 오류",
        "405" => "잔액 부족",
        "406" => "한도 초과",
        "409" => "PIN 입력 횟수 초과",
        _ => "참가기관 거부",
    }
}
fn amount(value: &str) -> Option<i64> {
    if value.len() > 12
        || !value
            .as_bytes()
            .first()
            .is_some_and(|c| (b'1'..=b'9').contains(c))
        || !value.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    value
        .parse()
        .ok()
        .filter(|n| (1..=999_999_999_999).contains(n))
}
fn reject(
    s: &mut Session,
    p: &Plan,
    row: &SessionRow,
    t: &Terminal,
    amount: Option<i64>,
    bank: &'static str,
) -> Result<ResultRow, DbError> {
    let body = json!({"bank_rsp_code": bank, "bank_rsp_message": bank_message(bank)});
    if p.op.cash() {
        store(s, p, &body, Some(bank))?;
    }
    operation(s, p, row, t, amount, Some(bank))?;
    let mut e = completed(p, row, 422, codes::BANK_REJECTED, Some(bank));
    if let Some(amount) = amount {
        e.payload["tran_amt"] = amount.into();
    }
    outbox::record(s, &e)?;
    s.commit()?;
    Ok(ResultRow {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: codes::BANK_REJECTED,
        body,
        replayed: false,
    })
}
fn locked(s: &mut Session, p: &Plan, row: &SessionRow) -> Result<bool, DbError> {
    let credential: Option<(String, i64, Option<At>)> = s.query_opt(&CREDENTIAL, &[&row.1])?;
    Ok(credential.is_some_and(|(_, _, until)| until.is_some_and(|at| at > p.now)))
}
fn attempt(s: &mut Session, p: &Plan) -> Result<ResultRow, DbError> {
    let unauthorized = || ResultRow::error(StatusCode::UNAUTHORIZED, codes::UNAUTHORIZED);
    let Some(hash) = &p.hash else {
        return Ok(unauthorized());
    };
    let found: Option<String> = s.query_opt(&LOCK_SESSION, &[hash])?;
    if found.is_none() {
        return Ok(unauthorized());
    }
    let row: SessionRow = s.query_row(&SESSION, &[hash])?;
    {
        let mut identity = p.identity.lock().unwrap_or_else(|e| e.into_inner());
        identity.terminal_id = Some(row.5.clone());
        identity.session_id = Some(row.0.clone());
        identity.account_no = Some(row.1.clone());
    }
    if row.3.is_some() {
        return Ok(unauthorized());
    }
    let t: Terminal = s.query_row(&TERMINAL, &[&row.5])?;
    if row.2 <= p.now {
        end_row(s, &p.ctx, p.now, &t, &row, "expired", row.2)?;
        let mut e = completed(p, &row, 401, codes::UNAUTHORIZED, None);
        if p.op.cash() {
            let (id, reused) = outcome::reject_attempt(s, &p.ctx, &e, None, Some(&row.5))?;
            if reused {
                e.payload["attempt_id"] = id.into();
            }
        }
        outbox::record(s, &e)?;
        s.commit()?;
        return Ok(unauthorized());
    }
    if locked(s, p, &row)? {
        end_row(s, &p.ctx, p.now, &t, &row, "locked", p.now)?;
        let req = serde_json::from_slice::<Cash>(&p.body).ok();
        let amount = req.as_ref().and_then(|r| amount(&r.amount));
        let mut e = completed(p, &row, 422, codes::BANK_REJECTED, Some("409"));
        if p.op.cash() {
            let (id, reused) = outcome::reject_attempt(
                s,
                &p.ctx,
                &e,
                // Only a well-formed key fits the bank_tran_id column.
                req.as_ref()
                    .map(|r| r.idempotency_key.as_str())
                    .filter(|k| {
                        (1..=20).contains(&k.len()) && k.bytes().all(|c| (32..=126).contains(&c))
                    }),
                Some(&row.5),
            )?;
            if reused {
                e.payload["attempt_id"] = id.into();
            }
        }
        operation(s, p, &row, &t, amount, Some("409"))?;
        let mut completion = completed(p, &row, 422, codes::BANK_REJECTED, Some("409"));
        completion.payload = e.payload;
        if let Some(amount) = amount {
            completion.payload["tran_amt"] = amount.into();
        }
        outbox::record(s, &completion)?;
        s.commit()?;
        return Ok(ResultRow {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: codes::BANK_REJECTED,
            body: json!({"bank_rsp_code": "409", "bank_rsp_message": bank_message("409")}),
            replayed: false,
        });
    }
    if !p.op.cash() {
        let (_, balance, status): (i64, i64, String) = s.query_row(&ACCOUNT, &[&row.1])?;
        if status == "CLOSED" {
            return reject(s, p, &row, &t, None, "403");
        }
        operation(s, p, &row, &t, None, None)?;
        outbox::record(s, &completed(p, &row, 200, codes::SUCCESS, Some("000")))?;
        s.commit()?;
        return Ok(ResultRow {
            status: StatusCode::OK,
            code: codes::SUCCESS,
            body: json!({"balance": balance.to_string()}),
            replayed: false,
        });
    }
    let req = serde_json::from_slice::<Cash>(&p.body).ok();
    let valid = req.as_ref().and_then(|r| {
        let amount = amount(&r.amount)?;
        (r.idempotency_key.len() <= 20
            && !r.idempotency_key.is_empty()
            && r.idempotency_key.bytes().all(|c| (32..=126).contains(&c)))
        .then_some(amount)
    });
    let (Some(req), Some(amount)) = (req, valid) else {
        return Ok(ResultRow::error(
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMETER,
        ));
    };
    let scope = format!("ATM:{}", row.5);
    let request_hash = auth::hex(&Sha256::digest(
        format!("{}|{}|{}", p.op.name(), row.1, req.amount).as_bytes(),
    ));
    // A reused request ID takes precedence over replay, as in transfers.
    let claimed = outcome::claimed_elsewhere(s, &p.ctx.request_id, &scope, &req.idempotency_key)?;
    let insert = if claimed {
        false
    } else {
        match s.execute(
            &INSERT,
            &[
                &p.ctx.request_id,
                &p.op.kind(),
                &p.ctx.listener.as_str(),
                &scope,
                &req.idempotency_key,
                &request_hash,
                &req.idempotency_key,
                &row.5,
                &row.0,
                &p.ctx.device_id,
                &p.ctx.ip,
                &p.ctx.region,
                &p.now,
            ],
        ) {
            Ok(_) => true,
            Err(e) if e.ora_code() == Some(1) => false,
            Err(e) => return Err(e),
        }
    };
    if !insert {
        s.rollback()?;
        let stored: Option<Stored> = s.query_opt(&STORED, &[&scope, &req.idempotency_key])?;
        let claimed =
            outcome::claimed_elsewhere(s, &p.ctx.request_id, &scope, &req.idempotency_key)?;
        if !claimed
            && let Some((hash, status, code, body)) = stored
            && hash == request_hash
        {
            let stored: Value = body
                .as_deref()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or(Value::Null);
            return Ok(ResultRow {
                status: if status == "DONE" {
                    StatusCode::OK
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                },
                code: codes::known(&code).unwrap_or(codes::SYSTEM_ERROR),
                body: stored["body"].clone(),
                replayed: true,
            });
        }
        outcome::conflict(
            s,
            &p.ctx,
            p.now,
            p.op.kind(),
            "ATM",
            &req.idempotency_key,
            (None, Some(&row.0)),
            Some(&row.5),
        )?;
        return Ok(ResultRow::error(
            StatusCode::CONFLICT,
            codes::DUPLICATE_REQUEST,
        ));
    }
    let (account_id, _, _): (i64, i64, String) = s.query_row(&ACCOUNT, &[&row.1])?;
    let mut locks = ledger::lock_accounts(s, &[account_id])?;
    let account = locks
        .get(account_id)
        .cloned()
        .ok_or_else(|| DbError::Config("locked ATM account missing".into()))?;
    if account.status == "CLOSED" {
        return reject(s, p, &row, &t, Some(amount), "403");
    }
    let withdrawal = matches!(p.op, Operation::Withdraw);
    if withdrawal {
        if amount > p.limits.atm_withdraw {
            return reject(s, p, &row, &t, Some(amount), "406");
        }
        let daily = ledger::lock_daily(s, account_id, business_date(p.now))?;
        if daily.cash_out + amount > p.limits.atm_withdraw_daily {
            return reject(s, p, &row, &t, Some(amount), "406");
        }
        if account.balance < amount {
            return reject(s, p, &row, &t, Some(amount), "405");
        }
    }
    let movement = Movement {
        kind: Kind::AtmCash,
        terminal_id: Some(&row.5),
        payee_registered: None,
        request_id: Some(&p.ctx.request_id),
        ctx: Some(&p.ctx),
        bank_tran_id: Some(&req.idempotency_key),
        amount,
        channel: "ATM",
        occurred_at: p.now,
        legs: vec![Leg {
            customer_initiated: true,
            account_id,
            amount: if withdrawal { -amount } else { amount },
            inout_type: if withdrawal { "출금" } else { "입금" },
            tran_type: "현금",
            print_content: Some(if withdrawal { "ATM출금" } else { "ATM입금" }.into()),
            counterparty_bank_code: None,
            counterparty_account_no: None,
            counterparty_name: None,
            transfer_out: 0,
        }],
    };
    let posted = ledger::post(s, &mut locks, &movement).map_err(|e| match e {
        LedgerError::Db(e) => e,
        e => DbError::Config(e.to_string()),
    })?;
    let body = json!({"amount": req.amount, "balance_after": posted.balances_after[0].1.to_string(), "account_sequence": account.next_sequence.to_string()});
    store(s, p, &body, None)?;
    operation(s, p, &row, &t, Some(amount), None)?;
    let mut e = completed(p, &row, 200, codes::SUCCESS, Some("000"));
    e.payload["tran_amt"] = amount.into();
    e.payload["transfer_id"] = posted.transfer_id.into();
    outbox::record(s, &e)?;
    s.commit()?;
    Ok(ResultRow {
        status: StatusCode::OK,
        code: codes::SUCCESS,
        body,
        replayed: false,
    })
}
async fn run(state: AppState, stats: RequestStats, p: Plan) -> Result<Response, ApiError> {
    let p = std::sync::Arc::new(p);
    let mut retries = 0;
    loop {
        let plan = p.clone();
        match state.db.run_in(&stats, move |s| attempt(s, &plan)).await {
            Ok(r) => {
                let message = match r.code {
                    codes::SUCCESS => "",
                    codes::BANK_REJECTED => "참가기관 거부",
                    codes::INVALID_PARAMETER => "요청 항목 오류",
                    codes::UNAUTHORIZED => "인증 실패",
                    codes::DUPLICATE_REQUEST => "중복 요청",
                    _ => "시스템 오류",
                };
                let mut response = api::respond(&p.ctx, r.status, r.code, message, r.body);
                outcome::annotate(
                    &mut response,
                    None,
                    Some(p.op.kind().into()),
                    r.replayed.then_some(true),
                );
                return Ok(response);
            }
            Err(e) if e.ora_code() == Some(60) && retries < 2 => {
                retries += 1;
                metrics::counter!("bank_deadlock_retries_total").increment(1);
                tokio::time::sleep(std::time::Duration::from_millis(10 * retries)).await;
            }
            Err(e) => {
                return Err(
                    if matches!(e.ora_code(), Some(30006 | 1653 | 1654 | 1691)) {
                        ApiError::busy(&p.ctx)
                    } else {
                        db_error(&p.ctx, e)
                    },
                );
            }
        }
    }
}
fn token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(auth::token_hash)
}
macro_rules! cash_handler {
    ($name:ident, $op:expr) => {
        pub(super) async fn $name(
            BusinessTime(now): BusinessTime,
            State(state): State<AppState>,
            ctx: RequestContext,
            Extension(stats): Extension<RequestStats>,
            Extension(identity): Extension<IdentitySlot>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Result<Response, ApiError> {
            let limits = state.limits;
            run(
                state,
                stats,
                Plan {
                    ctx,
                    now,
                    hash: token(&headers),
                    identity,
                    op: $op,
                    body,
                    limits,
                },
            )
            .await
        }
    };
}
cash_handler!(withdraw, Operation::Withdraw);
cash_handler!(deposit, Operation::Deposit);
pub(super) async fn balance(
    BusinessTime(now): BusinessTime,
    State(state): State<AppState>,
    ctx: RequestContext,
    Extension(stats): Extension<RequestStats>,
    Extension(identity): Extension<IdentitySlot>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let limits = state.limits;
    run(
        state,
        stats,
        Plan {
            ctx,
            now,
            hash: token(&headers),
            identity,
            op: Operation::Balance,
            body: Bytes::new(),
            limits,
        },
    )
    .await
}
