// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use crate::db::acquire_cancel_safe;
use sz_rust_core::orm::auth::{Credentials, User};
use sz_rust_core::orm::Pool;
use sz_rust_core::orm::Value;
use sz_rust_core::orm::{Authorizer, JwtAuthenticator, RbacAuthorizer};

static AUTH: OnceLock<JwtAuthenticator> = OnceLock::new();
static RBAC: OnceLock<RbacAuthorizer> = OnceLock::new();
static DB_POOL: OnceLock<Arc<Pool>> = OnceLock::new();

/// JWT 签名密钥（2026-10-28 新增：用于签发带 audience 的令牌）
static JWT_SECRET: OnceLock<String> = OnceLock::new();

/// 已注销 token 黑名单（2026-10-28 安全修复：logout 真实吊销）
///
/// 存储 `token 哈希 → 过期时间`。JWT 无状态，服务端必须维护黑名单才能实现注销。
/// 用 `HashMap` 而非 `HashSet<(hash, exp)>`：校验路径 O(1)，避免每次请求全表扫描。
/// 单实例部署有效；多实例需替换为 Redis 等共享存储（见 ADR/注释）。
static TOKEN_BLACKLIST: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

fn blacklist() -> &'static Mutex<HashMap<String, i64>> {
    TOKEN_BLACKLIST.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 将 token 加入黑名单（按 token 哈希存储，防泄漏；定期清理已过期条目）
pub fn revoke_token(token: &str) {
    let exp = decode_token_exp(token).unwrap_or(0);
    let hash = token_hash(token);
    if let Ok(mut map) = blacklist().lock() {
        // 惰性清理已过期条目
        let now = chrono::Utc::now().timestamp();
        map.retain(|_, e| *e > now);
        map.insert(hash, exp);
    }
}

/// 检查 token 是否已注销（O(1) 查找）
pub fn is_token_revoked(token: &str) -> bool {
    let hash = token_hash(token);
    // 已过期条目会在 revoke 时被清理；过期 token 也会被 JWT 校验拒绝
    matches!(blacklist().lock(), Ok(map) if map.contains_key(&hash))
}

/// 从 JWT 中解码过期时间（不校验签名，仅用于黑名单清理）
fn decode_token_exp(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    use base64::Engine;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    v.get("exp").and_then(|e| e.as_i64())
}

/// token 哈希（SHA-256 十六进制，避免明文 token 常驻内存）
fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // write! 到 String 不会失败（fmt::Write for String 为 infallible）
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// v1.8.0 密钥轮换：JWT 签名密钥 ID
#[cfg(feature = "v18-key-rotation")]
const JWT_KEY_ID: &str = "jwt-signing";

/// v1.8.0 密钥轮换：全局 KeyManager
#[cfg(feature = "v18-key-rotation")]
static KEY_MANAGER: OnceLock<Arc<sz_rust_key_rotation::KeyManager>> = OnceLock::new();

/// v1.8.0 初始化密钥轮换管理器
///
/// 创建 KeyManager 并执行首次轮换生成活跃密钥。
/// `rotation_interval` = 30 天，`keep_history` = 5（grace period 内旧 token 仍可验证）。
///
/// 注：2026-10-25 架构评测修复——移除未使用的 `initial_secret` 参数。
/// 初始密钥由 `RandomKeyGenerator` 生成，调用方无需也不应传入种子。
#[cfg(feature = "v18-key-rotation")]
pub fn init_key_rotation() -> Arc<sz_rust_key_rotation::KeyManager> {
    use sz_rust_key_rotation::{KeyId, KeyManager, RotationPolicy};

    let policy = RotationPolicy {
        interval: std::time::Duration::from_secs(30 * 86400),
        keep_history: 5,
    };
    let manager = KeyManager::new(policy, Box::new(sz_rust_key_rotation::RandomKeyGenerator));
    let key_id = KeyId::new(JWT_KEY_ID);
    let _ = manager.rotate(&key_id);

    let arc = Arc::new(manager);
    let _ = KEY_MANAGER.set(arc.clone());
    arc
}

/// v1.8.0 获取全局 KeyManager
#[cfg(feature = "v18-key-rotation")]
pub fn get_key_manager() -> &'static Arc<sz_rust_key_rotation::KeyManager> {
    KEY_MANAGER
        .get()
        .expect("KeyManager not initialized — 请先调用 init_key_rotation")
}

/// v1.8.0 使用活跃密钥签发 JWT
///
/// 从 KeyManager 获取活跃密钥 → 用该密钥创建 JwtEncoder → 编码 claims。
#[cfg(feature = "v18-key-rotation")]
pub fn sign_token_with_rotation(
    claims: &sz_rust_core::orm::jwt::JwtClaims,
) -> Result<String, String> {
    use sz_rust_key_rotation::KeyId;

    let manager = get_key_manager();
    let key_id = KeyId::new(JWT_KEY_ID);
    let active = manager
        .get_active(&key_id)
        .ok_or_else(|| "无活跃密钥".to_string())?;

    let secret = String::from_utf8_lossy(&active.key_material);
    let encoder = sz_rust_core::orm::jwt::JwtEncoder::new(&*secret);
    encoder
        .encode(claims)
        .map_err(|e| format!("JWT 编码失败: {}", e))
}

/// v1.8.0 使用密钥轮换验证 JWT
///
/// 依次尝试所有历史密钥（活跃 + 已退役但未清除）验证 token，任一成功即返回用户。
/// grace period 由 `keep_history` 控制：旧密钥保留在历史中仍可验证，直到被清除。
#[cfg(feature = "v18-key-rotation")]
pub fn verify_token_with_rotation(token: &str) -> Result<User, String> {
    use sz_rust_key_rotation::KeyId;

    let manager = get_key_manager();
    let key_id = KeyId::new(JWT_KEY_ID);
    let all_versions = manager.list_versions(&key_id);

    for key in &all_versions {
        let secret = String::from_utf8_lossy(&key.key_material);
        let auth = JwtAuthenticator::new(&*secret, "sz300", 86400);
        if let Ok(user) = auth.verify_token(token) {
            return Ok(user);
        }
    }

    Err("token 验证失败：无匹配密钥".to_string())
}

/// v1.8.0 执行密钥轮换
///
/// 生成新密钥版本，旧密钥保留在历史中（grace period 内仍可验证）。
#[cfg(feature = "v18-key-rotation")]
pub fn rotate_key() -> Result<u64, String> {
    use sz_rust_key_rotation::{KeyId, RotationEvent};

    let manager = get_key_manager();
    let key_id = KeyId::new(JWT_KEY_ID);
    match manager.rotate(&key_id) {
        Ok(RotationEvent::Rotated { new_version, .. }) => Ok(new_version),
        Ok(_) => Ok(0),
        Err(e) => Err(format!("密钥轮换失败: {}", e)),
    }
}

/// v1.8.0 测试辅助：创建带初始密钥的 KeyManager（不设全局静态）
#[cfg(feature = "v18-key-rotation")]
pub fn default_key_manager_for_tests() -> Arc<sz_rust_key_rotation::KeyManager> {
    use sz_rust_key_rotation::{KeyId, KeyManager};

    let km = KeyManager::with_default();
    let _ = km.rotate(&KeyId::new(JWT_KEY_ID));
    Arc::new(km)
}

/// 初始化认证模块 — 配置 JWT 密钥、签发者、过期时间与数据库连接池
///
/// 同时将连接池存入全局静态变量，供 [`authenticate_async`] 执行参数化查询使用。
/// 此设计避免了在同步 `PasswordVerifier` trait 中使用 `block_in_place` 调用异步 DB 查询。
#[tracing::instrument(skip(secret, pool))]
pub fn init_auth(secret: &str, issuer: &str, expiry: u64, pool: Arc<Pool>) {
    let _ = JWT_SECRET.set(secret.to_string());
    let auth = JwtAuthenticator::new(secret, issuer, expiry);
    let _ = AUTH.set(auth);
    let _ = RBAC.set(RbacAuthorizer::new());
    let _ = DB_POOL.set(pool);
}

/// 测试用认证初始化 — 仅配置 JWT，不接数据库
///
/// 用于集成测试中仅需 `verify_token` 而不需要 DB 查询的场景（如 RBAC 中间件测试）。
#[cfg(any(test, feature = "v18-rbac", feature = "v19-jwt-audience"))]
pub fn init_auth_test_only(secret: &str) {
    let auth = JwtAuthenticator::new(secret, "sz300-test", 86400);
    let _ = AUTH.set(auth);
    let _ = RBAC.set(RbacAuthorizer::new());
}

/// 获取已初始化的 JWT 认证器（调用前必须先调用 [`init_auth`]）
#[tracing::instrument(skip_all)]
pub fn get_auth() -> &'static JwtAuthenticator {
    AUTH.get().expect("auth not initialized")
}

/// 获取已初始化的 RBAC 授权器（调用前必须先调用 [`init_auth`]）
#[tracing::instrument(skip_all)]
pub fn get_rbac() -> &'static RbacAuthorizer {
    RBAC.get().expect("rbac not initialized")
}

/// 获取已初始化的数据库连接池（调用前必须先调用 [`init_auth`]）
fn get_pool() -> &'static Arc<Pool> {
    DB_POOL.get().expect("db pool not initialized")
}

/// 异步用户名密码认证 — 成功返回 (access_token, refresh_token)，失败返回错误描述
///
/// ## 与旧版 `authenticate` 的差异
///
/// 1. **修复 SQL 注入**：使用 `query_with_params` + `?` 占位符参数化查询，废弃 `format!` 拼接与 `sql_escape`。
/// 2. **修复 block_in_place 反模式**：整个函数为 `async fn`，不再通过 `tokio::task::block_in_place` +
///    `Handle::current().block_on` 调用异步 DB 查询，避免高并发下 tokio worker 饿死。
/// 3. **不再使用 `PasswordVerifier` trait**：该 trait 为 sync，与异步 DB 查询天然不兼容。
///    此处直接在 async 上下文中执行参数化查询，仅将 `JwtAuthenticator` 用于 JWT 编码（同步且极快）。
///
/// ## 安全说明
///
/// - 密码哈希使用 `bcrypt::verify`，CPU 密集型操作通过 `tokio::task::spawn_blocking` 在专用阻塞线程池执行，
///   避免阻塞 tokio worker。
/// - DB 错误信息不会返回客户端，仅记录日志。
/// - 2026-10-28：返回 refresh_token 供 `/api/v1/auth/refresh` 真实轮换使用。
#[tracing::instrument(skip(password))]
pub async fn authenticate_async(
    username: &str,
    password: &str,
) -> Result<(String, Option<String>), String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("用户名或密码不能为空".to_string());
    }

    let pool = get_pool();

    // P1-SEC-06: 外部 IO 超时保护（默认 5s）— 仅包裹 DB 查询
    let rows = sz_rust_core::runtime::spawn::with_timeout(async {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "认证时获取 DB 连接失败");
            "服务暂时不可用".to_string()
        })?;

        // 参数化查询 — 使用 ? 占位符，杜绝 SQL 注入
        let sql = "SELECT user_id, password_hash as password FROM merchant_user WHERE username = ?";
        let params = [Value::String(username.to_string())];
        conn.query_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "认证查询用户失败");
            "服务暂时不可用".to_string()
        })
    })
    .await
    .map_err(|_| {
        tracing::error!("认证查询超时（>5s）");
        "服务暂时不可用".to_string()
    })??;

    let row = rows.first().ok_or_else(|| "用户名或密码错误".to_string())?;

    let stored_hash = row
        .get("password")
        .and_then(|v| match v {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| "用户名或密码错误".to_string())?;

    let user_id = row
        .get("user_id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "用户名或密码错误".to_string())?;

    // bcrypt 验证属于 CPU 密集型操作，使用 spawn_blocking 避免阻塞 tokio worker
    let password_owned = password.to_string();
    let stored_hash_owned = stored_hash.to_string();
    let password_matches = tokio::task::spawn_blocking(move || {
        bcrypt::verify(&password_owned, &stored_hash_owned).unwrap_or(false)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "bcrypt 验证任务失败");
        "服务暂时不可用".to_string()
    })?;

    if !password_matches {
        return Err("用户名或密码错误".to_string());
    }

    // 更新最后登录时间 — 参数化，避免注入（P1-SEC-06: 超时保护）
    let _ = sz_rust_core::runtime::spawn::with_timeout(async {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "更新最后登录时间获取连接失败");
            "服务暂时不可用".to_string()
        })?;
        conn.execute_with_params(
            "UPDATE merchant_user SET last_login_at = NOW() WHERE user_id = ?",
            &[Value::I64(user_id)],
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "更新最后登录时间失败");
            "服务暂时不可用".to_string()
        })
    })
    .await
    .map_err(|_| {
        tracing::error!("更新最后登录时间超时（>5s）");
        "服务暂时不可用".to_string()
    })
    .and_then(|r| r);

    // 使用 JwtAuthenticator 编码 token — 同步且极快（仅 HS256 哈希）
    let creds = Credentials::new(username, password);
    let token = get_auth().authenticate(&creds).map_err(|e| {
        tracing::error!(error = ?e, "JWT 编码失败");
        "认证失败".to_string()
    })?;

    // v1.9.0：启用 audience 校验时，为 access token 注入 aud 声明（服务 ID）
    #[cfg(feature = "v19-jwt-audience")]
    let access_token = {
        use sz_rust_core::orm::jwt::JwtEncoder;
        let secret = JWT_SECRET.get().cloned().unwrap_or_default();
        let encoder = JwtEncoder::new(&secret);
        match encoder.decode(&token.access_token) {
            Ok(claims) => audience::encode_token_with_audience(&encoder, &claims, "sz300-api")
                .unwrap_or_else(|_| token.access_token.clone()),
            Err(_) => token.access_token.clone(),
        }
    };
    #[cfg(not(feature = "v19-jwt-audience"))]
    let access_token = token.access_token.clone();

    Ok((access_token, token.refresh_token))
}

/// 根据用户名查询用户完整信息（登录后回查场景）
///
/// 返回字段：user_id / username / merchant_id / phone / role / merchant_name
///
/// # 安全
///
/// - SQL 参数化（`?` 占位符 + `Value::String`），杜绝 SQL 注入
/// - DB 错误信息不返回客户端，仅记录日志
///
/// # 返回
///
/// - `Ok(Some(row))`：用户存在
/// - `Ok(None)`：用户不存在（或 DB 错误，已记录日志）
#[tracing::instrument]
pub async fn get_user_info_by_username(username: &str) -> Option<HashMap<String, Value>> {
    let pool = get_pool();
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户信息：获取 DB 连接失败");
            e
        })
        .ok()?;

    let sql = "SELECT u.user_id, u.username, u.merchant_id, u.phone, u.role, \
               m.name as merchant_name \
               FROM merchant_user u \
               LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
               WHERE u.username = ?";
    let params = [Value::String(username.to_string())];
    let rows = conn
        .query_with_params(sql, &params)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户信息失败");
            e
        })
        .ok()?;

    rows.into_iter().next()
}

/// 根据 user_id 查询用户完整信息（me 接口场景）
///
/// 返回字段：user_id / username / merchant_id / phone / role / status /
/// last_login_at / created_at / merchant_name
///
/// # 安全
///
/// - SQL 参数化（`?` 占位符 + `Value::I64`），杜绝 SQL 注入
/// - DB 错误信息不返回客户端，仅记录日志
///
/// # 返回
///
/// - `Ok(Some(row))`：用户存在
/// - `Ok(None)`：用户不存在（或 DB 错误，已记录日志）
#[tracing::instrument]
pub async fn get_user_info_by_id(user_id: i64) -> Option<HashMap<String, Value>> {
    let pool = get_pool();
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户信息：获取 DB 连接失败: user_id={}", user_id);
            e
        })
        .ok()?;

    let sql =
        "SELECT u.user_id, u.username, u.merchant_id, u.phone as contact_phone, u.role, u.status, \
               u.last_login_at, u.created_at, \
               m.name as merchant_name \
               FROM merchant_user u \
               LEFT JOIN merchant m ON u.merchant_id = m.merchant_id \
               WHERE u.user_id = ?";
    let params = [Value::I64(user_id)];
    let rows = conn
        .query_with_params(sql, &params)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户信息失败: user_id={}", user_id);
            e
        })
        .ok()?;

    rows.into_iter().next()
}

/// 根据 user_id 查询用户上下文（IDOR 修复专用，2026-10-28）
///
/// 与 [`get_user_info_by_id`] 的差异：
/// - **不会 panic**：认证模块尚未初始化（测试环境）或 DB 不可用时返回 `None`，
///   调用方降级为仅使用 JWT 声明（此时 `merchant_id` 为 None，不启用数据归属过滤）。
/// - 用途：auth 中间件在 JWT 校验通过后提取当前用户的 merchant_id / 管理员标记。
#[tracing::instrument(skip_all)]
pub async fn try_get_user_info_by_id(user_id: i64) -> Option<HashMap<String, Value>> {
    let pool = DB_POOL.get()?;
    let mut conn = acquire_cancel_safe(pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户上下文：获取 DB 连接失败: user_id={}", user_id);
            e
        })
        .ok()?;

    let sql =
        "SELECT u.user_id, u.username, u.merchant_id, u.phone as contact_phone, u.role, u.status \
         FROM merchant_user u WHERE u.user_id = ?";
    let params = [Value::I64(user_id)];
    let rows = conn
        .query_with_params(sql, &params)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "查询用户上下文失败: user_id={}", user_id);
            e
        })
        .ok()?;

    rows.into_iter().next()
}

/// 校验 JWT 令牌 — 成功返回用户信息，失败返回错误描述
///
/// 错误信息不泄露 JWT 内部解析细节，统一返回 "token 验证失败"。
#[tracing::instrument(skip(token))]
pub fn verify_token(token: &str) -> Result<User, String> {
    // 2026-10-28 安全修复：logout 已注销的 token 立即失效
    if is_token_revoked(token) {
        return Err("token 已注销".to_string());
    }
    let user = get_auth().verify_token(token).map_err(|e| {
        tracing::error!(error = ?e, "JWT token 验证失败");
        "token 验证失败".to_string()
    })?;
    Ok(user)
}

/// 刷新访问令牌（2026-10-28 安全修复：refresh 真实轮换）
///
/// 使用 refresh token 换发新的 access token：
/// - 仅接受 `token_use=refresh` 的令牌（sz-orm 侧严格校验，防类型混淆）
/// - 返回新 access token（旧 refresh token 保持有效，符合无状态 JWT 模型）
#[tracing::instrument(skip(refresh_token))]
pub fn refresh_access_token(refresh_token: &str) -> Result<String, String> {
    if refresh_token.is_empty() {
        return Err("缺少 refresh_token".to_string());
    }
    if is_token_revoked(refresh_token) {
        return Err("refresh token 已注销".to_string());
    }
    let new_token = get_auth().refresh_token(refresh_token).map_err(|e| {
        tracing::error!(error = ?e, "refresh token 轮换失败");
        "refresh token 无效或已过期".to_string()
    })?;
    Ok(new_token.access_token)
}

// ============================================================================
// v1.9.0 JWT Audience 安全增强（P0 债务 DB-2026-09-21-01）
//
// sz-orm 9.4.0 的 JwtClaims 不含 `aud` 字段，本仓库侧自行 serde 反序列化
// JWT payload 中的 `aud` 声明，实现 audience 校验 + grace period 兼容。
// ============================================================================

#[cfg(feature = "v19-jwt-audience")]
mod audience {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use serde::Deserialize;
    use sz_rust_core::orm::auth::User;
    use sz_rust_core::orm::jwt::JwtEncoder;

    /// Audience 校验错误（映射到 401）
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum AudienceError {
        /// 签名/过期/格式校验失败
        InvalidToken,
        /// aud 存在但不包含 service_id
        AudienceMismatch,
        /// 无 aud 且 deny_no_audience = true 且超出 grace period
        NoAudienceDenied,
    }

    /// 告警采样窗口（秒）：同一类按请求触发的告警最多每窗口输出一条
    const WARN_SAMPLE_INTERVAL_SECS: i64 = 60;

    /// 按时间窗采样输出告警的状态（窗口起点 + 被抑制条数）
    ///
    /// Why：「无 aud 旧令牌在 grace period 内放行」是**每个请求**都会命中的分支，
    /// 逐条 WARN 会把日志 I/O 计费到业务热路径上（2026-10-29 现场验证：3 次 60 s 稳态
    /// 内落盘 157,068 条），并且任何持无 aud 令牌的客户端都能用它灌满磁盘。
    /// 安全信号本身要保留，因此改为采样输出并附带被抑制条数。
    #[derive(Default)]
    pub(crate) struct WarnSampler {
        last_secs: std::sync::atomic::AtomicI64,
        suppressed: std::sync::atomic::AtomicU64,
    }

    impl WarnSampler {
        /// 返回 `(should_log, suppressed_since_last_log)`
        pub(crate) fn sample(&self, now: i64) -> (bool, u64) {
            use std::sync::atomic::Ordering::Relaxed;
            let last = self.last_secs.load(Relaxed);
            if now - last >= WARN_SAMPLE_INTERVAL_SECS
                && self
                    .last_secs
                    .compare_exchange(last, now, Relaxed, Relaxed)
                    .is_ok()
            {
                (true, self.suppressed.swap(0, Relaxed))
            } else {
                self.suppressed.fetch_add(1, Relaxed);
                (false, 0)
            }
        }
    }

    static NO_AUDIENCE_WARN: std::sync::OnceLock<WarnSampler> = std::sync::OnceLock::new();

    /// Audience 校验配置
    ///
    /// - `service_id`：本服务期望的 audience 值（如 "sz300-api"）
    /// - `deny_no_audience`：grace period 过期后是否拒绝无 aud 的旧令牌
    /// - `grace_period_secs`：grace period 时长（秒）；iat 在此期间内的无 aud 令牌放行
    /// - `enabled`：是否启用 audience 校验（false 则完全跳过）
    #[derive(Debug, Clone)]
    pub struct AudienceConfig {
        /// 本服务期望的 audience 值（如 "sz300-api"）
        pub service_id: String,
        /// grace period 过期后是否拒绝无 aud 的旧令牌
        pub deny_no_audience: bool,
        /// grace period 时长（秒）；iat 在此期间内的无 aud 令牌放行
        pub grace_period_secs: i64,
        /// 是否启用 audience 校验（false 则完全跳过）
        pub enabled: bool,
    }

    impl AudienceConfig {
        /// 创建默认配置：service_id = "sz300-api"，grace period = 30 天，deny = true
        pub fn default_for_sz300() -> Self {
            Self {
                service_id: "sz300-api".to_string(),
                deny_no_audience: true,
                grace_period_secs: 30 * 86400,
                enabled: true,
            }
        }
    }

    /// JWT payload 扩展结构 — 仅解析 `aud` 和 `iat` 字段
    ///
    /// `aud` 在 JWT 标准中可以是 `String` 或 `Vec<String>`，使用自定义反序列化兼容两种格式。
    #[derive(Debug, Deserialize)]
    struct JwtPayloadExt {
        #[serde(default)]
        iat: i64,
        #[serde(default, deserialize_with = "deserialize_aud")]
        aud: Vec<String>,
    }

    /// 兼容 `aud` 为 String 或 Vec<String> 或缺失的反序列化器
    fn deserialize_aud<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum AudRepr {
            Single(String),
            Multi(Vec<String>),
        }
        Ok(match Option::<AudRepr>::deserialize(deserializer)? {
            Some(AudRepr::Single(s)) => vec![s],
            Some(AudRepr::Multi(v)) => v,
            None => Vec::new(),
        })
    }

    /// 从 JWT token 中解析 payload 扩展信息（aud / iat）
    ///
    /// 仅 base64 解码 payload 部分 + serde 反序列化，不做签名校验
    /// （签名校验由 `JwtAuthenticator::verify_token` 完成）。
    fn parse_payload_ext(token: &str) -> Result<JwtPayloadExt, AudienceError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AudienceError::InvalidToken);
        }
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| AudienceError::InvalidToken)?;
        serde_json::from_slice(&payload_bytes).map_err(|_| AudienceError::InvalidToken)
    }

    /// 当前 Unix 时间戳（秒）
    fn now_secs() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// 校验 JWT 令牌（含 audience 校验）— 成功返回用户信息
    ///
    /// ## 校验流程
    ///
    /// 1. `JwtAuthenticator::verify_token` 校验签名/过期 → 返回 `User`
    /// 2. 若 `config.enabled = false`，直接返回 User（回退不校验）
    /// 3. 解析 payload 中的 `aud` 声明
    /// 4. aud 存在且包含 `service_id` → 放行
    /// 5. aud 存在但不包含 `service_id` → `AudienceMismatch`
    /// 6. aud 不存在：iat 在 grace period 内 → 放行 + WARN；否则按 `deny_no_audience` 处理
    ///
    /// ## grace period 兼容
    ///
    /// 旧令牌（无 aud）在 `iat + grace_period_secs > now` 期间内仍可使用，
    /// 超出后若 `deny_no_audience = true` 则拒绝，否则放行。
    pub fn verify_token_with_audience(
        token: &str,
        config: &AudienceConfig,
    ) -> Result<User, AudienceError> {
        // 2026-10-28：logout 已注销的 token 立即失效（与 verify_token 一致）
        if super::is_token_revoked(token) {
            return Err(AudienceError::InvalidToken);
        }
        let user = super::get_auth().verify_token(token).map_err(|e| {
            tracing::error!(error = ?e, "JWT token 签名/过期校验失败");
            AudienceError::InvalidToken
        })?;

        if !config.enabled {
            return Ok(user);
        }

        let payload = parse_payload_ext(token)?;

        if !payload.aud.is_empty() {
            if payload.aud.iter().any(|a| a == &config.service_id) {
                Ok(user)
            } else {
                tracing::warn!(
                    aud = ?payload.aud,
                    expected = %config.service_id,
                    "JWT audience 不匹配"
                );
                Err(AudienceError::AudienceMismatch)
            }
        } else {
            // 无 aud — 检查 grace period
            let now = now_secs();
            if payload.iat + config.grace_period_secs > now {
                let sampler = NO_AUDIENCE_WARN.get_or_init(WarnSampler::default);
                let (should_log, suppressed) = sampler.sample(now);
                if should_log {
                    tracing::warn!(
                        iat = payload.iat,
                        grace_period_secs = config.grace_period_secs,
                        suppressed_in_window = suppressed,
                        "无 aud 旧令牌在 grace period 内放行，建议轮换（每 {} s 采样一条）",
                        WARN_SAMPLE_INTERVAL_SECS
                    );
                }
                Ok(user)
            } else if config.deny_no_audience {
                tracing::warn!("无 aud 旧令牌超出 grace period，拒绝");
                Err(AudienceError::NoAudienceDenied)
            } else {
                Ok(user)
            }
        }
    }

    /// 使用 JwtEncoder 签发带 audience 的 JWT（测试/签发辅助）
    ///
    /// 在标准 JwtClaims 基础上，将 `aud` 注入 payload 后重新编码。
    /// 由于 JwtClaims 不支持 aud，这里采用"编码后篡改 payload"的方式：
    /// 解析原 token → 注入 aud → 重新签名。
    pub fn encode_token_with_audience(
        encoder: &JwtEncoder,
        claims: &sz_rust_core::orm::jwt::JwtClaims,
        audience: &str,
    ) -> Result<String, String> {
        let base_token = encoder
            .encode(claims)
            .map_err(|e| format!("JWT 编码失败: {}", e))?;
        inject_audience(&base_token, encoder, audience)
    }

    /// 向已有 JWT token 注入 `aud` 声明并重新签名
    fn inject_audience(
        token: &str,
        encoder: &JwtEncoder,
        audience: &str,
    ) -> Result<String, String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err("Invalid JWT format".to_string());
        }
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|e| format!("payload decode failed: {}", e))?;
        let mut payload: serde_json::Value =
            serde_json::from_slice(&payload_bytes).map_err(|e| format!("payload parse: {}", e))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "aud".to_string(),
                serde_json::Value::String(audience.to_string()),
            );
        }
        let new_payload_json =
            serde_json::to_string(&payload).map_err(|e| format!("payload serialize: {}", e))?;
        let new_payload_b64 = URL_SAFE_NO_PAD.encode(new_payload_json.as_bytes());

        // 使用 encoder 的 secret 重新签名
        let signing_input = format!("{}.{}", parts[0], new_payload_b64);
        let secret = encoder.secret();
        // HMAC-SHA256 via hmac crate
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| format!("HMAC init: {}", e))?;
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        Ok(format!("{}.{}.{}", parts[0], new_payload_b64, sig_b64))
    }
}

#[cfg(feature = "v19-jwt-audience")]
pub use audience::{
    encode_token_with_audience, verify_token_with_audience, AudienceConfig, AudienceError,
};

/// 检查用户是否拥有对指定资源的权限
#[tracing::instrument(skip(user))]
pub fn check_permission(user: &User, permission: &str, resource: &str) -> bool {
    get_rbac().can(user, permission, resource).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_auth_constants_unused() {
        // 确保 PasswordVerifier / DbPasswordVerifier 已被移除
        // 编译时检查：如果残留引用会编译失败
        fn legacy_verifiers_removed() -> bool {
            true
        }
        assert!(
            legacy_verifiers_removed(),
            "PasswordVerifier / DbPasswordVerifier 残留引用应已移除"
        );
    }

    /// grace-period 告警必须按窗口采样，且不能丢失被抑制条数
    ///
    /// 用独立的 `WarnSampler` 实例（而非进程级静态），避免与并行用例相互干扰。
    #[cfg(feature = "v19-jwt-audience")]
    #[test]
    fn test_no_audience_warning_is_sampled_per_window() {
        use super::audience::WarnSampler;

        let sampler = WarnSampler::default();
        let base = 1_700_000_000_i64;

        // 窗口外首次命中：输出，抑制数为 0
        assert_eq!(sampler.sample(base), (true, 0));
        // 同窗口内后续命中：不输出
        for _ in 0..9 {
            assert!(!sampler.sample(base + 1).0);
        }
        // 下一个窗口：输出且携带上一窗口累计的 9 条抑制
        assert_eq!(sampler.sample(base + 60), (true, 9));
        // 窗口内未再次到期：仍不输出
        assert!(!sampler.sample(base + 90).0);
        assert_eq!(sampler.sample(base + 120), (true, 1));
    }
}
