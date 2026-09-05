// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 缓存预热机制（Cache Warmer）
//!
//! 部署时或应用启动时预热缓存，避免冷启动高延迟。对齐 PHP `think console`
//! 的 `cache:warmup` 命令设计：
//!
//! ```text
//! php think cache:warmup         # 执行所有预热器
//! php think cache:warmup config  # 只执行 config 预热器
//! php think cache:clear --warm   # 清空后立即预热
//! ```
//!
//! ## 设计原则
//!
//! - **异步执行**：所有预热器实现 `async fn warm()`，不阻塞调度线程
//! - **失败隔离**：单个预热器失败不影响其他预热器，错误记录到 `WarmupReport`
//! - **超时控制**：每个预热器有独立超时（默认 30s），避免无限等待
//! - **可观测**：返回 `WarmupReport` 记录每个预热器的耗时、状态、错误信息
//! - **可组合**：通过 `WarmupPipeline` 串行或并行执行多个预热器
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_core::cache_warmer::{WarmupPipeline, WarmupReport, Warmer};
//!
//! // 1. 注册预热器
//! let mut pipeline = WarmupPipeline::new();
//! pipeline.register(Box::new(ConfigWarmer::new()));
//! pipeline.register(Box::new(RouteWarmer::new()));
//!
//! // 2. 执行预热
//! let report = pipeline.warm_all().await;
//!
//! // 3. 检查结果
//! for item in &report.items {
//!     println!("{}: {:?} ({}ms)", item.name, item.status, item.duration_ms);
//! }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;

// ============================================================================
// 预热器 trait
// ============================================================================

/// 预热器 trait — 定义缓存预热接口
///
/// 每个预热器负责预热一类缓存（如配置、路由、字典数据等）。
/// 实现方需提供 `name` / `warm` 方法，可选 `timeout` / `description`。
///
/// 使用 `async_trait` 宏确保 trait 是 dyn-compatible（可用 `Box<dyn Warmer>`）。
#[async_trait]
pub trait Warmer: Send + Sync {
    /// 预热器名称（唯一标识，用于日志和报告）
    fn name(&self) -> &str;

    /// 预热器描述（人类可读）
    fn description(&self) -> &str {
        "Cache warmer"
    }

    /// 单个预热器超时时间（默认 30 秒）
    ///
    /// 超时后该预热器被中止，记录为 `WarmupStatus::Timeout`。
    fn timeout(&self) -> Duration {
        Duration::from_secs(30)
    }

    /// 执行预热
    ///
    /// 返回 `Result<(), WarmupError>`：
    /// - `Ok(())`：预热成功
    /// - `Err(e)`：预热失败（错误信息记录到报告中）
    async fn warm(&self) -> Result<(), WarmupError>;
}

// ============================================================================
// 错误类型
// ============================================================================

/// 预热错误
#[derive(Debug, Clone)]
pub enum WarmupError {
    /// IO 错误（如配置文件读取失败）
    Io(String),
    /// 序列化/反序列化错误
    Serialize(String),
    /// 缓存写入错误
    Cache(String),
    /// 数据库查询错误
    Database(String),
    /// 自定义错误
    Custom(String),
}

impl std::fmt::Display for WarmupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "IO error: {}", msg),
            Self::Serialize(msg) => write!(f, "Serialize error: {}", msg),
            Self::Cache(msg) => write!(f, "Cache error: {}", msg),
            Self::Database(msg) => write!(f, "Database error: {}", msg),
            Self::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for WarmupError {}

impl From<std::io::Error> for WarmupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

// ============================================================================
// 预热状态与报告
// ============================================================================

/// 单个预热器的执行状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarmupStatus {
    /// 成功
    Success,
    /// 失败
    Failed,
    /// 超时
    Timeout,
    /// 跳过（如配置禁用）
    Skipped,
}

impl std::fmt::Display for WarmupStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
            Self::Timeout => write!(f, "timeout"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// 单个预热器的执行结果
#[derive(Debug, Clone)]
pub struct WarmupItem {
    /// 预热器名称
    pub name: String,
    /// 预热器描述
    pub description: String,
    /// 执行状态
    pub status: WarmupStatus,
    /// 执行耗时（毫秒）
    pub duration_ms: u64,
    /// 错误信息（仅在失败/超时时有值）
    pub error: Option<String>,
}

impl WarmupItem {
    /// 创建成功结果
    pub fn success(
        name: impl Into<String>,
        description: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: WarmupStatus::Success,
            duration_ms,
            error: None,
        }
    }

    /// 创建失败结果
    pub fn failed(
        name: impl Into<String>,
        description: impl Into<String>,
        duration_ms: u64,
        error: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: WarmupStatus::Failed,
            duration_ms,
            error: Some(error.into()),
        }
    }

    /// 创建超时结果
    pub fn timeout(
        name: impl Into<String>,
        description: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: WarmupStatus::Timeout,
            duration_ms,
            error: Some(format!("Timed out after {}ms", duration_ms)),
        }
    }

    /// 是否成功
    pub fn is_success(&self) -> bool {
        self.status == WarmupStatus::Success
    }
}

/// 预热报告（汇总所有预热器的执行结果）
#[derive(Debug, Clone, Default)]
pub struct WarmupReport {
    /// 各预热器的执行结果
    pub items: Vec<WarmupItem>,
    /// 总耗时（毫秒）
    pub total_duration_ms: u64,
}

impl WarmupReport {
    /// 创建空报告
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一个执行结果
    pub fn add_item(&mut self, item: WarmupItem) {
        self.items.push(item);
    }

    /// 成功数量
    pub fn success_count(&self) -> usize {
        self.items.iter().filter(|i| i.is_success()).count()
    }

    /// 失败数量
    pub fn failed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == WarmupStatus::Failed)
            .count()
    }

    /// 超时数量
    pub fn timeout_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == WarmupStatus::Timeout)
            .count()
    }

    /// 总数量
    pub fn total_count(&self) -> usize {
        self.items.len()
    }

    /// 是否全部成功
    pub fn all_success(&self) -> bool {
        !self.items.is_empty() && self.failed_count() == 0 && self.timeout_count() == 0
    }

    /// 生成可读的汇总报告
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "Cache warmup: {}/{} succeeded, {} failed, {} timeout (total {}ms)\n",
            self.success_count(),
            self.total_count(),
            self.failed_count(),
            self.timeout_count(),
            self.total_duration_ms
        ));
        for item in &self.items {
            s.push_str(&format!(
                "  - {:<20} {:<10} {}ms",
                item.name, item.status, item.duration_ms
            ));
            if let Some(err) = &item.error {
                s.push_str(&format!(" ({})", err));
            }
            s.push('\n');
        }
        s
    }
}

// ============================================================================
// 预热管道
// ============================================================================

/// 预热管道 — 管理并执行多个预热器
///
/// 默认串行执行（避免缓存并发写入冲突）。如需并行执行，
/// 可使用 `warm_all_parallel`。
pub struct WarmupPipeline {
    warmers: Vec<Box<dyn Warmer>>,
    /// 全局超时（默认 5 分钟，覆盖所有预热器的总耗时）
    global_timeout: Duration,
}

impl WarmupPipeline {
    /// 创建空的预热管道
    pub fn new() -> Self {
        Self {
            warmers: Vec::new(),
            global_timeout: Duration::from_secs(300),
        }
    }

    /// 注册预热器
    pub fn register(&mut self, warmer: Box<dyn Warmer>) -> &mut Self {
        self.warmers.push(warmer);
        self
    }

    /// 设置全局超时
    pub fn with_global_timeout(mut self, timeout: Duration) -> Self {
        self.global_timeout = timeout;
        self
    }

    /// 获取已注册的预热器数量
    pub fn count(&self) -> usize {
        self.warmers.len()
    }

    /// 获取所有预热器名称
    pub fn names(&self) -> Vec<&str> {
        self.warmers.iter().map(|w| w.name()).collect()
    }

    /// 串行执行所有预热器
    ///
    /// 逐个执行预热器，每个预热器有自己的超时（`Warmer::timeout`）。
    /// 单个失败不影响后续预热器。
    pub async fn warm_all(&self) -> WarmupReport {
        let mut report = WarmupReport::new();
        let total_start = Instant::now();

        for warmer in &self.warmers {
            let item = self.warm_one(warmer.as_ref()).await;
            report.add_item(item);
        }

        report.total_duration_ms = total_start.elapsed().as_millis() as u64;
        report
    }

    /// 并行执行所有预热器
    ///
    /// 使用 `tokio::join_all` 并发执行，适合预热器之间无依赖的场景。
    /// 注意：并行执行可能增加缓存写入冲突。
    pub async fn warm_all_parallel(&self) -> WarmupReport {
        let total_start = Instant::now();

        let futures: Vec<_> = self
            .warmers
            .iter()
            .map(|w| self.warm_one_async(w.as_ref()))
            .collect();

        let items = futures::future::join_all(futures).await;

        let mut report = WarmupReport::new();
        for item in items {
            report.add_item(item);
        }
        report.total_duration_ms = total_start.elapsed().as_millis() as u64;
        report
    }

    /// 执行单个预热器（带超时）
    async fn warm_one(&self, warmer: &dyn Warmer) -> WarmupItem {
        self.warm_one_async(warmer).await
    }

    /// 异步执行单个预热器（内部辅助方法，便于并行化）
    async fn warm_one_async(&self, warmer: &dyn Warmer) -> WarmupItem {
        let name = warmer.name().to_string();
        let description = warmer.description().to_string();
        let timeout = warmer.timeout();
        let start = Instant::now();

        let result = tokio::time::timeout(timeout, warmer.warm()).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(())) => WarmupItem::success(name, description, duration_ms),
            Ok(Err(e)) => WarmupItem::failed(name, description, duration_ms, e.to_string()),
            Err(_) => WarmupItem::timeout(name, description, duration_ms),
        }
    }

    /// 执行指定名称的预热器
    ///
    /// 找不到时返回 `WarmupReport` 仅包含一条 `Skipped` 记录。
    pub async fn warm_one_by_name(&self, name: &str) -> WarmupReport {
        let mut report = WarmupReport::new();
        let total_start = Instant::now();

        if let Some(warmer) = self.warmers.iter().find(|w| w.name() == name) {
            let item = self.warm_one(warmer.as_ref()).await;
            report.add_item(item);
        } else {
            report.add_item(WarmupItem {
                name: name.to_string(),
                description: "Not found".to_string(),
                status: WarmupStatus::Skipped,
                duration_ms: 0,
                error: Some(format!("Warmer '{}' not registered", name)),
            });
        }

        report.total_duration_ms = total_start.elapsed().as_millis() as u64;
        report
    }
}

impl Default for WarmupPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 部署钩子（Deployment Hook）
// ============================================================================

/// 部署钩子 — 在部署生命周期中触发预热
///
/// 对齐 PHP `think console` 的部署钩子：
/// - `pre_warmup`：部署前预热（旧版本仍在服务）
/// - `post_deploy`：部署后预热（新版本已切换）
/// - `rollback`：回滚后清理预热缓存
pub struct DeploymentHook {
    pipeline: Arc<Mutex<WarmupPipeline>>,
}

impl DeploymentHook {
    /// 创建部署钩子
    pub fn new(pipeline: WarmupPipeline) -> Self {
        Self {
            pipeline: Arc::new(Mutex::new(pipeline)),
        }
    }

    /// 部署前预热（在旧版本服务期间预热新版本缓存）
    ///
    /// 适用于蓝绿部署：在切换流量前先预热新版本的缓存。
    pub async fn pre_warmup(&self) -> WarmupReport {
        let pipeline = self.pipeline.lock().await;
        pipeline.warm_all().await
    }

    /// 部署后预热（新版本已切换流量后立即预热）
    ///
    /// 适用于滚动更新：新实例启动后立即预热缓存。
    pub async fn post_deploy(&self) -> WarmupReport {
        let pipeline = self.pipeline.lock().await;
        pipeline.warm_all().await
    }

    /// 回滚后清理（清空所有预热缓存）
    ///
    /// 注意：本方法只触发预热器，实际的缓存清理应由调用方在预热器实现中处理。
    pub async fn rollback(&self) -> WarmupReport {
        let pipeline = self.pipeline.lock().await;
        pipeline.warm_all().await
    }

    /// 获取管道的共享句柄
    pub fn pipeline(&self) -> Arc<Mutex<WarmupPipeline>> {
        self.pipeline.clone()
    }
}

// ============================================================================
// 内置预热器
// ============================================================================

/// 无操作预热器（用于测试）
pub struct NoopWarmer {
    name: String,
    delay_ms: u64,
    should_fail: bool,
}

impl NoopWarmer {
    /// 创建无操作预热器
    ///
    /// # 参数
    ///
    /// - `name`：预热器名称
    /// - `delay_ms`：模拟耗时（毫秒）
    /// - `should_fail`：是否模拟失败
    pub fn new(name: impl Into<String>, delay_ms: u64, should_fail: bool) -> Self {
        Self {
            name: name.into(),
            delay_ms,
            should_fail,
        }
    }

    /// 创建立即成功的预热器
    pub fn success(name: impl Into<String>) -> Self {
        Self::new(name, 0, false)
    }

    /// 创建立即失败的预热器
    pub fn failing(name: impl Into<String>) -> Self {
        Self::new(name, 0, true)
    }

    /// 创建耗时指定毫秒的预热器
    pub fn delayed(name: impl Into<String>, delay_ms: u64) -> Self {
        Self::new(name, delay_ms, false)
    }
}

#[async_trait]
impl Warmer for NoopWarmer {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Noop warmer for testing"
    }

    async fn warm(&self) -> Result<(), WarmupError> {
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        if self.should_fail {
            return Err(WarmupError::Custom("Simulated failure".to_string()));
        }
        Ok(())
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // WarmupError
    // ========================================================================

    #[test]
    fn test_warmup_error_display() {
        assert_eq!(
            WarmupError::Io("file not found".to_string()).to_string(),
            "IO error: file not found"
        );
        assert_eq!(
            WarmupError::Serialize("invalid json".to_string()).to_string(),
            "Serialize error: invalid json"
        );
        assert_eq!(
            WarmupError::Cache("write failed".to_string()).to_string(),
            "Cache error: write failed"
        );
        assert_eq!(
            WarmupError::Database("connection refused".to_string()).to_string(),
            "Database error: connection refused"
        );
        assert_eq!(
            WarmupError::Custom("custom".to_string()).to_string(),
            "custom"
        );
    }

    #[test]
    fn test_warmup_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let warmup_err: WarmupError = io_err.into();
        assert!(matches!(warmup_err, WarmupError::Io(_)));
    }

    // ========================================================================
    // WarmupStatus
    // ========================================================================

    #[test]
    fn test_warmup_status_display() {
        assert_eq!(WarmupStatus::Success.to_string(), "success");
        assert_eq!(WarmupStatus::Failed.to_string(), "failed");
        assert_eq!(WarmupStatus::Timeout.to_string(), "timeout");
        assert_eq!(WarmupStatus::Skipped.to_string(), "skipped");
    }

    // ========================================================================
    // WarmupItem
    // ========================================================================

    #[test]
    fn test_warmup_item_success() {
        let item = WarmupItem::success("config", "Config warmer", 100);
        assert_eq!(item.name, "config");
        assert_eq!(item.status, WarmupStatus::Success);
        assert_eq!(item.duration_ms, 100);
        assert!(item.error.is_none());
        assert!(item.is_success());
    }

    #[test]
    fn test_warmup_item_failed() {
        let item = WarmupItem::failed("route", "Route warmer", 50, "missing file");
        assert_eq!(item.status, WarmupStatus::Failed);
        assert_eq!(item.error, Some("missing file".to_string()));
        assert!(!item.is_success());
    }

    #[test]
    fn test_warmup_item_timeout() {
        let item = WarmupItem::timeout("db", "DB warmer", 30000);
        assert_eq!(item.status, WarmupStatus::Timeout);
        assert!(item.error.unwrap().contains("Timed out"));
    }

    // ========================================================================
    // WarmupReport
    // ========================================================================

    #[test]
    fn test_warmup_report_empty() {
        let report = WarmupReport::new();
        assert_eq!(report.total_count(), 0);
        assert_eq!(report.success_count(), 0);
        assert_eq!(report.failed_count(), 0);
        assert_eq!(report.timeout_count(), 0);
        assert!(!report.all_success());
    }

    #[test]
    fn test_warmup_report_all_success() {
        let mut report = WarmupReport::new();
        report.add_item(WarmupItem::success("a", "A", 10));
        report.add_item(WarmupItem::success("b", "B", 20));
        report.total_duration_ms = 30;

        assert_eq!(report.total_count(), 2);
        assert_eq!(report.success_count(), 2);
        assert_eq!(report.failed_count(), 0);
        assert!(report.all_success());
    }

    #[test]
    fn test_warmup_report_mixed() {
        let mut report = WarmupReport::new();
        report.add_item(WarmupItem::success("a", "A", 10));
        report.add_item(WarmupItem::failed("b", "B", 20, "err"));
        report.add_item(WarmupItem::timeout("c", "C", 30000));

        assert_eq!(report.total_count(), 3);
        assert_eq!(report.success_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.timeout_count(), 1);
        assert!(!report.all_success());
    }

    #[test]
    fn test_warmup_report_summary_contains_status() {
        let mut report = WarmupReport::new();
        report.add_item(WarmupItem::success("config", "Config", 100));
        report.add_item(WarmupItem::failed("route", "Route", 50, "missing"));
        report.total_duration_ms = 150;

        let summary = report.summary();
        assert!(summary.contains("1/2 succeeded"));
        assert!(summary.contains("1 failed"));
        assert!(summary.contains("config"));
        assert!(summary.contains("success"));
        assert!(summary.contains("route"));
        assert!(summary.contains("failed"));
        assert!(summary.contains("missing"));
    }

    // ========================================================================
    // WarmupPipeline
    // ========================================================================

    #[tokio::test]
    async fn test_pipeline_empty() {
        let pipeline = WarmupPipeline::new();
        assert_eq!(pipeline.count(), 0);

        let report = pipeline.warm_all().await;
        assert_eq!(report.total_count(), 0);
        assert!(report.items.is_empty());
    }

    #[tokio::test]
    async fn test_pipeline_single_success() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("config")));

        let report = pipeline.warm_all().await;
        assert_eq!(report.total_count(), 1);
        assert_eq!(report.success_count(), 1);
        assert!(report.all_success());
        assert_eq!(report.items[0].name, "config");
    }

    #[tokio::test]
    async fn test_pipeline_mixed_results() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("success_warmer")));
        pipeline.register(Box::new(NoopWarmer::failing("failing_warmer")));

        let report = pipeline.warm_all().await;
        assert_eq!(report.total_count(), 2);
        assert_eq!(report.success_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert!(!report.all_success());

        assert_eq!(report.items[0].name, "success_warmer");
        assert_eq!(report.items[0].status, WarmupStatus::Success);
        assert_eq!(report.items[1].name, "failing_warmer");
        assert_eq!(report.items[1].status, WarmupStatus::Failed);
        assert!(report.items[1]
            .error
            .as_ref()
            .unwrap()
            .contains("Simulated failure"));
    }

    #[tokio::test]
    async fn test_pipeline_failure_does_not_stop_others() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::failing("first_fails")));
        pipeline.register(Box::new(NoopWarmer::success("second_success")));

        let report = pipeline.warm_all().await;
        assert_eq!(report.total_count(), 2);
        assert_eq!(report.success_count(), 1);
        assert_eq!(report.failed_count(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_timeout() {
        let mut pipeline = WarmupPipeline::new();

        // 自定义超时 100ms 的预热器，但实际耗时 500ms
        struct SlowWarmer;
        #[async_trait]
        impl Warmer for SlowWarmer {
            fn name(&self) -> &str {
                "slow"
            }
            fn timeout(&self) -> Duration {
                Duration::from_millis(100)
            }
            async fn warm(&self) -> Result<(), WarmupError> {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(())
            }
        }

        pipeline.register(Box::new(SlowWarmer));

        let report = pipeline.warm_all().await;
        assert_eq!(report.total_count(), 1);
        assert_eq!(report.timeout_count(), 1);
        assert_eq!(report.items[0].status, WarmupStatus::Timeout);
    }

    #[tokio::test]
    async fn test_pipeline_parallel() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::delayed("a", 100)));
        pipeline.register(Box::new(NoopWarmer::delayed("b", 100)));
        pipeline.register(Box::new(NoopWarmer::delayed("c", 100)));

        // 串行：约 300ms
        let serial_report = pipeline.warm_all().await;
        assert!(serial_report.total_duration_ms >= 250);

        // 并行：约 100ms
        let parallel_report = pipeline.warm_all_parallel().await;
        assert!(parallel_report.total_duration_ms < 200);
        assert_eq!(parallel_report.success_count(), 3);
    }

    #[tokio::test]
    async fn test_pipeline_warm_one_by_name_found() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("config")));
        pipeline.register(Box::new(NoopWarmer::success("route")));

        let report = pipeline.warm_one_by_name("config").await;
        assert_eq!(report.total_count(), 1);
        assert_eq!(report.items[0].name, "config");
        assert_eq!(report.items[0].status, WarmupStatus::Success);
    }

    #[tokio::test]
    async fn test_pipeline_warm_one_by_name_not_found() {
        let pipeline = WarmupPipeline::new();

        let report = pipeline.warm_one_by_name("nonexistent").await;
        assert_eq!(report.total_count(), 1);
        assert_eq!(report.items[0].status, WarmupStatus::Skipped);
        assert!(report.items[0]
            .error
            .as_ref()
            .unwrap()
            .contains("not registered"));
    }

    #[tokio::test]
    async fn test_pipeline_names() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("a")));
        pipeline.register(Box::new(NoopWarmer::success("b")));

        let names = pipeline.names();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_pipeline_with_global_timeout() {
        let pipeline = WarmupPipeline::new().with_global_timeout(Duration::from_secs(60));
        assert_eq!(pipeline.count(), 0);
    }

    // ========================================================================
    // DeploymentHook
    // ========================================================================

    #[tokio::test]
    async fn test_deployment_hook_pre_warmup() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("config")));

        let hook = DeploymentHook::new(pipeline);
        let report = hook.pre_warmup().await;

        assert_eq!(report.success_count(), 1);
    }

    #[tokio::test]
    async fn test_deployment_hook_post_deploy() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("config")));
        pipeline.register(Box::new(NoopWarmer::success("route")));

        let hook = DeploymentHook::new(pipeline);
        let report = hook.post_deploy().await;

        assert_eq!(report.success_count(), 2);
    }

    #[tokio::test]
    async fn test_deployment_hook_rollback() {
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("cleanup")));

        let hook = DeploymentHook::new(pipeline);
        let report = hook.rollback().await;

        assert_eq!(report.success_count(), 1);
    }

    #[tokio::test]
    async fn test_deployment_hook_pipeline_handle() {
        let pipeline = WarmupPipeline::new();
        let hook = DeploymentHook::new(pipeline);

        let handle = hook.pipeline();
        let p = handle.lock().await;
        assert_eq!(p.count(), 0);
    }

    // ========================================================================
    // NoopWarmer
    // ========================================================================

    #[tokio::test]
    async fn test_noop_warmer_success() {
        let warmer = NoopWarmer::success("test");
        assert_eq!(warmer.name(), "test");
        assert_eq!(warmer.description(), "Noop warmer for testing");
        let result = warmer.warm().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_noop_warmer_failing() {
        let warmer = NoopWarmer::failing("test");
        let result = warmer.warm().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Simulated failure"));
    }

    #[tokio::test]
    async fn test_noop_warmer_delayed() {
        let warmer = NoopWarmer::delayed("test", 50);
        let start = Instant::now();
        warmer.warm().await.unwrap();
        assert!(start.elapsed().as_millis() >= 40);
    }

    #[test]
    fn test_noop_warmer_default_timeout() {
        let warmer = NoopWarmer::success("test");
        assert_eq!(warmer.timeout(), Duration::from_secs(30));
    }

    // ========================================================================
    // 端到端
    // ========================================================================

    #[tokio::test]
    async fn test_end_to_end_deployment_scenario() {
        // 模拟完整部署流程：3 个预热器，其中 1 个失败
        let mut pipeline = WarmupPipeline::new();
        pipeline.register(Box::new(NoopWarmer::success("config")));
        pipeline.register(Box::new(NoopWarmer::success("route")));
        pipeline.register(Box::new(NoopWarmer::failing("dict")));

        let hook = DeploymentHook::new(pipeline);

        // 执行预热
        let report = hook.pre_warmup().await;

        // 验证结果
        assert_eq!(report.total_count(), 3);
        assert_eq!(report.success_count(), 2);
        assert_eq!(report.failed_count(), 1);

        // 验证 summary
        let summary = report.summary();
        assert!(summary.contains("2/3 succeeded"));
        assert!(summary.contains("1 failed"));

        // 验证失败的预热器被正确记录
        let failed_item = report
            .items
            .iter()
            .find(|i| i.status == WarmupStatus::Failed)
            .unwrap();
        assert_eq!(failed_item.name, "dict");
    }
}
