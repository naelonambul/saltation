//! Router composition per listener (spec: Bank listeners).
//! - public: customer and admin routes, reached through the tunnel
//! - generator: everything public plus internal (and later ATM, inbound)
//! - metrics: `/metrics` only

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method};
use axum::middleware::from_fn;
use axum::routing::get;
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::features;
use crate::shared::clock::Clock;
use crate::shared::context::{self, Listener};
use crate::shared::db::Db;
use crate::shared::telemetry;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub clock: Clock,
}

fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(features::auth::router())
        .merge(features::accounts::router())
        .merge(features::transfers::router())
}

fn with_layers(router: Router<AppState>, listener: Listener, state: AppState) -> Router {
    router
        .layer(from_fn(move |req, next| {
            telemetry::request_log(listener, req, next)
        }))
        .layer(from_fn(move |req, next| {
            context::attach(listener, req, next)
        }))
        .with_state(state)
}

/// CORS for the public listener (spec: Runtime topology): only the SPA
/// origins listed in `SALTATION_CORS_ORIGINS` (comma-separated).
fn cors() -> CorsLayer {
    let origins: Vec<HeaderValue> = std::env::var("SALTATION_CORS_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            HeaderName::from_static("authorization"),
            HeaderName::from_static("content-type"),
            HeaderName::from_static("x-request-id"),
        ])
        .expose_headers([HeaderName::from_static("x-request-id")])
}

pub fn public_router(state: AppState) -> Router {
    with_layers(public_routes(), Listener::Public, state).layer(cors())
}

pub fn generator_router(state: AppState) -> Router {
    with_layers(
        public_routes().merge(features::internal::router()),
        Listener::Generator,
        state,
    )
}

pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route("/metrics", get(move || std::future::ready(handle.render())))
}
