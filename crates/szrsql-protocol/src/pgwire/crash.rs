//! Phase 4.12 — Crash Handler（崩溃日志捕获）。
//!
//! 通过 `std::panic::set_hook` 安装全局 panic hook，捕获 panic 后写入崩溃日志文件。
//!
//! # 崩溃日志内容
//!
//! - 时间戳（ISO 8601 UTC）
//! - 进程 PID
//! - 线程名
//! - panic 消息（payload）
//! - panic 位置（file:line:col）
//! - 最后 WAL LSN（当前为占位 "N/A"，Phase 5 持久化层实现后填充真实值）
//! - 完整 backtrace（`std::backtrace::Backtrace::force_capture`）
//!
//! # 设计
//!
//! - 使用 `std::sync::Once` 保证全局只安装一次 hook（重复调用静默忽略）
//! - hook 闭包捕获 `CrashConfig`（`Fn` 语义，多次调用共享配置）
//! - 调用原 hook（`take_hook`）保持默认 stderr 输出行为
//! - 崩溃日志文件名格式：`szrsql-crash-{YYYYMMDDTHHMMSSZ}.log`（RFC 3339 变体，文件系统友好）
//! - 日志目录可通过 `CrashConfig::log_dir` 配置（默认当前目录 `.`）
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_protocol::pgwire::crash::{install_crash_handler, CrashConfig};
//! use std::path::PathBuf;
//!
//! install_crash_handler(CrashConfig {
//!     log_dir: PathBuf::from("/var/log/szrsql"),
//!     capture_backtrace: true,
//! });
//! ```
//!
//! # 限制
//!
//! - **不能捕获 SIGKILL/SIGSEGV**：`set_hook` 仅捕获 Rust panic（unwind）；OS 信号导致的崩溃
//!   （如段错误）需通过操作系统机制（core dump / systemd-coredump）处理
//! - **不能捕获 `panic = "abort"` 模式下的 panic**：abort 模式下 panic 直接终止进程，hook 不执行
//! - **hook 内部不能再 panic**：hook 闭包内的 panic 会触发 abort，因此所有 IO 操作都用 `let _ =`
//!   忽略错误
//! - **最后 WAL LSN 占位**：szrsql 在 Phase 4.12 尚无 WAL，日志中该字段固定为 "N/A"

use std::backtrace::Backtrace;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Once;

use chrono::Utc;

/// 崩溃日志配置。
#[derive(Debug, Clone)]
pub struct CrashConfig {
    /// 崩溃日志输出目录（默认当前目录 `.`）。
    ///
    /// 文件名格式为 `szrsql-crash-{UTC时间戳}.log`，写入此目录下。
    pub log_dir: PathBuf,
    /// 是否捕获 backtrace（默认 true）。
    ///
    /// 关闭可减少 hook 开销，但会丢失崩溃调用栈信息。
    pub capture_backtrace: bool,
}

impl Default for CrashConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("."),
            capture_backtrace: true,
        }
    }
}

impl CrashConfig {
    /// 构造默认配置（当前目录，启用 backtrace）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置崩溃日志输出目录。
    pub fn with_log_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.log_dir = dir.into();
        self
    }

    /// 是否捕获 backtrace。
    pub fn with_backtrace(mut self, capture: bool) -> Self {
        self.capture_backtrace = capture;
        self
    }
}

/// 安装全局 panic hook，捕获 panic 并写入崩溃日志文件。
///
/// 使用 `Once` 保证全局只安装一次；重复调用静默忽略（不覆盖已安装的 hook）。
///
/// # 参数
///
/// - `config`：崩溃日志配置（目录、是否捕获 backtrace）
///
/// # 行为
///
/// 1. 调用 `std::panic::take_hook()` 获取当前 hook（保留默认 stderr 输出）
/// 2. 安装新 hook：写入崩溃日志文件 → 调用原 hook
/// 3. hook 内所有 IO 错误静默忽略（避免 hook 内 panic 导致 abort）
///
/// # 示例
///
/// ```ignore
/// install_crash_handler(CrashConfig::default());
/// // 此后任何 panic 都会触发崩溃日志写入
/// panic!("test crash");
/// ```
pub fn install_crash_handler(config: CrashConfig) {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(move || {
        let log_dir = config.log_dir.clone();
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // 写入崩溃日志文件（所有错误静默忽略，避免 hook 内 panic）
            let _ = write_crash_log(info, &config);

            // 调用原 hook 保持默认行为（打印到 stderr）
            previous_hook(info);
        }));
        tracing::info!(
            log_dir = ?log_dir,
            "crash handler installed"
        );
    });
}

/// 写入崩溃日志文件（内部函数，hook 闭包调用）。
///
/// 返回 `Ok(path)` 表示日志写入成功；`Err(io::Error)` 表示文件创建或写入失败。
fn write_crash_log(
    info: &std::panic::PanicHookInfo<'_>,
    config: &CrashConfig,
) -> std::io::Result<PathBuf> {
    let now = Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let timestamp_iso = now.to_rfc3339();

    // 文件名：szrsql-crash-{UTC时间戳}.log
    let filename = format!("szrsql-crash-{timestamp}.log");
    let path = config.log_dir.join(filename);

    // 确保目录存在
    fs::create_dir_all(&config.log_dir)?;

    let mut file = File::create(&path)?;

    // 提取 panic 消息
    let payload = info.payload();
    let message = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };

    // 提取 panic 位置
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());

    // 提取线程名
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_string();

    // 进程 PID
    let pid = std::process::id();

    // 写入日志内容
    writeln!(file, "=== SzRSQL Crash Log ===")?;
    writeln!(file, "Timestamp: {timestamp_iso}")?;
    writeln!(file, "PID: {pid}")?;
    writeln!(file, "Thread: {thread_name}")?;
    writeln!(file, "Panic: {message}")?;
    writeln!(file, "Location: {location}")?;
    writeln!(
        file,
        "Last WAL LSN: N/A (WAL not implemented in Phase 4.12)"
    )?;

    // backtrace
    if config.capture_backtrace {
        let backtrace = Backtrace::force_capture();
        writeln!(file)?;
        writeln!(file, "Backtrace:")?;
        writeln!(file, "{backtrace}")?;
    }

    writeln!(file)?;
    writeln!(file, "=== End Crash Log ===")?;

    // 显式 flush 确保写入磁盘
    file.flush()?;

    Ok(path)
}

/// Phase 4.12：仅用于测试的辅助函数 — 直接写入崩溃日志（不安装全局 hook）。
///
/// 测试场景下 `install_crash_handler` 的 `Once` 会导致多个测试互相干扰，
/// 因此提供此函数让测试直接验证日志格式。
#[cfg(test)]
fn write_crash_log_for_test(
    message: &str,
    location: &str,
    thread_name: &str,
    config: &CrashConfig,
) -> std::io::Result<PathBuf> {
    let now = Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let timestamp_iso = now.to_rfc3339();
    let filename = format!("szrsql-crash-{timestamp}.log");
    let path = config.log_dir.join(filename);

    fs::create_dir_all(&config.log_dir)?;
    let mut file = File::create(&path)?;

    let pid = std::process::id();
    writeln!(file, "=== SzRSQL Crash Log ===")?;
    writeln!(file, "Timestamp: {timestamp_iso}")?;
    writeln!(file, "PID: {pid}")?;
    writeln!(file, "Thread: {thread_name}")?;
    writeln!(file, "Panic: {message}")?;
    writeln!(file, "Location: {location}")?;
    writeln!(
        file,
        "Last WAL LSN: N/A (WAL not implemented in Phase 4.12)"
    )?;
    if config.capture_backtrace {
        let backtrace = Backtrace::force_capture();
        writeln!(file)?;
        writeln!(file, "Backtrace:")?;
        writeln!(file, "{backtrace}")?;
    }
    writeln!(file)?;
    writeln!(file, "=== End Crash Log ===")?;
    file.flush()?;
    Ok(path)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_crash_config_default() {
        let config = CrashConfig::default();
        assert_eq!(config.log_dir, PathBuf::from("."));
        assert!(config.capture_backtrace);
    }

    #[test]
    fn test_crash_config_builder() {
        let config = CrashConfig::new()
            .with_log_dir("/tmp/crash")
            .with_backtrace(false);
        assert_eq!(config.log_dir, PathBuf::from("/tmp/crash"));
        assert!(!config.capture_backtrace);
    }

    #[test]
    fn test_write_crash_log_for_test_creates_file() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "szrsql-crash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = CrashConfig::new()
            .with_log_dir(&tmp_dir)
            .with_backtrace(true);

        let path = write_crash_log_for_test(
            "test panic message",
            "src/lib.rs:42:5",
            "test-thread",
            &config,
        )
        .expect("write_crash_log_for_test should succeed");

        // 文件应存在
        assert!(path.exists(), "crash log file should exist at {path:?}");

        // 读取文件内容验证
        let mut content = String::new();
        File::open(&path)
            .expect("open crash log")
            .read_to_string(&mut content)
            .expect("read crash log");

        // 验证关键字段
        assert!(
            content.contains("=== SzRSQL Crash Log ==="),
            "missing header"
        );
        assert!(content.contains("Timestamp: "), "missing timestamp");
        assert!(content.contains("PID: "), "missing PID");
        assert!(
            content.contains("Thread: test-thread"),
            "missing or wrong thread name"
        );
        assert!(
            content.contains("Panic: test panic message"),
            "missing or wrong panic message"
        );
        assert!(
            content.contains("Location: src/lib.rs:42:5"),
            "missing or wrong location"
        );
        assert!(
            content.contains("Last WAL LSN: N/A"),
            "missing WAL LSN placeholder"
        );
        assert!(content.contains("Backtrace:"), "missing backtrace section");
        assert!(content.contains("=== End Crash Log ==="), "missing footer");

        // 清理
        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_write_crash_log_without_backtrace() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "szrsql-crash-test-nobt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = CrashConfig::new()
            .with_log_dir(&tmp_dir)
            .with_backtrace(false);

        let path =
            write_crash_log_for_test("no backtrace test", "src/main.rs:1:1", "main", &config)
                .expect("write should succeed");

        let mut content = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();

        assert!(
            !content.contains("Backtrace:"),
            "backtrace should be absent"
        );

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_install_crash_handler_idempotent() {
        // 多次调用不应 panic（Once 保证）
        install_crash_handler(CrashConfig::default());
        install_crash_handler(CrashConfig::default());
        install_crash_handler(CrashConfig::default());
        // 没有断言 —— 不 panic 即通过
    }

    #[test]
    fn test_crash_log_filename_format() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "szrsql-crash-fn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = CrashConfig::new().with_log_dir(&tmp_dir);
        let path = write_crash_log_for_test("msg", "loc:1:1", "t", &config).unwrap();

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("filename should be valid UTF-8");

        // 文件名应以 szrsql-crash- 开头，以 .log 结尾
        assert!(
            filename.starts_with("szrsql-crash-"),
            "filename should start with 'szrsql-crash-': {filename}"
        );
        assert!(
            filename.ends_with(".log"),
            "filename should end with '.log': {filename}"
        );

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
