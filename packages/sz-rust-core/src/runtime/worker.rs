//! Worker 数量配置
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-swoole` 的 `worker_num` 配置项：
//!
//! ```php
//! // vendor/topthink/think-swoole/src/config/swoole.php:17
//! 'options' => [
//!     'reactor_num'     => swoole_cpu_num(),
//!     'worker_num'      => swoole_cpu_num(),   // ← 默认 = CPU 核数
//!     'task_worker_num' => swoole_cpu_num(),
//! ]
//! ```
//!
//! Rust 端使用 `num_cpus::get()` 获取 CPU 核数，并提供 builder 模式自定义。

use std::fmt;

/// 默认 worker 数（对齐 `swoole_cpu_num()`）
pub const DEFAULT_WORKER_NUM: usize = 0; // 0 表示使用 num_cpus::get()

/// 最小 worker 数（避免 0 worker 导致 runtime 无法启动）
pub const MIN_WORKER_NUM: usize = 1;

/// 最大 worker 数（避免过度创建线程导致调度开销过大）
pub const MAX_WORKER_NUM: usize = 256;

/// Worker 数量配置
///
/// 对齐 PHP `swoole.php` 配置中的 `worker_num` / `reactor_num` / `task_worker_num`。
///
/// ## 字段
///
/// - `worker_num`：业务 worker 数（处理 HTTP 请求等），默认 = CPU 核数
/// - `reactor_num`：reactor 线程数（对齐 Swoole reactor，Rust 端保留为元数据）
/// - `task_worker_num`：task worker 数（对齐 Swoole task worker，用于异步任务）
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::worker::WorkerConfig;
///
/// let config = WorkerConfig::new();
/// assert_eq!(config.worker_num(), num_cpus::get());
///
/// let custom = WorkerConfig::new().with_worker_num(8);
/// assert_eq!(custom.worker_num(), 8);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    /// 业务 worker 数（0 表示使用 CPU 核数）
    worker_num: usize,
    /// reactor 线程数（保留字段，对齐 PHP）
    reactor_num: usize,
    /// task worker 数（保留字段，对齐 PHP）
    task_worker_num: usize,
}

impl WorkerConfig {
    /// 创建默认配置：所有字段 = `num_cpus::get()`（对齐 `swoole_cpu_num()`）
    pub fn new() -> Self {
        let cpu = num_cpus::get();
        Self {
            worker_num: cpu,
            reactor_num: cpu,
            task_worker_num: cpu,
        }
    }

    /// 自定义 worker_num（对齐 `worker_num` 配置项）
    ///
    /// - `n = 0` 会被强制为 `num_cpus::get()`
    /// - `n > MAX_WORKER_NUM` 会被截断为 `MAX_WORKER_NUM`
    pub fn with_worker_num(mut self, n: usize) -> Self {
        self.worker_num = if n == 0 {
            num_cpus::get()
        } else {
            n.clamp(MIN_WORKER_NUM, MAX_WORKER_NUM)
        };
        self
    }

    /// 自定义 reactor_num（对齐 `reactor_num` 配置项）
    pub fn with_reactor_num(mut self, n: usize) -> Self {
        self.reactor_num = if n == 0 {
            num_cpus::get()
        } else {
            n.clamp(MIN_WORKER_NUM, MAX_WORKER_NUM)
        };
        self
    }

    /// 自定义 task_worker_num（对齐 `task_worker_num` 配置项）
    pub fn with_task_worker_num(mut self, n: usize) -> Self {
        self.task_worker_num = if n == 0 {
            num_cpus::get()
        } else {
            n.clamp(MIN_WORKER_NUM, MAX_WORKER_NUM)
        };
        self
    }

    /// 获取 worker_num
    pub fn worker_num(&self) -> usize {
        self.worker_num
    }

    /// 获取 reactor_num
    pub fn reactor_num(&self) -> usize {
        self.reactor_num
    }

    /// 获取 task_worker_num
    pub fn task_worker_num(&self) -> usize {
        self.task_worker_num
    }

    /// 获取 CPU 核数（对齐 `swoole_cpu_num()`）
    pub fn cpu_num(&self) -> usize {
        num_cpus::get()
    }

    /// 验证配置是否有效
    pub fn validate(&self) -> bool {
        self.worker_num >= MIN_WORKER_NUM
            && self.worker_num <= MAX_WORKER_NUM
            && self.reactor_num >= MIN_WORKER_NUM
            && self.reactor_num <= MAX_WORKER_NUM
            && self.task_worker_num >= MIN_WORKER_NUM
            && self.task_worker_num <= MAX_WORKER_NUM
    }

    /// 从环境变量读取 worker_num（对齐 PHP `env('SWOOLE_WORKER_NUM')`）
    ///
    /// - 环境变量 `SZ_RUST_WORKER_NUM` 优先级最高
    /// - 未设置或解析失败时使用默认值（CPU 核数）
    pub fn from_env() -> Self {
        let mut config = Self::new();
        if let Ok(val) = std::env::var("SZ_RUST_WORKER_NUM") {
            if let Ok(n) = val.parse::<usize>() {
                config = config.with_worker_num(n);
            }
        }
        config
    }

    /// 测试辅助构造器：跳过 clamp，仅 `#[cfg(test)]` 可用
    ///
    /// 直接赋值三个字段，用于 `validate()` 越界场景测试
    /// （`with_*` 方法会 clamp，无法构造越界值）。
    #[cfg(test)]
    pub(crate) fn new_unchecked(
        worker_num: usize,
        reactor_num: usize,
        task_worker_num: usize,
    ) -> Self {
        Self {
            worker_num,
            reactor_num,
            task_worker_num,
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WorkerConfig{{worker_num={}, reactor_num={}, task_worker_num={}, cpu={}}}",
            self.worker_num,
            self.reactor_num,
            self.task_worker_num,
            self.cpu_num()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// env 测试全局互斥锁：`std::env::set_var/remove_var` 非线程安全，
    /// 并行测试共享进程环境变量会互相污染（P3 竞态修复：test_from_env_* 串行化）
    static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_new_defaults_to_cpu_count() {
        let config = WorkerConfig::new();
        let cpu = num_cpus::get();
        assert_eq!(config.worker_num(), cpu);
        assert_eq!(config.reactor_num(), cpu);
        assert_eq!(config.task_worker_num(), cpu);
    }

    #[test]
    fn test_with_worker_num_custom() {
        let config = WorkerConfig::new().with_worker_num(8);
        assert_eq!(config.worker_num(), 8);
    }

    #[test]
    fn test_with_worker_num_zero_falls_back_to_cpu() {
        let config = WorkerConfig::new().with_worker_num(0);
        assert_eq!(config.worker_num(), num_cpus::get());
    }

    #[test]
    fn test_with_worker_num_exceeds_max_clamped() {
        let config = WorkerConfig::new().with_worker_num(1024);
        assert_eq!(config.worker_num(), MAX_WORKER_NUM);
    }

    #[test]
    fn test_with_reactor_num_custom() {
        let config = WorkerConfig::new().with_reactor_num(4);
        assert_eq!(config.reactor_num(), 4);
    }

    #[test]
    fn test_with_task_worker_num_custom() {
        let config = WorkerConfig::new().with_task_worker_num(16);
        assert_eq!(config.task_worker_num(), 16);
    }

    #[test]
    fn test_cpu_num_matches_num_cpus() {
        let config = WorkerConfig::new();
        assert_eq!(config.cpu_num(), num_cpus::get());
    }

    #[test]
    fn test_validate_valid_config() {
        let config = WorkerConfig::new();
        assert!(config.validate());
    }

    #[test]
    fn test_validate_boundary_values() {
        let config_min = WorkerConfig::new()
            .with_worker_num(MIN_WORKER_NUM)
            .with_reactor_num(MIN_WORKER_NUM)
            .with_task_worker_num(MIN_WORKER_NUM);
        assert!(config_min.validate());

        let config_max = WorkerConfig::new()
            .with_worker_num(MAX_WORKER_NUM)
            .with_reactor_num(MAX_WORKER_NUM)
            .with_task_worker_num(MAX_WORKER_NUM);
        assert!(config_max.validate());
    }

    #[test]
    fn test_from_env_default() {
        // 清除环境变量确保使用默认值
        // 注意：env 读写非线程安全，多个 env 测试并行时需互斥串行（P3 竞态修复）
        let _guard = ENV_TEST_LOCK.lock();
        std::env::remove_var("SZ_RUST_WORKER_NUM");
        let config = WorkerConfig::from_env();
        assert_eq!(config.worker_num(), num_cpus::get());
    }

    #[test]
    fn test_from_env_custom() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("SZ_RUST_WORKER_NUM", "12");
        let config = WorkerConfig::from_env();
        assert_eq!(config.worker_num(), 12);
        std::env::remove_var("SZ_RUST_WORKER_NUM");
    }

    #[test]
    fn test_from_env_invalid_falls_back_to_default() {
        let _guard = ENV_TEST_LOCK.lock();
        std::env::set_var("SZ_RUST_WORKER_NUM", "not-a-number");
        let config = WorkerConfig::from_env();
        assert_eq!(config.worker_num(), num_cpus::get());
        std::env::remove_var("SZ_RUST_WORKER_NUM");
    }

    #[test]
    fn test_display_format() {
        let config = WorkerConfig::new().with_worker_num(4);
        let s = format!("{}", config);
        assert!(s.contains("worker_num=4"));
        assert!(s.contains("reactor_num="));
        assert!(s.contains("task_worker_num="));
    }

    #[test]
    fn test_default_equals_new() {
        let config1 = WorkerConfig::default();
        let config2 = WorkerConfig::new();
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_clone_and_equality() {
        let config1 = WorkerConfig::new().with_worker_num(4);
        let config2 = config1.clone();
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_builder_chaining() {
        let config = WorkerConfig::new()
            .with_worker_num(4)
            .with_reactor_num(2)
            .with_task_worker_num(8);
        assert_eq!(config.worker_num(), 4);
        assert_eq!(config.reactor_num(), 2);
        assert_eq!(config.task_worker_num(), 8);
        assert!(config.validate());
    }

    // 捕获 validate -> true 与 5 个 && -> || 变异体（missed.txt 第 86-91 行）
    // 通过 new_unchecked 跳过 clamp 构造越界字段
    #[test]
    fn test_validate_false_when_worker_num_below_min() {
        let cfg = WorkerConfig::new_unchecked(0, 4, 4);
        assert!(!cfg.validate());
    }

    #[test]
    fn test_validate_false_when_worker_num_above_max() {
        let cfg = WorkerConfig::new_unchecked(usize::MAX, 4, 4);
        assert!(!cfg.validate());
    }

    #[test]
    fn test_validate_false_when_reactor_num_below_min() {
        let cfg = WorkerConfig::new_unchecked(4, 0, 4);
        assert!(!cfg.validate());
    }

    #[test]
    fn test_validate_false_when_reactor_num_above_max() {
        let cfg = WorkerConfig::new_unchecked(4, usize::MAX, 4);
        assert!(!cfg.validate());
    }

    #[test]
    fn test_validate_false_when_task_worker_below_min() {
        let cfg = WorkerConfig::new_unchecked(4, 4, 0);
        assert!(!cfg.validate());
    }

    #[test]
    fn test_validate_false_when_task_worker_above_max() {
        let cfg = WorkerConfig::new_unchecked(4, 4, usize::MAX);
        assert!(!cfg.validate());
    }
}
