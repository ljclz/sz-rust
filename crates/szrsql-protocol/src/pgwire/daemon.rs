//! Phase 4.13 — 进程守护化 + PID 文件。
//!
//! 提供：
//! - [`PidFile`]：PID 文件 RAII 管理（原子创建、重复启动检测、stale 文件清理、自动删除）
//! - [`daemonize()`]：Unix 双 fork + setsid 守护进程化（Windows 不支持，返回 `Unsupported`）
//!
//! # PID 文件生命周期
//!
//! ```text
//!   create(path)
//!     ├── 文件不存在 → 原子创建（O_CREAT|O_EXCL）→ 写入 PID → 返回 PidFile
//!     ├── 文件存在 + PID 对应进程存活 → 返回 AlreadyRunning 错误
//!     └── 文件存在 + PID 对应进程已死 → 删除 stale 文件 → 重试原子创建
//!
//!   Drop / cleanup()
//!     └── 删除 PID 文件
//! ```
//!
//! # 守护进程化流程（Unix）
//!
//! ```text
//!   parent ──fork──▶ child ──setsid──▶ fork ──▶ grandchild（守护进程）
//!     │                  │                          │
//!     └── exit(0)        └── exit(0)                ├── chdir("/")
//!                                                   ├── umask(0)
//!                                                   ├── redirect stdin/stdout/stderr → /dev/null
//!                                                   └── 继续执行（返回调用者）
//! ```
//!
//! 双 fork 的目的：第二次 fork 防止守护进程重新获取控制终端。

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;

use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// PID 文件操作错误。
#[derive(Debug, Error)]
pub enum PidFileError {
    /// PID 文件已存在且对应进程仍在运行（拒绝重复启动）。
    #[error("PID file already exists at {path} (PID {pid} is still running)")]
    AlreadyRunning { path: PathBuf, pid: u32 },

    /// PID 文件存在但内容格式无效（无法解析为 PID）。
    #[error("invalid PID content in file {path}: {content}")]
    InvalidPidContent { path: PathBuf, content: String },

    /// I/O 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 守护进程化错误。
#[derive(Debug, Error)]
pub enum DaemonError {
    /// fork 系统调用失败。
    #[error("fork failed ({context}): {errno}")]
    ForkFailed { context: &'static str, errno: i32 },

    /// setsid 系统调用失败。
    #[error("setsid failed: {errno}")]
    SetsidFailed { errno: i32 },

    /// I/O 错误（如打开 /dev/null 失败、chdir 失败）。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 当前平台不支持守护进程化（Windows 无 fork/setsid）。
    #[error("daemon mode is not supported on this platform")]
    Unsupported,
}

// =====================================================================
//  PidFile
// =====================================================================

/// PID 文件 RAII 管理器。
///
/// 创建时原子写入当前进程 PID，Drop 时自动删除文件。
/// 支持检测重复启动（PID 文件存在 + 进程存活）和清理 stale 文件（进程已死）。
///
/// # 线程安全
///
/// `PidFile` 本身不是 `Sync`（内部 `cleaned` 标志无需跨线程共享），
/// 但其创建和清理操作是进程级安全的（依赖文件系统的原子性）。
///
/// # 示例
///
/// ```no_run
/// use szrsql_protocol::pgwire::daemon::PidFile;
///
/// // 创建 PID 文件（如果已存在且进程存活，返回 AlreadyRunning 错误）
/// let pid_file = PidFile::create("/tmp/szrsql.pid")?;
/// // ... 运行服务器 ...
/// // pid_file 在 Drop 时自动删除 PID 文件
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct PidFile {
    path: PathBuf,
    pid: u32,
    /// 是否已清理（避免 Drop 时重复删除）。
    cleaned: bool,
}

impl PidFile {
    /// 创建 PID 文件并写入当前进程 PID。
    ///
    /// 流程：
    /// 1. 尝试原子创建文件（`O_CREAT | O_EXCL`）
    /// 2. 如果文件已存在：
    ///    a. 读取文件中的 PID
    ///    b. 检查该进程是否存活
    ///    c. 存活 → 返回 [`PidFileError::AlreadyRunning`]（拒绝重复启动）
    ///    d. 已死 → 删除 stale 文件，重试原子创建
    /// 3. 写入当前 PID 并 `sync_all` 确保落盘
    ///
    /// # 参数
    ///
    /// - `path`：PID 文件路径（如 `/tmp/szrsql.pid`）
    ///
    /// # 错误
    ///
    /// - [`PidFileError::AlreadyRunning`]：文件已存在且进程存活
    /// - [`PidFileError::InvalidPidContent`]：文件内容无法解析为 PID
    /// - [`PidFileError::Io`]：文件创建/写入/删除失败
    pub fn create(path: impl Into<PathBuf>) -> Result<Self, PidFileError> {
        let path = path.into();
        let pid = process::id();

        // 最多重试 3 次（处理 stale 文件清理后再次冲突的极端情况）
        for _attempt in 0..3 {
            // 尝试原子创建（create_new = O_CREAT | O_EXCL）
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    // 写入 PID
                    write!(file, "{pid}")?;
                    file.sync_all()?;
                    tracing::info!(path = %path.display(), pid, "PID file created");
                    return Ok(Self {
                        path,
                        pid,
                        cleaned: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // 文件已存在，读取 PID 并检查进程是否存活
                    let existing_pid = read_pid_from_file(&path)?;

                    if is_process_running(existing_pid) {
                        return Err(PidFileError::AlreadyRunning {
                            path: path.clone(),
                            pid: existing_pid,
                        });
                    }

                    // 进程已死，清理 stale 文件并重试
                    tracing::warn!(
                        path = %path.display(),
                        stale_pid = existing_pid,
                        "found stale PID file (process not running), removing"
                    );
                    fs::remove_file(&path)?;
                    // 继续循环重试
                }
                Err(e) => return Err(e.into()),
            }
        }

        // 重试次数耗尽（极端竞态：多个实例同时启动且 stale 文件反复出现）
        Err(PidFileError::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "failed to atomically create PID file after retries: {}",
                path.display()
            ),
        )))
    }

    /// 返回 PID 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回写入文件的 PID。
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 主动清理（删除 PID 文件）。Drop 时会自动调用，也可提前调用。
    pub fn cleanup(&mut self) {
        if !self.cleaned {
            match fs::remove_file(&self.path) {
                Ok(()) => {
                    tracing::info!(path = %self.path.display(), "PID file removed");
                }
                Err(e) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %e,
                        "failed to remove PID file"
                    );
                }
            }
            self.cleaned = true;
        }
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// =====================================================================
//  进程存活检测
// =====================================================================

/// 检测指定 PID 的进程是否仍在运行。
///
/// - **Unix**：`kill(pid, 0)` 返回 0 表示进程存活，-1（errno=ESRCH）表示不存在
/// - **Windows**：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, ...)` 返回非空句柄表示存活
///
/// # 注意
///
/// - PID 可能被复用（进程 A 死后，进程 B 复用了相同 PID），这是 PID 文件机制的固有限制
/// - 调用者需要权限向目标进程发信号（Unix）或打开进程（Windows）
fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) 不发送信号，仅检查进程是否存在
        // 返回 0 = 存在，-1 = 不存在（errno=ESRCH）或无权限（errno=EPERM）
        // EPERM 也意味着进程存在（只是无权限），所以 < 0 且 errno != ESRCH 时视为存活
        // SAFETY: libc::kill 和 libc::__errno_location 都是线程安全的 POSIX FFI 调用，
        // signal=0 不发送信号仅做存在性检查，pid 由调用方提供（u32 转 i32，非负）。
        unsafe {
            let result = libc::kill(pid as i32, 0);
            if result == 0 {
                return true;
            }
            // errno == ESRCH 表示进程不存在；其他错误（如 EPERM）视为存活
            let errno = *libc::__errno_location();
            errno != libc::ESRCH
        }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: OpenProcess/CloseHandle 是 Win32 线程安全 API；
        // PROCESS_QUERY_LIMITED_INFORMATION 是最小权限查询，不会影响目标进程；
        // 句柄通过 CloseHandle 释放，不会泄漏。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false as i32, pid);
            // HANDLE 是 *mut c_void，需用 null_mut() 比较
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return false;
            }
            let _ = CloseHandle(handle);
            true
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// 从 PID 文件中读取 PID。
fn read_pid_from_file(path: &Path) -> Result<u32, PidFileError> {
    let mut content = String::new();
    OpenOptions::new()
        .read(true)
        .open(path)?
        .read_to_string(&mut content)?;

    let trimmed = content.trim();
    let pid: u32 = trimmed
        .parse()
        .map_err(|_| PidFileError::InvalidPidContent {
            path: path.to_path_buf(),
            content: content.clone(),
        })?;

    Ok(pid)
}

// =====================================================================
//  守护进程化（Unix 双 fork + setsid）
// =====================================================================

/// 将当前进程守护进程化（后台运行）。
///
/// Unix 实现：双 fork + setsid + chdir("/") + umask(0) + 重定向 stdin/stdout/stderr 到 /dev/null。
/// Windows 实现：不支持，返回 [`DaemonError::Unsupported`]。
///
/// # 流程（Unix）
///
/// 1. **第一次 fork**：父进程 `exit(0)`，子进程继续
/// 2. **setsid**：子进程成为新会话组长，脱离控制终端
/// 3. **第二次 fork**：会话组长（子进程）`exit(0)`，孙进程继续
///    - 第二次 fork 防止守护进程重新获取控制终端
/// 4. **chdir("/")**：不占用任何目录
/// 5. **umask(0)**：清除文件创建权限掩码
/// 6. **重定向 stdin/stdout/stderr** → `/dev/null`
///
/// # 返回
///
/// - `Ok(())`：守护进程化成功（仅孙进程会到达此处，父进程已 exit）
/// - `Err(DaemonError::Unsupported)`：Windows 平台不支持
/// - `Err(DaemonError::ForkFailed)`：fork 系统调用失败
/// - `Err(DaemonError::SetsidFailed)`：setsid 系统调用失败
/// - `Err(DaemonError::Io)`：chdir 或打开 /dev/null 失败
///
/// # 注意
///
/// 调用此函数后，进程 PID 已变更为孙进程 PID。
/// **PID 文件应在调用 `daemonize()` 之后创建**，以写入正确的 PID。
pub fn daemonize() -> Result<(), DaemonError> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        // 第一次 fork：父进程退出，子进程继续
        // SAFETY: libc::fork 是 POSIX 标准线程安全系统调用，无前置条件。
        // fork 后父子进程各自拥有独立的内存空间，此处的局部变量（如 errno）
        // 不会跨进程共享。返回值 < 0 表示失败，== 0 表示子进程，> 0 表示父进程。
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            // SAFETY: __errno_location 是线程安全的 TLS 访问，读取 fork 失败原因。
            let errno = unsafe { *libc::__errno_location() };
            return Err(DaemonError::ForkFailed {
                context: "first fork",
                errno,
            });
        }
        if pid > 0 {
            // 父进程：立即退出
            process::exit(0);
        }

        // 子进程：成为新会话组长，脱离控制终端
        // SAFETY: libc::setsid 是 POSIX 标准系统调用，无前置条件。
        // 调用进程不能是进程组长（fork 后的子进程满足此条件）。
        // 成功返回新会话 ID（>0），失败返回 -1。
        if unsafe { libc::setsid() } < 0 {
            // SAFETY: __errno_location 线程安全，读取 setsid 失败原因。
            let errno = unsafe { *libc::__errno_location() };
            return Err(DaemonError::SetsidFailed { errno });
        }

        // 第二次 fork：会话组长退出，孙进程继续（防止重新获取控制终端）
        // SAFETY: 同第一次 fork，POSIX 标准线程安全系统调用。
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            // SAFETY: __errno_location 线程安全，读取 fork 失败原因。
            let errno = unsafe { *libc::__errno_location() };
            return Err(DaemonError::ForkFailed {
                context: "second fork",
                errno,
            });
        }
        if pid > 0 {
            // 第一次 fork 的子进程（会话组长）：退出
            process::exit(0);
        }

        // 孙进程：守护进程主体

        // 切换工作目录到根目录，避免占用挂载点
        std::env::set_current_dir("/")?;

        // 重置文件创建权限掩码
        // SAFETY: libc::umask 是 POSIX 标准系统调用，无前置条件。
        // 参数 0 表示清除所有创建权限限制，返回之前的 umask 值（此处忽略）。
        // 仅影响当前进程后续的文件创建权限，线程安全。
        unsafe {
            libc::umask(0);
        }

        // 重定向 stdin/stdout/stderr 到 /dev/null
        let devnull = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        let fd = devnull.as_raw_fd();
        // SAFETY: libc::dup2 是 POSIX 标准系统调用。
        // 前置条件：fd 是有效的 /dev/null 文件描述符（由 OpenOptions 成功打开保证）。
        // STDIN_FILENO=0/STDOUT_FILENO=1/STDERR_FILENO=2 是 POSIX 标准描述符。
        // dup2 会原子地关闭并重新赋值目标 fd，线程安全。
        unsafe {
            libc::dup2(fd, libc::STDIN_FILENO);
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::dup2(fd, libc::STDERR_FILENO);
        }
        // devnull 在此 drop，关闭原始 fd；duped 的 fd（0/1/2）保持打开

        tracing::info!(pid = process::id(), "daemonized (double fork + setsid)");
        Ok(())
    }

    #[cfg(not(unix))]
    {
        Err(DaemonError::Unsupported)
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：创建临时目录用于测试。
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "szrsql-phase413-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create_dir_all failed");
        dir
    }

    // ==================== PidFile 基本功能 ====================

    #[test]
    fn test_pid_file_create_writes_current_pid() {
        let dir = temp_dir();
        let pid_path = dir.join("test-create.pid");

        let pid_file = PidFile::create(&pid_path).expect("create failed");

        // 文件应存在
        assert!(pid_path.exists(), "PID file should exist");

        // 内容应为当前 PID
        let content = std::fs::read_to_string(&pid_path).unwrap();
        let written_pid: u32 = content.trim().parse().unwrap();
        assert_eq!(written_pid, std::process::id());
        assert_eq!(pid_file.pid(), std::process::id());

        // cleanup
        drop(pid_file);
        assert!(!pid_path.exists(), "PID file should be removed after drop");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_rejects_duplicate_when_running() {
        let dir = temp_dir();
        let pid_path = dir.join("test-duplicate.pid");

        // 第一次创建
        let _first = PidFile::create(&pid_path).expect("first create failed");

        // 第二次创建应失败（AlreadyRunning），因为当前进程仍然存活
        let result = PidFile::create(&pid_path);
        match result {
            Err(PidFileError::AlreadyRunning { pid, .. }) => {
                assert_eq!(pid, std::process::id());
            }
            other => panic!(
                "expected AlreadyRunning error, got: {:?}",
                other.map(|f| f.pid())
            ),
        }

        // 第一次的 PidFile drop 时删除文件
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_cleans_stale_file() {
        let dir = temp_dir();
        let pid_path = dir.join("test-stale.pid");

        // 写入一个不存在的 PID（如 999999，几乎不可能有此 PID 的进程）
        // PID 999999 在大多数系统上都不存在
        let stale_pid = 999_999;
        std::fs::write(&pid_path, stale_pid.to_string()).unwrap();

        // 创建 PidFile 应检测到 stale 文件并清理后重新创建
        let pid_file = PidFile::create(&pid_path).expect("create with stale file failed");

        // 内容应为当前 PID（而非 stale PID）
        let content = std::fs::read_to_string(&pid_path).unwrap();
        let written_pid: u32 = content.trim().parse().unwrap();
        assert_eq!(written_pid, std::process::id());

        drop(pid_file);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_invalid_content_errors() {
        let dir = temp_dir();
        let pid_path = dir.join("test-invalid.pid");

        // 写入无效内容
        std::fs::write(&pid_path, "not-a-pid").unwrap();

        // 创建应失败（InvalidPidContent）
        let result = PidFile::create(&pid_path);
        assert!(matches!(
            result,
            Err(PidFileError::InvalidPidContent { .. })
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_cleanup_idempotent() {
        let dir = temp_dir();
        let pid_path = dir.join("test-cleanup.pid");

        let mut pid_file = PidFile::create(&pid_path).expect("create failed");
        assert!(pid_path.exists());

        // 第一次 cleanup
        pid_file.cleanup();
        assert!(!pid_path.exists());

        // 第二次 cleanup 不应 panic 也不应报错
        pid_file.cleanup();
        pid_file.cleanup();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_drop_removes_file() {
        let dir = temp_dir();
        let pid_path = dir.join("test-drop.pid");

        {
            let _pid_file = PidFile::create(&pid_path).expect("create failed");
            assert!(pid_path.exists());
        } // _pid_file drop here

        assert!(!pid_path.exists(), "PID file should be removed on drop");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pid_file_path_accessor() {
        let dir = temp_dir();
        let pid_path = dir.join("test-path.pid");

        let pid_file = PidFile::create(&pid_path).expect("create failed");

        assert_eq!(pid_file.path(), &pid_path as &Path);

        drop(pid_file);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ==================== 进程存活检测 ====================

    #[test]
    fn test_is_process_running_current_process() {
        // 当前进程必然存活
        assert!(is_process_running(std::process::id()));
    }

    #[test]
    fn test_is_process_running_nonexistent_pid() {
        // PID 999999 在大多数系统上不存在
        // 注意：极小概率会误判（如果恰好有此 PID 的进程），但概率极低
        assert!(!is_process_running(999_999));
    }

    // ==================== daemonize() ====================

    #[test]
    fn test_daemonize_on_windows_returns_unsupported() {
        // Windows 平台应返回 Unsupported
        #[cfg(windows)]
        {
            let result = daemonize();
            assert!(matches!(result, Err(DaemonError::Unsupported)));
        }

        // Unix 平台跳过此测试（daemonize 会 fork，不适合在单元测试中调用）
        #[cfg(not(windows))]
        {
            // 在 Unix 上不测试 daemonize()，因为它会 fork 导致测试进程行为异常
        }
    }
}
