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
use crate::shared::outcome::IdentitySlot;

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
    let identity = req
        .extensions_mut()
        .get_or_insert_default::<IdentitySlot>()
        .clone();

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
    let session_id = identity
        .lock()
        .ok()
        .and_then(|g| g.session_id.clone())
        .or_else(|| s.pending_touch.as_ref().map(|p| p.session_id.clone()))
        .unwrap_or_default();
    tracing::info!(
        target: "request",
        request_id,
        session_id,
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

/// Parses `VmRSS` and `RssAnon` (in bytes) from Linux `/proc/self/status` text.
fn parse_proc_status_memory(text: &str) -> (Option<u64>, Option<u64>) {
    let mut vm_rss = None;
    let mut rss_anon = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            vm_rss = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .map(|kb| kb.saturating_mul(1024));
        } else if let Some(rest) = line.strip_prefix("RssAnon:") {
            rss_anon = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .map(|kb| kb.saturating_mul(1024));
        }
    }
    (vm_rss, rss_anon)
}

/// Updates process memory gauges from `/proc/self/status` on Linux; no-op elsewhere.
pub fn sample_process_memory() {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return;
    };
    let (vm_rss, rss_anon) = parse_proc_status_memory(&text);
    if let Some(bytes) = vm_rss {
        metrics::gauge!("bank_process_resident_memory_bytes").set(bytes as f64);
    }
    if let Some(bytes) = rss_anon {
        metrics::gauge!("bank_process_anon_memory_bytes").set(bytes as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_proc_status_memory;

    #[test]
    fn parses_vm_rss_and_rss_anon_in_bytes() {
        let sample = "\
Name:\tbank
VmSize:\t 1309084 kB
VmRSS:\t   20616 kB
RssAnon:\t    9192 kB
RssFile:\t   11424 kB
";
        assert_eq!(
            parse_proc_status_memory(sample),
            (Some(20_616 * 1024), Some(9_192 * 1024))
        );
        assert_eq!(parse_proc_status_memory("Name:\tbank\n"), (None, None));
    }
}
