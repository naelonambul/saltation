//! Real relay metrics keep sampling during Mongo backoff and after a drain.
use super::*;

const BACKLOG: SqlStmt = sql_stmt!(
    "outbox.select_backlog",
    "../../src/shared/outbox/sql/select_backlog.sql"
);
const PREVIOUS: SqlStmt = sql_stmt!(
    "test.outbox_tune_previous",
    "../sql/accounts_tune_previous.sql"
);
const ACCESS: SqlStmt = sql_stmt!("test.outbox_tune_access", "../sql/outbox_tune_access.sql");

fn gauge(handle: &metrics_exporter_prometheus::PrometheusHandle, name: &str) -> Option<f64> {
    handle
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')?.parse().ok())
}

async fn wait_count(handle: &metrics_exporter_prometheus::PrometheusHandle, count: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while gauge(handle, "bank_outbox_undelivered") != Some(count as f64) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("backlog gauge follows the database while delivery is backing off");
}

#[tokio::test]
async fn oldest_event_uses_index_only_minmax() {
    let api = api().await;
    login_attempts(&api, 3).await;
    api.state
        .db
        .run(|s| {
            let (count, age): (i64, f64) = s.query_row(&BACKLOG, &[])?;
            assert!(count >= 6 && age >= 0.0);
            let (id, child): (String, i64) = s.query_row(&PREVIOUS, &[])?;
            let access: Vec<(String, Option<String>)> = s.query_all(&ACCESS, &[&id, &child])?;
            assert!(
                access
                    .iter()
                    .any(|(operation, options)| operation == "INDEX"
                        && options
                            .as_ref()
                            .is_some_and(|options| options.contains("MIN/MAX"))),
                "{access:?}"
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn backlog_gauges_change_while_delivery_is_unavailable_and_after_drain() {
    let api = api().await;
    let events = collection().await;
    drain(&api, &events).await;
    login_attempts(&api, 3).await;
    let count = outbox_count(&api).await;
    let handle = bank::shared::telemetry::metrics_handle();
    let relay = tokio::spawn(outbox_relay::run(
        api.state.db.clone(),
        "mongodb://127.0.0.1:1/bank_logs?serverSelectionTimeoutMS=100&connectTimeoutMS=100".into(),
    ));
    wait_count(&handle, count).await;
    assert!(gauge(&handle, "bank_outbox_oldest_age_seconds").is_some_and(|age| age >= 0.0));
    drain(&api, &events).await;
    wait_count(&handle, 0).await;
    assert_eq!(gauge(&handle, "bank_outbox_oldest_age_seconds"), Some(0.0));
    login_attempts(&api, 2).await;
    wait_count(&handle, outbox_count(&api).await).await;
    relay.abort();
    assert!(relay.await.unwrap_err().is_cancelled());
    drain(&api, &events).await;
}
