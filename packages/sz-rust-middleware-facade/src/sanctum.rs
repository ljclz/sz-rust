//! @REVIEW_REQUIRED（铁律 R12）：人类必须审查此文件
//!
//! 审查要点：
//! - Token 生成算法（熵源、碰撞概率）
//! - Token 存储安全（是否明文存储、是否有加密）
//! - 能力（ability）检查逻辑是否可被绕过
//!
//! 审查者签名：__________  日期：__________  结论：__________
//!
//! Sanctum 中间件 — 个人访问令牌（Personal Access Token）认证
//!
//! 对齐 Laravel Sanctum 的核心能力，提供基于缓存层存储的轻量级 token 管理器。
//!
//! ## 与 Laravel Sanctum 的对齐
//!
//! | Laravel Sanctum | sz-rust Sanctum | 说明 |
//! |-----------------|-----------------|------|
//! | `personal_access_tokens` 表 | `Cache::set("sanctum:token:<hash>", ...)` | 以 cache 替代 DB 表 |
//! | `$user->createToken('name', ['ability'])` | [`Sanctum::create_token`] | 签发 token |
//! | `$request->user()` | `req.extensions().get::<SanctumUser>()` | 注入扩展 |
//! | `Token::currentAccessToken()` | `req.extensions().get::<SanctumUser>()` | 读取 token 信息 |
//! | `$token->revoke()` | [`Sanctum::revoke`] | 撤销单个 token |
//! | `$user->tokens()->delete()` | [`Sanctum::revoke_all_for_user`] | 撤销用户所有 token |
//! | `tokenCan('ability')` | [`SanctumUser::token_can`] | 权限检查 |
//! | 中间件 `EnsureFrontendRequestsAreStateful` | [`sanctum_middleware`] | axum 风格中间件 |
//!
//! ## Token 格式
//!
//! 采用 Laravel Sanctum 兼容格式：`<token_id>|<random_64_char_hex>`。
//! - `token_id`：i64 自增（基于 cache 自增计数器）
//! - `<random_64_char_hex>`：32 字节随机数 hex 编码
//!
//! **存储原则**：cache 中只存 `sha256(plain_token)` 的 hex → `PersonalAccessToken` 映射，
//! 明文 token 仅在签发时返回一次，丢失不可恢复。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::cache::Cache;
//! use sz_rust_core::middleware::sanctum::{Sanctum, SanctumConfig, sanctum_middleware};
//! use axum::{Router, routing::get, middleware};
//!
//! // 初始化 Sanctum（基于全局默认 Cache）
//! let sanctum = Sanctum::with_default_cache(SanctumConfig::default());
//!
//! // 签发 token
//! let plain = sanctum.create_token(1, "web", vec!["read".into()], None).unwrap();
//! // 返回明文 token 给客户端：`1|a1b2c3...`
//!
//! // 校验（中间件内部行为，调用方一般无需直接调用）
//! let user = sanctum.validate(&plain).unwrap();
//! assert_eq!(user.user_id, 1);
//! assert!(user.token_can("read"));
//!
//! // 路由
//! let app = Router::new()
//!     .route("/me", get(me_handler))
//!     .layer(middleware::from_fn_with_state(sanctum, sanctum_middleware));
//! ```
//!
//! ## 错误码对齐
//!
//! | 场景 | ErrorCode | HTTP Status |
//! |------|-----------|-------------|
//! | 缺少 Authorization header + 非白名单 | `NotLogin` | 401 |
//! | Token 格式非法 | `NotLogin` | 401 |
//! | Token 已撤销/不存在 | `NotLogin` | 401 |
//! | Token 已过期 | `NotLogin` | 401 |
//! | 白名单路由 | — | 放行 |

use crate::auth::{base_exception_to_response, extract_route_uri, is_route_allowed};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::IntoResponse;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use sz_rust_cache_facade::{Cache, MemoryCacheDriver};
use sz_rust_http_facade::BaseException;

/// Sanctum 默认 cache key 前缀
pub const DEFAULT_KEY_PREFIX: &str = "sanctum:token:";
/// Sanctum 默认 token id 计数器 key
pub const DEFAULT_ID_COUNTER_KEY: &str = "sanctum:id_counter";
/// Sanctum 默认用户 token 索引 key 前缀（`sanctum:user:<user_id>:tokens` → `Vec<i64\>`）
pub const DEFAULT_USER_INDEX_PREFIX: &str = "sanctum:user:";
/// 默认 token TTL：30 天（对齐 PHP JWT 默认有效期）
pub const DEFAULT_TOKEN_TTL_SECS: u64 = 3600 * 24 * 30;
/// Token 明文分隔符（对齐 Laravel Sanctum `|`）
pub const TOKEN_DELIMITER: &str = "|";
/// 随机部分字节数（32 字节 → 64 hex 字符）
pub const RANDOM_BYTES_LEN: usize = 32;

/// Sanctum 配置
#[derive(Debug, Clone)]
pub struct SanctumConfig {
    /// Cache key 前缀（存储 token hash → PersonalAccessToken 映射）
    pub key_prefix: String,
    /// Token id 自增计数器 cache key
    pub id_counter_key: String,
    /// 用户 token 索引 key 前缀
    pub user_index_prefix: String,
    /// Token 默认 TTL（None = 永不过期）
    pub default_ttl: Option<Duration>,
    /// 白名单路由（对齐 `auth::AuthConfig::allow_all_action`）
    pub allow_all_action: Vec<String>,
}

impl Default for SanctumConfig {
    fn default() -> Self {
        Self {
            key_prefix: DEFAULT_KEY_PREFIX.to_string(),
            id_counter_key: DEFAULT_ID_COUNTER_KEY.to_string(),
            user_index_prefix: DEFAULT_USER_INDEX_PREFIX.to_string(),
            default_ttl: Some(Duration::from_secs(DEFAULT_TOKEN_TTL_SECS)),
            allow_all_action: Vec::new(),
        }
    }
}

impl SanctumConfig {
    /// 设置白名单路由
    pub fn with_allow_all_action(mut self, allow: Vec<String>) -> Self {
        self.allow_all_action = allow;
        self
    }

    /// 设置默认 TTL
    pub fn with_default_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.default_ttl = ttl;
        self
    }

    /// 设置 cache key 前缀
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }
}

/// Token 权限能力
pub type Ability = String;

/// 个人访问令牌实体（对齐 Laravel `personal_access_tokens` 表）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersonalAccessToken {
    /// Token ID（主键，自增）
    pub id: i64,
    /// 用户 ID
    pub user_id: i64,
    /// Token 名称（如 "web"、"mobile"）
    pub name: String,
    /// Token 权限能力列表（如 `["read", "write"]`，`["*"]` 表示全部权限）
    pub abilities: Vec<Ability>,
    /// 创建时间戳（Unix 秒）
    pub created_at: i64,
    /// 最后使用时间戳（Unix 秒）
    pub last_used_at: Option<i64>,
    /// 过期时间戳（Unix 秒，None = 永不过期）
    pub expires_at: Option<i64>,
}

impl PersonalAccessToken {
    /// 检查 token 是否拥有指定权限
    ///
    /// 对齐 Laravel `$token->can($ability)`：
    /// - `["*"]` 通配符表示拥有全部权限
    /// - 空列表表示无任何权限
    pub fn can(&self, ability: &str) -> bool {
        self.abilities.iter().any(|a| a == "*" || a == ability)
    }

    /// 检查 token 是否无指定权限
    pub fn cannot(&self, ability: &str) -> bool {
        !self.can(ability)
    }
}

/// Sanctum 已认证用户（插入 request extensions，供 handler 使用）
#[derive(Debug, Clone)]
pub struct SanctumUser {
    /// 用户 ID
    pub user_id: i64,
    /// Token ID
    pub token_id: i64,
    /// Token 名称
    pub token_name: String,
    /// Token 权限能力
    pub abilities: Vec<Ability>,
}

impl SanctumUser {
    /// 检查 token 是否拥有指定权限（对齐 Laravel `$user->tokenCan($ability)`）
    pub fn token_can(&self, ability: &str) -> bool {
        self.abilities.iter().any(|a| a == "*" || a == ability)
    }

    /// 检查 token 是否无指定权限
    pub fn token_cannot(&self, ability: &str) -> bool {
        !self.token_can(ability)
    }
}

/// Sanctum 错误类型
#[derive(Debug, thiserror::Error)]
pub enum SanctumError {
    /// Cache 操作失败
    #[error("cache error: {0}")]
    Cache(String),
    /// Token 格式非法
    #[error("invalid token format")]
    InvalidFormat,
    /// Token 不存在或已撤销
    #[error("token not found or revoked")]
    NotFound,
    /// Token 已过期
    #[error("token expired")]
    Expired,
    /// 随机数生成失败
    #[error("rng error: {0}")]
    Rng(String),
}

/// Sanctum Token 管理器
///
/// 基于 [`Cache`] 层存储的轻量级 token 管理，对齐 Laravel Sanctum 核心能力。
/// 内部使用 `parking_lot::Mutex` 保护 token id 自增和用户索引更新。
#[derive(Clone)]
pub struct Sanctum {
    cache: Arc<Cache>,
    config: SanctumConfig,
    /// 互斥锁：保护 token id 自增和用户索引更新的原子性
    ///
    /// 注：Cache::inc 本身是原子的，但「create_token + 更新用户索引」需要跨键原子性，
    /// 故加锁。锁粒度仅限签发/撤销操作，validate 不加锁。
    lock: Arc<Mutex<()>>,
}

impl Sanctum {
    /// 使用指定 Cache 实例和配置创建 Sanctum
    pub fn new(cache: Arc<Cache>, config: SanctumConfig) -> Self {
        Self {
            cache,
            config,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// 使用独立的内存 Cache 创建 Sanctum
    ///
    /// 注：此方法创建独立的 Cache 实例（不共享全局 default_cache）。
    /// 如需共享全局 cache，请使用 [Sanctum::new] 并传入 Arc::new(default_cache().clone())。
    /// 但因 Cache 未实现 Clone，推荐做法是直接使用此方法或 [Sanctum::new] 传入自定义 cache。
    pub fn with_default_cache(config: SanctumConfig) -> Self {
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());
        Self::new(cache, config)
    }

    /// 获取配置引用
    pub fn config(&self) -> &SanctumConfig {
        &self.config
    }

    /// 签发新的个人访问令牌
    ///
    /// 对齐 Laravel `$user->createToken('name', $abilities)`。
    ///
    /// # 参数
    ///
    /// - `user_id`：用户 ID
    /// - `name`：Token 名称（如 "web"、"mobile"）
    /// - `abilities`：权限能力列表（`["*"]` 表示全部权限）
    /// - `ttl`：可选 TTL，None 使用配置默认值
    ///
    /// # 返回
    ///
    /// 返回明文 token 字符串（格式 `<id>|<random_hex>`），仅此一次返回。
    /// 客户端应妥善保存，后续请求通过 `Authorization: Bearer <plain_token>` 携带。
    ///
    /// # 错误
    ///
    /// - [`SanctumError::Cache`]：cache 写入失败
    /// - [`SanctumError::Rng`]：随机数生成失败
    pub fn create_token(
        &self,
        user_id: i64,
        name: impl Into<String>,
        abilities: Vec<Ability>,
        ttl: Option<Duration>,
    ) -> Result<String, SanctumError> {
        let _guard = self.lock.lock();

        // 1. 生成 token id（自增）
        let token_id = self
            .cache
            .inc(&self.config.id_counter_key, 1)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;
        // 注：inc 返回自增后的值，首次调用从 0 自增到 1

        // 2. 生成随机部分（32 字节 → 64 hex）
        let random_hex = generate_random_hex().map_err(SanctumError::Rng)?;

        // 3. 拼接明文 token：`<id>|<random_hex>`
        let plain_token = format!("{}{}{}", token_id, TOKEN_DELIMITER, random_hex);

        // 4. 计算 hash（sha256）作为 cache key
        let token_hash = hash_token(&plain_token);
        let cache_key = format!("{}{}", self.config.key_prefix, token_hash);

        // 5. 构造 PersonalAccessToken 实体
        let now = chrono::Utc::now().timestamp();
        let effective_ttl = ttl.or(self.config.default_ttl);
        let expires_at = effective_ttl.map(|d| now + d.as_secs() as i64);
        let token = PersonalAccessToken {
            id: token_id,
            user_id,
            name: name.into(),
            abilities,
            created_at: now,
            last_used_at: None,
            expires_at,
        };

        // 6. 写入 cache
        self.cache
            .set(&cache_key, &token, None) // cache 不自动过期，由 token.expires_at 控制
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        // 7. 更新用户 token 索引（sanctum:user:<user_id>:tokens → Vec<i64>）
        let user_index_key = format!("{}{}:tokens", self.config.user_index_prefix, user_id);
        let mut user_tokens: Vec<i64> = self
            .cache
            .get(&user_index_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?
            .unwrap_or_default();
        user_tokens.push(token_id);
        self.cache
            .set(&user_index_key, &user_tokens, None)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        // 8. 同时建立 token_id → token_hash 的反向索引，便于按 id 撤销
        let id_to_hash_key = format!("{}id:{}:hash", self.config.key_prefix, token_id);
        self.cache
            .set(&id_to_hash_key, &token_hash, None) // 反向索引同样不自动过期
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        Ok(plain_token)
    }

    /// 校验明文 token 并返回 [`SanctumUser`]
    ///
    /// 对齐 Laravel `$request->user()` + `EnsureFrontendRequestsAreStateful`。
    ///
    /// # 流程
    /// 1. 解析 token 格式（`<id>|<random_hex>`）
    /// 2. 计算 hash 并查询 cache
    /// 3. 检查 expires_at（若已过期返回 `Expired`）
    /// 4. 更新 `last_used_at`（best-effort，失败忽略）
    ///
    /// # 错误
    /// - [`SanctumError::InvalidFormat`]：token 格式非法
    /// - [`SanctumError::NotFound`]：token 不存在或已撤销
    /// - [`SanctumError::Expired`]：token 已过期
    pub fn validate(&self, plain_token: &str) -> Result<SanctumUser, SanctumError> {
        // 1. 解析格式
        let (_id, _random) = parse_token(plain_token)?;

        // 2. 计算 hash 并查询
        let token_hash = hash_token(plain_token);
        let cache_key = format!("{}{}", self.config.key_prefix, token_hash);
        let mut token: PersonalAccessToken = self
            .cache
            .get(&cache_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?
            .ok_or(SanctumError::NotFound)?;

        // 3. 检查过期
        let now = chrono::Utc::now().timestamp();
        if let Some(exp) = token.expires_at {
            if now >= exp {
                return Err(SanctumError::Expired);
            }
        }

        // 4. 更新 last_used_at（best-effort）
        token.last_used_at = Some(now);
        let _ = self.cache.set(&cache_key, &token, None); // cache 不自动过期，由 token.expires_at 控制

        Ok(SanctumUser {
            user_id: token.user_id,
            token_id: token.id,
            token_name: token.name,
            abilities: token.abilities,
        })
    }

    /// 撤销指定明文 token
    ///
    /// 对齐 Laravel `$token->revoke()`。
    pub fn revoke(&self, plain_token: &str) -> Result<bool, SanctumError> {
        let _guard = self.lock.lock();

        let token_hash = hash_token(plain_token);
        let cache_key = format!("{}{}", self.config.key_prefix, token_hash);

        // 先查询以获取 token_id（用于清理反向索引）
        let token: Option<PersonalAccessToken> = self
            .cache
            .get(&cache_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        let existed = token.is_some();
        if let Some(t) = token {
            let id_to_hash_key = format!("{}id:{}:hash", self.config.key_prefix, t.id);
            let _ = self.cache.delete(&id_to_hash_key);
            // 从用户索引中移除
            self.remove_token_from_user_index(t.user_id, t.id)?;
        }

        self.cache
            .delete(&cache_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        Ok(existed)
    }

    /// 按 token_id 撤销（无需明文 token）
    ///
    /// 适用于管理员强制撤销场景。
    pub fn revoke_by_id(&self, token_id: i64) -> Result<bool, SanctumError> {
        let _guard = self.lock.lock();

        let id_to_hash_key = format!("{}id:{}:hash", self.config.key_prefix, token_id);
        let token_hash: Option<String> = self
            .cache
            .get(&id_to_hash_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        let Some(token_hash) = token_hash else {
            return Ok(false);
        };

        let cache_key = format!("{}{}", self.config.key_prefix, token_hash);
        let token: Option<PersonalAccessToken> = self
            .cache
            .get(&cache_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        let existed = token.is_some();
        if let Some(t) = token {
            self.remove_token_from_user_index(t.user_id, t.id)?;
        }

        let _ = self.cache.delete(&id_to_hash_key);
        self.cache
            .delete(&cache_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        Ok(existed)
    }

    /// 撤销指定用户的所有 token
    ///
    /// 对齐 Laravel `$user->tokens()->delete()`。
    pub fn revoke_all_for_user(&self, user_id: i64) -> Result<usize, SanctumError> {
        let _guard = self.lock.lock();

        let user_index_key = format!("{}{}:tokens", self.config.user_index_prefix, user_id);
        let token_ids: Vec<i64> = self
            .cache
            .get(&user_index_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?
            .unwrap_or_default();

        let mut revoked = 0usize;
        for token_id in &token_ids {
            // 复用 revoke_by_id 的核心逻辑（不重新加锁，已持有 _guard）
            let id_to_hash_key = format!("{}id:{}:hash", self.config.key_prefix, token_id);
            let token_hash: Option<String> = self
                .cache
                .get(&id_to_hash_key)
                .map_err(|e| SanctumError::Cache(e.to_string()))?;
            if let Some(hash) = token_hash {
                let cache_key = format!("{}{}", self.config.key_prefix, hash);
                let _ = self.cache.delete(&id_to_hash_key);
                let _ = self.cache.delete(&cache_key);
                revoked += 1;
            }
        }

        // 清空用户索引
        self.cache
            .delete(&user_index_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;

        Ok(revoked)
    }

    /// 获取指定用户的所有 token（不包含明文，仅元数据）
    pub fn tokens_for_user(&self, user_id: i64) -> Result<Vec<PersonalAccessToken>, SanctumError> {
        let user_index_key = format!("{}{}:tokens", self.config.user_index_prefix, user_id);
        let token_ids: Vec<i64> = self
            .cache
            .get(&user_index_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?
            .unwrap_or_default();

        let mut tokens = Vec::with_capacity(token_ids.len());
        for token_id in &token_ids {
            let id_to_hash_key = format!("{}id:{}:hash", self.config.key_prefix, token_id);
            let token_hash: Option<String> = self
                .cache
                .get(&id_to_hash_key)
                .map_err(|e| SanctumError::Cache(e.to_string()))?;
            if let Some(hash) = token_hash {
                let cache_key = format!("{}{}", self.config.key_prefix, hash);
                if let Some(token) = self
                    .cache
                    .get::<PersonalAccessToken>(&cache_key)
                    .map_err(|e| SanctumError::Cache(e.to_string()))?
                {
                    tokens.push(token);
                }
            }
        }
        Ok(tokens)
    }

    /// 从用户索引中移除指定 token_id
    fn remove_token_from_user_index(
        &self,
        user_id: i64,
        token_id: i64,
    ) -> Result<(), SanctumError> {
        let user_index_key = format!("{}{}:tokens", self.config.user_index_prefix, user_id);
        let mut user_tokens: Vec<i64> = self
            .cache
            .get(&user_index_key)
            .map_err(|e| SanctumError::Cache(e.to_string()))?
            .unwrap_or_default();
        user_tokens.retain(|id| *id != token_id);
        self.cache
            .set(&user_index_key, &user_tokens, None)
            .map_err(|e| SanctumError::Cache(e.to_string()))?;
        Ok(())
    }
}

impl std::fmt::Debug for Sanctum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sanctum")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 计算 token 的 sha256 hash（hex 编码）
fn hash_token(plain_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plain_token.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result)
}

/// 解析 token 格式：`<id>|<random_hex>`
///
/// 返回 `(id, random_hex)`，格式非法返回 `InvalidFormat`。
fn parse_token(plain_token: &str) -> Result<(i64, &str), SanctumError> {
    let trimmed = plain_token.trim();
    let Some((id_str, random)) = trimmed.split_once(TOKEN_DELIMITER) else {
        return Err(SanctumError::InvalidFormat);
    };
    let id: i64 = id_str.parse().map_err(|_| SanctumError::InvalidFormat)?;
    if random.is_empty() {
        return Err(SanctumError::InvalidFormat);
    }
    Ok((id, random))
}

/// 生成 32 字节随机数并 hex 编码（64 字符）
///
/// 安全约束：使用 `rand::rngs::OsRng`（操作系统级密码学安全 RNG），
/// 替代之前的 xorshift64+（非密码学安全，种子可预测）。
/// 对齐 PHP `bin2hex(random_bytes(32))` 的安全级别。
fn generate_random_hex() -> Result<String, String> {
    use rand::RngCore;
    let mut bytes = [0u8; RANDOM_BYTES_LEN];
    // OsRng 是操作系统级密码学安全随机数生成器
    // 在 Windows 上使用 BCryptGenRandom，Linux 上使用 getrandom()，macOS 上使用 arc4random_buf
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    Ok(hex_encode(&bytes))
}

/// hex 编码（小写）
fn hex_encode(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    s
}

// ============================================================================
// axum 中间件
// ============================================================================

/// Sanctum 认证中间件
///
/// 对齐 Laravel `EnsureFrontendRequestsAreStateful` + `auth:sanctum`。
///
/// # 流程
/// 1. 路由白名单检查（与 `auth_middleware` 一致）
/// 2. 取 `Authorization` header，提取 bearer token
/// 3. 调用 [`Sanctum::validate`] 校验
/// 4. 注入 [`SanctumUser`] 到 request extensions
///
/// # 用法
///
/// ```ignore
/// use sz_rust_core::middleware::sanctum::{Sanctum, SanctumConfig, sanctum_middleware};
/// use axum::middleware;
///
/// let sanctum = Sanctum::with_default_cache(SanctumConfig::default());
/// let app = Router::new()
///     .route("/me", get(me_handler))
///     .layer(middleware::from_fn_with_state(sanctum, sanctum_middleware));
/// ```
pub async fn sanctum_middleware(
    State(sanctum): State<Sanctum>,
    req: Request,
    next: Next,
) -> axum::response::Response {
    let config = sanctum.config();

    // 1. 路由白名单检查
    let route_uri = extract_route_uri(&req);
    if is_route_allowed(&route_uri, &config.allow_all_action) {
        return next.run(req).await.into_response();
    }

    // 2. 取 Authorization header 并提取 token
    let auth_header = req.headers().get(axum::http::header::AUTHORIZATION);
    let token = match auth_header {
        Some(value) => {
            let raw = value.to_str().unwrap_or("");
            crate::auth::extract_token_from_header(raw)
        }
        None => None,
    };

    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return base_exception_to_response(BaseException::not_login(
                "缺少必要的参数,请重新登陆!",
            ));
        }
    };

    // 3. Sanctum 校验
    match sanctum.validate(&token) {
        Ok(user) => {
            let mut req = req;
            req.extensions_mut().insert(user);
            next.run(req).await.into_response()
        }
        Err(SanctumError::Expired) => {
            base_exception_to_response(BaseException::not_login("token 已过期,请重新登陆!"))
        }
        Err(_) => {
            base_exception_to_response(BaseException::not_login("缺少必要的参数,请重新登陆!"))
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_cache_facade::{Cache, MemoryCacheDriver};

    /// 构造独立 Cache 的 Sanctum 实例（避免测试间状态污染）
    fn make_sanctum() -> Sanctum {
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());
        Sanctum::new(cache, SanctumConfig::default())
    }

    #[test]
    fn test_create_and_validate_token() {
        let sanctum = make_sanctum();
        let plain = sanctum
            .create_token(1, "web", vec!["read".into(), "write".into()], None)
            .expect("签发 token 应成功");

        // 格式校验
        assert!(plain.contains(TOKEN_DELIMITER));

        // 校验 token
        let user = sanctum.validate(&plain).expect("校验应成功");
        assert_eq!(user.user_id, 1);
        assert_eq!(user.token_name, "web");
        assert!(user.token_can("read"));
        assert!(user.token_can("write"));
        assert!(!user.token_can("delete"));
    }

    #[test]
    fn test_wildcard_ability() {
        let sanctum = make_sanctum();
        let plain = sanctum
            .create_token(2, "admin", vec!["*".into()], None)
            .expect("签发 token 应成功");

        let user = sanctum.validate(&plain).expect("校验应成功");
        assert!(user.token_can("read"));
        assert!(user.token_can("write"));
        assert!(user.token_can("anything"));
    }

    #[test]
    fn test_revoke_token() {
        let sanctum = make_sanctum();
        let plain = sanctum
            .create_token(3, "web", vec!["read".into()], None)
            .expect("签发 token 应成功");

        // 校验成功
        assert!(sanctum.validate(&plain).is_ok());

        // 撤销
        let revoked = sanctum.revoke(&plain).expect("撤销应成功");
        assert!(revoked);

        // 撤销后校验失败
        assert!(matches!(
            sanctum.validate(&plain),
            Err(SanctumError::NotFound)
        ));

        // 再次撤销返回 false
        let revoked_again = sanctum.revoke(&plain).expect("撤销应成功");
        assert!(!revoked_again);
    }

    #[test]
    fn test_revoke_by_id() {
        let sanctum = make_sanctum();
        let plain = sanctum
            .create_token(4, "mobile", vec!["read".into()], None)
            .expect("签发 token 应成功");
        let user = sanctum.validate(&plain).expect("校验应成功");
        let token_id = user.token_id;

        // 按 id 撤销
        let revoked = sanctum.revoke_by_id(token_id).expect("撤销应成功");
        assert!(revoked);

        // 撤销后校验失败
        assert!(matches!(
            sanctum.validate(&plain),
            Err(SanctumError::NotFound)
        ));
    }

    #[test]
    fn test_revoke_all_for_user() {
        let sanctum = make_sanctum();
        // 为用户 5 签发 3 个 token
        let t1 = sanctum
            .create_token(5, "web", vec!["*".into()], None)
            .unwrap();
        let t2 = sanctum
            .create_token(5, "mobile", vec!["*".into()], None)
            .unwrap();
        let t3 = sanctum
            .create_token(5, "tablet", vec!["*".into()], None)
            .unwrap();

        // 为用户 6 签发 1 个 token（不应被撤销）
        let other = sanctum
            .create_token(6, "web", vec!["*".into()], None)
            .unwrap();

        // 撤销用户 5 的所有 token
        let count = sanctum.revoke_all_for_user(5).expect("撤销应成功");
        assert_eq!(count, 3);

        // 用户 5 的 token 全部失效
        assert!(matches!(sanctum.validate(&t1), Err(SanctumError::NotFound)));
        assert!(matches!(sanctum.validate(&t2), Err(SanctumError::NotFound)));
        assert!(matches!(sanctum.validate(&t3), Err(SanctumError::NotFound)));

        // 用户 6 的 token 仍有效
        assert!(sanctum.validate(&other).is_ok());
    }

    #[test]
    fn test_tokens_for_user() {
        let sanctum = make_sanctum();
        sanctum
            .create_token(7, "web", vec!["read".into()], None)
            .unwrap();
        sanctum
            .create_token(7, "mobile", vec!["write".into()], None)
            .unwrap();
        sanctum
            .create_token(8, "web", vec!["*".into()], None)
            .unwrap();

        let tokens = sanctum.tokens_for_user(7).expect("查询应成功");
        assert_eq!(tokens.len(), 2);

        let tokens_other = sanctum.tokens_for_user(8).expect("查询应成功");
        assert_eq!(tokens_other.len(), 1);
    }

    #[test]
    fn test_token_expiry() {
        let sanctum = make_sanctum();
        // 1 秒 TTL
        let plain = sanctum
            .create_token(9, "web", vec!["*".into()], Some(Duration::from_secs(1)))
            .expect("签发应成功");

        // 立即校验：成功
        assert!(sanctum.validate(&plain).is_ok());

        // 等待过期
        std::thread::sleep(Duration::from_millis(1100));

        // 过期后校验：Expired
        assert!(matches!(
            sanctum.validate(&plain),
            Err(SanctumError::Expired)
        ));
    }

    #[test]
    fn test_invalid_token_format() {
        let sanctum = make_sanctum();

        // 缺少分隔符
        assert!(matches!(
            sanctum.validate("invalidtoken"),
            Err(SanctumError::InvalidFormat)
        ));

        // id 非数字
        assert!(matches!(
            sanctum.validate("abc|def"),
            Err(SanctumError::InvalidFormat)
        ));

        // 空随机部分
        assert!(matches!(
            sanctum.validate("1|"),
            Err(SanctumError::InvalidFormat)
        ));
    }

    #[test]
    fn test_personal_access_token_can() {
        let token = PersonalAccessToken {
            id: 1,
            user_id: 1,
            name: "test".into(),
            abilities: vec!["read".into(), "write".into()],
            created_at: 0,
            last_used_at: None,
            expires_at: None,
        };

        assert!(token.can("read"));
        assert!(token.can("write"));
        assert!(!token.can("delete"));
        assert!(token.cannot("delete"));

        // 通配符
        let wildcard = PersonalAccessToken {
            abilities: vec!["*".into()],
            ..token
        };
        assert!(wildcard.can("anything"));
    }

    #[test]
    fn test_parse_token_format() {
        // 合法格式
        let (id, random) = parse_token("123|abcdef").unwrap();
        assert_eq!(id, 123);
        assert_eq!(random, "abcdef");

        // 带空格
        let (id, _) = parse_token("  456|xyz  ").unwrap();
        assert_eq!(id, 456);

        // 非法格式
        assert!(parse_token("no-delimiter").is_err());
        assert!(parse_token("|nonumber").is_err());
        assert!(parse_token("1|").is_err());
    }

    #[test]
    fn test_hash_token_deterministic() {
        let h1 = hash_token("test_token");
        let h2 = hash_token("test_token");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // sha256 = 32 bytes = 64 hex chars

        // 不同输入不同 hash
        let h3 = hash_token("other_token");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_last_used_at_updated() {
        let sanctum = make_sanctum();
        let plain = sanctum
            .create_token(10, "web", vec!["*".into()], None)
            .unwrap();

        // 第一次校验
        let before = chrono::Utc::now().timestamp();
        let _ = sanctum.validate(&plain).unwrap();
        let after = chrono::Utc::now().timestamp();

        // 查询 token，验证 last_used_at 已更新
        let tokens = sanctum.tokens_for_user(10).unwrap();
        assert_eq!(tokens.len(), 1);
        let last_used = tokens[0].last_used_at.expect("last_used_at 应已更新");
        assert!(last_used >= before - 1 && last_used <= after + 1);
    }

    #[test]
    fn test_sanctum_config_builder() {
        let config = SanctumConfig::default()
            .with_allow_all_action(vec!["/login".into()])
            .with_default_ttl(Some(Duration::from_secs(3600)))
            .with_key_prefix("custom:");

        assert_eq!(config.allow_all_action, vec!["/login".to_string()]);
        assert_eq!(config.default_ttl, Some(Duration::from_secs(3600)));
        assert_eq!(config.key_prefix, "custom:");
    }

    #[test]
    fn test_create_multiple_tokens_unique() {
        let sanctum = make_sanctum();
        let t1 = sanctum.create_token(1, "a", vec![], None).unwrap();
        let t2 = sanctum.create_token(1, "b", vec![], None).unwrap();
        let t3 = sanctum.create_token(1, "c", vec![], None).unwrap();

        // 三个 token 互不相同
        assert_ne!(t1, t2);
        assert_ne!(t1, t3);
        assert_ne!(t2, t3);

        // 都能校验通过
        assert!(sanctum.validate(&t1).is_ok());
        assert!(sanctum.validate(&t2).is_ok());
        assert!(sanctum.validate(&t3).is_ok());
    }

    // ========================================================================
    // P0-SEC-07：generate_random_hex 使用密码学安全 RNG（OsRng）
    // ========================================================================

    /// 验证 generate_random_hex 输出格式正确（64 字符十六进制 = 32 字节）
    #[test]
    fn test_generate_random_hex_format() {
        let hex = generate_random_hex().expect("generate_random_hex 不应失败");
        // 32 字节 → 64 hex 字符
        assert_eq!(hex.len(), 64, "random_hex 应为 64 字符（32 字节 * 2）");
        // 全部为十六进制字符（大小写均可，安全上无差异）
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "random_hex 应全为十六进制字符: {hex}"
        );
    }

    /// 验证 generate_random_hex 具有密码学不可预测性（连续调用结果不同）
    ///
    /// 这是 P0-SEC-07 的核心回归测试：旧版 xorshift64+ 在极短时间内
    /// 可能产生相同或可预测的输出，OsRng 应始终产生唯一值。
    #[test]
    fn test_generate_random_hex_uniqueness() {
        // 连续生成 10 个随机值，全部应互不相同
        let values: Vec<String> = (0..10).map(|_| generate_random_hex().unwrap()).collect();

        for (i, v1) in values.iter().enumerate() {
            for v2 in values.iter().skip(i + 1) {
                assert_ne!(
                    v1, v2,
                    "OsRng 生成的随机值出现重复（索引 {i}），可能存在 RNG 安全问题"
                );
            }
        }
    }

    /// 验证 token 随机部分的熵：同一 token_id 下，不同次 create_token 的
    /// 随机部分不应相同（防止令牌可预测/伪造）
    #[test]
    fn test_token_random_part_uniqueness() {
        let sanctum = make_sanctum();

        // 为同一用户创建多个 token，验证随机部分唯一
        let random_parts: Vec<String> = (0..5)
            .map(|_| {
                let plain = sanctum.create_token(42, "api", vec![], None).unwrap();
                let (_, random) = parse_token(&plain).unwrap();
                random.to_string()
            })
            .collect();

        for (i, r1) in random_parts.iter().enumerate() {
            for r2 in random_parts.iter().skip(i + 1) {
                assert_ne!(
                    r1, r2,
                    "同一用户的 token 随机部分出现重复（索引 {i}），攻击者可能伪造令牌"
                );
            }
        }
    }
}
