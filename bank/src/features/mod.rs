//! Banking features. Each feature exposes only its router; everything else in
//! a feature is private to it, so one feature cannot use another's internals.

pub mod accounts;
pub mod auth;
pub mod internal;
