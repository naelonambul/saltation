//! The ledger (spec F-20 to F-24): the only code that writes account balances,
//! transaction rows, `TRANSFER` records and daily totals. Single-entry: each
//! movement writes one row per in-bank account it affects.
//!
//! Callers lock every account an operation touches with [`lock_accounts`]
//! (ascending account ID, deadlock-free), validate, then call [`post`] inside
//! the same database transaction.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate};

use crate::shared::clock::business_date;
use crate::shared::db::{DbError, Session, SqlStmt};
use crate::sql_stmt;

const LOCK_ACCOUNT: SqlStmt = sql_stmt!("ledger.lock_account", "sql/lock_account.sql");
const INSERT_TRANSFER: SqlStmt = sql_stmt!("ledger.insert_transfer", "sql/insert_transfer.sql");
const SELECT_TRANSFER_ID: SqlStmt =
    sql_stmt!("ledger.select_transfer_id", "sql/select_transfer_id.sql");
const INSERT_ROW: SqlStmt = sql_stmt!("ledger.insert_row", "sql/insert_row.sql");
const UPDATE_ACCOUNT: SqlStmt = sql_stmt!("ledger.update_account", "sql/update_account.sql");
const ENSURE_DAILY: SqlStmt = sql_stmt!("ledger.ensure_daily", "sql/ensure_daily.sql");
const LOCK_DAILY: SqlStmt = sql_stmt!("ledger.lock_daily", "sql/lock_daily.sql");
const ADD_DAILY: SqlStmt = sql_stmt!("ledger.add_daily", "sql/add_daily.sql");

/// A locked account's current state.
#[derive(Debug, Clone)]
pub struct Locked {
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
    for id in ids {
        let (balance, next_sequence, status, subtype): (i64, i64, String, String) =
            s.query_row(&LOCK_ACCOUNT, &[&id])?;
        set.accounts.insert(
            id,
            Locked {
                balance,
                next_sequence,
                status,
                subtype,
            },
        );
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

/// Reads (creating if needed) and locks the daily totals of an account that
/// the caller has already locked.
pub fn lock_daily(s: &mut Session, account_id: i64, date: NaiveDate) -> Result<Daily, DbError> {
    s.execute(&ENSURE_DAILY, &[&account_id, &date])?;
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
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Transfer => "transfer",
            Kind::Inbound => "inbound",
        }
    }
}

/// One in-bank account's side of a movement.
#[derive(Debug, Clone)]
pub struct Leg {
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
    pub request_id: &'a str,
    pub bank_tran_id: Option<&'a str>,
    pub amount: i64,
    pub channel: &'static str,
    pub occurred_at: DateTime<FixedOffset>,
    pub legs: Vec<Leg>,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
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
    for leg in &m.legs {
        let locked = locks
            .accounts
            .get(&leg.account_id)
            .ok_or(LedgerError::NotLocked(leg.account_id))?;
        if locked.balance + leg.amount < 0 {
            return Err(LedgerError::Negative(leg.account_id));
        }
    }
    s.execute(
        &INSERT_TRANSFER,
        &[
            &m.kind.as_str(),
            &m.request_id,
            &m.bank_tran_id,
            &m.amount,
            &m.occurred_at,
        ],
    )?;
    let transfer_id: i64 = s.query_row(&SELECT_TRANSFER_ID, &[&m.request_id])?;
    let date = business_date(m.occurred_at);
    let mut balances_after = Vec::with_capacity(m.legs.len());
    for leg in &m.legs {
        let locked = locks
            .accounts
            .get_mut(&leg.account_id)
            .ok_or(LedgerError::NotLocked(leg.account_id))?;
        let sequence = locked.next_sequence;
        let balance = locked.balance + leg.amount;
        s.execute(
            &INSERT_ROW,
            &[
                &leg.account_id,
                &sequence,
                &leg.amount,
                &balance,
                &leg.inout_type,
                &leg.tran_type,
                &leg.print_content,
                &leg.counterparty_bank_code,
                &leg.counterparty_account_no,
                &leg.counterparty_name,
                &m.channel,
                &None::<String>,
                &m.request_id,
                &transfer_id,
                &date,
                &m.occurred_at,
            ],
        )?;
        s.execute(
            &UPDATE_ACCOUNT,
            &[&balance, &(sequence + 1), &m.occurred_at, &leg.account_id],
        )?;
        locked.balance = balance;
        locked.next_sequence = sequence + 1;
        if leg.transfer_out != 0 {
            s.execute(&ENSURE_DAILY, &[&leg.account_id, &date])?;
            s.execute(
                &ADD_DAILY,
                &[&leg.transfer_out, &0i64, &0i64, &leg.account_id, &date],
            )?;
        }
        balances_after.push((leg.account_id, balance));
    }
    Ok(Posted {
        transfer_id,
        balances_after,
    })
}
