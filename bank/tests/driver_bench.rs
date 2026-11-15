//! Driver-independent benchmark. One ignored workload per fresh test process.
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use axum::{Router, body::Body, http::Request};
use bank::app::{self, AppState};
use bank::features::outbox_relay;
use bank::shared::{
    clock::Clock,
    db::{Db, DbConfig, SqlStmt},
    outbox,
    rollover::Registry,
    telemetry,
};
use bank::sql_stmt;
use chrono::{DateTime, FixedOffset};
use http_body_util::BodyExt;
use rand::{Rng, SeedableRng, rngs::StdRng, seq::SliceRandom};
use serde_json::{Value, json};
use tokio::task::JoinSet;
use tower::ServiceExt;
use tracing_subscriber::prelude::*;

const HISTORY: SqlStmt = sql_stmt!("bench.history", "sql/driver_bench_history.sql");
const OUTBOX: SqlStmt = sql_stmt!("bench.outbox", "sql/driver_bench_outbox.sql");
const BALANCES: SqlStmt = sql_stmt!("bench.balances", "sql/driver_bench_balances.sql");
const COUNT: SqlStmt = sql_stmt!("bench.count", "sql/driver_bench_count.sql");
const TRANSFER: &str = "/v2.0/transfer/withdraw/fin_num";
const OPENING: i64 = 100_000_000;

fn id() -> String {
    ulid::Ulid::new().to_string()
}
fn btid(n: usize) -> String {
    format!("T000000001U{n:09}")
}

// Unlike common::call, transport/body failures are data, counted in the result.
async fn call(
    router: &Router,
    method: &str,
    path: &str,
    request: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<(u16, Value), String> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-request-id", request);
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let req = if let Some(body) = body {
        req.header("content-type", "application/json")
            .body(Body::from(body.to_string()))
    } else {
        req.body(Body::empty())
    }
    .map_err(|e| e.to_string())?;
    let response = router
        .clone()
        .oneshot(req)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    let body = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok((status, body))
}
async fn seed_call(router: &Router, path: &str, body: Value) -> Value {
    let (status, body) = call(router, "POST", path, &id(), None, Some(body))
        .await
        .expect("seed response");
    assert_eq!(status, 200, "seed {path}: {body}");
    body
}
#[derive(Clone)]
struct Customer {
    phone: String,
    account: String,
    fin: String,
    token: String,
}

async fn seed(generator: &Router, public: &Router, rng: &mut StdRng) -> Vec<Customer> {
    let mut numbers: Vec<usize> = (0..1000).collect();
    numbers.shuffle(rng);
    let mut customers: Vec<Customer> = numbers
        .iter()
        .map(|n| Customer {
            phone: format!("010{n:08}"),
            account: format!("9600{n:08}"),
            fin: String::new(),
            token: String::new(),
        })
        .collect();
    // Stay below both the 500-row contract implementation limit and body limits.
    for batch in customers.chunks_mut(100) {
        let rows: Vec<Value> = batch.iter().map(|c| json!({"customer_no": format!("C{}", c.phone), "name": "벤치마크", "phone": c.phone, "customer_type": "INDIVIDUAL", "password": "pw-bench"})).collect();
        seed_call(
            generator,
            "/internal/v1/seed/customers",
            json!({"customers": rows}),
        )
        .await;
        let rows: Vec<Value> = batch.iter().map(|c| json!({"customer_no": format!("C{}", c.phone), "account_no": c.account, "atm_pin": "7391", "opening_balance": OPENING})).collect();
        let body = seed_call(
            generator,
            "/internal/v1/seed/accounts",
            json!({"accounts": rows}),
        )
        .await;
        for c in batch {
            c.fin = body["accounts"]
                .as_array()
                .expect("accounts")
                .iter()
                .find(|a| a["account_no"] == c.account)
                .expect("account")["fintech_use_num"]
                .as_str()
                .expect("fin")
                .to_owned();
        }
    }
    // Eight simultaneous Argon2 logins, under the brief's maximum of sixteen.
    for batch in customers.chunks_mut(8) {
        let mut tasks = JoinSet::new();
        for (i, c) in batch.iter().enumerate() {
            let router = public.clone();
            let phone = c.phone.clone();
            tasks.spawn(async move { (i, seed_call(&router, "/v2.0/ext/auth/login", json!({"phone": phone, "password": "pw-bench", "device_id": "driver-bench"})).await) });
        }
        while let Some(result) = tasks.join_next().await {
            let (i, body) = result.expect("login task");
            batch[i].token = body["access_token"].as_str().expect("token").to_owned();
        }
    }
    customers
}

fn transfer(
    customers: &[Customer],
    from: usize,
    to: usize,
    n: usize,
    now: DateTime<FixedOffset>,
) -> Value {
    json!({"bank_tran_id": btid(n), "fintech_use_num": customers[from].fin,
        "tran_amt": "1000", "tran_dtime": now.format("%Y%m%d%H%M%S").to_string(),
        "transfer_purpose": "TR", "recv_client_name": "수취인", "recv_client_bank_code": "998",
        "recv_client_account_num": customers[to].account})
}

#[derive(Default)]
struct Results {
    successes: u64,
    business_rejections: u64,
    busy: u64,
    other_errors: u64,
    latencies: Vec<f64>,
}
impl Results {
    fn record(&mut self, result: Result<(u16, Value), String>, elapsed: Duration) {
        self.latencies.push(elapsed.as_secs_f64() * 1000.0);
        match result {
            Ok((503, _)) => self.busy += 1,
            Ok((500..=599, _)) | Err(_) => self.other_errors += 1,
            Ok((200..=299, body)) if body["rsp_code"] == "A0000" => self.successes += 1,
            _ => self.business_rejections += 1,
        }
    }
    fn merge(&mut self, other: Self) {
        self.successes += other.successes;
        self.business_rejections += other.business_rejections;
        self.busy += other.busy;
        self.other_errors += other.other_errors;
        self.latencies.extend(other.latencies);
    }
    fn percentile(&self, p: f64) -> f64 {
        if self.latencies.is_empty() {
            return 0.0;
        }
        self.latencies[((self.latencies.len() as f64 * p).ceil() as usize).saturating_sub(1)]
    }
}

// A single atomic cursor assigns the deterministic request sequence to workers.
// Scheduling can change completion order; the cursor fixes accounts and pairs.
async fn window(
    router: &Router,
    customers: Arc<Vec<Customer>>,
    pairs: Arc<Vec<usize>>,
    workload: &str,
    now: DateTime<FixedOffset>,
    cursor: Arc<AtomicUsize>,
    config: (usize, Duration),
) -> Results {
    let (concurrency, duration) = config;
    let deadline = Instant::now() + duration;
    let mut tasks = JoinSet::new();
    for _ in 0..concurrency {
        let router = router.clone();
        let customers = customers.clone();
        let pairs = pairs.clone();
        let workload = workload.to_owned();
        let cursor = cursor.clone();
        tasks.spawn(async move {
            let mut result = Results::default();
            while Instant::now() < deadline {
                let n = cursor.fetch_add(1, Ordering::Relaxed);
                let from = n % customers.len(); let c = &customers[from];
                let (method, path, body) = match workload.as_str() {
                    "transfer" => ("POST", TRANSFER.to_owned(), Some(transfer(&customers, from, pairs[n % pairs.len()], n, now))),
                    "lookup" => ("GET", "/v2.0/account/list".to_owned(), None),
                    "balance" => ("GET", format!("/v2.0/account/balance/fin_num?bank_tran_id={}&fintech_use_num={}&tran_dtime={}", btid(n), c.fin, now.format("%Y%m%d%H%M%S")), None),
                    "history" => ("GET", format!("/v2.0/account/transaction_list/fin_num?bank_tran_id={}&fintech_use_num={}&inquiry_type=A&inquiry_base=D&from_date={}&to_date={}&sort_order=D&tran_dtime={}", btid(n), c.fin, (now - chrono::Months::new(3)).format("%Y%m%d"), now.format("%Y%m%d"), now.format("%Y%m%d%H%M%S")), None),
                    _ => unreachable!(),
                };
                let request = id();
                let started = Instant::now();
                let response = call(&router, method, &path, &request, Some(&c.token), body).await;
                result.record(response, started.elapsed());
            }
            result
        });
    }
    let mut result = Results::default();
    while let Some(task) = tasks.join_next().await {
        match task {
            Ok(r) => result.merge(r),
            Err(_) => result.other_errors += 1,
        }
    }
    result
}

// Prometheus cumulative buckets keyed by statement and boundary.
type Buckets = BTreeMap<String, Vec<(f64, f64)>>;
fn buckets(rendered: &str) -> Buckets {
    let mut out = Buckets::new();
    for line in rendered
        .lines()
        .filter(|l| l.starts_with("bank_db_statement_duration_seconds_bucket{"))
    {
        let (labels, count) = line.rsplit_once(' ').expect("bucket count");
        let label = |name: &str| -> &str {
            labels
                .split(&format!("{name}=\""))
                .nth(1)
                .expect("label")
                .split('"')
                .next()
                .expect("value")
        };
        let bound = label("le").parse::<f64>().unwrap_or(f64::INFINITY);
        out.entry(label("statement").to_owned())
            .or_default()
            .push((bound, count.parse().expect("count")));
    }
    out
}
fn slowest(before: Buckets, after: Buckets) -> (String, f64) {
    let mut slowest = (String::new(), 0.0_f64);
    for (statement, mut bins) in after {
        for (bound, count) in &mut bins {
            let old = before
                .get(&statement)
                .and_then(|b| b.iter().find(|(v, _)| v == bound))
                .map_or(0.0, |(_, c)| *c);
            *count -= old;
        }
        bins.sort_by(|a, b| a.0.total_cmp(&b.0));
        let total = bins.last().map_or(0.0, |b| b.1);
        if total <= 0.0 {
            continue;
        }
        let target = total * 0.95;
        let (mut previous_bound, mut previous_count) = (0.0, 0.0);
        for (bound, count) in bins {
            if count >= target {
                // Prometheus histogram_quantile interpolation; +Inf uses the last finite bound.
                let p95 = if bound.is_infinite() {
                    previous_bound
                } else {
                    previous_bound
                        + (bound - previous_bound) * (target - previous_count)
                            / (count - previous_count)
                } * 1000.0;
                if p95 > slowest.1 {
                    slowest = (statement, p95);
                }
                break;
            }
            previous_bound = bound;
            previous_count = count;
        }
    }
    slowest
}
async fn balances(db: &Db) -> BTreeMap<String, i64> {
    db.run(|s| s.query_all::<(String, i64)>(&BALANCES, &[]))
        .await
        .expect("balances")
        .into_iter()
        .collect()
}
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} required"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn driver_bench() {
    let workload = env("BENCH_WORKLOAD");
    let concurrency: usize = env("BENCH_CONCURRENCY").parse().expect("concurrency");
    assert!(matches!(
        (workload.as_str(), concurrency),
        ("lookup" | "balance" | "history", 8)
            | ("transfer", 1 | 8 | 32 | 64)
            | ("duplicate", 200)
            | ("outbox", 1)
    ));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::filter::LevelFilter::WARN),
        )
        .init();
    let metrics = telemetry::metrics_handle();
    let db = Db::connect(&DbConfig::from_env().expect("config")).expect("connect");
    db.migrate().await.expect("migrate");
    let clock = Clock::init(db.clone(), Registry::default())
        .await
        .expect("clock");
    let state = AppState {
        db: db.clone(),
        clock,
        limits: Default::default(),
    };
    let public = app::public_router(state.clone());
    let generator = app::generator_router(state.clone());
    let seeded = Instant::now();
    let mut rng = StdRng::seed_from_u64(20261115);
    let customers = Arc::new(seed(&generator, &public, &mut rng).await);
    let now = state
        .clock
        .state()
        .await
        .expect("clock state")
        .business_time;
    db.run(move |s| {
        s.execute(&HISTORY, &[&now.date_naive(), &id()])?;
        s.commit()
    })
    .await
    .expect("history fixture");
    // The history workload must measure real pages, not empty ones.
    let first = &customers[0];
    let (status, page) = call(&public, "GET", &format!("/v2.0/account/transaction_list/fin_num?bank_tran_id={}&fintech_use_num={}&inquiry_type=A&inquiry_base=D&from_date={}&to_date={}&sort_order=D&tran_dtime={}", btid(0), first.fin, (now - chrono::Months::new(3)).format("%Y%m%d"), now.format("%Y%m%d"), now.format("%Y%m%d%H%M%S")), &id(), Some(&first.token), None)
        .await
        .expect("history probe");
    let listed = page["res_list"].as_array().map_or(0, Vec::len);
    assert!(
        status == 200 && listed >= 25,
        "history fixture not listed: {status} {listed} rows"
    );
    eprintln!("driver-bench history_probe_rows={listed}");
    eprintln!(
        "driver-bench seeding_secs={:.3}",
        seeded.elapsed().as_secs_f64()
    );
    // A repeatable stream of random distinct recipients, with senders spread evenly.
    let pairs: Arc<Vec<usize>> = Arc::new(
        (0..1_000_000)
            .map(|n| {
                let from = n % 1000;
                let to = rng.random_range(0..999);
                if to >= from { to + 1 } else { to }
            })
            .collect(),
    );
    let cursor = Arc::new(AtomicUsize::new(0));
    let collection = if workload == "outbox" {
        Some(
            outbox_relay::events_collection(&env("MONGO_URI"))
                .await
                .expect("collection"),
        )
    } else {
        None
    };
    if let Some(events) = &collection {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if outbox_relay::relay_once(&db, events, true)
                .await
                .expect("relay warm-up")
                == 0
            {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    } else {
        let warm_workload = if workload == "duplicate" {
            "transfer"
        } else {
            &workload
        };
        let warm_concurrency = if workload == "duplicate" {
            8
        } else {
            concurrency
        };
        let warm = window(
            &public,
            customers.clone(),
            pairs.clone(),
            warm_workload,
            now,
            cursor.clone(),
            (warm_concurrency, Duration::from_secs(30)),
        )
        .await;
        assert_eq!(
            warm.business_rejections + warm.other_errors,
            0,
            "warm-up errors"
        );
    }
    let before_balances = balances(&db).await;
    let mut duplicate_id = String::new();
    let mut event_ids = Vec::new();
    if let Some(events) = &collection {
        // Drain all seed and warm-up events outside the measured window.
        while outbox_relay::relay_once(&db, events, true)
            .await
            .expect("pre-drain")
            > 0
        {}
        event_ids = (0..5000).map(|_| id()).collect();
        let ids = event_ids.clone();
        db.run(move |s| {
            let at = now.to_rfc3339();
            let date = now.date_naive().to_string();
            for id in ids {
                s.execute(&OUTBOX, &[&id, &id, &at, &at, &date])?;
            }
            s.commit()
        })
        .await
        .expect("outbox fixture");
    }
    let duplicate_body = if workload == "duplicate" {
        let n = cursor.fetch_add(1, Ordering::Relaxed);
        duplicate_id = btid(n);
        Some(transfer(&customers, 0, 1, n, now))
    } else {
        None
    };
    let snapshot = buckets(&metrics.render());
    let started = Instant::now();
    let mut result = if let Some(events) = &collection {
        let mut r = Results::default();
        loop {
            let at = Instant::now();
            match outbox_relay::relay_once(&db, events, true).await {
                Ok(0) => break,
                Ok(count) => {
                    r.successes += count as u64;
                    r.latencies.push(at.elapsed().as_secs_f64() * 1000.0);
                }
                Err(_) => {
                    r.other_errors += 1;
                    break;
                }
            }
        }
        r
    } else if let Some(body) = duplicate_body {
        let mut tasks = JoinSet::new();
        let gate = Arc::new(tokio::sync::Barrier::new(concurrency));
        let request = id();
        for _ in 0..concurrency {
            let router = public.clone();
            let token = customers[0].token.clone();
            let body = body.clone();
            let gate = gate.clone();
            let request = request.clone();
            tasks.spawn(async move {
                gate.wait().await;
                let at = Instant::now();
                let response = call(
                    &router,
                    "POST",
                    TRANSFER,
                    &request,
                    Some(&token),
                    Some(body),
                )
                .await;
                (response, at.elapsed())
            });
        }
        let mut r = Results::default();
        while let Some(task) = tasks.join_next().await {
            match task {
                Ok((response, elapsed)) => r.record(response, elapsed),
                Err(_) => r.other_errors += 1,
            }
        }
        r
    } else {
        window(
            &public,
            customers.clone(),
            pairs,
            &workload,
            now,
            cursor,
            (concurrency, Duration::from_secs(60)),
        )
        .await
    };
    let measured_secs = started.elapsed().as_secs_f64();
    let (slowest_statement, slowest_statement_p95_ms) =
        slowest(snapshot, buckets(&metrics.render()));
    let proc = std::fs::read_to_string("/proc/self/status").expect("Linux process status");
    let proc_value = |key: &str| -> u64 {
        proc.lines()
            .find_map(|l| l.strip_prefix(key))
            .expect("proc field")
            .split_whitespace()
            .next()
            .expect("proc value")
            .parse()
            .expect("number")
    };
    result.latencies.sort_by(f64::total_cmp);
    let line = json!({"driver": env("BENCH_DRIVER"), "workload": workload, "concurrency": concurrency, "run": env("BENCH_RUN"),
        "successes": result.successes, "business_rejections": result.business_rejections, "busy": result.busy, "other_errors": result.other_errors,
        "tps": result.successes as f64 / measured_secs, "p50_ms": result.percentile(0.50), "p95_ms": result.percentile(0.95), "p99_ms": result.percentile(0.99),
        "slowest_statement": slowest_statement, "slowest_statement_p95_ms": slowest_statement_p95_ms,
        "peak_rss_kb": proc_value("VmHWM:"), "threads": proc_value("Threads:"), "cpus": env("BENCH_CPUS"), "memory": env("BENCH_MEMORY"), "measured_secs": measured_secs});
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(env("BENCH_OUT"))
        .expect("output");
    writeln!(file, "{line}").expect("write JSON line");
    file.flush().expect("flush");
    println!("{line}");
    assert_eq!(result.business_rejections, 0, "business rejections");
    assert_eq!(result.other_errors, 0, "other errors");
    assert!(result.successes > 0, "completed work");
    if workload == "transfer" || workload == "duplicate" {
        let after = balances(&db).await;
        assert_eq!(
            before_balances.values().sum::<i64>(),
            after.values().sum::<i64>(),
            "conservation"
        );
        if workload == "duplicate" {
            assert_eq!(
                result.successes, 200,
                "every duplicate must return documented success"
            );
            assert_eq!(
                before_balances[&customers[0].account] - after[&customers[0].account],
                1000
            );
            let count: i64 = db
                .run(move |s| s.query_row(&COUNT, &[&duplicate_id]))
                .await
                .expect("transfer count");
            assert_eq!(count, 1);
        }
    }
    if let Some(events) = collection {
        assert_eq!(result.successes, 5000);
        assert_eq!(outbox::backlog(&db).await.expect("backlog").0, 0);
        let count = events
            .count_documents(mongodb::bson::doc! {"_id": {"$in": event_ids}})
            .await
            .expect("Mongo event count");
        assert_eq!(count, 5000);
    }
}
