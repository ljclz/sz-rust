//! Gateway 模块 — GatewayWorker Gateway API 抽象（对齐 PHP `GatewayWorker\Gateway`）
//!
//! 提供 WebSocket 客户端管理、群组广播、消息推送等能力。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Gateway::sendToClient($client_id, $message)` | [`Gateway::send_to_client`] | 向指定客户端发送消息 |
//! | `Gateway::sendToAll($message)` | [`Gateway::send_to_all`] | 向所有在线客户端广播 |
//! | `Gateway::sendToGroup($group, $message)` | [`Gateway::send_to_group`] | 向指定群组发送消息 |
//! | `Gateway::joinGroup($client_id, $group)` | [`Gateway::join_group`] | 将客户端加入群组 |
//! | `Gateway::leaveGroup($client_id, $group)` | [`Gateway::leave_group`] | 将客户端离开群组 |
//! | `Gateway::ungroup($group)` | [`Gateway::ungroup`] | 解散指定群组 |
//! | `Gateway::isOnline($client_id)` | [`Gateway::is_online`] | 判断客户端是否在线 |
//! | `Gateway::getClientCount()` | [`Gateway::get_client_count`] | 获取在线客户端数量 |
//! | `Gateway::getClientCountByGroup($group)` | [`Gateway::get_client_count_by_group`] | 获取群组客户端数量 |
//! | `Gateway::getAllClientIds()` | [`Gateway::get_all_client_ids`] | 获取所有在线 client_id |
//! | `Gateway::getClientIdListByGroup($group)` | [`Gateway::get_client_id_list_by_group`] | 获取群组中的 client_id |
//! | `Gateway::closeClient($client_id)` | [`Gateway::close_client`] | 关闭客户端连接 |
//!
//! ### PHP 行为对齐
//!
//! - **静态方法 → 实例方法**：PHP `Gateway` 全部为静态方法，Rust 通过 [`Gateway`] 实例方法
//!   表达，配置与传输层通过构造函数注入，便于测试和多实例隔离。
//! - **Register 转发 → Transport 抽象**：PHP GatewayWorker 通过 Register 服务器转发命令到
//!   BusinessWorker/Worker。Rust 端通过 [`GatewayTransport`] trait 抽象底层通信，业务方可
//!   实现具体通信（如 Redis pub/sub、HTTP、TCP）。
//! - **client_id 格式**：对齐 PHP GatewayWorker 的 20 字符 hex 字符串。
//!
//! ## 架构说明
//!
//! - [`GatewayTransport`] trait：抽象 Gateway API 的底层通信
//! - [`MemoryGatewayTransport`]：内存实现，用于测试和开发环境
//! - [`Gateway`]：面向业务的 Gateway API 客户端，委托 [`GatewayTransport`] 执行具体操作

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// Gateway 错误
///
/// 对齐 PHP `GatewayWorker\Gateway` 在各类异常场景下抛出的错误。
#[derive(Debug, Error)]
pub enum GatewayError {
    /// 客户端未找到（client_id 不在线或不存在）
    #[error("客户端未找到: {0}")]
    ClientNotFound(String),
    /// 群组未找到
    #[error("群组未找到: {0}")]
    GroupNotFound(String),
    /// 发送失败
    #[error("发送失败: {0}")]
    SendFailed(String),
    /// 无效的 client_id（格式非法或为空）
    #[error("无效的 client_id: {0}")]
    InvalidClientId(String),
    /// 传输错误
    #[error("传输错误: {0}")]
    Transport(String),
    /// 序列化失败
    #[error("序列化失败: {0}")]
    Serialize(String),
}

// ============================================================================
// ClientId 类型
// ============================================================================

/// 客户端 ID — 对齐 PHP GatewayWorker 的 client_id（20 字符 hex 字符串）
pub type ClientId = String;

// ============================================================================
// GatewayConfig
// ============================================================================

/// Gateway 配置 — 对齐 PHP `GatewayWorker\Gateway` 的 Register 地址配置
///
/// # PHP 对齐
///
/// ```php
/// // PHP GatewayWorker Register 地址配置
/// Gateway::$registerAddress = '127.0.0.1:1238';
/// Gateway::$defaultGroup = 'default';
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_auth_facade::gateway::GatewayConfig;
///
/// let config = GatewayConfig::new("127.0.0.1:1238")
///     .with_heartbeat_interval(30)
///     .with_default_group("default");
/// ```
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Register 服务器地址（如 `127.0.0.1:1238`）
    pub register_address: String,
    /// 心跳间隔（秒，默认 55）
    pub heartbeat_interval: u64,
    /// 默认群组名（对齐 PHP `Gateway::$defaultGroup`）
    pub default_group: Option<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            register_address: "127.0.0.1:1238".to_string(),
            heartbeat_interval: 55,
            default_group: None,
        }
    }
}

impl GatewayConfig {
    /// 创建新配置
    ///
    /// # 参数
    ///
    /// - `register_address`: Register 服务器地址
    pub fn new(register_address: impl Into<String>) -> Self {
        Self {
            register_address: register_address.into(),
            heartbeat_interval: 55,
            default_group: None,
        }
    }

    /// 设置心跳间隔（秒）
    pub fn with_heartbeat_interval(mut self, interval: u64) -> Self {
        self.heartbeat_interval = interval;
        self
    }

    /// 设置默认群组名
    pub fn with_default_group(mut self, group: impl Into<String>) -> Self {
        self.default_group = Some(group.into());
        self
    }
}

// ============================================================================
// GatewayTransport trait
// ============================================================================

/// Gateway 传输层 trait — 抽象 Gateway API 的底层通信
///
/// PHP GatewayWorker 通过 Register 服务器转发命令到 BusinessWorker/Worker。
/// Rust 端通过此 trait 抽象，业务方可实现具体通信（如 Redis pub/sub、HTTP、TCP）。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 [`Gateway`] 通常作为单例在多线程下使用。
pub trait GatewayTransport: Send + Sync {
    /// 向指定 client_id 发送消息
    ///
    /// # 参数
    ///
    /// - `client_id`: 目标客户端 ID
    /// - `message`: 消息内容
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，客户端不在线返回 [`GatewayError::ClientNotFound`]。
    fn send_to_client(&self, client_id: &str, message: &str) -> Result<(), GatewayError>;

    /// 向多个 client_id 发送消息
    ///
    /// # 参数
    ///
    /// - `client_ids`: 目标客户端 ID 列表
    /// - `message`: 消息内容
    ///
    /// # 返回
    ///
    /// 任一客户端不在线即返回 [`GatewayError::ClientNotFound`]。
    fn send_to_clients(&self, client_ids: &[String], message: &str) -> Result<(), GatewayError>;

    /// 向所有在线 client_id 发送广播
    ///
    /// # 参数
    ///
    /// - `message`: 消息内容
    fn send_to_all(&self, message: &str) -> Result<(), GatewayError>;

    /// 向指定群组发送消息
    ///
    /// # 参数
    ///
    /// - `group`: 群组名
    /// - `message`: 消息内容
    fn send_to_group(&self, group: &str, message: &str) -> Result<(), GatewayError>;

    /// 获取所有在线 client_id
    fn get_all_client_ids(&self) -> Result<Vec<String>, GatewayError>;

    /// 获取指定群组中的 client_id
    ///
    /// # 参数
    ///
    /// - `group`: 群组名
    fn get_client_id_list_by_group(&self, group: &str) -> Result<Vec<String>, GatewayError>;

    /// 获取 client_id 所在的群组
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    ///
    /// # 返回
    ///
    /// 客户端不在线返回 [`GatewayError::ClientNotFound`]。
    fn get_groups_by_client_id(&self, client_id: &str) -> Result<Vec<String>, GatewayError>;

    /// 将 client_id 加入群组
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    /// - `group`: 群组名
    ///
    /// # 返回
    ///
    /// 客户端不在线返回 [`GatewayError::ClientNotFound`]。
    fn join_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError>;

    /// 将 client_id 离开群组
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    /// - `group`: 群组名
    ///
    /// # 返回
    ///
    /// 客户端不在线返回 [`GatewayError::ClientNotFound`]，群组不存在返回 [`GatewayError::GroupNotFound`]。
    fn leave_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError>;

    /// 解散指定群组（对齐 PHP `Gateway::ungroup()`）
    ///
    /// 移除群组，并将群组内所有 client_id 的群组成员关系清除。
    ///
    /// # 参数
    ///
    /// - `group`: 群组名
    ///
    /// # 返回
    ///
    /// 群组不存在返回 [`GatewayError::GroupNotFound`]。
    fn ungroup(&self, group: &str) -> Result<(), GatewayError>;

    /// 判断 client_id 是否在线
    fn is_online(&self, client_id: &str) -> Result<bool, GatewayError>;

    /// 获取在线 client_id 数量
    fn get_client_count(&self) -> Result<usize, GatewayError>;

    /// 获取指定群组的在线 client_id 数量
    ///
    /// # 参数
    ///
    /// - `group`: 群组名
    fn get_client_count_by_group(&self, group: &str) -> Result<usize, GatewayError>;

    /// 关闭指定 client_id 的连接
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    ///
    /// # 返回
    ///
    /// 客户端不在线返回 [`GatewayError::ClientNotFound`]。
    fn close_client(&self, client_id: &str) -> Result<(), GatewayError>;
}

// ============================================================================
// MemoryGatewayTransport
// ============================================================================

/// 内存 Gateway 传输实现 — 用于测试和开发环境
///
/// 不进行实际网络通信，所有状态保存在内存中，供测试断言使用。
///
/// # 数据结构
///
/// - `client_messages`: client_id → messages 队列（键存在即表示 client 在线）
/// - `client_groups`: client_id → groups 映射
/// - `group_clients`: group → client_ids 映射
///
/// # 线程安全
///
/// 通过 `Arc<Mutex<GatewayState>>` 保护，支持并发访问。所有操作在单个锁内完成，
/// 保证多字段状态的一致性。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_auth_facade::gateway::{MemoryGatewayTransport, GatewayTransport};
///
/// let transport = MemoryGatewayTransport::new();
/// transport.register_client("7f00000108fc00000001");
/// transport.send_to_client("7f00000108fc00000001", "hello").unwrap();
/// assert_eq!(transport.client_messages("7f00000108fc00000001"), vec!["hello".to_string()]);
/// ```
#[derive(Debug, Default)]
pub struct MemoryGatewayTransport {
    /// Gateway 内存状态
    state: Mutex<GatewayState>,
}

/// Gateway 内存状态
#[derive(Debug, Default)]
struct GatewayState {
    /// client_id → messages 队列（键存在即表示 client 在线）
    client_messages: HashMap<ClientId, Vec<String>>,
    /// client_id → groups 映射
    client_groups: HashMap<ClientId, Vec<String>>,
    /// group → client_ids 映射
    group_clients: HashMap<String, Vec<ClientId>>,
}

impl MemoryGatewayTransport {
    /// 创建新的内存传输
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册客户端（标记为在线）
    ///
    /// 非 trait 方法，供测试/开发注册在线客户端。对齐 PHP GatewayWorker 中客户端连接
    /// 建立后自动上线的行为。重复注册为幂等操作。
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    pub fn register_client(&self, client_id: &str) {
        self.state
            .lock()
            .client_messages
            .entry(client_id.to_string())
            .or_default();
    }

    /// 获取客户端收到的所有消息（快照）
    ///
    /// 非 trait 方法，供测试断言。客户端不在线时返回空 Vec。
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    pub fn client_messages(&self, client_id: &str) -> Vec<String> {
        self.state
            .lock()
            .client_messages
            .get(client_id)
            .cloned()
            .unwrap_or_default()
    }
}

impl GatewayTransport for MemoryGatewayTransport {
    fn send_to_client(&self, client_id: &str, message: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut state = self.state.lock();
        match state.client_messages.get_mut(client_id) {
            Some(messages) => {
                messages.push(message.to_string());
                Ok(())
            }
            None => Err(GatewayError::ClientNotFound(client_id.to_string())),
        }
    }

    fn send_to_clients(&self, client_ids: &[String], message: &str) -> Result<(), GatewayError> {
        let mut state = self.state.lock();
        // 先校验所有客户端在线，再统一发送，避免部分发送
        for client_id in client_ids {
            if client_id.is_empty() {
                return Err(GatewayError::InvalidClientId(client_id.to_string()));
            }
            if !state.client_messages.contains_key(client_id) {
                return Err(GatewayError::ClientNotFound(client_id.to_string()));
            }
        }
        for client_id in client_ids {
            if let Some(messages) = state.client_messages.get_mut(client_id) {
                messages.push(message.to_string());
            }
        }
        Ok(())
    }

    fn send_to_all(&self, message: &str) -> Result<(), GatewayError> {
        let mut state = self.state.lock();
        for messages in state.client_messages.values_mut() {
            messages.push(message.to_string());
        }
        Ok(())
    }

    fn send_to_group(&self, group: &str, message: &str) -> Result<(), GatewayError> {
        let mut state = self.state.lock();
        // 群组不存在时为空操作（对齐 PHP 广播到空群组的行为）
        if let Some(client_ids) = state.group_clients.get(group) {
            let client_ids = client_ids.clone();
            for client_id in client_ids {
                if let Some(messages) = state.client_messages.get_mut(&client_id) {
                    messages.push(message.to_string());
                }
            }
        }
        Ok(())
    }

    fn get_all_client_ids(&self) -> Result<Vec<String>, GatewayError> {
        let state = self.state.lock();
        Ok(state.client_messages.keys().cloned().collect())
    }

    fn get_client_id_list_by_group(&self, group: &str) -> Result<Vec<String>, GatewayError> {
        let state = self.state.lock();
        Ok(state.group_clients.get(group).cloned().unwrap_or_default())
    }

    fn get_groups_by_client_id(&self, client_id: &str) -> Result<Vec<String>, GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let state = self.state.lock();
        if !state.client_messages.contains_key(client_id) {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        Ok(state
            .client_groups
            .get(client_id)
            .cloned()
            .unwrap_or_default())
    }

    fn join_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut state = self.state.lock();
        if !state.client_messages.contains_key(client_id) {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        // 维护 client_id → groups 映射（去重）
        let client_groups = state
            .client_groups
            .entry(client_id.to_string())
            .or_default();
        if !client_groups.iter().any(|g| g == group) {
            client_groups.push(group.to_string());
        }
        // 维护 group → client_ids 映射（去重）
        let group_clients = state.group_clients.entry(group.to_string()).or_default();
        if !group_clients.iter().any(|c| c == client_id) {
            group_clients.push(client_id.to_string());
        }
        Ok(())
    }

    fn leave_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut state = self.state.lock();
        if !state.client_messages.contains_key(client_id) {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        let group_exists = state.group_clients.contains_key(group);
        if !group_exists {
            return Err(GatewayError::GroupNotFound(group.to_string()));
        }
        // 从 client_id → groups 移除
        if let Some(groups) = state.client_groups.get_mut(client_id) {
            groups.retain(|g| g != group);
        }
        // 从 group → client_ids 移除
        if let Some(client_ids) = state.group_clients.get_mut(group) {
            client_ids.retain(|c| c != client_id);
        }
        Ok(())
    }

    fn ungroup(&self, group: &str) -> Result<(), GatewayError> {
        let mut state = self.state.lock();
        if !state.group_clients.contains_key(group) {
            return Err(GatewayError::GroupNotFound(group.to_string()));
        }
        // 移除群组
        state.group_clients.remove(group);
        // 从所有 client 的群组列表中移除该群组
        for groups in state.client_groups.values_mut() {
            groups.retain(|g| g != group);
        }
        Ok(())
    }

    fn is_online(&self, client_id: &str) -> Result<bool, GatewayError> {
        let state = self.state.lock();
        Ok(state.client_messages.contains_key(client_id))
    }

    fn get_client_count(&self) -> Result<usize, GatewayError> {
        let state = self.state.lock();
        Ok(state.client_messages.len())
    }

    fn get_client_count_by_group(&self, group: &str) -> Result<usize, GatewayError> {
        let state = self.state.lock();
        Ok(state.group_clients.get(group).map(|v| v.len()).unwrap_or(0))
    }

    fn close_client(&self, client_id: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut state = self.state.lock();
        if state.client_messages.remove(client_id).is_none() {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        // 移除 client 的群组列表
        state.client_groups.remove(client_id);
        // 从所有群组中移除该 client
        for client_ids in state.group_clients.values_mut() {
            client_ids.retain(|c| c != client_id);
        }
        Ok(())
    }
}

// ============================================================================
// Gateway
// ============================================================================

/// Gateway API 客户端 — 对齐 PHP `GatewayWorker\Gateway`
///
/// 提供 WebSocket 客户端管理、群组广播、消息推送等能力。
/// 通过 [`GatewayTransport`] trait 抽象底层通信。
///
/// # PHP 对齐
///
/// PHP `Gateway` 全部为静态方法，Rust 通过实例方法表达，配置与传输层通过构造函数注入，
/// 便于测试和多实例隔离。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_auth_facade::gateway::{
///     Gateway, GatewayConfig, MemoryGatewayTransport, GatewayTransport,
/// };
/// use std::sync::Arc;
///
/// let transport = Arc::new(MemoryGatewayTransport::new());
/// let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());
///
/// transport.register_client("7f00000108fc00000001");
/// gateway.send_to_client("7f00000108fc00000001", "hello").unwrap();
/// ```
pub struct Gateway {
    /// Gateway 配置
    config: GatewayConfig,
    /// Gateway 传输层实现
    transport: Arc<dyn GatewayTransport>,
}

impl Gateway {
    /// 创建 Gateway 客户端
    ///
    /// # 参数
    ///
    /// - `config`: Gateway 配置
    /// - `transport`: 传输层实现（业务方注入 Redis pub/sub / HTTP / TCP 等具体实现）
    pub fn new(config: GatewayConfig, transport: Arc<dyn GatewayTransport>) -> Self {
        Self { config, transport }
    }

    /// 向指定 client_id 发送消息 — 对齐 `Gateway::sendToClient()`
    pub fn send_to_client(&self, client_id: &str, message: &str) -> Result<(), GatewayError> {
        self.transport.send_to_client(client_id, message)
    }

    /// 向所有在线 client_id 发送广播 — 对齐 `Gateway::sendToAll()`
    pub fn send_to_all(&self, message: &str) -> Result<(), GatewayError> {
        self.transport.send_to_all(message)
    }

    /// 向指定群组发送消息 — 对齐 `Gateway::sendToGroup()`
    pub fn send_to_group(&self, group: &str, message: &str) -> Result<(), GatewayError> {
        self.transport.send_to_group(group, message)
    }

    /// 将 client_id 加入群组 — 对齐 `Gateway::joinGroup()`
    pub fn join_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        self.transport.join_group(client_id, group)
    }

    /// 将 client_id 离开群组 — 对齐 `Gateway::leaveGroup()`
    pub fn leave_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        self.transport.leave_group(client_id, group)
    }

    /// 解散指定群组 — 对齐 `Gateway::ungroup()`
    pub fn ungroup(&self, group: &str) -> Result<(), GatewayError> {
        self.transport.ungroup(group)
    }

    /// 判断 client_id 是否在线 — 对齐 `Gateway::isOnline()`
    pub fn is_online(&self, client_id: &str) -> Result<bool, GatewayError> {
        self.transport.is_online(client_id)
    }

    /// 获取在线 client_id 数量 — 对齐 `Gateway::getClientCount()`
    pub fn get_client_count(&self) -> Result<usize, GatewayError> {
        self.transport.get_client_count()
    }

    /// 获取指定群组的在线 client_id 数量 — 对齐 `Gateway::getClientCountByGroup()`
    pub fn get_client_count_by_group(&self, group: &str) -> Result<usize, GatewayError> {
        self.transport.get_client_count_by_group(group)
    }

    /// 获取所有在线 client_id — 对齐 `Gateway::getAllClientIds()`
    pub fn get_all_client_ids(&self) -> Result<Vec<String>, GatewayError> {
        self.transport.get_all_client_ids()
    }

    /// 获取指定群组中的 client_id — 对齐 `Gateway::getClientIdListByGroup()`
    pub fn get_client_id_list_by_group(&self, group: &str) -> Result<Vec<String>, GatewayError> {
        self.transport.get_client_id_list_by_group(group)
    }

    /// 关闭指定 client_id 的连接 — 对齐 `Gateway::closeClient()`
    pub fn close_client(&self, client_id: &str) -> Result<(), GatewayError> {
        self.transport.close_client(client_id)
    }

    /// 获取 Gateway 配置
    pub fn config(&self) -> &GatewayConfig {
        &self.config
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // 辅助常量
    // ------------------------------------------------------------------------

    /// 测试用 client_id（对齐 PHP GatewayWorker 20 字符 hex 格式）
    const CLIENT_A: &str = "7f00000108fc00000001";
    /// 测试用 client_id B
    const CLIENT_B: &str = "7f00000108fc00000002";
    /// 测试用 client_id C
    const CLIENT_C: &str = "7f00000108fc00000003";

    // ------------------------------------------------------------------------
    // GatewayConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 GatewayConfig builder 模式
    #[test]
    fn test_gateway_config_builder() {
        let config = GatewayConfig::new("127.0.0.1:1238")
            .with_heartbeat_interval(30)
            .with_default_group("default");

        assert_eq!(config.register_address, "127.0.0.1:1238");
        assert_eq!(config.heartbeat_interval, 30);
        assert_eq!(config.default_group.as_deref(), Some("default"));
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — send_to_client
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 向单个客户端发送消息
    #[test]
    fn test_memory_gateway_transport_send_to_client() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);

        transport.send_to_client(CLIENT_A, "hello").unwrap();
        transport.send_to_client(CLIENT_A, "world").unwrap();

        let messages = transport.client_messages(CLIENT_A);
        assert_eq!(messages, vec!["hello".to_string(), "world".to_string()]);
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — send_to_clients
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 向多个客户端发送消息
    #[test]
    fn test_memory_gateway_transport_send_to_clients() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        transport
            .send_to_clients(&[CLIENT_A.to_string(), CLIENT_B.to_string()], "broadcast")
            .unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["broadcast".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_B),
            vec!["broadcast".to_string()]
        );
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — send_to_all
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 向所有在线客户端广播
    #[test]
    fn test_memory_gateway_transport_send_to_all() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);
        transport.register_client(CLIENT_C);

        transport.send_to_all("announcement").unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["announcement".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_B),
            vec!["announcement".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_C),
            vec!["announcement".to_string()]
        );
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — send_to_group
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 向群组发送消息
    #[test]
    fn test_memory_gateway_transport_send_to_group() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);
        transport.register_client(CLIENT_C);

        // A、B 加入 room1，C 不加入
        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();

        transport.send_to_group("room1", "group-msg").unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["group-msg".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_B),
            vec!["group-msg".to_string()]
        );
        // C 不在群组，不应收到消息
        assert!(transport.client_messages(CLIENT_C).is_empty());

        // 向不存在的群组发送消息应为空操作（不报错）
        transport.send_to_group("nonexistent", "msg").unwrap();
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — join_group / leave_group
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 加入和离开群组
    #[test]
    fn test_memory_gateway_transport_join_leave_group() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);

        // 加入群组
        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_A, "room2").unwrap();

        let groups = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert_eq!(groups, vec!["room1".to_string(), "room2".to_string()]);

        let clients = transport.get_client_id_list_by_group("room1").unwrap();
        assert_eq!(clients, vec![CLIENT_A.to_string()]);

        // 重复加入同一群组应为幂等操作
        transport.join_group(CLIENT_A, "room1").unwrap();
        let groups = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert_eq!(groups.len(), 2);

        // 离开群组
        transport.leave_group(CLIENT_A, "room1").unwrap();
        let groups = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert_eq!(groups, vec!["room2".to_string()]);

        let clients = transport.get_client_id_list_by_group("room1").unwrap();
        assert!(clients.is_empty());

        // 离开不存在的群组应返回 GroupNotFound
        let err = transport.leave_group(CLIENT_A, "nonexistent").unwrap_err();
        match err {
            GatewayError::GroupNotFound(group) => assert_eq!(group, "nonexistent"),
            other => panic!("期望 GroupNotFound, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — ungroup
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 解散群组
    #[test]
    fn test_memory_gateway_transport_ungroup() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();

        // 解散群组
        transport.ungroup("room1").unwrap();

        // 群组已不存在
        let clients = transport.get_client_id_list_by_group("room1").unwrap();
        assert!(clients.is_empty());

        // client 的群组列表中应移除 room1
        let groups_a = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert!(!groups_a.iter().any(|g| g == "room1"));
        let groups_b = transport.get_groups_by_client_id(CLIENT_B).unwrap();
        assert!(!groups_b.iter().any(|g| g == "room1"));

        // 解散不存在的群组应返回 GroupNotFound
        let err = transport.ungroup("nonexistent").unwrap_err();
        match err {
            GatewayError::GroupNotFound(group) => assert_eq!(group, "nonexistent"),
            other => panic!("期望 GroupNotFound, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — is_online
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 判断客户端在线状态
    #[test]
    fn test_memory_gateway_transport_is_online() {
        let transport = MemoryGatewayTransport::new();

        // 未注册客户端不在线
        assert!(!transport.is_online(CLIENT_A).unwrap());

        transport.register_client(CLIENT_A);
        assert!(transport.is_online(CLIENT_A).unwrap());

        // 关闭后不在线
        transport.close_client(CLIENT_A).unwrap();
        assert!(!transport.is_online(CLIENT_A).unwrap());
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — get_client_count
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 获取在线客户端数量
    #[test]
    fn test_memory_gateway_transport_get_client_count() {
        let transport = MemoryGatewayTransport::new();
        assert_eq!(transport.get_client_count().unwrap(), 0);

        transport.register_client(CLIENT_A);
        assert_eq!(transport.get_client_count().unwrap(), 1);

        transport.register_client(CLIENT_B);
        transport.register_client(CLIENT_C);
        assert_eq!(transport.get_client_count().unwrap(), 3);

        // 重复注册不应增加计数
        transport.register_client(CLIENT_A);
        assert_eq!(transport.get_client_count().unwrap(), 3);

        // 关闭后计数减少
        transport.close_client(CLIENT_B).unwrap();
        assert_eq!(transport.get_client_count().unwrap(), 2);
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — get_client_count_by_group
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 获取群组客户端数量
    #[test]
    fn test_memory_gateway_transport_get_client_count_by_group() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);
        transport.register_client(CLIENT_C);

        // 群组不存在时返回 0
        assert_eq!(transport.get_client_count_by_group("room1").unwrap(), 0);

        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();
        transport.join_group(CLIENT_C, "room2").unwrap();

        assert_eq!(transport.get_client_count_by_group("room1").unwrap(), 2);
        assert_eq!(transport.get_client_count_by_group("room2").unwrap(), 1);

        // A 离开 room1
        transport.leave_group(CLIENT_A, "room1").unwrap();
        assert_eq!(transport.get_client_count_by_group("room1").unwrap(), 1);
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — get_all_client_ids
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 获取所有在线 client_id
    #[test]
    fn test_memory_gateway_transport_get_all_client_ids() {
        let transport = MemoryGatewayTransport::new();
        assert!(transport.get_all_client_ids().unwrap().is_empty());

        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        let mut ids = transport.get_all_client_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec![CLIENT_A.to_string(), CLIENT_B.to_string()]);
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — get_client_id_list_by_group
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 获取群组中的 client_id
    #[test]
    fn test_memory_gateway_transport_get_client_id_list_by_group() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        // 群组不存在时返回空 Vec
        assert!(transport
            .get_client_id_list_by_group("room1")
            .unwrap()
            .is_empty());

        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();

        let mut clients = transport.get_client_id_list_by_group("room1").unwrap();
        clients.sort();
        assert_eq!(clients, vec![CLIENT_A.to_string(), CLIENT_B.to_string()]);
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — get_groups_by_client_id
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 获取客户端所在的群组
    #[test]
    fn test_memory_gateway_transport_get_groups_by_client_id() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);

        // 无群组时返回空 Vec
        assert!(transport
            .get_groups_by_client_id(CLIENT_A)
            .unwrap()
            .is_empty());

        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_A, "room2").unwrap();

        let groups = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert_eq!(groups, vec!["room1".to_string(), "room2".to_string()]);

        // 不在线的客户端返回 ClientNotFound
        let err = transport.get_groups_by_client_id(CLIENT_B).unwrap_err();
        match err {
            GatewayError::ClientNotFound(client_id) => assert_eq!(client_id, CLIENT_B),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — close_client
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 关闭客户端连接
    #[test]
    fn test_memory_gateway_transport_close_client() {
        let transport = MemoryGatewayTransport::new();
        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();

        // 关闭 A
        transport.close_client(CLIENT_A).unwrap();

        // A 不再在线
        assert!(!transport.is_online(CLIENT_A).unwrap());
        assert_eq!(transport.get_client_count().unwrap(), 1);

        // A 应从群组中移除
        let clients = transport.get_client_id_list_by_group("room1").unwrap();
        assert_eq!(clients, vec![CLIENT_B.to_string()]);

        // 关闭不在线的客户端返回 ClientNotFound
        let err = transport.close_client(CLIENT_A).unwrap_err();
        match err {
            GatewayError::ClientNotFound(client_id) => assert_eq!(client_id, CLIENT_A),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemoryGatewayTransport — ClientNotFound
    // ------------------------------------------------------------------------

    /// 测试 MemoryGatewayTransport 客户端未找到错误
    #[test]
    fn test_memory_gateway_transport_client_not_found() {
        let transport = MemoryGatewayTransport::new();

        // 向未注册客户端发送消息
        let err = transport.send_to_client(CLIENT_A, "msg").unwrap_err();
        match err {
            GatewayError::ClientNotFound(client_id) => assert_eq!(client_id, CLIENT_A),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }

        // 向未注册客户端加入群组
        let err = transport.join_group(CLIENT_A, "room1").unwrap_err();
        match err {
            GatewayError::ClientNotFound(client_id) => assert_eq!(client_id, CLIENT_A),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }

        // send_to_clients 中任一客户端不在线
        transport.register_client(CLIENT_A);
        let err = transport
            .send_to_clients(&[CLIENT_A.to_string(), CLIENT_B.to_string()], "msg")
            .unwrap_err();
        match err {
            GatewayError::ClientNotFound(client_id) => assert_eq!(client_id, CLIENT_B),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }

        // send_to_clients 失败时不应部分发送
        assert!(transport.client_messages(CLIENT_A).is_empty());
    }

    // ------------------------------------------------------------------------
    // Gateway 测试
    // ------------------------------------------------------------------------

    /// 测试 Gateway 向单个客户端发送消息
    #[test]
    fn test_gateway_send_to_client() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client(CLIENT_A);
        gateway.send_to_client(CLIENT_A, "hello").unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["hello".to_string()]
        );
    }

    /// 测试 Gateway 向所有在线客户端广播
    #[test]
    fn test_gateway_send_to_all() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);

        gateway.send_to_all("broadcast").unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["broadcast".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_B),
            vec!["broadcast".to_string()]
        );
    }

    /// 测试 Gateway 向群组发送消息
    #[test]
    fn test_gateway_send_to_group() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);
        transport.join_group(CLIENT_A, "room1").unwrap();
        transport.join_group(CLIENT_B, "room1").unwrap();

        gateway.send_to_group("room1", "group-msg").unwrap();

        assert_eq!(
            transport.client_messages(CLIENT_A),
            vec!["group-msg".to_string()]
        );
        assert_eq!(
            transport.client_messages(CLIENT_B),
            vec!["group-msg".to_string()]
        );
    }

    /// 测试 Gateway 加入群组
    #[test]
    fn test_gateway_join_group() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client(CLIENT_A);

        gateway.join_group(CLIENT_A, "room1").unwrap();

        let groups = transport.get_groups_by_client_id(CLIENT_A).unwrap();
        assert_eq!(groups, vec!["room1".to_string()]);

        let clients = gateway.get_client_id_list_by_group("room1").unwrap();
        assert_eq!(clients, vec![CLIENT_A.to_string()]);
    }

    /// 测试 Gateway 判断客户端在线状态
    #[test]
    fn test_gateway_is_online() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        assert!(!gateway.is_online(CLIENT_A).unwrap());

        transport.register_client(CLIENT_A);
        assert!(gateway.is_online(CLIENT_A).unwrap());
    }

    /// 测试 Gateway 获取在线客户端数量
    #[test]
    fn test_gateway_get_client_count() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        assert_eq!(gateway.get_client_count().unwrap(), 0);

        transport.register_client(CLIENT_A);
        transport.register_client(CLIENT_B);
        assert_eq!(gateway.get_client_count().unwrap(), 2);

        // 验证 config 访问器
        assert_eq!(gateway.config().register_address, "127.0.0.1:1238");
        assert_eq!(gateway.config().heartbeat_interval, 55);
    }

    /// 测试 Gateway 关闭客户端连接
    #[test]
    fn test_gateway_close_client() {
        let transport = Arc::new(MemoryGatewayTransport::new());
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client(CLIENT_A);
        transport.join_group(CLIENT_A, "room1").unwrap();

        gateway.close_client(CLIENT_A).unwrap();

        assert!(!gateway.is_online(CLIENT_A).unwrap());
        assert_eq!(gateway.get_client_count().unwrap(), 0);
        assert!(gateway
            .get_client_id_list_by_group("room1")
            .unwrap()
            .is_empty());
    }
}
