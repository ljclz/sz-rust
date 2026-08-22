//! 系统信息采集器（sysinfo backend）
//!
//! 基于 `sysinfo` crate 采集服务器运行时系统信息，
//! 供 `GET /api/admin/server/info` 端点使用。
//!
//! ## 数据结构（对齐 FssAdmin `/api/core/server/monitor`）
//!
//! ```json
//! {
//!   "memory": { "total": "32.00 GB", "used": "18.50 GB", "free": "13.50 GB", "rate": "57.8" },
//!   "env": { "rust_version": "rustc 1.78.0", "os": "Windows 10", "arch": "x86_64", ... },
//!   "disk": [{ "filesystem": "C:", "size": "500.00 GB", "used": "250.00 GB", "available": "250.00 GB", "use_percentage": "50.0", "mounted_on": "C:\\" }]
//! }
//! ```

use once_cell::sync::Lazy;
use std::env;
use std::process::Command;

use sysinfo::{Disks, System};

/// 服务器系统信息（`GET /api/admin/server/info` 响应体 data 字段）
///
/// 对齐 FssAdmin `ServerMonitorService::getServerInfo()` 的三层结构：
/// `memory`（内存）+ `env`（运行环境）+ `disk`（磁盘分区数组）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerInfo {
    /// 内存信息（总量 / 已用 / 空闲 / 使用率）
    pub memory: MemoryInfo,
    /// 运行环境信息（Rust版本 / OS / 架构等）
    pub env: EnvInfo,
    /// 磁盘分区列表（每个挂载点一条记录）
    pub disk: Vec<DiskPartition>,
}

/// 内存信息（人类可读格式，对齐 FssAdmin `getMemoryInfo()`）
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryInfo {
    /// 物理内存总量（人类可读，如 "32.00 GB"）
    pub total: String,
    /// 已用物理内存（人类可读）
    pub used: String,
    /// 空闲物理内存（人类可读）
    pub free: String,
    /// 内存使用率（百分比字符串，如 "57.8"）
    pub rate: String,
}

/// 运行环境信息（对齐 FssAdmin `getPhpEnvInfo()`，Rust 语境下为运行时信息）
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvInfo {
    /// rustc 编译版本（如 "rustc 1.78.0 (abc123 2025-01-01)"）
    pub rust_version: String,
    /// 操作系统名称 + 版本（如 "Windows 10 Pro" / "Ubuntu 22.04"）
    pub os: String,
    /// CPU 架构（如 "x86_64" / "aarch64"）
    pub arch: String,
    /// 主机名
    pub hostname: String,
    /// 当前进程启动时间（Unix 秒）
    pub process_start_time: u64,
    /// 1 分钟系统负载均值（Windows 上为 CPU 使用率/100）
    pub load_avg_one: f64,
    /// 5 分钟系统负载均值
    pub load_avg_five: f64,
    /// 15 分钟系统负载均值
    pub load_avg_fifteen: f64,
    /// CPU 全局使用率（0-100）
    pub cpu_usage_percent: f32,
}

/// 磁盘分区信息（对齐 FssAdmin `getDiskInfo()` 的数组元素）
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiskPartition {
    /// 文件系统标识（Windows 为盘符如 "C:"，Linux 为设备名如 "/dev/sda1"）
    pub filesystem: String,
    /// 分区总量（人类可读，如 "500.00 GB"）
    pub size: String,
    /// 已用空间（人类可读）
    pub used: String,
    /// 可用空间（人类可读）
    pub available: String,
    /// 使用率百分比字符串（如 "50.0"）
    pub use_percentage: String,
    /// 挂载点（如 "C:\\" / "/"）
    pub mounted_on: String,
}

/// 系统负载均值
#[derive(Debug, Clone, Default)]
pub struct LoadAvg {
    /// 1 分钟均值
    pub one: f64,
    /// 5 分钟均值
    pub five: f64,
    /// 15 分钟均值
    pub fifteen: f64,
}

/// 编译时检测到的 rustc 版本字符串（懒初始化）
static RUST_VERSION: Lazy<String> = Lazy::new(|| {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
});

/// 采集当前服务器系统信息
///
/// ## 性能说明
///
/// - `System::new_all()` 会枚举所有 CPU/内存/磁盘/网络/进程，耗时约 10-50ms
/// - 建议调用间隔 ≥ 5s，避免频繁采集影响服务性能
pub async fn collect_server_info() -> ServerInfo {
    let mut sys = System::new_all();
    sys.refresh_all();

    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let free_mem = total_mem.saturating_sub(used_mem);
    let load = System::load_average();

    ServerInfo {
        memory: MemoryInfo {
            total: format_bytes(total_mem),
            used: format_bytes(used_mem),
            free: format_bytes(free_mem),
            rate: if total_mem == 0 {
                "0.0".to_string()
            } else {
                format!("{:.1}", used_mem as f64 / total_mem as f64 * 100.0)
            },
        },
        env: EnvInfo {
            rust_version: RUST_VERSION.clone(),
            os: format!("{} {}", env::consts::OS, os_version().await),
            arch: env::consts::ARCH.to_string(),
            hostname: get_hostname(),
            process_start_time: get_current_process_start_time(),
            load_avg_one: load.one,
            load_avg_five: load.five,
            load_avg_fifteen: load.fifteen,
            cpu_usage_percent: sys.global_cpu_usage(),
        },
        disk: collect_disk_partitions(),
    }
}

/// 采集所有磁盘分区信息（返回分区数组，对齐 FssAdmin `getDiskInfo()`）
fn collect_disk_partitions() -> Vec<DiskPartition> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| {
            let total = disk.total_space();
            let avail = disk.available_space();
            let used = total.saturating_sub(avail);
            let pct = if total == 0 {
                0.0
            } else {
                used as f64 / total as f64 * 100.0
            };
            // sysinfo 的 mount_point 在 Windows 上是 "C:\\"，Linux 上是 "/" 等
            let mount_point = disk.mount_point().to_string_lossy().to_string();
            let filesystem = if mount_point.is_empty() {
                format!("{:?}", disk.kind())
            } else {
                mount_point.clone()
            };
            DiskPartition {
                filesystem,
                size: format_bytes(total),
                used: format_bytes(used),
                available: format_bytes(avail),
                use_percentage: format!("{:.1}", pct),
                mounted_on: mount_point,
            }
        })
        .collect()
}

/// 获取操作系统版本字符串（跨平台）
async fn os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        // Windows: 通过 ver 命令或 OS 环境变量
        env::var("OS")
            .ok()
            .or_else(|| env::var("COMPUTERNAME").ok().map(|_| "Windows".to_string()))
            .unwrap_or_else(|| "Windows".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        // Linux: 读取 /etc/os-release（tokio::fs 异步读取）
        tokio::fs::read_to_string("/etc/os-release")
            .await
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("PRETTY_NAME="))
                    .map(|l| l.trim_matches('"').replace("PRETTY_NAME=", "").to_string())
            })
            .unwrap_or_else(|| "Linux".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("sw_vers")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.contains("ProductVersion"))
                    .map(|l| l.split_whitespace().nth(1).unwrap_or("macOS").to_string())
            })
            .unwrap_or_else(|| "macOS".to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        env::consts::OS.to_string()
    }
}

/// 获取当前进程启动时间（Unix 秒）
fn get_current_process_start_time() -> u64 {
    match sysinfo::get_current_pid() {
        Ok(pid) => {
            let sys = System::new_all();
            sys.process(pid).map(|p| p.start_time()).unwrap_or(0)
        }
        Err(_) => 0,
    }
}

/// 获取主机名（跨平台）
fn get_hostname() -> String {
    #[cfg(target_os = "windows")]
    {
        env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var("HOSTNAME").unwrap_or_else(|_| {
            Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        })
    }
}

/// 将字节数格式化为人类可读字符串（对齐 FssAdmin `formatBytes()`）
///
/// 示例：`17179869184` → `"16.00 GB"`
fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let i = ((bytes as f64).log2() / 10.0) as usize;
    let i = i.min(units.len() - 1);
    let value = bytes as f64 / 1024_f64.powi(i as i32);
    format!("{:.2} {}", value, units[i])
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_server_info_returns_all_sections() {
        let info = collect_server_info().await;

        // memory 段
        assert!(!info.memory.total.is_empty());
        assert!(!info.memory.used.is_empty());
        assert!(!info.memory.free.is_empty());
        assert!(!info.memory.rate.is_empty());
        assert!(info.memory.rate.parse::<f64>().is_ok());

        // env 段
        assert!(!info.env.rust_version.is_empty());
        assert!(!info.env.os.is_empty());
        assert!(!info.env.arch.is_empty());
        assert!(!info.env.hostname.is_empty());
        assert!(info.env.cpu_usage_percent >= 0.0 && info.env.cpu_usage_percent <= 100.0);

        // disk 段：至少有一个分区
        assert!(!info.disk.is_empty());
        for p in &info.disk {
            assert!(!p.filesystem.is_empty());
            assert!(!p.size.is_empty());
            assert!(!p.mounted_on.is_empty());
        }
    }

    #[tokio::test]
    async fn test_memory_rate_consistency() {
        let info = collect_server_info().await;
        let rate_parsed = info.memory.rate.parse::<f64>().unwrap();
        assert!(rate_parsed >= 0.0 && rate_parsed <= 100.0);
    }

    #[tokio::test]
    async fn test_load_avg_fields_are_finite() {
        let info = collect_server_info().await;
        assert!(info.env.load_avg_one.is_finite());
        assert!(info.env.load_avg_five.is_finite());
        assert!(info.env.load_avg_fifteen.is_finite());
    }

    #[tokio::test]
    async fn test_server_info_serializes_to_json() {
        let info = collect_server_info().await;
        let json = serde_json::to_string(&info).expect("should serialize");

        // 顶层三段结构
        assert!(json.contains("\"memory\""));
        assert!(json.contains("\"env\""));
        assert!(json.contains("\"disk\""));
        // memory 字段
        assert!(json.contains("\"total\""));
        assert!(json.contains("\"used\""));
        assert!(json.contains("\"free\""));
        assert!(json.contains("\"rate\""));
        // env 字段
        assert!(json.contains("\"rust_version\""));
        assert!(json.contains("\"hostname\""));
        assert!(json.contains("\"cpu_usage_percent\""));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }

    #[test]
    fn test_collect_disk_partitions_non_negative() {
        let partitions = collect_disk_partitions();
        assert!(!partitions.is_empty());
        for p in &partitions {
            // size ≥ used（已用不应超过总量）
            let size_bytes = parse_human_bytes(&p.size);
            let used_bytes = parse_human_bytes(&p.used);
            assert!(
                used_bytes <= size_bytes,
                "used > total for {}",
                p.filesystem
            );
        }
    }

    #[test]
    fn test_get_hostname_not_empty() {
        assert!(!get_hostname().is_empty());
    }

    /// 辅助：将人类可读字节字符串解析回 u64（仅用于测试断言）
    fn parse_human_bytes(s: &str) -> u64 {
        let s = s.trim();
        let units = ["B", "KB", "MB", "GB", "TB"];
        for (i, unit) in units.iter().enumerate() {
            if s.ends_with(unit) {
                let num: f64 = s[..s.len() - unit.len()].trim().parse().unwrap_or(0.0);
                return (num * 1024_f64.powi(i as i32)) as u64;
            }
        }
        s.parse().unwrap_or(0)
    }
}
