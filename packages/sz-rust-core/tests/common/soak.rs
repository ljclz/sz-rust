// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Soak Test 共享监控工具（Web 框架场景）
//!
//! 提供 Soak Test 期间的关键指标采集、快照记录、退化检测能力。
//! 供 tests/soak.rs 使用，结构与 sz-orm 的 soak 工具保持一致。
//!
//! # 监控指标
//!
//! - `elapsed_secs`：已运行时长（秒）
//! - `ops_completed`：累计操作数
//! - `ops_per_sec`：当前吞吐
//! - `pool_idle / pool_active / pool_max`：保留字段（Web 框架无连接池，始终为 0）
//! - `rss_bytes`：进程 RSS（Linux 读 /proc/self/status；其他平台返回 0）
//! - `fd_count`：文件句柄数（Linux 读 /proc/self/fd）
//! - `thread_count`：进程线程数（Linux 读 /proc/self/status 的 Threads 行）
//! - `p99_latency_us`：P99 延迟（基于最近 1000 个样本的滑动窗口）
//! - `error_count`：累计错误数
//!
//! # 退化检测
//!
//! - RSS 增长 > 50MB → 内存泄漏
//! - fd_count 增长 > 10 → 句柄泄漏
//! - 终态 pool_active != pool_idle → 连接池泄漏（Web 框架中 pool 始终为 0，不会触发）
//! - ops_per_sec 衰减 > 10% → 性能衰减
//! - p99_latency 增长 > 2x → 慢退化
//! - error_count > 0 → 偶发错误

use std::collections::VecDeque;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 单次 Soak 快照（某一时刻的指标采样）
///
/// 共 11 个数据字段 + 1 个时间戳字段，CSV 输出 12 列，与 sz-orm 保持一致。
#[derive(Debug, Clone)]
pub struct SoakSnapshot {
    /// 已运行时长（秒）
    pub elapsed_secs: u64,
    /// 累计操作数
    pub ops_completed: u64,
    /// 当前吞吐（ops/s）
    pub ops_per_sec: f64,
    /// 保留字段（Web 框架无连接池，始终为 0）
    pub pool_idle: u32,
    /// 保留字段（始终为 0）
    pub pool_active: u32,
    /// 保留字段（始终为 0）
    pub pool_max: u32,
    /// 进程 RSS（字节）
    pub rss_bytes: u64,
    /// 文件句柄数
    pub fd_count: u32,
    /// 进程线程数
    pub thread_count: u32,
    /// P99 延迟（微秒，基于滑动窗口）
    pub p99_latency_us: u64,
    /// 累计错误数
    pub error_count: u64,
}

impl SoakSnapshot {
    /// 写入 CSV 一行（12 列，末尾含 RFC3339 时间戳）
    pub fn to_csv_line(&self) -> String {
        format!(
            "{},{},{:.2},{},{},{},{},{},{},{},{},{}\n",
            self.elapsed_secs,
            self.ops_completed,
            self.ops_per_sec,
            self.pool_idle,
            self.pool_active,
            self.pool_max,
            self.rss_bytes,
            self.fd_count,
            self.thread_count,
            self.p99_latency_us,
            self.error_count,
            chrono::Utc::now().to_rfc3339()
        )
    }

    /// CSV 表头（12 列）
    pub fn csv_header() -> &'static str {
        "elapsed_secs,ops_completed,ops_per_sec,pool_idle,pool_active,pool_max,rss_bytes,fd_count,thread_count,p99_latency_us,error_count,timestamp\n"
    }
}

/// Soak 监控器
///
/// 负责周期性采样、退化检测和 CSV 报告导出。
pub struct SoakMonitor {
    /// 测试开始时间
    start: Instant,
    /// 测试总时长（达到后停止）
    duration: Duration,
    /// 快照采样间隔
    sample_interval: Duration,
    /// 累计操作数（原子计数器）
    ops: Arc<AtomicU64>,
    /// 累计错误数（原子计数器）
    errors: Arc<AtomicU64>,
    /// P99 延迟滑动窗口（最近 1000 次操作的延迟，微秒）
    latency_window: Arc<std::sync::Mutex<VecDeque<u64>>>,
    /// 已采集的快照列表
    snapshots: Vec<SoakSnapshot>,
    /// 第一帧 RSS（用于判定内存泄漏）
    first_rss: Option<u64>,
    /// 第一帧 fd_count（用于判定句柄泄漏）
    first_fd: Option<u32>,
    /// 第一帧 pool_active（用于判定连接泄漏）
    first_pool_active: Option<u32>,
}

impl SoakMonitor {
    /// 创建新监控器
    ///
    /// # 参数
    ///
    /// - `duration`：测试总时长
    /// - `sample_interval`：快照采样间隔（建议 5s 或 60s）
    pub fn new(duration: Duration, sample_interval: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
            sample_interval,
            ops: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
            latency_window: Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(1000))),
            snapshots: Vec::new(),
            first_rss: None,
            first_fd: None,
            first_pool_active: None,
        }
    }

    /// 获取 ops 计数器（用于工作线程 fetch_add）
    pub fn ops_counter(&self) -> Arc<AtomicU64> {
        self.ops.clone()
    }

    /// 获取 errors 计数器（用于工作线程记录错误）
    pub fn errors_counter(&self) -> Arc<AtomicU64> {
        self.errors.clone()
    }

    /// 获取延迟滑动窗口（用于工作线程记录单次操作延迟）
    pub fn latency_window(&self) -> Arc<std::sync::Mutex<VecDeque<u64>>> {
        self.latency_window.clone()
    }

    /// 测试是否已结束
    pub fn is_finished(&self) -> bool {
        self.start.elapsed() >= self.duration
    }

    /// 已运行时长
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// 剩余时长
    pub fn remaining(&self) -> Duration {
        self.duration.saturating_sub(self.start.elapsed())
    }

    /// 采集一次快照
    ///
    /// # 参数
    ///
    /// - `pool_status`：连接池状态 (idle, active, max)；Web 框架场景传 (0, 0, 0)
    pub fn snapshot(&mut self, pool_status: (u32, u32, u32)) -> SoakSnapshot {
        let elapsed_secs = self.start.elapsed().as_secs();
        let ops_completed = self.ops.load(Ordering::Relaxed);
        let error_count = self.errors.load(Ordering::Relaxed);

        // 计算最近窗口的吞吐（ops/s）
        let prev_ops = self.snapshots.last().map(|s| s.ops_completed).unwrap_or(0);
        let prev_elapsed = self.snapshots.last().map(|s| s.elapsed_secs).unwrap_or(0);
        let dt = elapsed_secs.saturating_sub(prev_elapsed);
        let ops_per_sec = if dt > 0 {
            (ops_completed - prev_ops) as f64 / dt as f64
        } else {
            0.0
        };

        // P99 延迟：滑动窗口排序后取第 99 百分位
        let p99_latency_us = {
            let window = self
                .latency_window
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if window.is_empty() {
                0
            } else {
                let mut sorted: Vec<u64> = window.iter().copied().collect();
                sorted.sort_unstable();
                let idx = ((sorted.len() as f64) * 0.99) as usize;
                sorted[idx.min(sorted.len() - 1)]
            }
        };

        let snap = SoakSnapshot {
            elapsed_secs,
            ops_completed,
            ops_per_sec,
            pool_idle: pool_status.0,
            pool_active: pool_status.1,
            pool_max: pool_status.2,
            rss_bytes: read_rss_bytes(),
            fd_count: read_fd_count(),
            thread_count: read_thread_count(),
            p99_latency_us,
            error_count,
        };

        // 记录首帧基线
        if self.first_rss.is_none() {
            self.first_rss = Some(snap.rss_bytes);
        }
        if self.first_fd.is_none() {
            self.first_fd = Some(snap.fd_count);
        }
        if self.first_pool_active.is_none() {
            self.first_pool_active = Some(snap.pool_active);
        }

        self.snapshots.push(snap.clone());
        let _ = self.sample_interval; // 保留字段供未来扩展
        snap
    }

    /// 获取所有快照
    pub fn snapshots(&self) -> &[SoakSnapshot] {
        &self.snapshots
    }

    /// 导出 CSV 报告
    ///
    /// 自动创建父目录（如 target/ 不存在时）。
    pub fn export_csv(&self, path: &str) -> std::io::Result<()> {
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = String::from(SoakSnapshot::csv_header());
        for snap in &self.snapshots {
            content.push_str(&snap.to_csv_line());
        }
        fs::write(path, content)
    }

    /// 退化检测：返回所有检测到的问题
    ///
    /// 共 6 种检测，与 sz-orm 保持一致：
    /// 1. RSS 增长 > 50MB → MemoryLeak
    /// 2. fd_count 增长 > 10 → FdLeak
    /// 3. 终态 pool_active != pool_idle → PoolLeak
    /// 4. ops_per_sec 衰减 > 10% → ThroughputDecay
    /// 5. p99_latency 增长 > 2x → LatencyRegression
    /// 6. error_count > 0 → ErrorsObserved
    pub fn detect_regressions(&self) -> Vec<SoakRegression> {
        let mut issues = Vec::new();
        if self.snapshots.len() < 3 {
            return issues;
        }

        let first = &self.snapshots[0];
        let last = self.snapshots.last().unwrap();

        // 1. RSS 线性增长（> 50MB）
        if let Some(first_rss) = self.first_rss {
            let delta = last.rss_bytes.saturating_sub(first_rss);
            if delta > 50 * 1024 * 1024 {
                issues.push(SoakRegression::MemoryLeak {
                    delta_bytes: delta,
                    duration_secs: last.elapsed_secs,
                });
            }
        }

        // 2. fd_count 单调上升（> 10）
        if let Some(first_fd) = self.first_fd {
            let delta = last.fd_count.saturating_sub(first_fd);
            if delta > 10 {
                issues.push(SoakRegression::FdLeak {
                    delta,
                    duration_secs: last.elapsed_secs,
                });
            }
        }

        // 3. pool_active 不可逆上升（终态不等于 pool_idle）
        // Web 框架中 pool 始终为 (0, 0, 0)，此检测不会触发
        if last.pool_active != last.pool_idle {
            issues.push(SoakRegression::PoolLeak {
                final_active: last.pool_active,
                final_idle: last.pool_idle,
            });
        }

        // 4. ops_per_sec 衰减（> 10%）
        // 最终快照在 worker 停止后采集，ops/s 可能为 0（虚假衰减）
        // 如最终快照 ops/s < 1.0，使用倒数第二个快照
        let throughput_last = if last.ops_per_sec < 1.0 && self.snapshots.len() >= 2 {
            &self.snapshots[self.snapshots.len() - 2]
        } else {
            last
        };
        let first_ops = first.ops_per_sec.max(1.0);
        let last_ops = throughput_last.ops_per_sec;
        if last_ops < first_ops * 0.9 {
            issues.push(SoakRegression::ThroughputDecay {
                initial: first_ops,
                final_ops: last_ops,
                decay_pct: (1.0 - last_ops / first_ops) * 100.0,
            });
        }

        // 5. p99_latency 增长（> 2x）
        if first.p99_latency_us > 0 && last.p99_latency_us > first.p99_latency_us * 2 {
            issues.push(SoakRegression::LatencyRegression {
                initial_us: first.p99_latency_us,
                final_us: last.p99_latency_us,
            });
        }

        // 6. error_count > 0
        if last.error_count > 0 {
            issues.push(SoakRegression::ErrorsObserved {
                count: last.error_count,
            });
        }

        issues
    }
}

/// 退化检测结果
#[derive(Debug)]
pub enum SoakRegression {
    /// 内存泄漏：RSS 在测试期间增长超过 50MB
    MemoryLeak {
        /// RSS 增长字节数
        delta_bytes: u64,
        /// 测试持续秒数
        duration_secs: u64,
    },
    /// 文件句柄泄漏：fd_count 增长超过 10
    FdLeak {
        /// fd_count 增长数
        delta: u32,
        /// 测试持续秒数
        duration_secs: u64,
    },
    /// 连接池泄漏：终态 pool_active != pool_idle
    PoolLeak {
        /// 终态 active 连接数
        final_active: u32,
        /// 终态 idle 连接数
        final_idle: u32,
    },
    /// 吞吐衰减：ops_per_sec 下降超过 10%
    ThroughputDecay {
        /// 初始吞吐
        initial: f64,
        /// 最终吞吐
        final_ops: f64,
        /// 衰减百分比
        decay_pct: f64,
    },
    /// 延迟退化：p99 增长超过 2x
    LatencyRegression {
        /// 初始 P99 延迟（微秒）
        initial_us: u64,
        /// 最终 P99 延迟（微秒）
        final_us: u64,
    },
    /// 观察到错误
    ErrorsObserved {
        /// 错误总数
        count: u64,
    },
}

impl std::fmt::Display for SoakRegression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoakRegression::MemoryLeak {
                delta_bytes,
                duration_secs,
            } => write!(
                f,
                "MemoryLeak: RSS grew {} bytes over {}s",
                delta_bytes, duration_secs
            ),
            SoakRegression::FdLeak {
                delta,
                duration_secs,
            } => write!(f, "FdLeak: fd_count grew {} over {}s", delta, duration_secs),
            SoakRegression::PoolLeak {
                final_active,
                final_idle,
            } => write!(
                f,
                "PoolLeak: final active={} but idle={} (should be equal)",
                final_active, final_idle
            ),
            SoakRegression::ThroughputDecay {
                initial,
                final_ops,
                decay_pct,
            } => write!(
                f,
                "ThroughputDecay: {:.0} -> {:.0} ops/s ({:.1}% decay)",
                initial, final_ops, decay_pct
            ),
            SoakRegression::LatencyRegression {
                initial_us,
                final_us,
            } => {
                let ratio = if *initial_us > 0 {
                    *final_us as f64 / *initial_us as f64
                } else {
                    0.0
                };
                write!(
                    f,
                    "LatencyRegression: p99 {}us -> {}us ({:.1}x)",
                    initial_us, final_us, ratio
                )
            }
            SoakRegression::ErrorsObserved { count } => {
                write!(f, "ErrorsObserved: {} errors during soak", count)
            }
        }
    }
}

/// 读取进程 RSS（Resident Set Size），单位字节
///
/// Linux: 读 /proc/self/status 的 VmRSS 行（kB → bytes）
/// 其他平台：返回 0（仅 Linux 支持精确 RSS）
fn read_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    // 格式："VmRSS:\t      1234 kB"
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Ok(kb) = parts[0].parse::<u64>() {
                        return kb * 1024;
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// 读取进程文件句柄数
///
/// Linux: 统计 /proc/self/fd 目录下的条目数
/// 其他平台：返回 0
fn read_fd_count() -> u32 {
    #[cfg(target_os = "linux")]
    {
        fs::read_dir("/proc/self/fd")
            .map(|entries| entries.count() as u32)
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// 读取进程线程数
///
/// Linux: 读 /proc/self/status 的 Threads 行
/// 其他平台：返回 0
fn read_thread_count() -> u32 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("Threads:") {
                    // 格式："Threads:\t123"
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Ok(n) = parts[0].parse::<u32>() {
                        return n;
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// 记录单次操作延迟到滑动窗口
///
/// 窗口保留最近 1000 个样本，超出后丢弃最旧样本。
pub fn record_latency(window: &Arc<std::sync::Mutex<VecDeque<u64>>>, latency_us: u64) {
    let mut w = window.lock().unwrap_or_else(|e| e.into_inner());
    if w.len() >= 1000 {
        w.pop_front();
    }
    w.push_back(latency_us);
}

/// 解析时长字符串为 Duration
fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_str, unit) = if let Some(rest) = s.strip_suffix('s') {
        (rest, "s")
    } else if let Some(rest) = s.strip_suffix('m') {
        (rest, "m")
    } else if let Some(rest) = s.strip_suffix('h') {
        (rest, "h")
    } else {
        let rest = s.strip_suffix('d')?;
        (rest, "d")
    };
    let num: u64 = num_str.parse().ok()?;
    let secs = match unit {
        "s" => num,
        "m" => num.saturating_mul(60),
        "h" => num.saturating_mul(3600),
        "d" => num.saturating_mul(86400),
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// 从环境变量或命令行参数解析 Soak 时长
///
/// # 用法
///
/// ## 方式一：环境变量（推荐，绕过 Rust test harness 参数限制）
/// ```bash
/// SOAK_DURATION=6h cargo test --test soak -- --ignored --nocapture
/// ```
///
/// ## 方式二：命令行参数（需在 `--` 之后传递）
/// ```bash
/// cargo test --test soak -- --ignored --nocapture --soak-duration=6h
/// ```
///
/// # 支持格式
///
/// - `60s` → 60 秒
/// - `5m` → 5 分钟
/// - `2h` → 2 小时
/// - `1d` → 1 天
///
/// 默认值：60 秒（CI 快速验证）
pub fn parse_duration_from_args() -> Duration {
    // 优先读取环境变量 SOAK_DURATION（绕过 Rust test harness 参数限制）
    if let Ok(val) = std::env::var("SOAK_DURATION") {
        if let Some(d) = parse_duration_str(&val) {
            return d;
        }
        eprintln!("[soak] 警告：SOAK_DURATION={} 无法解析，使用默认 60s", val);
    }
    // 兼容命令行参数：在 -- 之后传递
    let args: Vec<String> = std::env::args().collect();
    for arg in &args {
        if let Some(rest) = arg.strip_prefix("--soak-duration=") {
            return parse_duration_str(rest).unwrap_or(Duration::from_secs(60));
        }
    }
    Duration::from_secs(60)
}

/// Soak 覆盖矩阵：6 路径 × 采样点覆盖表
///
/// 返回 6 个覆盖路径的名称列表，供 soak 测试输出和文档引用。
pub fn soak_coverage_matrix() -> [&'static str; 6] {
    [
        "route_parse",    // parse_path 路由解析
        "handler_ref",    // HandlerRef::parse
        "json_serialize", // ApiResponse::to_json_string
        "middleware",     // MiddlewareChain::has_duplicates
        "di_container",   // Container::singleton + make
        "cache_rw",       // Cache::set + get
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_str() {
        assert_eq!(parse_duration_str("60s"), Some(Duration::from_secs(60)));
        assert_eq!(parse_duration_str("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration_str("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration_str("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration_str("invalid"), None);
    }

    #[test]
    fn test_snapshot_csv() {
        let snap = SoakSnapshot {
            elapsed_secs: 60,
            ops_completed: 1000,
            ops_per_sec: 16.66,
            pool_idle: 0,
            pool_active: 0,
            pool_max: 0,
            rss_bytes: 10 * 1024 * 1024,
            fd_count: 10,
            thread_count: 8,
            p99_latency_us: 500,
            error_count: 0,
        };
        let line = snap.to_csv_line();
        assert!(line.starts_with("60,1000,16.66,"));
        assert!(line.ends_with('\n'));
        // CSV 必须包含 12 列（11 个逗号）
        assert_eq!(SoakSnapshot::csv_header().matches(',').count(), 11);
        assert_eq!(line.matches(',').count(), 11);
    }

    #[test]
    fn test_csv_header_includes_thread_count() {
        let header = SoakSnapshot::csv_header();
        assert!(header.contains("thread_count"));
        assert_eq!(header.matches(',').count(), 11);
    }

    #[test]
    fn test_snapshot_includes_thread_count_field() {
        let snap = SoakSnapshot {
            elapsed_secs: 1,
            ops_completed: 1,
            ops_per_sec: 1.0,
            pool_idle: 0,
            pool_active: 0,
            pool_max: 0,
            rss_bytes: 0,
            fd_count: 0,
            thread_count: 42,
            p99_latency_us: 100,
            error_count: 0,
        };
        assert_eq!(snap.thread_count, 42);
        let line = snap.to_csv_line();
        assert!(line.contains(",42,"));
    }

    #[test]
    fn test_read_thread_count_returns_zero_on_non_linux() {
        let count = read_thread_count();
        #[cfg(target_os = "linux")]
        {
            assert!(count >= 1, "Linux thread_count should be >= 1");
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(count, 0, "Non-Linux thread_count should be 0");
            let _ = count;
        }
    }

    #[test]
    fn test_regression_detection() {
        let mut monitor = SoakMonitor::new(Duration::from_secs(60), Duration::from_secs(10));

        // 模拟 3 次快照，触发多种退化
        monitor.first_rss = Some(10 * 1024 * 1024);
        monitor.first_fd = Some(5);
        monitor.first_pool_active = Some(0);

        monitor.snapshots.push(SoakSnapshot {
            elapsed_secs: 0,
            ops_completed: 0,
            ops_per_sec: 100.0,
            pool_idle: 0,
            pool_active: 0,
            pool_max: 0,
            rss_bytes: 10 * 1024 * 1024,
            fd_count: 5,
            thread_count: 4,
            p99_latency_us: 100,
            error_count: 0,
        });
        monitor.snapshots.push(SoakSnapshot {
            elapsed_secs: 30,
            ops_completed: 3000,
            ops_per_sec: 100.0,
            pool_idle: 0,
            pool_active: 0,
            pool_max: 0,
            rss_bytes: 30 * 1024 * 1024,
            fd_count: 8,
            thread_count: 4,
            p99_latency_us: 150,
            error_count: 0,
        });
        monitor.snapshots.push(SoakSnapshot {
            elapsed_secs: 60,
            ops_completed: 5000,
            ops_per_sec: 66.0,
            pool_idle: 0,
            pool_active: 0,
            pool_max: 0,
            rss_bytes: 80 * 1024 * 1024,
            fd_count: 20,
            thread_count: 4,
            p99_latency_us: 300,
            error_count: 3,
        });

        let issues = monitor.detect_regressions();
        // Web 框架中 pool 始终为 0，PoolLeak 不会触发，共 5 项退化
        assert_eq!(issues.len(), 5);
        assert!(issues
            .iter()
            .any(|i| matches!(i, SoakRegression::MemoryLeak { .. })));
        assert!(issues
            .iter()
            .any(|i| matches!(i, SoakRegression::FdLeak { .. })));
        assert!(!issues
            .iter()
            .any(|i| matches!(i, SoakRegression::PoolLeak { .. })));
        assert!(issues
            .iter()
            .any(|i| matches!(i, SoakRegression::ThroughputDecay { .. })));
        assert!(issues
            .iter()
            .any(|i| matches!(i, SoakRegression::LatencyRegression { .. })));
        assert!(issues
            .iter()
            .any(|i| matches!(i, SoakRegression::ErrorsObserved { .. })));
    }

    #[test]
    fn test_no_regressions_on_healthy_snapshots() {
        let mut monitor = SoakMonitor::new(Duration::from_secs(60), Duration::from_secs(10));
        monitor.first_rss = Some(10 * 1024 * 1024);
        monitor.first_fd = Some(5);
        monitor.first_pool_active = Some(0);

        for (elapsed, ops) in [(0u64, 0u64), (30, 3000), (60, 6000)] {
            monitor.snapshots.push(SoakSnapshot {
                elapsed_secs: elapsed,
                ops_completed: ops,
                ops_per_sec: 100.0,
                pool_idle: 0,
                pool_active: 0,
                pool_max: 0,
                rss_bytes: 10 * 1024 * 1024,
                fd_count: 5,
                thread_count: 4,
                p99_latency_us: 100,
                error_count: 0,
            });
        }

        let issues = monitor.detect_regressions();
        assert!(
            issues.is_empty(),
            "Healthy snapshots should have no regressions, got: {:?}",
            issues
        );
    }
}
