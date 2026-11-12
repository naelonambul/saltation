//! Router composition per listener (spec: Bank listeners).
//! - public: customer and admin routes, reached through the tunnel
//! - generator: everything public plus internal (and later ATM, inbound)
//! - metrics: `/metrics` only

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::get;
use metrics_exporter_prometheus::PrometheusHandle;

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

pub fn public_router(state: AppState) -> Router {
    with_layers(public_routes(), Listener::Public, state)
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
