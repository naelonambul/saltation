//! Banking features. Each feature exposes only its router; everything else in
//! a feature is private to it, so one feature cannot use another's internals.

pub mod accounts;
pub mod accounts_lifecycle;
pub mod auth;
pub mod internal;
pub mod outbox_relay;
pub mod payees;
pub mod transfers;

pub mod inbound;

pub mod password_reset;

pub mod deposits;

pub mod loans;

pub mod daily_close;
