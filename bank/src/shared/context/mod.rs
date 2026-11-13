//! Per-request context: request ID, listener, source IP, region, device and
//! whether the session is human (spec F-31, F-34, Simulated context headers).
//! Simulated-context headers are trusted only on the generator listener.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset};
use ulid::Ulid;

use crate::app::AppState;
use crate::shared::api::ApiError;
use crate::shared::db::DbError;

pub const REQUEST_ID: &str = "x-request-id";
const SIM_IP: &str = "x-sim-ip";
const SIM_REGION: &str = "x-sim-region";
const SIM_DEVICE: &str = "x-sim-device";
const CF_IP: &str = "cf-connecting-ip";
const CF_COUNTRY: &str = "cf-ipcountry";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Listener {
    Public,
    Generator,
}

impl Listener {
    pub fn event_label(self) -> &'static str {
        match self {
            Listener::Public => "public",
            Listener::Generator => "generator",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Listener::Public => "PUBLIC",
            Listener::Generator => "GENERATOR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RequestContext {
    pub request_id: String,
    pub listener: Listener,
    pub ip: Option<String>,
    pub region: Option<String>,
    pub device_id: Option<String>,
    pub human: bool,
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .map(str::to_owned)
}

impl RequestContext {
    /// Builds the context from request headers and the peer address.
    pub fn from_headers(listener: Listener, headers: &HeaderMap, peer: Option<SocketAddr>) -> Self {
        // Not `unwrap_or_default`: `Ulid::default()` is the nil ULID.
        let request_id = match header(headers, REQUEST_ID).and_then(|v| Ulid::from_string(&v).ok())
        {
            Some(id) => id,
            None => Ulid::new(),
        }
        .to_string();
        let peer_ip = peer.map(|p| p.ip().to_string());
        match listener {
            Listener::Generator => Self {
                request_id,
                listener,
                ip: header(headers, SIM_IP).or(peer_ip),
                region: header(headers, SIM_REGION),
                device_id: header(headers, SIM_DEVICE),
                human: false,
            },
            Listener::Public => Self {
                request_id,
                listener,
                ip: header(headers, CF_IP).or(peer_ip),
                region: header(headers, CF_COUNTRY),
                device_id: None,
                human: true,
            },
        }
    }
}

/// Middleware: attaches a [`RequestContext`] and echoes `X-Request-Id`.
pub async fn attach(listener: Listener, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ctx = RequestContext::from_headers(listener, req.headers(), peer);
    let request_id = ctx.request_id.clone();
    req.extensions_mut()
        .insert(crate::shared::outcome::Arrival(std::time::Instant::now()));
    req.extensions_mut().insert(ctx);
    let mut res = next.run(req).await;
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        res.headers_mut().insert(REQUEST_ID, v);
    }
    res
}

impl<S: Send + Sync> FromRequestParts<S> for RequestContext {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<RequestContext>()
            .cloned()
            .unwrap_or_else(|| {
                RequestContext::from_headers(Listener::Public, &parts.headers, None)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).expect("header"));
        }
        h
    }

    #[test]
    fn generator_listener_trusts_simulated_context() {
        let id = Ulid::new().to_string();
        let h = headers(&[
            (REQUEST_ID, &id),
            (SIM_IP, "203.0.113.7"),
            (SIM_REGION, "KR-11"),
            (SIM_DEVICE, "d-1"),
        ]);
        let ctx = RequestContext::from_headers(Listener::Generator, &h, None);
        assert_eq!(ctx.request_id, id);
        assert_eq!(ctx.ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(ctx.region.as_deref(), Some("KR-11"));
        assert_eq!(ctx.device_id.as_deref(), Some("d-1"));
        assert!(!ctx.human);
    }

    #[test]
    fn public_listener_ignores_simulated_context_and_is_human() {
        let h = headers(&[
            (SIM_IP, "203.0.113.7"),
            (SIM_REGION, "US"),
            (SIM_DEVICE, "d-1"),
            (CF_IP, "198.51.100.2"),
            (CF_COUNTRY, "KR"),
        ]);
        let ctx = RequestContext::from_headers(Listener::Public, &h, None);
        assert_eq!(ctx.ip.as_deref(), Some("198.51.100.2"));
        assert_eq!(ctx.region.as_deref(), Some("KR"));
        assert_eq!(ctx.device_id, None);
        assert!(ctx.human);
    }

    #[test]
    fn invalid_request_id_is_replaced_with_a_new_ulid() {
        let ctx = RequestContext::from_headers(
            Listener::Generator,
            &headers(&[(REQUEST_ID, "not-a-ulid")]),
            None,
        );
        assert!(Ulid::from_string(&ctx.request_id).is_ok());
        assert_ne!(ctx.request_id, "not-a-ulid");
    }
}

/// One immutable business time for authentication and the entire operation.
#[derive(Clone, Copy)]
pub struct BusinessTime(pub DateTime<FixedOffset>);

impl<S: Send + Sync> FromRequestParts<S> for BusinessTime {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ctx = RequestContext::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|e| match e {});
        parts
            .extensions
            .get::<Self>()
            .copied()
            .ok_or_else(|| ApiError::internal(&ctx))
    }
}

pub async fn operation(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let ctx = req
        .extensions()
        .get::<RequestContext>()
        .cloned()
        .unwrap_or_else(|| RequestContext::from_headers(Listener::Public, req.headers(), None));
    let recorder = crate::shared::outcome::Recorder::attach(&mut req, ctx.clone());
    let (parts, body) = req.into_parts();
    let read = tokio::time::timeout(Duration::from_secs(10), to_bytes(body, 1_048_576)).await;
    let (bytes, rejection) = match read {
        Ok(Ok(bytes)) => (bytes, None),
        Ok(Err(_)) => (
            Default::default(),
            Some(
                ApiError::new(&ctx, StatusCode::PAYLOAD_TOO_LARGE, "A0002", "invalid body")
                    .into_response(),
            ),
        ),
        Err(_) => (
            Default::default(),
            Some(
                ApiError::new(&ctx, StatusCode::REQUEST_TIMEOUT, "A0002", "body timeout")
                    .into_response(),
            ),
        ),
    };
    req = Request::from_parts(parts, Body::from(bytes));
    let ticket = match state.clock.enter().await {
        Ok(ticket) => ticket,
        Err(error) => {
            recorder.unrecorded();
            return if rejection.is_some() {
                ApiError::busy(&ctx).into_response()
            } else {
                match error {
                    DbError::Busy => ApiError::busy(&ctx).into_response(),
                    _ => ApiError::internal(&ctx).into_response(),
                }
            };
        }
    };
    let now = ticket.now;
    req.extensions_mut().insert(BusinessTime(now));
    let task_recorder = recorder.clone();
    let task_state = state.clone();
    // Detach on cancellation: blocking database work must retain the read guard.
    match tokio::spawn(async move {
        let response = match rejection {
            Some(response) => response,
            None => next.run(req).await,
        };
        let response = task_recorder.finish(&task_state, now, response).await;
        drop(ticket);
        response
    })
    .await
    {
        Ok(response) => response,
        Err(_) => {
            recorder
                .finish(&state, now, ApiError::internal(&ctx).into_response())
                .await
        }
    }
}
