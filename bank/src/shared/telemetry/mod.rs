//! Operational telemetry (spec N-12): `app.log` as JSON lines with one line
//! per request, rotated daily with 7 files kept, and Prometheus metrics served
//! on the metrics listener. SQL text appears only at debug level.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;

use crate::shared::context::{Listener, RequestContext};
use crate::shared::db::StatementStats;

/// Statement statistics collected for one HTTP request.
pub type RequestStats = Arc<Mutex<StatementStats>>;

const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.2, 0.3, 0.5, 1.0, 2.0, 5.0,
];

/// Installs the JSON `app.log` writer. Keep the guard alive for the process.
pub fn init_logging(log_dir: &str) -> anyhow::Result<WorkerGuard> {
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("app")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(false)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .init();
    Ok(guard)
}

/// Installs the global Prometheus recorder once and returns its handle.
pub fn metrics_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .set_buckets_for_metric(Matcher::Suffix("_seconds".into()), LATENCY_BUCKETS)
                .expect("valid buckets")
                .install_recorder()
                .expect("install the Prometheus recorder once")
        })
        .clone()
}

/// Middleware: one `app.log` line and one latency observation per request.
pub async fn request_log(listener: Listener, mut req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let endpoint = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let request_id = req
        .extensions()
        .get::<RequestContext>()
        .map(|c| c.request_id.clone())
        .unwrap_or_default();
    let stats: RequestStats = Arc::default();
    req.extensions_mut().insert(stats.clone());

    let res = next.run(req).await;

    let elapsed = started.elapsed();
    let status = res.status().as_u16();
    metrics::histogram!(
        "bank_http_request_duration_seconds",
        "listener" => listener.as_str(),
        "endpoint" => endpoint.clone(),
        "status" => status.to_string()
    )
    .record(elapsed.as_secs_f64());
    let s = stats.lock().map(|g| g.clone()).unwrap_or_default();
    tracing::info!(
        target: "request",
        request_id,
        listener = listener.as_str(),
        method = %method,
        endpoint,
        status,
        duration_ms = elapsed.as_secs_f64() * 1000.0,
        statements = s.count,
        slowest_statement = s.slowest.map(|(n, _)| n).unwrap_or(""),
        slowest_ms = s.slowest.map(|(_, d)| d.as_secs_f64() * 1000.0).unwrap_or(0.0),
    );
    res
}
