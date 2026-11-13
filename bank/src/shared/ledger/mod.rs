//! The ledger (spec F-20 to F-24): the only code that writes account balances,
//! transaction rows, `TRANSFER` records and daily totals. Single-entry: each
//! movement writes one row per in-bank account it affects.
//!
//! Callers lock every account an operation touches with [`lock_accounts`]
//! (ascending account ID, deadlock-free), validate, then call [`post`] inside
//! the same database transaction.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate};

use oracle::sql_type::ToSql;

use crate::shared::clock::business_date;
use crate::shared::context::RequestContext;
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::shared::outbox::{self, Event};
use crate::sql_stmt;
use serde_json::json;

const LOCK_ACCOUNTS: SqlStmt = sql_stmt!("ledger.lock_accounts", "sql/lock_accounts.sql");
const INSERT_TRANSFER: SqlStmt = sql_stmt!("ledger.insert_transfer", "sql/insert_transfer.sql");
const UPDATE_STATUS: SqlStmt = sql_stmt!("ledger.update_status", "sql/update_status.sql");
const INSERT_STATUS: SqlStmt = sql_stmt!("ledger.insert_status", "sql/insert_status.sql");
const INSERT_ROW: SqlStmt = sql_stmt!("ledger.insert_row", "sql/insert_row.sql");
const UPDATE_ACCOUNT: SqlStmt = sql_stmt!("ledger.update_account", "sql/update_account.sql");
const LOCK_DAILY: SqlStmt = sql_stmt!("ledger.lock_daily", "sql/lock_daily.sql");
const ADD_DAILY: SqlStmt = sql_stmt!("ledger.add_daily", "sql/add_daily.sql");

/// A locked account's current state.
#[derive(Debug, Clone)]
pub struct Locked {
    pub account_no: String,
    pub balance: i64,
    pub next_sequence: i64,
    pub status: String,
    pub subtype: String,
}

/// The accounts locked by one operation, keyed by account ID.
#[derive(Debug, Default)]
pub struct LockSet {
    accounts: BTreeMap<i64, Locked>,
}

impl LockSet {
    pub fn get(&self, account_id: i64) -> Option<&Locked> {
        self.accounts.get(&account_id)
    }
}

/// Locks the accounts in ascending ID order (`SELECT ... FOR UPDATE WAIT 1`).
pub fn lock_accounts(s: &mut Session, account_ids: &[i64]) -> Result<LockSet, DbError> {
    let mut ids = account_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut set = LockSet::default();
    // Two per round trip, ascending: the time the first lock is held is what queues waiters.
    for pair in ids.chunks(2) {
        let first = pair[0];
        let second = pair.get(1).copied().unwrap_or(first);
        let rows: Vec<(i64, i64, i64, String, String, String)> =
            s.query_all(&LOCK_ACCOUNTS, &[&first, &second])?;
        if rows.len() != pair.len() {
            return Err(DbError::Config("account to lock does not exist".into()));
        }
        for (id, balance, next_sequence, status, subtype, account_no) in rows {
            set.accounts.insert(
                id,
                Locked {
                    account_no,
                    balance,
                    next_sequence,
                    status,
                    subtype,
                },
            );
        }
    }
    Ok(set)
}

/// Daily totals of a locked account for a business date.
#[derive(Debug, Clone, Copy, Default)]
pub struct Daily {
    pub transfer_out: i64,
    pub cash_in: i64,
    pub cash_out: i64,
}

/// Reads the daily totals of an account that the caller has already locked;
/// that lock is what keeps the totals from changing.
pub fn lock_daily(s: &mut Session, account_id: i64, date: NaiveDate) -> Result<Daily, DbError> {
    let (transfer_out, cash_in, cash_out) = s.query_row(&LOCK_DAILY, &[&account_id, &date])?;
    Ok(Daily {
        transfer_out,
        cash_in,
        cash_out,
    })
}

/// The kind of a `TRANSFER` record (spec F-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Transfer,
    Inbound,
    TermOpen,
    TermClose,
    LoanDisburse,
    LoanRepay,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Transfer => "transfer",
            Kind::Inbound => "inbound",
            Kind::TermOpen => "term_open",
            Kind::TermClose => "term_close",
            Kind::LoanDisburse => "loan_disburse",
            Kind::LoanRepay => "loan_repay",
        }
    }
}

/// One in-bank account's side of a movement.
#[derive(Debug, Clone)]
pub struct Leg {
    pub customer_initiated: bool,
    pub account_id: i64,
    /// Signed: positive credits the account, negative debits it.
    pub amount: i64,
    pub inout_type: &'static str,
    pub tran_type: &'static str,
    pub print_content: Option<String>,
    pub counterparty_bank_code: Option<String>,
    pub counterparty_account_no: Option<String>,
    pub counterparty_name: Option<String>,
    /// Counts against the daily transfer limit.
    pub transfer_out: i64,
}

/// A movement to post.
#[derive(Debug, Clone)]
pub struct Movement<'a> {
    pub kind: Kind,
    pub request_id: Option<&'a str>,
    pub ctx: Option<&'a RequestContext>,
    pub bank_tran_id: Option<&'a str>,
    pub amount: i64,
    pub channel: &'static str,
    pub occurred_at: DateTime<FixedOffset>,
    pub legs: Vec<Leg>,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("invalid movement shape: {0}")]
    Shape(&'static str),
    #[error("account {0} was not locked by this operation")]
    NotLocked(i64),
    #[error("account {0} would go below zero")]
    Negative(i64),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// The posted movement: its transfer ID and each leg's balance after.
#[derive(Debug, Clone)]
pub struct Posted {
    pub transfer_id: i64,
    pub balances_after: Vec<(i64, i64)>,
}

/// Writes the movement: `TRANSFER`, one row per leg, balances, sequences and
/// daily totals. Every leg's account must be in `locks`.
pub fn post(s: &mut Session, locks: &mut LockSet, m: &Movement<'_>) -> Result<Posted, LedgerError> {
    check_shape(
        m.kind,
        m.amount,
        &m.legs.iter().map(|leg| leg.amount).collect::<Vec<_>>(),
    )?;
    for leg in &m.legs {
        let locked = locks
            .accounts
            .get(&leg.account_id)
            .ok_or(LedgerError::NotLocked(leg.account_id))?;
        if locked.balance + leg.amount < 0 {
            return Err(LedgerError::Negative(leg.account_id));
        }
    }
    let transfer_id = s.insert_returning_id(
        &INSERT_TRANSFER,
        &[
            &m.kind.as_str(),
            &m.request_id,
            &m.bank_tran_id,
            &m.amount,
            &m.occurred_at,
        ],
    )?;
    let date = business_date(m.occurred_at);
    let mut balances_after = Vec::with_capacity(m.legs.len());
    let no_terminal = None::<String>;
    let no_cash = 0i64;
    let mut sequences = Vec::with_capacity(m.legs.len());
    let mut flags = Vec::with_capacity(m.legs.len());
    for leg in &m.legs {
        let locked = locks
            .accounts
            .get_mut(&leg.account_id)
            .ok_or(LedgerError::NotLocked(leg.account_id))?;
        sequences.push((locked.next_sequence, locked.next_sequence + 1));
        flags.push(i64::from(leg.customer_initiated));
        locked.balance += leg.amount;
        locked.next_sequence += 1;
        balances_after.push((leg.account_id, locked.balance));
    }
    // One round trip per table: the account locks are held until the caller commits.
    let mut rows: Vec<Vec<&dyn ToSql>> = Vec::with_capacity(m.legs.len());
    let mut updates: Vec<Vec<&dyn ToSql>> = Vec::with_capacity(m.legs.len());
    for (i, leg) in m.legs.iter().enumerate() {
        rows.push(vec![
            &leg.account_id,
            &sequences[i].0,
            &leg.amount,
            &balances_after[i].1,
            &leg.inout_type,
            &leg.tran_type,
            &leg.print_content,
            &leg.counterparty_bank_code,
            &leg.counterparty_account_no,
            &leg.counterparty_name,
            &m.channel,
            &no_terminal,
            &m.request_id,
            &transfer_id,
            &date,
            &m.occurred_at,
        ]);
        updates.push(vec![
            &balances_after[i].1,
            &sequences[i].1,
            &flags[i],
            &m.occurred_at,
            &leg.account_id,
        ]);
    }
    s.execute_batch(&INSERT_ROW, &rows)?;
    s.execute_batch(&UPDATE_ACCOUNT, &updates)?;
    for leg in &m.legs {
        if leg.transfer_out != 0 {
            s.execute(
                &ADD_DAILY,
                &[
                    &leg.account_id,
                    &date,
                    &leg.transfer_out,
                    &no_cash,
                    &no_cash,
                ],
            )?;
        }
    }
    for leg in &m.legs {
        if leg.customer_initiated
            && locks
                .get(leg.account_id)
                .is_some_and(|a| a.status == "DORMANT")
        {
            set_status(
                s,
                locks,
                leg.account_id,
                "ACTIVE",
                "customer_activity",
                m.occurred_at,
                m.ctx,
                m.channel,
            )?;
        }
    }
    Ok(Posted {
        transfer_id,
        balances_after,
    })
}

/// Checks F-21 without accessing a database. Use wide sums to avoid overflow.
fn check_shape(kind: Kind, amount: i64, amounts: &[i64]) -> Result<(), LedgerError> {
    let offsets = amounts.len() == 2 && i128::from(amounts[0]) + i128::from(amounts[1]) == 0;
    let valid = match kind {
        Kind::Transfer => (amounts.len() == 1 && amounts[0] < 0) || offsets,
        Kind::Inbound => amounts.len() == 1 && amounts[0] > 0,
        Kind::TermOpen | Kind::TermClose => offsets,
        Kind::LoanDisburse => amounts.len() == 2 && amounts[0] > 0 && amounts[0] == amounts[1],
        Kind::LoanRepay => amounts.len() == 2 && amounts[0] < 0 && amounts[0] == amounts[1],
    };
    if valid
        && amounts
            .iter()
            .all(|leg| *leg != 0 && i128::from(*leg).abs() == i128::from(amount))
    {
        Ok(())
    } else {
        Err(LedgerError::Shape(
            "leg count, signs or amounts do not match kind",
        ))
    }
}

/// Changes a locked account's status, history and event atomically. Never commits.
#[allow(clippy::too_many_arguments)]
pub fn set_status(
    s: &mut Session,
    locks: &mut LockSet,
    account_id: i64,
    to: &'static str,
    reason: &'static str,
    at: DateTime<FixedOffset>,
    ctx: Option<&RequestContext>,
    channel: &'static str,
) -> Result<(), LedgerError> {
    let account = locks
        .accounts
        .get_mut(&account_id)
        .ok_or(LedgerError::NotLocked(account_id))?;
    if account.status == to {
        return Ok(());
    }
    s.execute(&UPDATE_STATUS, &[&to, &account_id])?;
    s.execute(
        &INSERT_STATUS,
        &[&account_id, &account.status, &to, &reason, &at],
    )?;
    let event = match ctx {
        Some(ctx) => Event::for_request(ctx, "account.status_changed", at, channel, "completed"),
        None => Event::system("account.status_changed", at, "completed"),
    }
    .account(&account.account_no)
    .payload(json!({"from": account.status, "to": to, "reason": reason}));
    outbox::record(s, &event)?;
    account.status = to.to_owned();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_checks_its_shape() {
        for (kind, accepted, rejected) in [
            (Kind::Transfer, vec![-10, 10], vec![-10, 9]),
            (Kind::Inbound, vec![10], vec![-10]),
            (Kind::TermOpen, vec![-10, 10], vec![10]),
            (Kind::TermClose, vec![10, -10], vec![10, 10]),
            (Kind::LoanDisburse, vec![10, 10], vec![-10, -10]),
            (Kind::LoanRepay, vec![-10, -10], vec![10, 10]),
        ] {
            assert!(check_shape(kind, 10, &accepted).is_ok(), "{kind:?}");
            assert!(
                matches!(check_shape(kind, 10, &rejected), Err(LedgerError::Shape(_))),
                "{kind:?}"
            );
            assert!(check_shape(kind, 10, &[0]).is_err());
            assert!(check_shape(kind, 10, &[0, 0]).is_err());
            assert!(check_shape(kind, 11, &accepted).is_err());
            assert!(check_shape(kind, 10, &[]).is_err(), "{kind:?}");
            assert!(check_shape(kind, 10, &[1, 1, 1]).is_err(), "{kind:?}");
        }
        assert!(check_shape(Kind::Transfer, 10, &[-10]).is_ok());
        assert!(check_shape(Kind::Transfer, 10, &[10]).is_err());
        for kind in [Kind::LoanDisburse, Kind::LoanRepay] {
            assert!(check_shape(kind, 10, &[0, 0]).is_err());
            assert!(check_shape(kind, 10, &[10, 9]).is_err());
            assert!(check_shape(kind, 10, &[-10, -9]).is_err());
        }
        assert!(check_shape(Kind::Inbound, 10, &[0]).is_err());
        assert!(check_shape(Kind::TermOpen, 10, &[i64::MAX, 1]).is_err());
    }
}
