// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//! 插件隔离 — 独立资源边界 + panic 捕获（spec §5.2 规则 3）
//!
//! v1.7.0 新增模块，通过 Cargo feature `plugin-hot-reload` 控制。

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

/// 插件资源边界（内存/文件/网络/环境变量访问限制）
#[derive(Debug, Clone)]
pub struct ResourceBoundary {
    /// 内存上限（字节）
    pub memory_limit: usize,
    /// 文件访问白名单
    pub file_whitelist: Vec<PathBuf>,
    /// 网络访问白名单（host:port）
    pub network_whitelist: Vec<String>,
    /// 环境变量白名单
    pub env_whitelist: Vec<String>,
}

impl Default for ResourceBoundary {
    fn default() -> Self {
        Self {
            memory_limit: 64 * 1024 * 1024,
            file_whitelist: Vec::new(),
            network_whitelist: Vec::new(),
            env_whitelist: Vec::new(),
        }
    }
}

impl ResourceBoundary {
    /// 创建资源边界
    pub fn new(memory_limit: usize) -> Self {
        Self {
            memory_limit,
            ..Self::default()
        }
    }

    /// 添加文件白名单
    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file_whitelist.push(path.into());
        self
    }

    /// 添加网络白名单
    pub fn with_network(mut self, host: impl Into<String>) -> Self {
        self.network_whitelist.push(host.into());
        self
    }

    /// 添加环境变量白名单
    pub fn with_env(mut self, var: impl Into<String>) -> Self {
        self.env_whitelist.push(var.into());
        self
    }

    /// 检查文件路径是否允许访问
    pub fn is_file_allowed(&self, path: &PathBuf) -> bool {
        self.file_whitelist
            .iter()
            .any(|allowed| path.starts_with(allowed) || path == allowed)
    }

    /// 检查网络地址是否允许访问
    pub fn is_network_allowed(&self, host: &str) -> bool {
        self.network_whitelist
            .iter()
            .any(|allowed| host == *allowed)
    }

    /// 检查环境变量是否允许访问
    pub fn is_env_allowed(&self, var: &str) -> bool {
        self.env_whitelist.iter().any(|allowed| var == *allowed)
    }
}

/// 插件隔离错误
#[derive(Debug, thiserror::Error)]
pub enum IsolationError {
    /// 插件 panic 被捕获
    #[error("插件 `{plugin}` panic: {message}")]
    PanicCaught {
        /// 插件名
        plugin: String,
        /// panic 消息
        message: String,
    },
    /// 资源访问越界
    #[error("插件 `{plugin}` 访问沙箱外资源: {detail}")]
    SandboxViolation {
        /// 插件名
        plugin: String,
        /// 越界详情
        detail: String,
    },
    /// 资源泄漏
    #[error("插件 `{plugin}` 资源泄漏: {detail}")]
    ResourceLeak {
        /// 插件名
        plugin: String,
        /// 泄漏详情
        detail: String,
    },
    /// 等待进行中请求超时
    #[error("插件 `{plugin}` 等待进行中请求超时（{timeout_ms}ms）")]
    DrainTimeout {
        /// 插件名
        plugin: String,
        /// 超时毫秒数
        timeout_ms: u64,
    },
}

/// 插件隔离运行结果
pub type IsolationResult<T> = Result<T, IsolationError>;

/// 资源使用快照（用于泄漏检测）
#[derive(Debug, Clone)]
pub struct ResourceSnapshot {
    /// 插件名
    pub plugin_name: String,
    /// 已分配内存（字节）
    pub memory_used: usize,
    /// 打开文件句柄数
    pub file_handles: usize,
    /// 活跃定时器数
    pub active_timers: usize,
    /// 活跃监听器数
    pub active_listeners: usize,
}

impl ResourceSnapshot {
    /// 创建快照
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            memory_used: 0,
            file_handles: 0,
            active_timers: 0,
            active_listeners: 0,
        }
    }

    /// 检查是否有资源泄漏（卸载后快照应为零）
    pub fn has_leak(&self) -> bool {
        self.memory_used > 0
            || self.file_handles > 0
            || self.active_timers > 0
            || self.active_listeners > 0
    }

    /// 泄漏详情
    pub fn leak_detail(&self) -> String {
        let mut details = Vec::new();
        if self.memory_used > 0 {
            details.push(format!("内存 {} 字节", self.memory_used));
        }
        if self.file_handles > 0 {
            details.push(format!("文件句柄 {} 个", self.file_handles));
        }
        if self.active_timers > 0 {
            details.push(format!("定时器 {} 个", self.active_timers));
        }
        if self.active_listeners > 0 {
            details.push(format!("监听器 {} 个", self.active_listeners));
        }
        details.join(", ")
    }
}

/// 插件隔离器 trait
pub trait PluginIsolator: Send + Sync {
    /// 在隔离边界内执行插件函数，捕获 panic
    ///
    /// # 后置条件
    /// - 插件 panic 被捕获，返回 `IsolationError::PanicCaught`
    /// - 宿主进程不受影响
    fn run_isolated<F, R>(&self, plugin_name: &str, f: F) -> IsolationResult<R>
    where
        F: FnOnce() -> R + std::panic::RefUnwindSafe,
        R: Default;

    /// 检查资源泄漏
    ///
    /// # 后置条件
    /// - 返回 `Ok(())` 表示无泄漏
    /// - 返回 `Err(ResourceLeak)` 表示有资源未释放
    fn check_leak(&self, plugin_name: &str) -> IsolationResult<()>;

    /// 验证资源访问是否在边界内
    fn check_boundary(&self, plugin_name: &str, boundary: &ResourceBoundary)
        -> IsolationResult<()>;
}

/// 默认沙箱隔离器实现
pub struct SandboxIsolator {
    /// 资源快照（插件名 → 快照）
    snapshots: Arc<RwLock<std::collections::HashMap<String, ResourceSnapshot>>>,
    /// 资源边界（插件名 → 边界）
    boundaries: Arc<RwLock<std::collections::HashMap<String, ResourceBoundary>>>,
}

impl SandboxIsolator {
    /// 创建沙箱隔离器
    pub fn new() -> Self {
        Self {
            snapshots: Arc::new(RwLock::new(std::collections::HashMap::new())),
            boundaries: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// 设置插件资源边界
    pub fn set_boundary(&self, plugin_name: &str, boundary: ResourceBoundary) {
        self.boundaries
            .write()
            .insert(plugin_name.to_string(), boundary);
    }

    /// 更新资源快照
    pub fn update_snapshot(&self, snapshot: ResourceSnapshot) {
        self.snapshots
            .write()
            .insert(snapshot.plugin_name.clone(), snapshot);
    }

    /// 获取资源快照
    pub fn snapshot(&self, plugin_name: &str) -> Option<ResourceSnapshot> {
        self.snapshots.read().get(plugin_name).cloned()
    }

    /// 清除插件资源记录
    pub fn clear(&self, plugin_name: &str) {
        self.snapshots.write().remove(plugin_name);
        self.boundaries.write().remove(plugin_name);
    }
}

impl Default for SandboxIsolator {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginIsolator for SandboxIsolator {
    fn run_isolated<F, R>(&self, plugin_name: &str, f: F) -> IsolationResult<R>
    where
        F: FnOnce() -> R + std::panic::RefUnwindSafe,
        R: Default,
    {
        match std::panic::catch_unwind(AssertUnwindSafe(f)) {
            Ok(result) => Ok(result),
            Err(panic_payload) => {
                let message = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "未知 panic".to_string()
                };
                Err(IsolationError::PanicCaught {
                    plugin: plugin_name.to_string(),
                    message,
                })
            }
        }
    }

    fn check_leak(&self, plugin_name: &str) -> IsolationResult<()> {
        if let Some(snapshot) = self.snapshots.read().get(plugin_name) {
            if snapshot.has_leak() {
                return Err(IsolationError::ResourceLeak {
                    plugin: plugin_name.to_string(),
                    detail: snapshot.leak_detail(),
                });
            }
        }
        Ok(())
    }

    fn check_boundary(
        &self,
        plugin_name: &str,
        boundary: &ResourceBoundary,
    ) -> IsolationResult<()> {
        if let Some(snapshot) = self.snapshots.read().get(plugin_name) {
            if snapshot.memory_used > boundary.memory_limit {
                return Err(IsolationError::SandboxViolation {
                    plugin: plugin_name.to_string(),
                    detail: format!(
                        "内存使用 {} 超过上限 {}",
                        snapshot.memory_used, boundary.memory_limit
                    ),
                });
            }
        }
        Ok(())
    }
}

/// 热加载事件（spec §5.2 规则 5）
#[derive(Debug, Clone)]
pub enum HotReloadEvent {
    /// 插件加载
    Loaded {
        /// 插件名
        plugin: String,
        /// 版本号
        version: String,
    },
    /// 插件卸载
    Unloaded {
        /// 插件名
        plugin: String,
    },
    /// 插件重载
    Reloaded {
        /// 插件名
        plugin: String,
        /// 旧版本号
        old_version: String,
        /// 新版本号
        new_version: String,
    },
    /// 卸载失败
    UnloadFailed {
        /// 插件名
        plugin: String,
        /// 失败原因
        reason: String,
    },
}

/// 事件订阅者回调类型
type EventCallback = Box<dyn Fn(&HotReloadEvent) + Send + Sync>;

/// 热加载事件总线（spec §5.2 规则 5）
pub struct EventBus {
    subscribers: Arc<RwLock<Vec<EventCallback>>>,
}

impl EventBus {
    /// 创建事件总线
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 订阅事件
    pub fn subscribe<F>(&self, callback: F)
    where
        F: Fn(&HotReloadEvent) + Send + Sync + 'static,
    {
        self.subscribers.write().push(Box::new(callback));
    }

    /// 发出事件
    pub fn emit(&self, event: &HotReloadEvent) {
        for callback in self.subscribers.read().iter() {
            callback(event);
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 路由原子切换器（spec §5.2 禁止项：无 404 窗口）
pub struct AtomicRouteSwitcher {
    current_routes: Arc<RwLock<std::collections::HashMap<String, String>>>,
}

impl AtomicRouteSwitcher {
    /// 创建路由切换器
    pub fn new() -> Self {
        Self {
            current_routes: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// 原子切换路由表（无 404 窗口）
    pub fn swap(&self, new_routes: std::collections::HashMap<String, String>) {
        let mut routes = self.current_routes.write();
        *routes = new_routes;
    }

    /// 查询路由
    pub fn lookup(&self, path: &str) -> Option<String> {
        self.current_routes.read().get(path).cloned()
    }

    /// 当前路由数
    pub fn route_count(&self) -> usize {
        self.current_routes.read().len()
    }
}

impl Default for AtomicRouteSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_isolated_success() {
        let isolator = SandboxIsolator::new();
        let result = isolator.run_isolated("test-plugin", || 42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_run_isolated_panic_caught() {
        let isolator = SandboxIsolator::new();
        let result: IsolationResult<i32> = isolator.run_isolated("panic-plugin", || {
            panic!("插件爆炸");
        });
        match result {
            Err(IsolationError::PanicCaught { plugin, message }) => {
                assert_eq!(plugin, "panic-plugin");
                assert!(message.contains("插件爆炸"));
            }
            _ => panic!("期望 PanicCaught"),
        }
    }

    #[test]
    fn test_run_isolated_panic_with_string() {
        let isolator = SandboxIsolator::new();
        let msg = String::from("自定义错误");
        let result: IsolationResult<i32> = isolator.run_isolated("plugin", || {
            panic!("{}", msg);
        });
        match result {
            Err(IsolationError::PanicCaught { message, .. }) => {
                assert!(message.contains("自定义错误"));
            }
            _ => panic!("期望 PanicCaught"),
        }
    }

    #[test]
    fn test_check_leak_no_snapshot() {
        let isolator = SandboxIsolator::new();
        let result = isolator.check_leak("no-snapshot");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_leak_clean() {
        let isolator = SandboxIsolator::new();
        isolator.update_snapshot(ResourceSnapshot::new("clean-plugin"));
        let result = isolator.check_leak("clean-plugin");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_leak_detected() {
        let isolator = SandboxIsolator::new();
        let mut snapshot = ResourceSnapshot::new("leaky-plugin");
        snapshot.memory_used = 1024;
        snapshot.file_handles = 2;
        isolator.update_snapshot(snapshot);
        let result = isolator.check_leak("leaky-plugin");
        match result {
            Err(IsolationError::ResourceLeak { plugin, detail }) => {
                assert_eq!(plugin, "leaky-plugin");
                assert!(detail.contains("内存"));
                assert!(detail.contains("文件句柄"));
            }
            _ => panic!("期望 ResourceLeak"),
        }
    }

    #[test]
    fn test_boundary_file_check() {
        let boundary = ResourceBoundary::new(1024)
            .with_file("/safe/path")
            .with_network("localhost:8080")
            .with_env("PATH");

        assert!(boundary.is_file_allowed(&PathBuf::from("/safe/path")));
        assert!(boundary.is_file_allowed(&PathBuf::from("/safe/path/sub")));
        assert!(!boundary.is_file_allowed(&PathBuf::from("/dangerous/path")));

        assert!(boundary.is_network_allowed("localhost:8080"));
        assert!(!boundary.is_network_allowed("evil.com:443"));

        assert!(boundary.is_env_allowed("PATH"));
        assert!(!boundary.is_env_allowed("SECRET"));
    }

    #[test]
    fn test_check_boundary_memory_violation() {
        let isolator = SandboxIsolator::new();
        let mut snapshot = ResourceSnapshot::new("greedy-plugin");
        snapshot.memory_used = 100 * 1024 * 1024;
        isolator.update_snapshot(snapshot);

        let boundary = ResourceBoundary::new(50 * 1024 * 1024);
        let result = isolator.check_boundary("greedy-plugin", &boundary);
        match result {
            Err(IsolationError::SandboxViolation { plugin, detail }) => {
                assert_eq!(plugin, "greedy-plugin");
                assert!(detail.contains("内存"));
            }
            _ => panic!("期望 SandboxViolation"),
        }
    }

    #[test]
    fn test_check_boundary_ok() {
        let isolator = SandboxIsolator::new();
        let mut snapshot = ResourceSnapshot::new("ok-plugin");
        snapshot.memory_used = 1024;
        isolator.update_snapshot(snapshot);

        let boundary = ResourceBoundary::new(4096);
        let result = isolator.check_boundary("ok-plugin", &boundary);
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_bus() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let bus = EventBus::new();
        let count = Arc::new(AtomicUsize::new(0));

        let count_clone = count.clone();
        bus.subscribe(move |_event| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        });

        bus.emit(&HotReloadEvent::Loaded {
            plugin: "a".to_string(),
            version: "1.0.0".to_string(),
        });
        bus.emit(&HotReloadEvent::Unloaded {
            plugin: "a".to_string(),
        });

        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_atomic_route_switcher() {
        let switcher = AtomicRouteSwitcher::new();
        assert_eq!(switcher.route_count(), 0);

        let mut routes = std::collections::HashMap::new();
        routes.insert("/api/a".to_string(), "handler_a".to_string());
        routes.insert("/api/b".to_string(), "handler_b".to_string());
        switcher.swap(routes);

        assert_eq!(switcher.route_count(), 2);
        assert_eq!(switcher.lookup("/api/a").unwrap(), "handler_a");
        assert_eq!(switcher.lookup("/api/b").unwrap(), "handler_b");
        assert!(switcher.lookup("/api/c").is_none());

        let mut new_routes = std::collections::HashMap::new();
        new_routes.insert("/api/c".to_string(), "handler_c".to_string());
        switcher.swap(new_routes);

        assert_eq!(switcher.route_count(), 1);
        assert!(switcher.lookup("/api/a").is_none());
        assert_eq!(switcher.lookup("/api/c").unwrap(), "handler_c");
    }

    #[test]
    fn test_resource_snapshot_leak_detail() {
        let mut snapshot = ResourceSnapshot::new("test");
        snapshot.memory_used = 512;
        snapshot.active_timers = 3;
        assert!(snapshot.has_leak());
        let detail = snapshot.leak_detail();
        assert!(detail.contains("内存"));
        assert!(detail.contains("定时器"));
        assert!(!detail.contains("文件句柄"));
    }
}
