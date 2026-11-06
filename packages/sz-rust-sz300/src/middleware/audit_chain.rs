// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 审计链式哈希中间件
//!
//! 拦截 POST/PUT/DELETE 请求（排除 /health /metrics /api-docs），记录审计日志并维护链式哈希。
//!
//! ## 落库（2026-10-28 数据层修复 + 2026-10-29 本地缓冲重放）
//!
//! 此前审计记录只存在于内存（`ChainHashAuditor` 只保留 last_hash，记录本身直接丢弃），
//! 进程重启即丢失，不满足审计留痕要求。现在每条记录通过有界异步缓冲写入 `operate_log`：
//! `detail` 保存结构化 JSON（含 chain_hash），`operator` 取自 JWT 鉴权注入的
//! `CurrentUser`，`ip` 取自 `X-Forwarded-For` / `X-Real-IP`。
//!
//! ## 落库策略
//!
//! - 请求路径只把记录投递到有界内存通道（`AUDIT_BUFFER_CAPACITY`）后立即返回，
//!   不再同步等待数据库写入 —— 消除慢库对业务延迟的影响（旧实现每写请求最长等 1 s）。
//! - 后台任务从通道收拢批次（最多 [`AUDIT_BATCH_SIZE`] 条，只取已就绪记录、不等待凑批）
//!   合并为一条 multi-value INSERT 写入 `operate_log`（2026-11-04 批量化，消除
//!   单条串行 INSERT 在 c=50 下 10k 缓冲打满丢弃 16,375 条/腿的吞吐瓶颈）；
//!   失败按 `AUDIT_RETRY_DELAY` 间隔整批重试，超过 `AUDIT_MAX_RETRIES` 后丢弃并计数。
//! - 通道满（数据库持续不可写）时新记录被丢弃并计数，保证内存有界。
//! - 丢弃计数暴露为 Prometheus Counter（`sz300_audit_dropped_total`）与
//!   [`AuditSink::dropped_count`]。
//! - `detail` 落库前经 `redact_sensitive` 脱敏。
//!
//! ## 使用方式
//!
//! ```ignore
//! use axum::middleware;
//! use crate::middleware::audit_chain::{audit_chain_middleware, AuditSink};
//!
//! // 缓冲与后台落库任务由应用启动时创建并注入（见 main.rs），此处仅示意中间件挂载。
//! let app = Router::new()
//!     .layer(middleware::from_fn_with_state(
//!         (auditor, Some(audit_sink)),
//!         audit_chain_middleware,
//!     ));
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};
use sz_rust_core::orm::{Pool, Value};
use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};
use sz_rust_observability::Counter;
use tokio::sync::mpsc;

use crate::db::acquire_cancel_safe;
use crate::middleware::auth_middleware::CurrentUser;
use crate::services::redact_sensitive;

/// 排除审计的路径前缀
const EXCLUDED_PATHS: &[&str] = &["/health", "/metrics", "/api-docs"];

/// 单条审计落库最长等待时间（超时视为该次尝试失败，交由重试逻辑处理）
const PERSIST_TIMEOUT: Duration = Duration::from_secs(1);

/// 审计缓冲容量（数据库持续不可写时，新记录在此上限后开始丢弃）
const AUDIT_BUFFER_CAPACITY: usize = 10_000;

/// 单条记录最大重试次数（超过后丢弃并计数）
const AUDIT_MAX_RETRIES: u32 = 5;

/// 批量落库上限：单条 multi-value INSERT 的最大行数。
/// 2026-11-04（DB-2026-10-31-01②）：单条串行 INSERT 的落库吞吐（实测 ≈700 行/s）
/// 低于 c=50 压测的入队速率（≈1.2k 条/s），10k 缓冲打满后单腿丢弃 16,375 条。
/// 200 行 × 5 列 = 1000 参数，远低于 MySQL 65535 占位符上限。
const AUDIT_BATCH_SIZE: usize = 200;

/// 落库失败后的重试间隔
const AUDIT_RETRY_DELAY: Duration = Duration::from_millis(500);

/// 待落库的审计记录（请求侧投递到有界缓冲，后台任务消费）
pub struct AuditJob {
    pub(crate) record: AuditRecord,
    pub(crate) merchant_id: Option<i64>,
    pub(crate) ip: String,
    pub(crate) status_code: u16,
}

/// 审计落库的异步缓冲入口：请求侧只投递，落库由后台任务负责。
///
/// 有界通道（`AUDIT_BUFFER_CAPACITY`）保证内存不随数据库故障无限增长；
/// 丢弃计数同时写入内部原子计数器与可选的 Prometheus Counter。
pub struct AuditSink {
    tx: mpsc::Sender<AuditJob>,
    enqueued: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    dropped_metric: Option<Arc<Counter>>,
}

impl AuditSink {
    /// 创建有界缓冲与接收端。
    ///
    /// `dropped_metric` 非 `None` 时，每条被丢弃的记录会同步累加该 Prometheus Counter。
    /// 调用方负责把返回的 `Receiver` 交给 [`spawn_audit_worker`]（生产）或
    /// [`spawn_audit_drain`]（测试/降级）。
    pub fn new(dropped_metric: Option<Arc<Counter>>) -> (Arc<Self>, mpsc::Receiver<AuditJob>) {
        let (tx, rx) = mpsc::channel(AUDIT_BUFFER_CAPACITY);
        let sink = Arc::new(Self {
            tx,
            enqueued: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            dropped_metric,
        });
        (sink, rx)
    }

    /// 投递一条审计记录；缓冲满或通道关闭时丢弃并计数。
    pub fn enqueue(&self, job: AuditJob) {
        self.enqueued.fetch_add(1, Ordering::Relaxed);
        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(job))
            | Err(mpsc::error::TrySendError::Closed(job)) => {
                self.note_dropped(&job.record.target);
            }
        }
    }

    /// 已投递的审计记录总数（含被丢弃的）
    pub fn enqueued_count(&self) -> u64 {
        self.enqueued.load(Ordering::Relaxed)
    }

    /// 因缓冲满或重试耗尽而被丢弃的审计记录总数
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn note_dropped(&self, target: &str) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        if let Some(c) = &self.dropped_metric {
            c.inc();
        }
        tracing::error!(
            path = %target,
            dropped_total = self.dropped.load(Ordering::Relaxed),
            "审计缓冲已满或重试耗尽，记录丢弃"
        );
    }
}

/// 后台审计落库任务：从有界缓冲收拢批次写入，失败按固定间隔重试，超过上限后整批丢弃并计数。
///
/// 批量策略（2026-11-04，DB-2026-10-31-01②）：阻塞等首条 → `try_recv` 收拢缓冲中
/// 已就绪的最多 [`AUDIT_BATCH_SIZE`] 条合并为一条 multi-value INSERT。
/// 只取已入队记录、不主动等待凑批，低负载时批大小为 1，不增加落库延迟。
pub fn spawn_audit_worker(sink: Arc<AuditSink>, mut rx: mpsc::Receiver<AuditJob>, pool: Arc<Pool>) {
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = Vec::with_capacity(AUDIT_BATCH_SIZE);
            batch.push(first);
            while batch.len() < AUDIT_BATCH_SIZE {
                match rx.try_recv() {
                    Ok(job) => batch.push(job),
                    Err(_) => break,
                }
            }
            if persist_batch(&pool, &batch).await {
                continue;
            }
            let mut retries = 0u32;
            loop {
                tokio::time::sleep(AUDIT_RETRY_DELAY).await;
                if persist_batch(&pool, &batch).await {
                    break;
                }
                retries += 1;
                if retries >= AUDIT_MAX_RETRIES {
                    for job in &batch {
                        sink.note_dropped(&job.record.target);
                    }
                    break;
                }
            }
        }
    });
}

/// 丢弃所有审计记录的后台任务（测试/降级用）：保持通道畅通，避免缓冲满导致的测试噪音。
///
/// 生产环境请使用 [`spawn_audit_worker`] 真正落库。
pub fn spawn_audit_drain(mut rx: mpsc::Receiver<AuditJob>) {
    tokio::spawn(async move {
        while let Some(_job) = rx.recv().await {
            // 丢弃：仅用于测试/降级路径
        }
    });
}

/// 判断路径是否应排除审计
fn should_audit(path: &str, method: &str) -> bool {
    // 仅审计写操作
    if method != "POST" && method != "PUT" && method != "DELETE" {
        return false;
    }
    // 排除健康检查、指标、文档路径
    !EXCLUDED_PATHS.iter().any(|prefix| path.starts_with(prefix))
}

/// 审计链式哈希中间件
///
/// 对 POST/PUT/DELETE 请求（排除 /health /metrics /api-docs）记录审计日志：
/// 先追加到内存链（保证哈希连续性），再投递到有界异步缓冲由后台任务落库。
///
/// 状态为一个二元组：`(Arc<ChainHashAuditor>, Option<Arc<AuditSink>>)`。
/// `sink` 为 `None` 时仅维护内存链，不落库（测试/未接线场景）。
pub async fn audit_chain_middleware(
    State((auditor, sink)): State<(Arc<ChainHashAuditor>, Option<Arc<AuditSink>>)>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let should_record = should_audit(&path, &method);

    // 在 next.run 之前取出：鉴权中间件（外层）已把 CurrentUser 注入请求扩展
    let current_user = req.extensions().get::<CurrentUser>().cloned();
    let client_ip = client_ip(&req);

    let resp = next.run(req).await;

    if should_record {
        let status = resp.status();
        let result = if status.is_success() {
            "success"
        } else {
            "failure"
        };

        let mut record = AuditRecord::new(
            current_user
                .as_ref()
                .map(|u| u.username.clone())
                .unwrap_or_else(|| "system".to_string()),
            &method,
            &path,
            result,
        )
        .with_field("status_code", status.as_u16().to_string());

        auditor.append(&mut record);
        tracing::debug!(
            chain_hash = %record.chain_hash,
            method = %method,
            path = %path,
            "审计记录已追加"
        );

        if let Some(sink) = sink {
            let job = AuditJob {
                record,
                merchant_id: current_user.as_ref().and_then(|u| u.merchant_id),
                ip: client_ip,
                status_code: status.as_u16(),
            };
            sink.enqueue(job);
        }
    }

    resp
}

/// 将审计记录写入 `operate_log`；成功返回 `true`。
///
/// 失败/超时只记 error 日志并返回 `false`（由后台任务重试）：审计是旁路，不能因为库抖动让写请求失败。
async fn persist_batch(pool: &Pool, batch: &[AuditJob]) -> bool {
    if batch.is_empty() {
        return true;
    }

    // detail 构造与逐条版完全一致（含 chain_hash 与脱敏）
    let rows: Vec<(Option<i64>, String, String, String, String)> = batch
        .iter()
        .map(|job| {
            let record = &job.record;
            let detail = serde_json::json!({
                "method": record.action,
                "path": record.target,
                "result": record.result,
                "status_code": job.status_code,
                "chain_hash": record.chain_hash,
                "timestamp": record.timestamp.to_rfc3339(),
            })
            .to_string();
            (
                job.merchant_id,
                truncate_chars(&record.actor, 50),
                truncate_chars(&record.action, 50),
                redact_sensitive(&detail),
                truncate_chars(&job.ip, 45),
            )
        })
        .collect();

    let mut sql = String::with_capacity(batch.len() * 48 + 72);
    sql.push_str("INSERT INTO operate_log (merchant_id, operator, action, detail, ip) VALUES ");
    for i in 0..batch.len() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str("(?, ?, ?, ?, ?)");
    }

    let mut params = Vec::with_capacity(batch.len() * 5);
    for (merchant_id, operator, action, detail, ip) in &rows {
        // merchant_id 改 NULLABLE 后，None 写 NULL（019_nullable_operate_log_merchant.sql）
        params.push(merchant_id.map(Value::I64).unwrap_or(Value::Null));
        params.push(Value::String(operator.clone()));
        params.push(Value::String(action.clone()));
        params.push(Value::String(detail.clone()));
        params.push(Value::String(ip.clone()));
    }

    let write = async {
        // sz-orm 10.13 的 Pool::acquire 返回独立 PoolError，未提供 From<PoolError> for DbError，
        // 需显式下沉到 DbError::PoolError 才能与 execute_with_params 的错误类型统一。
        let mut conn = acquire_cancel_safe(pool)
            .await
            .map_err(sz_rust_core::orm::DbError::PoolError)?;
        conn.execute_with_params(&sql, &params).await
    };

    match tokio::time::timeout(PERSIST_TIMEOUT, write).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::error!(
                error = %e,
                path = %batch[0].record.target,
                batch_size = batch.len(),
                "审计记录批量落库失败（将整批重试）"
            );
            false
        }
        Err(_) => {
            tracing::error!(
                path = %batch[0].record.target,
                batch_size = batch.len(),
                "审计记录批量落库超时（{}s），将整批重试",
                PERSIST_TIMEOUT.as_secs()
            );
            false
        }
    }
}

/// 客户端 IP（优先 `X-Forwarded-For` 首段，其次 `X-Real-IP`）
///
/// 代理头可被客户端伪造，仅作审计线索，不用于任何授权判断。
fn client_ip(req: &Request<Body>) -> String {
    let forwarded = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(ip) = forwarded {
        return ip.to_string();
    }

    req.headers()
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 按字符（非字节）截断，避免切断多字节 UTF-8 导致写库失败
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

/// 提取审计记录的辅助函数（用于测试）
pub fn make_audit_record(method: &str, path: &str, result: &str) -> AuditRecord {
    AuditRecord::new("system", method, path, result)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> AuditJob {
        AuditJob {
            record: make_audit_record("POST", "/api/v1/test", "success"),
            merchant_id: None,
            ip: "127.0.0.1".to_string(),
            status_code: 200,
        }
    }

    #[tokio::test]
    async fn test_audit_sink_enqueue_reaches_receiver() {
        let (sink, mut rx) = AuditSink::new(None);
        sink.enqueue(job());
        let got = rx.recv().await.expect("应收到审计记录");
        assert_eq!(got.record.target, "/api/v1/test");
        assert_eq!(sink.enqueued_count(), 1);
        assert_eq!(sink.dropped_count(), 0);
    }

    #[test]
    fn test_audit_sink_enqueue_after_receiver_dropped_increments_dropped() {
        let (sink, rx) = AuditSink::new(None);
        drop(rx);
        sink.enqueue(job());
        assert_eq!(sink.enqueued_count(), 1);
        assert_eq!(sink.dropped_count(), 1, "通道关闭后投递应计为丢弃");
    }

    #[test]
    fn test_audit_sink_dropped_metric_increments() {
        let registry = sz_rust_observability::MetricsRegistry::new();
        let counter = registry.register_counter("test_audit_dropped_total", "test");
        let (sink, rx) = AuditSink::new(Some(counter.clone()));
        drop(rx);
        sink.enqueue(job());
        assert_eq!(counter.value(), 1.0, "Prometheus Counter 应同步递增");
        assert_eq!(sink.dropped_count(), 1);
    }

    // =========================================================================
    // should_audit 纯逻辑测试（验收标准 1-5）
    // =========================================================================

    #[test]
    fn should_audit_get_returns_false() {
        // GET 非写操作 → 不审计
        assert!(!should_audit("/api/v1/order", "GET"));
    }

    #[test]
    fn should_audit_post_health_returns_false() {
        // POST 但路径在排除列表 → 不审计
        assert!(!should_audit("/health", "POST"));
    }

    #[test]
    fn should_audit_post_order_returns_true() {
        // POST 且路径不在排除列表 → 审计
        assert!(should_audit("/api/v1/order", "POST"));
    }

    #[test]
    fn should_audit_delete_metrics_returns_false() {
        // DELETE 但路径在排除列表 → 不审计
        assert!(!should_audit("/metrics", "DELETE"));
    }

    #[test]
    fn should_audit_put_api_docs_returns_false() {
        // PUT 但路径在排除列表 → 不审计
        assert!(!should_audit("/api-docs", "PUT"));
    }

    // =========================================================================
    // client_ip 纯逻辑测试（验收标准 6-8）
    // =========================================================================

    #[test]
    fn client_ip_from_x_forwarded_for_first_segment() {
        let req = Request::builder()
            .header("x-forwarded-for", "  203.0.113.5 , 198.51.100.1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(client_ip(&req), "203.0.113.5");
    }

    #[test]
    fn client_ip_falls_back_to_x_real_ip() {
        let req = Request::builder()
            .header("x-real-ip", "  10.0.0.42  ")
            .body(Body::empty())
            .unwrap();
        assert_eq!(client_ip(&req), "10.0.0.42");
    }

    #[test]
    fn client_ip_no_proxy_headers_returns_empty() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(client_ip(&req), "");
    }

    // =========================================================================
    // truncate_chars 纯逻辑测试（验收标准 9-10）
    // =========================================================================

    #[test]
    fn truncate_chars_multibyte_truncates_by_char_not_byte() {
        // 6 个中文字符（18 字节），max_chars=3 → 保留前 3 个字符，不破坏 UTF-8
        let input = "鲜视达测试用";
        let out = truncate_chars(input, 3);
        assert_eq!(out, "鲜视达");
        assert_eq!(out.chars().count(), 3);
    }

    #[test]
    fn truncate_chars_within_limit_returns_unchanged() {
        let input = "abc鲜视达";
        let out = truncate_chars(input, 100);
        assert_eq!(out, input);
    }

    // =========================================================================
    // audit_chain_middleware oneshot 测试（验收标准 11-13）
    // =========================================================================

    #[tokio::test]
    async fn middleware_get_does_not_record_audit() {
        use axum::{middleware, routing::get, Router};
        use tower::ServiceExt;

        let auditor = Arc::new(ChainHashAuditor::new());
        let state = (auditor.clone(), None);
        let app = Router::new()
            .route("/api/v1/order", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, audit_chain_middleware));

        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/order")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(
            auditor.current_hash(),
            "",
            "GET 请求不应触发审计记录，链式哈希应保持初始空值"
        );
    }

    #[tokio::test]
    async fn middleware_post_sink_none_appends_hash_without_enqueue() {
        use axum::{middleware, routing::post, Router};
        use tower::ServiceExt;

        let auditor = Arc::new(ChainHashAuditor::new());
        let state = (auditor.clone(), None);
        let app = Router::new()
            .route("/api/v1/order", post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, audit_chain_middleware));

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/order")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_ne!(
            auditor.current_hash(),
            "",
            "POST 请求应追加链式哈希（sink=None 仅维护内存链）"
        );
    }

    #[tokio::test]
    async fn middleware_post_sink_some_appends_hash_and_enqueues_job() {
        use axum::{middleware, routing::post, Router};
        use tower::ServiceExt;

        let auditor = Arc::new(ChainHashAuditor::new());
        let (sink, mut rx) = AuditSink::new(None);
        let state = (auditor.clone(), Some(sink));
        let app = Router::new()
            .route("/api/v1/order", post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, audit_chain_middleware));

        let current_user = CurrentUser {
            user_id: 1,
            username: "test".into(),
            merchant_id: Some(1),
            is_admin: false,
        };
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/order")
            .extension(current_user)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_ne!(auditor.current_hash(), "", "POST 请求应追加链式哈希");
        let job = rx
            .try_recv()
            .expect("sink=Some 时 POST 请求应投递 AuditJob 到通道");
        assert_eq!(job.record.target, "/api/v1/order");
        assert_eq!(job.record.action, "POST");
        assert_eq!(job.merchant_id, Some(1));
    }

    // =========================================================================
    // persist_batch fake pool 测试（验收标准 14-15）
    // =========================================================================

    /// 构造不可达的 fake pool（端口 1 必 ECONNREFUSED），供 persist_batch 错误路径测试。
    fn make_fake_pool() -> Pool {
        use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
        use sz_rust_core::orm::PoolConfigBuilder;
        let connect_options = sqlx::mysql::MySqlConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("root")
            .password("")
            .database("nonexistent");
        let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy_with(connect_options);
        let factory =
            SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));
        let pool_cfg = PoolConfigBuilder::new()
            .max_size(1)
            .min_idle(0)
            .build()
            .unwrap();
        Pool::new(pool_cfg, Arc::new(factory)).unwrap()
    }

    #[tokio::test]
    async fn persist_batch_empty_returns_true() {
        let pool = make_fake_pool();
        let batch: Vec<AuditJob> = vec![];
        let ok = persist_batch(&pool, &batch).await;
        assert!(ok, "空 batch 应直接返回 true，不触碰数据库");
    }

    #[tokio::test]
    async fn persist_batch_nonempty_unreachable_pool_returns_false() {
        let pool = make_fake_pool();
        let batch = vec![job()];
        let ok = persist_batch(&pool, &batch).await;
        assert!(
            !ok,
            "pool 不可达（端口 1 ECONNREFUSED）时非空 batch 应返回 false 以触发重试"
        );
    }
}
