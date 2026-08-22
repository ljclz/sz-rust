//! Redis Gateway 集群广播 — 基于 Redis pub/sub 实现跨节点 WebSocket 消息广播
//!
//! ## 架构说明
//!
//! 在多节点部署场景下，每个 sz-rust 进程只持有本地 WebSocket 连接。
//! 当业务调用 `Gateway::send_to_all()` 等广播 API 时，需要将消息投递到所有节点
//! 的本地客户端。本模块通过 Redis pub/sub 实现：
//!
//! 1. **发送端**（本模块）：将 Gateway 命令序列化为 JSON，PUBLISH 到对应频道
//! 2. **接收端**（业务侧自行实现）：SUBSCRIBE 频道，收到消息后转发给本地客户端
//!
//! ## Redis 数据结构
//!
//! | Key | 类型 | 说明 |
//! |-----|------|------|
//! | `{prefix}:online` | SET | 所有在线 client_id |
//! | `{prefix}:cgroups:{client_id}` | SET | client_id 所在的群组 |
//! | `{prefix}:group:{group}` | SET | 群组中的 client_id |
//!
//! ## Pub/Sub 频道
//!
//! | 频道 | 说明 |
//! |------|------|
//! | `{prefix}:ch:all` | 广播给所有客户端 |
//! | `{prefix}:ch:group:{group}` | 广播给指定群组 |
//! | `{prefix}:ch:client:{client_id}` | 发送给指定客户端 |
//! | `{prefix}:ch:close:{client_id}` | 关闭指定客户端连接 |
//!
//! ## 消息格式
//!
//! 所有 PUBLISH 的 payload 均为 JSON，由 [`GatewayCommand`] 序列化产生。
//! 接收端反序列化后执行对应操作。
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_auth_facade::gateway::{Gateway, GatewayConfig, GatewayTransport};
//! use sz_rust_auth_facade::redis_gateway::RedisGatewayTransport;
//! use std::sync::Arc;
//!
//! let transport = Arc::new(
//!     RedisGatewayTransport::connect("redis://127.0.0.1:6379", "sz-rust")
//!         .expect("连接 Redis 失败")
//! );
//! let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport);
//!
//! // send_to_all 会 PUBLISH 到 "sz-rust:ch:all" 频道
//! gateway.send_to_all("hello cluster").unwrap();
//! ```
//!
//! ## 线程安全
//!
//! 通过 `parking_lot::Mutex` 保护 Redis 连接，支持并发调用。
//! 所有操作在单个锁内完成，保证状态一致性。
//!
//! ## 性能说明
//!
//! 本模块使用同步 Redis 连接（`redis::blocking::Connection`），因为
//! [`GatewayTransport`](crate::gateway::GatewayTransport) trait 为同步接口。
//! 在 tokio 异步上下文中调用时，建议使用 `tokio::task::spawn_blocking`
//! 包装，避免阻塞 tokio 运行时。

use parking_lot::Mutex;
use thiserror::Error;

use crate::gateway::{GatewayError, GatewayTransport};

// ============================================================================
// Redis 命令封装
// ============================================================================

/// Gateway 集群广播命令 — 通过 Redis pub/sub 传递的指令
///
/// 每个命令序列化为 JSON 后 PUBLISH 到对应频道，接收端反序列化后执行。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum GatewayCommand {
    /// 向指定客户端发送消息 — PUBLISH 到 `{prefix}:ch:client:{client_id}`
    SendToClient {
        /// 目标客户端 ID
        client_id: String,
        /// 消息内容
        message: String,
    },
    /// 向所有在线客户端广播 — PUBLISH 到 `{prefix}:ch:all`
    SendToAll {
        /// 消息内容
        message: String,
    },
    /// 向指定群组发送消息 — PUBLISH 到 `{prefix}:ch:group:{group}`
    SendToGroup {
        /// 群组名
        group: String,
        /// 消息内容
        message: String,
    },
    /// 关闭指定客户端连接 — PUBLISH 到 `{prefix}:ch:close:{client_id}`
    CloseClient {
        /// 目标客户端 ID
        client_id: String,
    },
}

impl GatewayCommand {
    /// 序列化为 JSON 字符串
    fn to_json(&self) -> Result<String, GatewayError> {
        serde_json::to_string(self)
            .map_err(|e| GatewayError::Serialize(format!("GatewayCommand 序列化失败: {e}")))
    }

    /// 从 JSON 反序列化（供接收端使用）
    pub fn from_json(json: &str) -> Result<Self, GatewayError> {
        serde_json::from_str(json)
            .map_err(|e| GatewayError::Serialize(format!("GatewayCommand 反序列化失败: {e}")))
    }
}

// ============================================================================
// RedisGatewayConfig
// ============================================================================

/// Redis Gateway 配置
///
/// # 参数
///
/// - `redis_url`: Redis 连接地址（如 `redis://127.0.0.1:6379`）
/// - `key_prefix`: Redis key/频道前缀（用于多实例隔离，默认 `sz-rust`）
#[derive(Debug, Clone)]
pub struct RedisGatewayConfig {
    /// Redis 连接地址
    pub redis_url: String,
    /// Redis key/频道前缀
    pub key_prefix: String,
}

impl Default for RedisGatewayConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            key_prefix: "sz-rust".to_string(),
        }
    }
}

impl RedisGatewayConfig {
    /// 创建新配置
    pub fn new(redis_url: impl Into<String>) -> Self {
        Self {
            redis_url: redis_url.into(),
            key_prefix: "sz-rust".to_string(),
        }
    }

    /// 设置 key 前缀
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// 在线客户端集合 key：`{prefix}:online`
    fn online_key(&self) -> String {
        format!("{}:online", self.key_prefix)
    }

    /// 客户端群组集合 key：`{prefix}:cgroups:{client_id}`
    fn client_groups_key(&self, client_id: &str) -> String {
        format!("{}:cgroups:{}", self.key_prefix, client_id)
    }

    /// 群组成员集合 key：`{prefix}:group:{group}`
    fn group_key(&self, group: &str) -> String {
        format!("{}:group:{}", self.key_prefix, group)
    }

    /// 广播频道：`{prefix}:ch:all`
    fn channel_all(&self) -> String {
        format!("{}:ch:all", self.key_prefix)
    }

    /// 群组频道：`{prefix}:ch:group:{group}`
    fn channel_group(&self, group: &str) -> String {
        format!("{}:ch:group:{}", self.key_prefix, group)
    }

    /// 客户端频道：`{prefix}:ch:client:{client_id}`
    fn channel_client(&self, client_id: &str) -> String {
        format!("{}:ch:client:{}", self.key_prefix, client_id)
    }

    /// 关闭频道：`{prefix}:ch:close:{client_id}`
    fn channel_close(&self, client_id: &str) -> String {
        format!("{}:ch:close:{}", self.key_prefix, client_id)
    }
}

// ============================================================================
// RedisGatewayTransport
// ============================================================================

/// Redis Gateway 传输实现 — 基于 Redis pub/sub 实现集群广播
///
/// 通过 Redis 维护在线客户端和群组状态，并通过 pub/sub 向所有节点广播消息。
///
/// # 线程安全
///
/// 通过 `parking_lot::Mutex<redis::blocking::Connection>` 保护连接，
/// 支持多线程并发调用。所有 Redis 操作在单个锁内完成。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_auth_facade::redis_gateway::{RedisGatewayTransport, RedisGatewayConfig};
/// use sz_rust_auth_facade::gateway::GatewayTransport;
///
/// let transport = RedisGatewayTransport::connect(
///     RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("my-app"),
/// ).expect("连接 Redis 失败");
///
/// // 注册客户端上线
/// transport.register_client("7f00000108fc00000001").unwrap();
///
/// // 发送消息（会 PUBLISH 到 Redis 频道）
/// transport.send_to_client("7f00000108fc00000001", "hello").unwrap();
/// ```
pub struct RedisGatewayTransport {
    /// Redis 连接（同步，受 Mutex 保护）
    conn: Mutex<redis::Connection>,
    /// 配置
    config: RedisGatewayConfig,
}

impl RedisGatewayTransport {
    /// 创建 Redis Gateway 传输（连接 Redis）
    ///
    /// # 参数
    ///
    /// - `config`: Redis Gateway 配置
    ///
    /// # 错误
    ///
    /// Redis 连接失败时返回 [`GatewayError::Transport`]。
    pub fn connect(config: RedisGatewayConfig) -> Result<Self, GatewayError> {
        let client = redis::Client::open(config.redis_url.as_str())
            .map_err(|e| GatewayError::Transport(format!("Redis Client 创建失败: {e}")))?;
        let conn = client
            .get_connection()
            .map_err(|e| GatewayError::Transport(format!("Redis 连接失败: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            config,
        })
    }

    /// 从已有 `redis::Client` 创建（复用连接池）
    ///
    /// # 参数
    ///
    /// - `client`: 已创建的 Redis Client
    /// - `config`: Redis Gateway 配置（使用其中的 key_prefix，redis_url 被忽略）
    pub fn from_client(
        client: redis::Client,
        config: RedisGatewayConfig,
    ) -> Result<Self, GatewayError> {
        let conn = client
            .get_connection()
            .map_err(|e| GatewayError::Transport(format!("Redis 连接失败: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            config,
        })
    }

    /// 注册客户端上线（非 trait 方法，供业务侧在客户端连接时调用）
    ///
    /// 将 client_id 加入 Redis 在线集合。幂等操作。
    pub fn register_client(&self, client_id: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        redis::cmd("SADD")
            .arg(self.config.online_key())
            .arg(client_id)
            .query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    /// 注销客户端下线（非 trait 方法，供业务侧在客户端断开时调用）
    ///
    /// 从在线集合移除 client_id，并从所有群组中移除。
    pub fn unregister_client(&self, client_id: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SREM")
            .arg(self.config.online_key())
            .arg(client_id)
            .ignore()
            .cmd("SMEMBERS")
            .arg(self.config.client_groups_key(client_id));
        let (removed, groups): (i64, Vec<String>) = pipe
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("注销客户端查询群组失败: {e}")))?;
        if removed == 0 {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        let mut cleanup_pipe = redis::pipe();
        cleanup_pipe.atomic();
        cleanup_pipe
            .cmd("DEL")
            .arg(self.config.client_groups_key(client_id))
            .ignore();
        for group in &groups {
            cleanup_pipe
                .cmd("SREM")
                .arg(self.config.group_key(group))
                .arg(client_id)
                .ignore();
        }
        cleanup_pipe
            .query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    /// 获取配置引用
    pub fn config(&self) -> &RedisGatewayConfig {
        &self.config
    }

    /// PUBLISH 命令到指定频道（内部辅助方法）
    fn publish(&self, channel: &str, command: &GatewayCommand) -> Result<(), GatewayError> {
        let json = command.to_json()?;
        let mut conn = self.conn.lock();
        redis::cmd("PUBLISH")
            .arg(channel)
            .arg(json)
            .query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    /// 执行 Redis 命令并返回结果（内部辅助方法）
    fn query<T: redis::FromRedisValue>(&self, cmd: &mut redis::Cmd) -> Result<T, GatewayError> {
        let mut conn = self.conn.lock();
        cmd.query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 命令执行失败: {e}")))
    }
}

impl GatewayTransport for RedisGatewayTransport {
    fn send_to_client(&self, client_id: &str, message: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        let is_member: bool = redis::cmd("SISMEMBER")
            .arg(self.config.online_key())
            .arg(client_id)
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))?;
        if !is_member {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        drop(conn);
        let command = GatewayCommand::SendToClient {
            client_id: client_id.to_string(),
            message: message.to_string(),
        };
        self.publish(&self.config.channel_client(client_id), &command)
    }

    fn send_to_clients(&self, client_ids: &[String], message: &str) -> Result<(), GatewayError> {
        if client_ids.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        for client_id in client_ids {
            if client_id.is_empty() {
                return Err(GatewayError::InvalidClientId(client_id.to_string()));
            }
            let is_member: bool = redis::cmd("SISMEMBER")
                .arg(self.config.online_key())
                .arg(client_id)
                .query(&mut *conn)
                .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))?;
            if !is_member {
                return Err(GatewayError::ClientNotFound(client_id.to_string()));
            }
        }
        drop(conn);
        for client_id in client_ids {
            let command = GatewayCommand::SendToClient {
                client_id: client_id.to_string(),
                message: message.to_string(),
            };
            self.publish(&self.config.channel_client(client_id), &command)?;
        }
        Ok(())
    }

    fn send_to_all(&self, message: &str) -> Result<(), GatewayError> {
        let command = GatewayCommand::SendToAll {
            message: message.to_string(),
        };
        self.publish(&self.config.channel_all(), &command)
    }

    fn send_to_group(&self, group: &str, message: &str) -> Result<(), GatewayError> {
        let command = GatewayCommand::SendToGroup {
            group: group.to_string(),
            message: message.to_string(),
        };
        self.publish(&self.config.channel_group(group), &command)
    }

    fn get_all_client_ids(&self) -> Result<Vec<String>, GatewayError> {
        self.query(redis::cmd("SMEMBERS").arg(self.config.online_key()))
    }

    fn get_client_id_list_by_group(&self, group: &str) -> Result<Vec<String>, GatewayError> {
        self.query(redis::cmd("SMEMBERS").arg(self.config.group_key(group)))
    }

    fn get_groups_by_client_id(&self, client_id: &str) -> Result<Vec<String>, GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        let is_member: bool = redis::cmd("SISMEMBER")
            .arg(self.config.online_key())
            .arg(client_id)
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))?;
        if !is_member {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        redis::cmd("SMEMBERS")
            .arg(self.config.client_groups_key(client_id))
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SMEMBERS 失败: {e}")))
    }

    fn join_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        let is_member: bool = redis::cmd("SISMEMBER")
            .arg(self.config.online_key())
            .arg(client_id)
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))?;
        if !is_member {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SADD")
            .arg(self.config.client_groups_key(client_id))
            .arg(group)
            .ignore()
            .cmd("SADD")
            .arg(self.config.group_key(group))
            .arg(client_id)
            .ignore();
        pipe.query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    fn leave_group(&self, client_id: &str, group: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        let mut conn = self.conn.lock();
        let is_member: bool = redis::cmd("SISMEMBER")
            .arg(self.config.online_key())
            .arg(client_id)
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))?;
        if !is_member {
            return Err(GatewayError::ClientNotFound(client_id.to_string()));
        }
        let group_exists: bool = redis::cmd("EXISTS")
            .arg(self.config.group_key(group))
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("EXISTS 失败: {e}")))?;
        if !group_exists {
            return Err(GatewayError::GroupNotFound(group.to_string()));
        }
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SREM")
            .arg(self.config.client_groups_key(client_id))
            .arg(group)
            .ignore()
            .cmd("SREM")
            .arg(self.config.group_key(group))
            .arg(client_id)
            .ignore();
        pipe.query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    fn ungroup(&self, group: &str) -> Result<(), GatewayError> {
        let mut conn = self.conn.lock();
        let client_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.config.group_key(group))
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SMEMBERS 失败: {e}")))?;
        if client_ids.is_empty() {
            let exists: bool = redis::cmd("EXISTS")
                .arg(self.config.group_key(group))
                .query(&mut *conn)
                .map_err(|e| GatewayError::Transport(format!("EXISTS 失败: {e}")))?;
            if !exists {
                return Err(GatewayError::GroupNotFound(group.to_string()));
            }
        }
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.cmd("DEL").arg(self.config.group_key(group)).ignore();
        for client_id in &client_ids {
            pipe.cmd("SREM")
                .arg(self.config.client_groups_key(client_id))
                .arg(group)
                .ignore();
        }
        pipe.query::<()>(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("Redis 写入失败: {e}")))?;
        Ok(())
    }

    fn is_online(&self, client_id: &str) -> Result<bool, GatewayError> {
        let mut conn = self.conn.lock();
        redis::cmd("SISMEMBER")
            .arg(self.config.online_key())
            .arg(client_id)
            .query(&mut *conn)
            .map_err(|e| GatewayError::Transport(format!("SISMEMBER 失败: {e}")))
    }

    fn get_client_count(&self) -> Result<usize, GatewayError> {
        let count: i64 = self.query(redis::cmd("SCARD").arg(self.config.online_key()))?;
        Ok(count as usize)
    }

    fn get_client_count_by_group(&self, group: &str) -> Result<usize, GatewayError> {
        let count: i64 = self.query(redis::cmd("SCARD").arg(self.config.group_key(group)))?;
        Ok(count as usize)
    }

    fn close_client(&self, client_id: &str) -> Result<(), GatewayError> {
        if client_id.is_empty() {
            return Err(GatewayError::InvalidClientId(client_id.to_string()));
        }
        self.unregister_client(client_id)?;
        let command = GatewayCommand::CloseClient {
            client_id: client_id.to_string(),
        };
        self.publish(&self.config.channel_close(client_id), &command)
    }
}

// ============================================================================
// RedisGatewaySubscriber — 接收端辅助
// ============================================================================

/// Redis Gateway 订阅器 — 订阅集群广播频道，返回收到的命令
///
/// 业务侧在每个 WebSocket 节点上启动一个订阅器，收到 [`GatewayCommand`] 后
/// 转发给本地客户端。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_auth_facade::redis_gateway::{RedisGatewaySubscriber, RedisGatewayConfig};
///
/// # tokio_test::block_on(async {
/// let mut subscriber = RedisGatewaySubscriber::connect(
///     RedisGatewayConfig::new("redis://127.0.0.1:6379"),
/// ).unwrap();
/// subscriber.subscribe_all().unwrap();
///
/// while let Ok(cmd) = subscriber.next_command().await {
///     // 将 cmd 转发给本地 WebSocket 客户端
///     match cmd {
///         GatewayCommand::SendToAll { message } => { /* ... */ }
///         _ => {}
///     }
/// }
/// # });
/// ```
pub struct RedisGatewaySubscriber {
    /// Redis pubsub 连接（异步）
    pubsub: redis::aio::PubSub,
    /// 配置
    config: RedisGatewayConfig,
}

/// 订阅错误
#[derive(Debug, Error)]
pub enum SubscribeError {
    /// Redis 错误
    #[error("Redis 错误: {0}")]
    Redis(#[from] redis::RedisError),
    /// 命令反序列化失败
    #[error("命令反序列化失败: {0}")]
    Deserialize(String),
}

impl RedisGatewaySubscriber {
    /// 创建订阅器（连接 Redis）
    pub async fn connect(config: RedisGatewayConfig) -> Result<Self, SubscribeError> {
        let client = redis::Client::open(config.redis_url.as_str())?;
        let pubsub = client.get_async_pubsub().await?;
        Ok(Self { pubsub, config })
    }

    /// 订阅所有集群广播频道
    ///
    /// 使用 PSUBSCRIBE 模式匹配，订阅：
    /// - `{prefix}:ch:all`
    /// - `{prefix}:ch:group:*`
    /// - `{prefix}:ch:client:*`
    /// - `{prefix}:ch:close:*`
    pub async fn subscribe_all(&mut self) -> Result<(), SubscribeError> {
        let prefix = &self.config.key_prefix;
        let patterns = [
            format!("{prefix}:ch:all"),
            format!("{prefix}:ch:group:*"),
            format!("{prefix}:ch:client:*"),
            format!("{prefix}:ch:close:*"),
        ];
        for pattern in &patterns {
            self.pubsub.psubscribe(pattern).await?;
        }
        Ok(())
    }

    /// 获取下一条命令（异步）
    ///
    /// 阻塞等待 Redis pub/sub 消息，反序列化为 [`GatewayCommand`]。
    pub async fn next_command(&mut self) -> Result<GatewayCommand, SubscribeError> {
        use futures::StreamExt;
        let msg = self.pubsub.on_message().next().await;
        match msg {
            Some(msg) => {
                let payload: String = msg.get_payload().map_err(|e| {
                    SubscribeError::Deserialize(format!("payload 类型转换失败: {e}"))
                })?;
                GatewayCommand::from_json(&payload)
                    .map_err(|e| SubscribeError::Deserialize(e.to_string()))
            }
            None => Err(SubscribeError::Deserialize("pub/sub 流结束".to_string())),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &RedisGatewayConfig {
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
    // GatewayCommand 序列化/反序列化测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_command_send_to_client_roundtrip() {
        let cmd = GatewayCommand::SendToClient {
            client_id: "7f00000108fc00000001".to_string(),
            message: "hello".to_string(),
        };
        let json = cmd.to_json().unwrap();
        let decoded = GatewayCommand::from_json(&json).unwrap();
        match decoded {
            GatewayCommand::SendToClient { client_id, message } => {
                assert_eq!(client_id, "7f00000108fc00000001");
                assert_eq!(message, "hello");
            }
            other => panic!("期望 SendToClient, 实际 {other:?}"),
        }
    }

    #[test]
    fn test_command_send_to_all_roundtrip() {
        let cmd = GatewayCommand::SendToAll {
            message: "broadcast".to_string(),
        };
        let json = cmd.to_json().unwrap();
        let decoded = GatewayCommand::from_json(&json).unwrap();
        match decoded {
            GatewayCommand::SendToAll { message } => {
                assert_eq!(message, "broadcast");
            }
            other => panic!("期望 SendToAll, 实际 {other:?}"),
        }
    }

    #[test]
    fn test_command_send_to_group_roundtrip() {
        let cmd = GatewayCommand::SendToGroup {
            group: "room1".to_string(),
            message: "group-msg".to_string(),
        };
        let json = cmd.to_json().unwrap();
        let decoded = GatewayCommand::from_json(&json).unwrap();
        match decoded {
            GatewayCommand::SendToGroup { group, message } => {
                assert_eq!(group, "room1");
                assert_eq!(message, "group-msg");
            }
            other => panic!("期望 SendToGroup, 实际 {other:?}"),
        }
    }

    #[test]
    fn test_command_close_client_roundtrip() {
        let cmd = GatewayCommand::CloseClient {
            client_id: "7f00000108fc00000001".to_string(),
        };
        let json = cmd.to_json().unwrap();
        let decoded = GatewayCommand::from_json(&json).unwrap();
        match decoded {
            GatewayCommand::CloseClient { client_id } => {
                assert_eq!(client_id, "7f00000108fc00000001");
            }
            other => panic!("期望 CloseClient, 实际 {other:?}"),
        }
    }

    #[test]
    fn test_command_from_json_invalid() {
        let result = GatewayCommand::from_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_command_json_tag_format() {
        let cmd = GatewayCommand::SendToAll {
            message: "x".to_string(),
        };
        let json = cmd.to_json().unwrap();
        assert!(json.contains("\"cmd\":\"send_to_all\""));
    }

    // ------------------------------------------------------------------------
    // RedisGatewayConfig 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_redis_gateway_config_default() {
        let config = RedisGatewayConfig::default();
        assert_eq!(config.redis_url, "redis://127.0.0.1:6379");
        assert_eq!(config.key_prefix, "sz-rust");
    }

    #[test]
    fn test_redis_gateway_config_builder() {
        let config = RedisGatewayConfig::new("redis://10.0.0.1:6380").with_prefix("my-app");
        assert_eq!(config.redis_url, "redis://10.0.0.1:6380");
        assert_eq!(config.key_prefix, "my-app");
    }

    #[test]
    fn test_redis_gateway_config_keys() {
        let config = RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test");
        assert_eq!(config.online_key(), "test:online");
        assert_eq!(config.client_groups_key("client1"), "test:cgroups:client1");
        assert_eq!(config.group_key("room1"), "test:group:room1");
        assert_eq!(config.channel_all(), "test:ch:all");
        assert_eq!(config.channel_group("room1"), "test:ch:group:room1");
        assert_eq!(config.channel_client("client1"), "test:ch:client:client1");
        assert_eq!(config.channel_close("client1"), "test:ch:close:client1");
    }

    // ------------------------------------------------------------------------
    // RedisGatewayTransport 集成测试（需要 Redis 服务器，标记 #[ignore]）
    // ------------------------------------------------------------------------

    /// 测试 Redis 连接和基本操作
    ///
    /// 运行方式：`cargo test --package sz-rust-auth-facade --features redis-gateway test_redis_connect -- --ignored`
    ///
    /// 前置条件：本地 Redis 服务器运行在 127.0.0.1:6379
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_connect_and_register() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        let client_id = "test-client-001";

        transport.register_client(client_id).unwrap();
        assert!(transport.is_online(client_id).unwrap());
        assert_eq!(transport.get_client_count().unwrap(), 1);

        transport.unregister_client(client_id).unwrap();
        assert!(!transport.is_online(client_id).unwrap());
        assert_eq!(transport.get_client_count().unwrap(), 0);
    }

    /// 测试群组操作
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_group_operations() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        let client_a = "test-client-a";
        let client_b = "test-client-b";

        transport.register_client(client_a).unwrap();
        transport.register_client(client_b).unwrap();

        transport.join_group(client_a, "room1").unwrap();
        transport.join_group(client_b, "room1").unwrap();

        assert_eq!(transport.get_client_count_by_group("room1").unwrap(), 2);

        let groups = transport.get_groups_by_client_id(client_a).unwrap();
        assert_eq!(groups, vec!["room1".to_string()]);

        transport.leave_group(client_a, "room1").unwrap();
        assert_eq!(transport.get_client_count_by_group("room1").unwrap(), 1);

        transport.unregister_client(client_a).unwrap();
        transport.unregister_client(client_b).unwrap();
    }

    /// 测试 send_to_all 不报错（PUBLISH 到频道）
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_send_to_all() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        transport.send_to_all("hello cluster").unwrap();
    }

    /// 测试 send_to_group 不报错
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_send_to_group() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        transport.send_to_group("room1", "group message").unwrap();
    }

    /// 测试 send_to_client 需要客户端在线
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_send_to_client_not_online() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        let err = transport.send_to_client("nonexistent", "msg").unwrap_err();
        match err {
            GatewayError::ClientNotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("期望 ClientNotFound, 实际 {other:?}"),
        }
    }

    /// 测试空 client_id 校验
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_redis_invalid_client_id() {
        let transport = RedisGatewayTransport::connect(
            RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
        )
        .expect("连接 Redis 失败");

        let err = transport.register_client("").unwrap_err();
        assert!(matches!(err, GatewayError::InvalidClientId(_)));

        let err = transport.send_to_client("", "msg").unwrap_err();
        assert!(matches!(err, GatewayError::InvalidClientId(_)));
    }

    /// 测试 Gateway 集成（通过 RedisGatewayTransport）
    #[test]
    #[ignore = "需要 Redis 服务器运行在 127.0.0.1:6379"]
    fn test_gateway_with_redis_transport() {
        use crate::gateway::{Gateway, GatewayConfig};
        use std::sync::Arc;

        let transport = Arc::new(
            RedisGatewayTransport::connect(
                RedisGatewayConfig::new("redis://127.0.0.1:6379").with_prefix("test-gateway"),
            )
            .expect("连接 Redis 失败"),
        );
        let gateway = Gateway::new(GatewayConfig::new("127.0.0.1:1238"), transport.clone());

        transport.register_client("gw-test-client").unwrap();
        assert!(gateway.is_online("gw-test-client").unwrap());

        gateway.send_to_all("gateway broadcast").unwrap();
        gateway.close_client("gw-test-client").unwrap();
        assert!(!gateway.is_online("gw-test-client").unwrap());
    }
}
