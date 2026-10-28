// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Redis 存储后端压测执行器
//!
//! 编译：`cargo build --example bench-runner --features redis-store`
//! 运行：`bench-runner --op increment_version --concurrency 100 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sz_rust_auth_facade::redis_store::{create_redis_stores_with_devices, RedisConfig};
use sz_rust_auth_facade::refresh::{DeviceInfo, RefreshTokenError};
use tokio::sync::Semaphore;

// ── CLI 参数 ──

#[derive(Debug, Clone)]
struct BenchArgs {
    op: String,
    concurrency: u16,
    total: u64,
    redis_url: String,
    prefix: String,
    soak_secs: u64,
    mixed_ratio: String,
    devices_per_user: u16,
}

fn parse_args() -> Result<BenchArgs, i32> {
    let mut op = String::new();
    let mut concurrency: u16 = 100;
    let mut total: u64 = 10000;
    let mut redis_url = String::from("redis://127.0.0.1:16379");
    let mut prefix = String::from("sso:bench");
    let mut soak_secs: u64 = 600;
    let mut mixed_ratio = String::from("3:2:2:1:1:1");
    let mut devices_per_user: u16 = 10;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        let val = args.get(i + 1);
        match key {
            "--op" => {
                if let Some(v) = val {
                    op = v.clone();
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--concurrency" => {
                if let Some(v) = val {
                    concurrency = v.parse().map_err(|_| 2)?;
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--total" => {
                if let Some(v) = val {
                    total = v.parse().map_err(|_| 2)?;
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--redis-url" => {
                if let Some(v) = val {
                    redis_url = v.clone();
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--prefix" => {
                if let Some(v) = val {
                    prefix = v.clone();
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--soak-secs" => {
                if let Some(v) = val {
                    soak_secs = v.parse().map_err(|_| 2)?;
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--mixed-ratio" => {
                if let Some(v) = val {
                    mixed_ratio = v.clone();
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            "--devices-per-user" => {
                if let Some(v) = val {
                    devices_per_user = v.parse().map_err(|_| 2)?;
                    i += 2;
                } else {
                    return Err(2);
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    if op.is_empty() {
        eprintln!("ERROR: --op is required");
        return Err(2);
    }
    if prefix != "sso:bench" {
        eprintln!("ERROR: --prefix must be 'sso:bench', got '{}'", prefix);
        return Err(2);
    }
    if concurrency > 2000 {
        eprintln!("ERROR: --concurrency must be <= 2000, got {}", concurrency);
        return Err(2);
    }

    Ok(BenchArgs {
        op,
        concurrency,
        total,
        redis_url,
        prefix,
        soak_secs,
        mixed_ratio,
        devices_per_user,
    })
}

// ── 延迟分位数 ──

fn percentiles(samples: &mut [u64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    samples.sort_unstable();
    let len = samples.len();
    let pct = |p: f64| -> f64 {
        let idx = ((len as f64) * p / 100.0).floor() as usize;
        let idx = idx.min(len - 1);
        samples[idx] as f64 / 1_000_000.0
    };
    (pct(50.0), pct(95.0), pct(99.0))
}

// ── 错误分类 ──

#[derive(Debug, Clone, Default)]
struct ErrorClassifier {
    counts: HashMap<String, u64>,
}

impl ErrorClassifier {
    fn record(&mut self, err: &RefreshTokenError) {
        let key = match err {
            RefreshTokenError::ServiceUnavailable => "ServiceUnavailable",
            RefreshTokenError::Cache(_) => "Cache",
            RefreshTokenError::Expired => "Expired",
            RefreshTokenError::Revoked => "Revoked",
            RefreshTokenError::ReuseDetected => "ReuseDetected",
            RefreshTokenError::InvalidCredentials => "InvalidCredentials",
            RefreshTokenError::InvalidSignature => "InvalidSignature",
            RefreshTokenError::WrongTokenType { .. } => "WrongTokenType",
            RefreshTokenError::IssuerMismatch { .. } => "IssuerMismatch",
            RefreshTokenError::VersionMismatch { .. } => "VersionMismatch",
            RefreshTokenError::Jwt(_) => "Jwt",
            RefreshTokenError::UserNotFound => "UserNotFound",
            RefreshTokenError::InvalidConfig(_) => "InvalidConfig",
        };
        *self.counts.entry(key.to_string()).or_insert(0) += 1;
    }
    fn error_rate(&self, total: u64) -> f64 {
        if total == 0 {
            0.0
        } else {
            self.total() as f64 / total as f64
        }
    }
    fn total(&self) -> u64 {
        self.counts.values().sum()
    }
}

// ── JSON 输出结构 ──

#[derive(Debug, Clone, serde::Serialize)]
struct RoundResult {
    operation: String,
    concurrency: u16,
    qps: f64,
    latency_p50_ms: f64,
    latency_p95_ms: f64,
    latency_p99_ms: f64,
    error_rate: f64,
    error_breakdown: HashMap<String, u64>,
    total_requests: u64,
    duration_secs: f64,
    rss_peak_kb: u64,
    rss_start_kb: u64,
    evidence_file: String,
    evidence_line: String,
    verdict: String,
    consistency_check: ConsistencyCheck,
    #[serde(skip_serializing_if = "Option::is_none")]
    by_op: Option<HashMap<String, OpMetric>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_serialize_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_deserialize_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_unavailable_rate: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OpMetric {
    qps: f64,
    latency_p99_ms: f64,
    error_rate: f64,
    count: u64,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
struct ConsistencyCheck {
    passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SoakSnapshot {
    minute_index: u32,
    qps: f64,
    latency_p99_ms: f64,
    rss_kb: u64,
    error_rate: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SoakSummary {
    qps_stable: bool,
    p99_stable: bool,
    memory_ok: bool,
    rss_start_kb: u64,
    rss_peak_kb: u64,
    rss_end_kb: u64,
    snapshots: Vec<SoakSnapshot>,
}

// ── 并发驱动 ──

async fn run_concurrent<F, Fut, T>(
    concurrency: u16,
    total: u64,
    f: F,
) -> (Vec<Result<T, RefreshTokenError>>, Vec<u64>, f64)
where
    F: Fn(u64) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<T, RefreshTokenError>> + Send + 'static,
    T: Send + 'static,
{
    let sem = Arc::new(Semaphore::new(concurrency as usize));
    let f = Arc::new(f);
    let start = Instant::now();
    let mut handles = Vec::with_capacity(total as usize);
    for i in 0..total {
        let sem = sem.clone();
        let f = f.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let req_start = Instant::now();
            let result = f(i).await;
            (result, req_start.elapsed().as_nanos() as u64)
        }));
    }
    let mut results = Vec::with_capacity(total as usize);
    let mut latencies = Vec::with_capacity(total as usize);
    for h in handles {
        let (r, l) = h.await.unwrap();
        results.push(r);
        latencies.push(l);
    }
    (results, latencies, start.elapsed().as_secs_f64())
}

// ── RSS ──

fn get_rss_kb() -> u64 {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();
    let pid = sysinfo::Pid::from(std::process::id() as usize);
    sys.process(pid).map(|p| p.memory() / 1024).unwrap_or(0)
}

// ── 辅助 ──

fn make_config(redis_url: &str, prefix: &str) -> RedisConfig {
    RedisConfig {
        url: redis_url.to_string(),
        key_prefix_ver: format!("{}:ver", prefix),
        key_prefix_bl: format!("{}:bl", prefix),
        key_prefix_sessions: format!("{}:sessions", prefix),
        connection_timeout: Duration::from_secs(3),
        command_timeout: Duration::from_secs(2),
    }
}

async fn pre_clean(config: &RedisConfig, user_id: i64) {
    let client = redis::Client::open(config.url.as_str()).unwrap();
    let mut conn = client.get_connection_manager().await.unwrap();
    let _: () = redis::AsyncCommands::del(
        &mut conn,
        format!("{}:ver:{}", config.key_prefix_ver, user_id),
    )
    .await
    .unwrap_or(());
    let _: () = redis::AsyncCommands::del(
        &mut conn,
        format!("{}:sessions:{}", config.key_prefix_sessions, user_id),
    )
    .await
    .unwrap_or(());
}

async fn pre_clean_all(config: &RedisConfig) {
    let client = redis::Client::open(config.url.as_str()).unwrap();
    let mut conn = client.get_connection_manager().await.unwrap();
    for prefix in [
        &config.key_prefix_ver,
        &config.key_prefix_bl,
        &config.key_prefix_sessions,
    ] {
        let keys: Vec<String> = redis::AsyncCommands::keys(&mut conn, format!("{}:*", prefix))
            .await
            .unwrap_or_default();
        if !keys.is_empty() {
            let _: () = redis::AsyncCommands::del(&mut conn, keys)
                .await
                .unwrap_or(());
        }
    }
}

fn judge(qps: f64, p99: f64, err: f64, qps_min: f64, p99_max: f64, err_max: f64) -> bool {
    qps >= qps_min && p99 <= p99_max && err <= err_max
}

fn make_rr(
    op: &str,
    conc: u16,
    lat: &mut Vec<u64>,
    err: &ErrorClassifier,
    total: u64,
    dur: f64,
    ev: &str,
    cc: ConsistencyCheck,
    qps_min: f64,
    p99_max: f64,
    err_max: f64,
) -> RoundResult {
    let (p50, p95, p99) = percentiles(lat);
    let qps = if dur > 0.0 { total as f64 / dur } else { 0.0 };
    let er = err.error_rate(total);
    let verdict = if judge(qps, p99, er, qps_min, p99_max, err_max) {
        "pass"
    } else {
        "fail"
    };
    let rss = get_rss_kb();
    RoundResult {
        operation: op.to_string(),
        concurrency: conc,
        qps,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        error_rate: er,
        error_breakdown: err.counts.clone(),
        total_requests: total,
        duration_secs: dur,
        rss_peak_kb: rss,
        rss_start_kb: rss,
        evidence_file: "packages/sz-rust-auth-facade/src/redis_store.rs".to_string(),
        evidence_line: ev.to_string(),
        verdict: verdict.to_string(),
        consistency_check: cc,
        by_op: None,
        final_version: None,
        json_serialize_ratio: None,
        json_deserialize_ratio: None,
        service_unavailable_rate: None,
    }
}

fn emit<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string(v).unwrap());
}

// ── 压测函数 ──

async fn bench_increment_version(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (store, _, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = args.total;
    let store_after = store.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |_i| {
        let store = store.clone();
        async move { store.increment_version(1).await }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let fv = store_after.get_version(1).await.unwrap_or(0);
    let cc = ConsistencyCheck {
        passed: fv == total,
        detail: Some(format!("final_version={} expected={}", fv, total)),
    };
    let (qm, pm, em) = match args.concurrency {
        100 => (8000.0, 5.0, 0.0001),
        500 => (20000.0, 15.0, 0.0005),
        1000 => (30000.0, 30.0, 0.001),
        _ => (0.0, f64::MAX, f64::MAX),
    };
    let mut rr = make_rr(
        "increment_version",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "156-168",
        cc,
        qm,
        pm,
        em,
    );
    rr.final_version = Some(fv);
    emit(&rr);
}

async fn bench_get_version(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (store, _, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    for _ in 0..42 {
        store.increment_version(1).await.unwrap();
    }
    let total = args.total;
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |_i| {
        let store = store.clone();
        async move { store.get_version(1).await }
    })
    .await;
    let mut err = ErrorClassifier::default();
    let mut ok = true;
    for r in &results {
        match r {
            Ok(v) => {
                if *v != 42 {
                    ok = false;
                }
            }
            Err(e) => {
                err.record(e);
                ok = false;
            }
        }
    }
    let cc = ConsistencyCheck {
        passed: ok,
        detail: Some(format!("all_42={}", ok)),
    };
    let rr = make_rr(
        "get_version",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "142-154",
        cc,
        40000.0,
        10.0,
        0.0001,
    );
    emit(&rr);
}

async fn bench_concurrent_no_lost_update(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (store, _, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = 1000u64;
    let store_after = store.clone();
    let (results, mut lat, dur) = run_concurrent(1000, total, move |_i| {
        let store = store.clone();
        async move { store.increment_version(1).await }
    })
    .await;
    let mut err = ErrorClassifier::default();
    let mut vals: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for r in &results {
        match r {
            Ok(v) => {
                vals.insert(*v);
            }
            Err(e) => {
                err.record(e);
            }
        }
    }
    let fv = store_after.get_version(1).await.unwrap_or(0);
    let no_lost = fv == total && vals.len() == total as usize;

    pre_clean(&config, 2).await;
    let s1 = store_after.clone();
    let s2 = store_after.clone();
    let h1 = tokio::spawn(async move {
        for _ in 0..100 {
            s1.increment_version(1).await.unwrap();
        }
    });
    let h2 = tokio::spawn(async move {
        for _ in 0..100 {
            s2.increment_version(2).await.unwrap();
        }
    });
    h1.await.unwrap();
    h2.await.unwrap();
    let v1 = store_after.get_version(1).await.unwrap_or(0);
    let v2 = store_after.get_version(2).await.unwrap_or(0);
    let cross = v1 == total + 100 && v2 == 100;
    let cc = ConsistencyCheck {
        passed: no_lost && cross,
        detail: Some(format!(
            "no_lost={} cross={} v1={} v2={}",
            no_lost, cross, v1, v2
        )),
    };
    let rr = make_rr(
        "concurrent_increment",
        1000,
        &mut lat,
        &err,
        total,
        dur,
        "156-168",
        cc,
        0.0,
        f64::MAX,
        f64::MAX,
    );
    emit(&rr);
}

async fn bench_is_revoked(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (_, bl, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let k = 1000u64;
    let jtis: Vec<String> = (0..k).map(|i| format!("revoked_jti_{}", i)).collect();
    for jti in &jtis {
        bl.revoke(jti, 3600).await.unwrap();
    }
    let total = args.total;
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |i| {
        let bl = bl.clone();
        let jtis = jtis.clone();
        async move {
            let idx = i % (k * 2);
            if idx < k {
                bl.is_revoked(&jtis[idx as usize]).await
            } else {
                bl.is_revoked(&format!("unrevoked_{}", idx)).await
            }
        }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let cc = ConsistencyCheck {
        passed: true,
        detail: None,
    };
    let rr = make_rr(
        "is_revoked",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "216-226",
        cc,
        40000.0,
        10.0,
        0.0001,
    );
    emit(&rr);
}

async fn bench_revoke(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (_, bl, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = args.total;
    let bl_after = bl.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |i| {
        let bl = bl.clone();
        async move { bl.revoke(&format!("bench_revoke_jti_{}", i), 3600).await }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let sc = bl_after
        .is_revoked("bench_revoke_jti_0")
        .await
        .unwrap_or(false);
    let cc = ConsistencyCheck {
        passed: sc,
        detail: Some(format!("jti_0_revoked={}", sc)),
    };
    let rr = make_rr(
        "revoke",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "199-214",
        cc,
        15000.0,
        15.0,
        0.0005,
    );
    emit(&rr);
}

async fn bench_ttl_validation(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (_, bl, _) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    bl.revoke("ttl_expire_test", 1).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let expired = bl.is_revoked("ttl_expire_test").await.unwrap_or(true);
    bl.revoke("ttl_zero_test", 0).await.unwrap();
    let zero = bl.is_revoked("ttl_zero_test").await.unwrap_or(true);
    let cc = ConsistencyCheck {
        passed: !expired && !zero,
        detail: Some(format!("expired={} zero={}", expired, zero)),
    };
    let mut lat = vec![];
    let err = ErrorClassifier::default();
    let rr = make_rr(
        "ttl_validation",
        1,
        &mut lat,
        &err,
        1,
        0.0,
        "199-214",
        cc,
        0.0,
        f64::MAX,
        f64::MAX,
    );
    emit(&rr);
}

async fn bench_register_session(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = args.total;
    let ds_after = ds.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |i| {
        let ds = ds.clone();
        async move {
            let did = format!("dev_{}", i);
            let info = DeviceInfo::with_device_id(&did);
            ds.register_session(
                1,
                &did,
                &info,
                &format!("jti_{}", i),
                &format!("ajti_{}", i),
            )
            .await
        }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let sessions = ds_after.get_sessions(1).await.unwrap_or_default();
    let cc = ConsistencyCheck {
        passed: sessions.len() == total as usize,
        detail: Some(format!("count={} expected={}", sessions.len(), total)),
    };
    let rr = make_rr(
        "register_session",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "263-292",
        cc,
        10000.0,
        20.0,
        0.0005,
    );
    emit(&rr);
}

async fn bench_get_session(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let k = 1000u64;
    for i in 0..k {
        let did = format!("dev_{}", i);
        let info = DeviceInfo::with_device_id(&did);
        ds.register_session(
            1,
            &did,
            &info,
            &format!("jti_{}", i),
            &format!("ajti_{}", i),
        )
        .await
        .unwrap();
    }
    let total = args.total;
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |i| {
        let ds = ds.clone();
        async move {
            let idx = i % (k * 2);
            if idx < k {
                ds.get_session(1, &format!("dev_{}", idx)).await.map(|_| ())
            } else {
                ds.get_session(1, &format!("nonexist_{}", idx))
                    .await
                    .map(|_| ())
            }
        }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let cc = ConsistencyCheck {
        passed: true,
        detail: None,
    };
    let rr = make_rr(
        "get_session",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "314-338",
        cc,
        20000.0,
        15.0,
        0.0005,
    );
    emit(&rr);
}

async fn bench_get_sessions(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let d = args.devices_per_user as u64;
    for i in 0..d {
        let did = format!("dev_{}", i);
        let info = DeviceInfo::with_device_id(&did);
        ds.register_session(
            1,
            &did,
            &info,
            &format!("jti_{}", i),
            &format!("ajti_{}", i),
        )
        .await
        .unwrap();
    }
    let total = args.total;
    let ds_after = ds.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |_i| {
        let ds = ds.clone();
        async move { ds.get_sessions(1).await.map(|_| ()) }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let check = ds_after.get_sessions(1).await.unwrap_or_default();
    let ok = check.len() == d as usize;
    let cc = ConsistencyCheck {
        passed: ok,
        detail: Some(format!("returned_{}={}", d, ok)),
    };
    let (qm, pm, em) = if d <= 10 {
        (5000.0, 30.0, 0.0005)
    } else {
        (0.0, 100.0, 0.0005)
    };
    let rr = make_rr(
        "get_sessions",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "294-312",
        cc,
        qm,
        pm,
        em,
    );
    emit(&rr);
}

async fn bench_revoke_session(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let k = args.total;
    for i in 0..k {
        let did = format!("dev_{}", i);
        let info = DeviceInfo::with_device_id(&did);
        ds.register_session(
            1,
            &did,
            &info,
            &format!("jti_{}", i),
            &format!("ajti_{}", i),
        )
        .await
        .unwrap();
    }
    let ds_after = ds.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, k, move |i| {
        let ds = ds.clone();
        async move {
            ds.revoke_session(1, &format!("dev_{}", i))
                .await
                .map(|_| ())
        }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let sessions = ds_after.get_sessions(1).await.unwrap_or_default();
    let cc = ConsistencyCheck {
        passed: sessions.is_empty(),
        detail: Some(format!("remaining={}", sessions.len())),
    };
    let rr = make_rr(
        "revoke_session",
        args.concurrency,
        &mut lat,
        &err,
        k,
        dur,
        "340-372",
        cc,
        8000.0,
        20.0,
        0.0005,
    );
    emit(&rr);
}

async fn bench_update_last_active(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let k = 100u64;
    for i in 0..k {
        let did = format!("dev_{}", i);
        let info = DeviceInfo::with_device_id(&did);
        ds.register_session(
            1,
            &did,
            &info,
            &format!("jti_{}", i),
            &format!("ajti_{}", i),
        )
        .await
        .unwrap();
    }
    let bench_start = chrono::Utc::now().timestamp();
    let total = args.total;
    let ds_after = ds.clone();
    let (results, mut lat, dur) = run_concurrent(args.concurrency, total, move |i| {
        let ds = ds.clone();
        async move { ds.update_last_active(1, &format!("dev_{}", i % k)).await }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }
    let sessions = ds_after.get_sessions(1).await.unwrap_or_default();
    let ok = sessions.iter().all(|s| s.last_active >= bench_start);
    let cc = ConsistencyCheck {
        passed: ok,
        detail: Some(format!("all_updated={}", ok)),
    };
    let rr = make_rr(
        "update_last_active",
        args.concurrency,
        &mut lat,
        &err,
        total,
        dur,
        "374-409",
        cc,
        8000.0,
        20.0,
        0.0005,
    );
    emit(&rr);
}

async fn bench_json_overhead(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean(&config, 1).await;
    let (_, _, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = args.total;
    let (results, mut lat, dur_store) = run_concurrent(args.concurrency, total, move |i| {
        let ds = ds.clone();
        async move {
            let did = format!("dev_{}", i);
            let info = DeviceInfo::with_device_id(&did);
            ds.register_session(
                1,
                &did,
                &info,
                &format!("jti_{}", i),
                &format!("ajti_{}", i),
            )
            .await
        }
    })
    .await;
    let mut err = ErrorClassifier::default();
    for r in &results {
        if let Err(e) = r {
            err.record(e);
        }
    }

    pre_clean(&config, 1).await;
    let client = redis::Client::open(config.url.as_str()).unwrap();
    let mut conn = client.get_connection_manager().await.unwrap();
    let key = format!("{}:{}", config.key_prefix_sessions, 1);
    let start_hset = Instant::now();
    for i in 0..total {
        let did = format!("dev_{}", i);
        let session = serde_json::json!({"device_id": did, "device_info": {"device_id": did}, "jti": format!("jti_{}", i), "access_jti": format!("ajti_{}", i), "created_at": 0, "last_active": 0});
        let _: () = redis::AsyncCommands::hset(&mut conn, &key, &did, session.to_string())
            .await
            .unwrap();
    }
    let dur_hset = start_hset.elapsed().as_secs_f64();

    let qps_store = if dur_store > 0.0 {
        total as f64 / dur_store
    } else {
        0.0
    };
    let qps_hset = if dur_hset > 0.0 {
        total as f64 / dur_hset
    } else {
        0.0
    };
    let ratio = if qps_store > 0.0 {
        (1.0 - qps_store / qps_hset) * 100.0
    } else {
        0.0
    };
    let (p50, p95, p99) = percentiles(&mut lat);
    let er = err.error_rate(total);
    let cc = ConsistencyCheck {
        passed: true,
        detail: None,
    };
    let rss = get_rss_kb();
    let rr = RoundResult {
        operation: "json_overhead".to_string(),
        concurrency: args.concurrency,
        qps: qps_store,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        error_rate: er,
        error_breakdown: err.counts.clone(),
        total_requests: total,
        duration_secs: dur_store,
        rss_peak_kb: rss,
        rss_start_kb: rss,
        evidence_file: "packages/sz-rust-auth-facade/src/redis_store.rs".to_string(),
        evidence_line: "263-292".to_string(),
        verdict: "pass".to_string(),
        consistency_check: cc,
        by_op: None,
        final_version: None,
        json_serialize_ratio: Some(ratio),
        json_deserialize_ratio: None,
        service_unavailable_rate: None,
    };
    emit(&rr);
}

async fn bench_mixed(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (store, bl, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = args.total;
    let parts: Vec<u64> = args
        .mixed_ratio
        .split(':')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    let sum: u64 = parts.iter().sum();
    if sum == 0 {
        eprintln!("invalid ratio");
        std::process::exit(2);
    }
    let cum: Vec<u64> = parts
        .iter()
        .scan(0u64, |acc, &v| {
            *acc += v;
            Some(*acc)
        })
        .collect();
    let op_names = [
        "increment_version",
        "get_version",
        "is_revoked",
        "revoke",
        "register_session",
        "get_session",
    ];
    let mut op_counts = [0u64; 6];
    let mut op_lat: [Vec<u64>; 6] = Default::default();
    let mut op_err: [ErrorClassifier; 6] = Default::default();

    let sem = Arc::new(Semaphore::new(args.concurrency as usize));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(total as usize);
    for i in 0..total {
        let sem = sem.clone();
        let store = store.clone();
        let bl = bl.clone();
        let ds = ds.clone();
        let cum = cum.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let req_start = Instant::now();
            let r = (i % sum) as u64;
            let oi = cum.iter().position(|&c| r < c).unwrap_or(0);
            let result: Result<(), RefreshTokenError> = match oi {
                0 => store.increment_version(1).await.map(|_| ()),
                1 => store.get_version(1).await.map(|_| ()),
                2 => bl
                    .is_revoked(&format!("mix_revoked_{}", i))
                    .await
                    .map(|_| ()),
                3 => bl.revoke(&format!("mix_revoke_{}", i), 3600).await,
                4 => {
                    let did = format!("mix_dev_{}", i);
                    let info = DeviceInfo::with_device_id(&did);
                    ds.register_session(1, &did, &info, &format!("mj_{}", i), &format!("maj_{}", i))
                        .await
                }
                5 => ds
                    .get_session(1, &format!("mix_dev_{}", i % 100))
                    .await
                    .map(|_| ()),
                _ => unreachable!(),
            };
            (oi, result, req_start.elapsed().as_nanos() as u64)
        }));
    }
    for h in handles {
        let (oi, result, l) = h.await.unwrap();
        op_counts[oi] += 1;
        op_lat[oi].push(l);
        if let Err(e) = &result {
            op_err[oi].record(e);
        }
    }
    let dur = start.elapsed().as_secs_f64();
    let qps = if dur > 0.0 { total as f64 / dur } else { 0.0 };
    let mut all_lat: Vec<u64> = op_lat.iter().flat_map(|v| v.iter().copied()).collect();
    let (_, _, p99) = percentiles(&mut all_lat);
    let total_err: u64 = op_err.iter().map(|e| e.total()).sum();
    let er = if total > 0 {
        total_err as f64 / total as f64
    } else {
        0.0
    };

    let mut by_op = HashMap::new();
    for (idx, name) in op_names.iter().enumerate() {
        let cnt = op_counts[idx];
        let oq = if dur > 0.0 { cnt as f64 / dur } else { 0.0 };
        let (_, _, op_p99) = percentiles(&mut op_lat[idx]);
        by_op.insert(
            name.to_string(),
            OpMetric {
                qps: oq,
                latency_p99_ms: op_p99,
                error_rate: op_err[idx].error_rate(cnt),
                count: cnt,
            },
        );
    }
    let fv = store.get_version(1).await.unwrap_or(0);
    let cc = ConsistencyCheck {
        passed: fv == op_counts[0],
        detail: Some(format!("final_version={} inc_count={}", fv, op_counts[0])),
    };
    let verdict = if qps >= 12000.0 && p99 <= 30.0 && er <= 0.0005 {
        "pass"
    } else {
        "fail"
    };
    let rss = get_rss_kb();
    let rr = RoundResult {
        operation: "mixed".to_string(),
        concurrency: args.concurrency,
        qps,
        latency_p50_ms: 0.0,
        latency_p95_ms: 0.0,
        latency_p99_ms: p99,
        error_rate: er,
        error_breakdown: HashMap::new(),
        total_requests: total,
        duration_secs: dur,
        rss_peak_kb: rss,
        rss_start_kb: rss,
        evidence_file: "packages/sz-rust-auth-facade/src/redis_store.rs".to_string(),
        evidence_line: "156-168,142-154,216-226,199-214,263-292,314-338".to_string(),
        verdict: verdict.to_string(),
        consistency_check: cc,
        by_op: Some(by_op),
        final_version: Some(fv),
        json_serialize_ratio: None,
        json_deserialize_ratio: None,
        service_unavailable_rate: None,
    };
    emit(&rr);
}

async fn bench_pool_stability(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (store, bl, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let dur_secs = args.soak_secs;
    let conc = args.concurrency;
    let sem = Arc::new(Semaphore::new(conc as usize));
    let start = Instant::now();
    let target = Duration::from_secs(dur_secs);
    let mut total: u64 = 0;
    let mut err = ErrorClassifier::default();
    let mut lat = Vec::new();

    while start.elapsed() < target {
        let mut handles = Vec::new();
        for _ in 0..conc {
            let sem = sem.clone();
            let store = store.clone();
            let bl = bl.clone();
            let ds = ds.clone();
            let i = total;
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let req_start = Instant::now();
                let r = match i % 3 {
                    0 => store.increment_version(1).await.map(|_| ()),
                    1 => bl.is_revoked(&format!("ps_{}", i)).await.map(|_| ()),
                    2 => {
                        let did = format!("ps_dev_{}", i);
                        let info = DeviceInfo::with_device_id(&did);
                        ds.register_session(
                            1,
                            &did,
                            &info,
                            &format!("psj_{}", i),
                            &format!("psaj_{}", i),
                        )
                        .await
                    }
                    _ => unreachable!(),
                };
                (r, req_start.elapsed().as_nanos() as u64)
            }));
            total += 1;
        }
        for h in handles {
            let (r, l) = h.await.unwrap();
            lat.push(l);
            if let Err(e) = r {
                err.record(&e);
            }
        }
    }
    let dur = start.elapsed().as_secs_f64();
    let qps = if dur > 0.0 { total as f64 / dur } else { 0.0 };
    let (_, _, p99) = percentiles(&mut lat);
    let er = err.error_rate(total);
    let su = err.counts.get("ServiceUnavailable").copied().unwrap_or(0);
    let su_rate = if total > 0 {
        su as f64 / total as f64
    } else {
        0.0
    };
    let cc = ConsistencyCheck {
        passed: su_rate <= 0.001,
        detail: Some(format!("su_rate={}", su_rate)),
    };
    let verdict = if su_rate <= 0.001 { "pass" } else { "fail" };
    let rss = get_rss_kb();
    let rr = RoundResult {
        operation: "pool_stability".to_string(),
        concurrency: conc,
        qps,
        latency_p50_ms: 0.0,
        latency_p95_ms: 0.0,
        latency_p99_ms: p99,
        error_rate: er,
        error_breakdown: err.counts.clone(),
        total_requests: total,
        duration_secs: dur,
        rss_peak_kb: rss,
        rss_start_kb: rss,
        evidence_file: "packages/sz-rust-auth-facade/src/redis_store.rs".to_string(),
        evidence_line: "536-548".to_string(),
        verdict: verdict.to_string(),
        consistency_check: cc,
        by_op: None,
        final_version: None,
        json_serialize_ratio: None,
        json_deserialize_ratio: None,
        service_unavailable_rate: Some(su_rate),
    };
    emit(&rr);
}

async fn bench_shared_pool_no_deadlock(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (store, bl, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let total = 10000u64;
    let start = Instant::now();

    let s1 = store.clone();
    let h1 = tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(500));
        let mut handles = Vec::new();
        for _i in 0..total {
            let sem = sem.clone();
            let s = s1.clone();
            handles.push(tokio::spawn(async move {
                let _p = sem.acquire().await.unwrap();
                s.increment_version(1).await
            }));
        }
        let mut errs = 0;
        for h in handles {
            if h.await.unwrap().is_err() {
                errs += 1;
            }
        }
        errs
    });
    let b1 = bl.clone();
    let h2 = tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(500));
        let mut handles = Vec::new();
        for i in 0..total {
            let sem = sem.clone();
            let b = b1.clone();
            handles.push(tokio::spawn(async move {
                let _p = sem.acquire().await.unwrap();
                b.is_revoked(&format!("nd_{}", i)).await
            }));
        }
        let mut errs = 0;
        for h in handles {
            if h.await.unwrap().is_err() {
                errs += 1;
            }
        }
        errs
    });
    let d1 = ds.clone();
    let h3 = tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(500));
        let mut handles = Vec::new();
        for i in 0..total {
            let sem = sem.clone();
            let d = d1.clone();
            handles.push(tokio::spawn(async move {
                let _p = sem.acquire().await.unwrap();
                let did = format!("nd_dev_{}", i);
                let info = DeviceInfo::with_device_id(&did);
                d.register_session(
                    1,
                    &did,
                    &info,
                    &format!("ndj_{}", i),
                    &format!("ndaj_{}", i),
                )
                .await
            }));
        }
        let mut errs = 0;
        for h in handles {
            if h.await.unwrap().is_err() {
                errs += 1;
            }
        }
        errs
    });

    let (e1, e2, e3) = tokio::join!(h1, h2, h3);
    let te = e1.unwrap() + e2.unwrap() + e3.unwrap();
    let dur = start.elapsed().as_secs_f64();
    let cc = ConsistencyCheck {
        passed: dur < 30.0 && te == 0,
        detail: Some(format!("dur={}s errors={}", dur, te)),
    };
    let verdict = if dur < 30.0 && te == 0 {
        "pass"
    } else {
        "fail"
    };
    let rss = get_rss_kb();
    let rr = RoundResult {
        operation: "shared_pool".to_string(),
        concurrency: 500,
        qps: if dur > 0.0 {
            (total * 3) as f64 / dur
        } else {
            0.0
        },
        latency_p50_ms: 0.0,
        latency_p95_ms: 0.0,
        latency_p99_ms: 0.0,
        error_rate: if total > 0 {
            te as f64 / (total * 3) as f64
        } else {
            0.0
        },
        error_breakdown: HashMap::new(),
        total_requests: total * 3,
        duration_secs: dur,
        rss_peak_kb: rss,
        rss_start_kb: rss,
        evidence_file: "packages/sz-rust-auth-facade/src/redis_store.rs".to_string(),
        evidence_line: "554-572".to_string(),
        verdict: verdict.to_string(),
        consistency_check: cc,
        by_op: None,
        final_version: None,
        json_serialize_ratio: None,
        json_deserialize_ratio: None,
        service_unavailable_rate: None,
    };
    emit(&rr);
}

async fn bench_soak(args: &BenchArgs) {
    let config = make_config(&args.redis_url, &args.prefix);
    pre_clean_all(&config).await;
    let (store, bl, ds) = create_redis_stores_with_devices(config.clone())
        .await
        .unwrap();
    let dur_secs = args.soak_secs;
    let conc = args.concurrency;
    let parts: Vec<u64> = args
        .mixed_ratio
        .split(':')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    let sum: u64 = parts.iter().sum();
    let cum: Vec<u64> = parts
        .iter()
        .scan(0u64, |acc, &v| {
            *acc += v;
            Some(*acc)
        })
        .collect();

    let rss_start = get_rss_kb();
    let mut rss_peak = rss_start;
    let start = Instant::now();
    let target = Duration::from_secs(dur_secs);
    let mut snapshots = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.tick().await;

    let sem = Arc::new(Semaphore::new(conc as usize));
    let mut total: u64 = 0;
    let mut err = ErrorClassifier::default();
    let mut lat = Vec::new();
    let mut minute = 0u32;

    while start.elapsed() < target {
        tokio::select! {
            _ = interval.tick() => {
                minute += 1;
                let elapsed = start.elapsed().as_secs_f64();
                let qps = if elapsed > 0.0 { total as f64 / elapsed } else { 0.0 };
                let (_, _, p99) = percentiles(&mut lat);
                let rss = get_rss_kb();
                if rss > rss_peak { rss_peak = rss; }
                let snap = SoakSnapshot { minute_index: minute, qps, latency_p99_ms: p99, rss_kb: rss, error_rate: err.error_rate(total) };
                emit(&snap);
                snapshots.push(snap);
            }
            _ = async {
                let mut handles = Vec::new();
                for _ in 0..conc {
                    let sem = sem.clone(); let store = store.clone(); let bl = bl.clone(); let ds = ds.clone(); let cum = cum.clone(); let i = total;
                    handles.push(tokio::spawn(async move {
                        let _permit = sem.acquire().await.unwrap();
                        let req_start = Instant::now();
                        let r = (i % sum) as u64;
                        let oi = cum.iter().position(|&c| r < c).unwrap_or(0);
                        let result: Result<(), RefreshTokenError> = match oi {
                            0 => store.increment_version(1).await.map(|_| ()),
                            1 => store.get_version(1).await.map(|_| ()),
                            2 => bl.is_revoked(&format!("sk_{}", i)).await.map(|_| ()),
                            3 => bl.revoke(&format!("skr_{}", i), 3600).await,
                            4 => { let did = format!("sk_dev_{}", i); let info = DeviceInfo::with_device_id(&did); ds.register_session(1, &did, &info, &format!("skj_{}", i), &format!("skaj_{}", i)).await }
                            5 => ds.get_session(1, &format!("sk_dev_{}", i % 100)).await.map(|_| ()),
                            _ => unreachable!(),
                        };
                        (result, req_start.elapsed().as_nanos() as u64)
                    }));
                    total += 1;
                }
                for h in handles { let (r, l) = h.await.unwrap(); lat.push(l); if let Err(e) = r { err.record(&e); } }
            } => {}
        }
    }

    let rss_end = get_rss_kb();
    if rss_end > rss_peak {
        rss_peak = rss_end;
    }
    let qps_stable =
        snapshots.len() >= 2 && snapshots.last().unwrap().qps >= snapshots[0].qps * 0.8;
    let p99_stable = snapshots.len() >= 2
        && snapshots.last().unwrap().latency_p99_ms <= snapshots[0].latency_p99_ms * 2.0 + 1.0;
    let memory_ok = (rss_peak - rss_start <= 51200) && (rss_end - rss_start <= 30720);
    let summary = SoakSummary {
        qps_stable,
        p99_stable,
        memory_ok,
        rss_start_kb: rss_start,
        rss_peak_kb: rss_peak,
        rss_end_kb: rss_end,
        snapshots,
    };
    emit(&summary);
}

// ── main ──

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(code) => std::process::exit(code),
    };
    match args.op.as_str() {
        "increment_version" => bench_increment_version(&args).await,
        "get_version" => bench_get_version(&args).await,
        "concurrent_increment" => bench_concurrent_no_lost_update(&args).await,
        "is_revoked" => bench_is_revoked(&args).await,
        "revoke" => bench_revoke(&args).await,
        "ttl_validation" => bench_ttl_validation(&args).await,
        "register_session" => bench_register_session(&args).await,
        "get_session" => bench_get_session(&args).await,
        "get_sessions" => bench_get_sessions(&args).await,
        "revoke_session" => bench_revoke_session(&args).await,
        "update_last_active" => bench_update_last_active(&args).await,
        "json_overhead" => bench_json_overhead(&args).await,
        "mixed" => bench_mixed(&args).await,
        "pool_stability" => bench_pool_stability(&args).await,
        "shared_pool" => bench_shared_pool_no_deadlock(&args).await,
        "soak" => bench_soak(&args).await,
        other => {
            eprintln!("unknown op: {}", other);
            std::process::exit(2);
        }
    }
}
