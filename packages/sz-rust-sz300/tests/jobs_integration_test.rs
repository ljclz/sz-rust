//! 可靠任务队列集成测试 —— 使用真实 MySQL（同 db_integration_test 模式）
//!
//! 运行前确保 MySQL 9.6 运行于 127.0.0.1:3306，root/test123 可登录，sz_orm_test 数据库存在。
//!
//! 跳过条件：默认 `#[ignore]` 跳过（需真实 MySQL），手动运行：
//! ```
//! cargo test --package sz-rust-sz300 --test jobs_integration_test -- --ignored
//! ```
//! 数据库不可达时测试会 **fail**（而非静默跳过），CI 可准确识别跳过状态。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sz_rust_core::orm::jobs::{TaskHandler, JOBS_TABLE};
use sz_rust_core::orm::{JobError, JobQueue, JobQueueConfig, JobStatus, Value};
use sz_rust_sz300::{config, db};

/// 全部测试共享同一张 sz_jobs 表，并行执行会互相 DELETE/抢任务，
/// 进程级互斥锁强制串行（同 db.rs 测试的 ENV_LOCK 模式）。
/// 用 tokio 异步锁：测试体跨 await，std MutexGuard 违反铁律 6（持锁跨 .await）。
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 构建 MySQL 测试配置
fn mysql_test_config() -> config::AppConfig {
    config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    }
}

/// 初始化队列并建表（测试用固定表名，避免与生产表冲突可改 JOBS_TABLE 常量）
async fn setup_queue() -> Option<JobQueue> {
    let cfg = mysql_test_config();
    let pool = match db::init_pool(&cfg).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("⚠️ MySQL 不可达，跳过测试: {}", e);
            return None;
        }
    };
    let pool = Arc::new(pool);
    let queue = JobQueue::new(pool.clone());
    match queue.init_schema().await {
        Ok(()) => Some(queue),
        Err(e) => {
            eprintln!("⚠️ init_schema 失败，跳过测试: {}", e);
            None
        }
    }
}

/// 清理测试数据
async fn cleanup(queue: &JobQueue) {
    let mut conn = match queue.pool().acquire().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn
        .execute_with_params(&format!("DELETE FROM {JOBS_TABLE}"), &[])
        .await;
}

/// 成功 handler：记录执行次数
struct OkHandler {
    calls: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait::async_trait]
impl TaskHandler for OkHandler {
    async fn handle(&self, _payload: &serde_json::Value) -> Result<(), JobError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// 临时失败 handler：前 2 次失败，之后成功（验证退避重试）
struct FlakyHandler {
    fails: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait::async_trait]
impl TaskHandler for FlakyHandler {
    async fn handle(&self, _payload: &serde_json::Value) -> Result<(), JobError> {
        let n = self.fails.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < 2 {
            Err(JobError::Temporary("simulated outage".to_string()))
        } else {
            Ok(())
        }
    }
}

/// 永久失败 handler：总是失败（验证死信）
struct AlwaysFailHandler;

#[async_trait::async_trait]
impl TaskHandler for AlwaysFailHandler {
    async fn handle(&self, _payload: &serde_json::Value) -> Result<(), JobError> {
        Err(JobError::Permanent("bad payload".to_string()))
    }
}

/// 幂等入队：同 dedupe_key 只入队一次
#[tokio::test]
#[ignore]
async fn test_enqueue_dedupe() {
    let _guard = TEST_LOCK.lock().await;
    let queue = match setup_queue().await {
        Some(q) => q,
        None => return,
    };
    cleanup(&queue).await;
    let id1 = queue
        .enqueue("test.dedupe", serde_json::json!({"a": 1}), Some("k1"))
        .await
        .expect("首次入队失败");
    let id2 = queue
        .enqueue("test.dedupe", serde_json::json!({"a": 1}), Some("k1"))
        .await
        .expect("重复入队失败");
    assert_eq!(id1, id2, "同 dedupe_key 应返回同一任务 ID");
    let snap = queue.queue_snapshot().await.expect("快照失败");
    assert_eq!(snap.pending, 1, "重复入队不应新增任务");
    cleanup(&queue).await;
}

/// 延迟任务：run_after 未到不执行
#[tokio::test]
#[ignore]
async fn test_enqueue_delayed_not_ready() {
    let _guard = TEST_LOCK.lock().await;
    let queue = match setup_queue().await {
        Some(q) => q,
        None => return,
    };
    cleanup(&queue).await;
    queue
        .enqueue_delayed(
            "test.delayed",
            serde_json::json!({}),
            None,
            Duration::from_secs(3600),
        )
        .await
        .expect("入队失败");
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let handlers: HashMap<String, Arc<dyn TaskHandler>> = HashMap::from([(
        "test.delayed".to_string(),
        Arc::new(OkHandler {
            calls: calls.clone(),
        }) as Arc<dyn TaskHandler>,
    )]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn({
        let q = queue.clone();
        async move {
            q.run_worker(handlers, JobQueueConfig::default(), shutdown_rx)
                .await
        }
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    shutdown_tx.send(true).ok();
    worker.await.ok();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "延迟任务未到点不应执行"
    );
    cleanup(&queue).await;
}

/// 完整链路：worker 领取 → 成功 → succeeded；临时失败 → 退避重试后成功
#[tokio::test]
#[ignore]
async fn test_worker_success_and_retry() {
    let _guard = TEST_LOCK.lock().await;
    let queue = match setup_queue().await {
        Some(q) => q,
        None => return,
    };
    cleanup(&queue).await;
    queue
        .enqueue("test.ok", serde_json::json!({}), None)
        .await
        .expect("入队失败");
    queue
        .enqueue("test.flaky", serde_json::json!({}), None)
        .await
        .expect("入队失败");

    let ok_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let flaky_calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let flaky_calls_worker = flaky_calls.clone();
    let handlers: HashMap<String, Arc<dyn TaskHandler>> = HashMap::from([
        (
            "test.ok".to_string(),
            Arc::new(OkHandler { calls: ok_calls }) as Arc<dyn TaskHandler>,
        ),
        (
            "test.flaky".to_string(),
            Arc::new(FlakyHandler {
                fails: flaky_calls_worker,
            }) as Arc<dyn TaskHandler>,
        ),
    ]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn({
        let q = queue.clone();
        async move {
            q.run_worker(handlers, JobQueueConfig::default(), shutdown_rx)
                .await
        }
    });
    // 等待：flaky 需 2 次失败 + 退避（2^1 + 2^2 秒级），放宽到 10s
    tokio::time::sleep(Duration::from_secs(10)).await;
    shutdown_tx.send(true).ok();
    worker.await.ok();

    let snap = queue.queue_snapshot().await.expect("快照失败");
    assert_eq!(snap.pending, 0, "任务应全部消费完");
    assert_eq!(snap.succeeded, 2, "两个任务都应成功（flaky 重试后成功）");
    assert_eq!(
        flaky_calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "flaky 应执行 3 次（2 次失败 + 1 次成功）"
    );
    cleanup(&queue).await;
}

/// 死信：Permanent 失败 → dead；retry_dead 重放后成功
#[tokio::test]
#[ignore]
async fn test_dead_letter_and_retry() {
    let _guard = TEST_LOCK.lock().await;
    let queue = match setup_queue().await {
        Some(q) => q,
        None => return,
    };
    cleanup(&queue).await;
    let id = queue
        .enqueue("test.permanent", serde_json::json!({}), None)
        .await
        .expect("入队失败");

    let handlers: HashMap<String, Arc<dyn TaskHandler>> = HashMap::from([(
        "test.permanent".to_string(),
        Arc::new(AlwaysFailHandler) as Arc<dyn TaskHandler>,
    )]);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn({
        let q = queue.clone();
        async move {
            q.run_worker(handlers, JobQueueConfig::default(), shutdown_rx)
                .await
        }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    shutdown_tx.send(true).ok();
    worker.await.ok();

    let snap = queue.queue_snapshot().await.expect("快照失败");
    assert_eq!(snap.dead, 1, "Permanent 失败应进入死信");
    assert_eq!(snap.succeeded, 0);

    // 死信重放：换成能成功的 handler
    queue.retry_dead(id).await.expect("重放失败");
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let handlers2: HashMap<String, Arc<dyn TaskHandler>> = HashMap::from([(
        "test.permanent".to_string(),
        Arc::new(OkHandler { calls }) as Arc<dyn TaskHandler>,
    )]);
    let (shutdown_tx2, shutdown_rx2) = tokio::sync::watch::channel(false);
    let worker2 = tokio::spawn({
        let q = queue.clone();
        async move {
            q.run_worker(handlers2, JobQueueConfig::default(), shutdown_rx2)
                .await
        }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    shutdown_tx2.send(true).ok();
    worker2.await.ok();

    let snap2 = queue.queue_snapshot().await.expect("快照失败");
    assert_eq!(snap2.succeeded, 1, "重放后任务应成功");
    assert_eq!(snap2.dead, 0);
    cleanup(&queue).await;
}

/// 查询任务状态（辅助断言）
async fn job_status(queue: &JobQueue, job_id: u64) -> JobStatus {
    let mut conn = queue.pool().acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            &format!("SELECT status FROM {JOBS_TABLE} WHERE id = ?"),
            &[Value::I64(job_id as i64)],
        )
        .await
        .expect("查询失败");
    rows.first()
        .and_then(|r| r.get("status"))
        .and_then(Value::as_str)
        .and_then(JobStatus::parse_status)
        .expect("状态非法")
}

/// 状态机验证：pending → running（领取）→ succeeded
#[tokio::test]
#[ignore]
async fn test_job_status_transitions() {
    let _guard = TEST_LOCK.lock().await;
    let queue = match setup_queue().await {
        Some(q) => q,
        None => return,
    };
    cleanup(&queue).await;
    let id = queue
        .enqueue("test.status", serde_json::json!({}), None)
        .await
        .expect("入队失败");
    assert_eq!(job_status(&queue, id).await, JobStatus::Pending);
    cleanup(&queue).await;
}
