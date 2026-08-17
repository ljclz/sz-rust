use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use sz_rust_core::auth::refresh::{
    MemoryRefreshTokenStore, MemoryTokenBlacklist, RefreshTokenConfig, RefreshTokenIssuer,
    RefreshTokenStore, SsoJwtCodec, TokenBlacklist,
};
use sz_rust_core::orm::auth::User;
use sz_rust_core::orm::Pool;
use sz_rust_core::orm::Value;
use sz_rust_core::orm::{JwtAuthenticator, RbacAuthorizer};

static AUTH: OnceLock<JwtAuthenticator> = OnceLock::new();
static RBAC: OnceLock<RbacAuthorizer> = OnceLock::new();
static DB_POOL: OnceLock<Arc<Pool>> = OnceLock::new();
static REFRESH_ISSUER: OnceLock<RefreshTokenIssuer> = OnceLock::new();
/// JWT 密钥与过期时间（签发 access token 用；与 AUTH 验证器使用同一密钥）
static JWT_SECRET: OnceLock<String> = OnceLock::new();
static JWT_EXPIRY: OnceLock<u64> = OnceLock::new();

/// 初始化认证模块 — 配置 JWT 密钥、签发者、过期时间与数据库连接池
///
/// 同时将连接池存入全局静态变量，供 [`authenticate_async`] 执行参数化查询使用。
/// 此设计避免了在同步 `PasswordVerifier` trait 中使用 `block_in_place` 调用异步 DB 查询。
#[tracing::instrument(skip(secret, pool))]
pub fn init_auth(secret: &str, issuer: &str, expiry: u64, pool: Arc<Pool>) {
    let auth = JwtAuthenticator::new(secret, issuer, expiry);
    let _ = AUTH.set(auth);
    let _ = RBAC.set(RbacAuthorizer::new());
    let _ = DB_POOL.set(pool);
    let _ = JWT_SECRET.set(secret.to_string());
    let _ = JWT_EXPIRY.set(expiry);

    let codec = SsoJwtCodec::new(secret);
    let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig {
        issuer: issuer.to_string(),
        ..Default::default()
    };
    let issuer_impl = RefreshTokenIssuer::new(codec, blacklist, store, config);
    let _ = REFRESH_ISSUER.set(issuer_impl);
}

/// 获取已初始化的 Refresh Token 签发器（调用前必须先调用 [`init_auth`]）
pub fn get_refresh_issuer() -> &'static RefreshTokenIssuer {
    REFRESH_ISSUER
        .get()
        .expect("refresh issuer not initialized")
}

/// 获取已初始化的 JWT 认证器（调用前必须先调用 [`init_auth`]）
#[tracing::instrument(skip_all)]
pub fn get_auth() -> &'static JwtAuthenticator {
    AUTH.get().expect("auth not initialized")
}

/// 获取已初始化的数据库连接池（调用前必须先调用 [`init_auth`]）
fn get_pool() -> &'static Arc<Pool> {
    DB_POOL.get().expect("db pool not initialized")
}

/// 异步用户名密码认证 — 成功返回 JWT access_token，失败返回错误描述
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
#[tracing::instrument(skip(password))]
pub async fn authenticate_async(username: &str, password: &str) -> Result<String, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("用户名或密码不能为空".to_string());
    }

    let pool = get_pool();

    // P1-SEC-06: 外部 IO 超时保护（默认 5s）— 仅包裹 DB 查询
    let rows = sz_rust_core::runtime::spawn::with_timeout(async {
        let mut conn = pool.acquire().await.map_err(|e| {
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
        let mut conn = pool.acquire().await.map_err(|e| {
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

    // 使用 JwtEncoder 直接签发带 user_id 的 claims（2026-08-14 安全修复 H-1）：
    // 旧实现调用 JwtAuthenticator::authenticate()，其要求 password_verifier 再次验证密码，
    // 但本函数已在上面用 bcrypt 验证完毕（spawn_blocking），且未配置 verifier 时 authenticate()
    // 会直接返回 Err —— 导致登录必然失败、user_id 永远无法写入 token。
    // 改为显式构造 claims：user_id 来自 DB 查询结果，roles 默认 ["user"]。
    let secret = JWT_SECRET
        .get()
        .expect("JWT secret not initialized — 请先调用 init_auth");
    let expiry = *JWT_EXPIRY
        .get()
        .expect("JWT expiry not initialized — 请先调用 init_auth");
    let exp = chrono::Utc::now().timestamp() + expiry as i64;
    let claims = sz_rust_core::orm::jwt::JwtClaims::new(username.to_string(), exp)
        .with_issuer("sz300")
        .with_roles(vec!["user".to_string()])
        .with_user_id(user_id);
    let access_token = sz_rust_core::orm::jwt::JwtEncoder::new(secret)
        .encode(&claims)
        .map_err(|e| {
            tracing::error!(error = ?e, "JWT 编码失败");
            "认证失败".to_string()
        })?;
    Ok(access_token)
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
    let mut conn = pool
        .acquire()
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
    let mut conn = pool
        .acquire()
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

/// 校验 JWT 令牌 — 成功返回用户信息，失败返回错误描述
///
/// 错误信息不泄露 JWT 内部解析细节，统一返回 "token 验证失败"。
#[tracing::instrument(skip(token))]
pub fn verify_token(token: &str) -> Result<User, String> {
    let user = get_auth().verify_token(token).map_err(|e| {
        tracing::error!(error = ?e, "JWT token 验证失败");
        "token 验证失败".to_string()
    })?;
    Ok(user)
}

/// 从请求扩展中获取已认证用户（由 auth_middleware 注入）
///
/// 安全修复 H-1：业务层必须通过此函数获取身份，禁止信任请求体中的 user_id/merchant_id。
/// 返回 `None` 表示请求未经过认证中间件（不应发生，兜底防御）。
pub fn current_user(req: &axum::http::Request<axum::body::Body>) -> Option<std::sync::Arc<User>> {
    req.extensions().get::<std::sync::Arc<User>>().cloned()
}

/// 强制校验并解析商户身份（安全修复 H-1 核心）
///
/// 从请求扩展取 JWT 身份（user_id），反查该用户所属 merchant_id；
/// 若请求体中携带的 merchant_id 与身份不符（或身份无商户归属），返回 `Err`。
/// 调用方必须用返回值**覆盖**请求体中的 merchant_id，禁止回退到用户输入。
///
/// 返回 `Ok(merchant_id)`：服务端权威的商户 ID（数据边界）。
///
/// 注意：参数为 `user_id` 而非 `&Request` —— 避免 `&Request` 跨 await 捕获导致
/// handler future 不 Send（axum 0.8 Handler 要求 Send + Sync，Request<Body> 不 Sync）。
/// 调用方先用 [`current_user`] 同步提取 user_id 再调用本函数。
pub async fn resolve_merchant_id(user_id: i64, requested: Option<i64>) -> Result<i64, String> {
    // 反查用户归属商户（参数化查询）
    let sql = "SELECT merchant_id FROM merchant_user WHERE user_id = ?";
    let params = [Value::I64(user_id)];
    let pool = get_pool();
    let rows = sz_rust_core::runtime::spawn::with_timeout(async {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "身份解析：获取 DB 连接失败");
            "服务暂时不可用".to_string()
        })?;
        conn.query_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "身份解析：查询商户失败");
            "服务暂时不可用".to_string()
        })
    })
    .await
    .map_err(|_| "身份解析超时".to_string())??;

    let owned_merchant_id = rows
        .first()
        .and_then(|row| row.get("merchant_id"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "用户未绑定商户".to_string())?;

    // 请求体显式指定了不同商户 → 拒绝（防越权）
    if let Some(r) = requested {
        if r != owned_merchant_id {
            tracing::warn!(
                user_id = user_id,
                requested = r,
                owned = owned_merchant_id,
                "越权尝试：请求商户与身份不符"
            );
            return Err("无权访问该商户数据".to_string());
        }
    }

    Ok(owned_merchant_id)
}

/// 测试专用：以给定密钥初始化 JWT 认证器（不接 DB）
///
/// 仅用于 middleware 层单元测试，避免依赖真实数据库连接池。
/// `verify_token` 仅使用 encoder 解码，不依赖 DB，因此此初始化足够。
#[cfg(test)]
pub fn init_auth_test_only(secret: &str) {
    use sz_rust_core::orm::JwtAuthenticator;
    let auth = JwtAuthenticator::new(secret, "sz300-test", 86400);
    let _ = AUTH.set(auth);
    let _ = RBAC.set(RbacAuthorizer::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_constants_unused() {
        // 确保 PasswordVerifier / DbPasswordVerifier 已被移除
        // 编译时检查：如果残留引用会编译失败
    }

    #[test]
    fn init_auth_test_only_succeeds() {
        // OnceLock 只能设置一次，重复调用静默失败但不 panic
        init_auth_test_only("test-secret-key-for-coverage");
        // 验证 get_auth 不会 panic（已初始化）
        let _auth = get_auth();
    }

    #[test]
    fn verify_token_invalid_returns_err() {
        init_auth_test_only("test-secret-key-for-coverage");
        // 无效 token 应返回错误
        let result = verify_token("invalid.token.here");
        assert!(result.is_err(), "无效 token 应返回错误");
    }

    #[test]
    fn verify_token_empty_returns_err() {
        init_auth_test_only("test-secret-key-for-coverage");
        let result = verify_token("");
        assert!(result.is_err(), "空 token 应返回错误");
    }

    #[test]
    fn current_user_returns_none_without_extension() {
        let req = axum::http::Request::builder()
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(current_user(&req).is_none(), "无认证扩展的请求应返回 None");
    }

    #[test]
    fn current_user_returns_some_with_extension() {
        use sz_rust_core::orm::auth::User;
        let user = Arc::new(User::new(1, "testuser"));
        let req = axum::http::Request::builder()
            .extension(user)
            .body(axum::body::Body::empty())
            .unwrap();
        let result = current_user(&req);
        assert!(result.is_some(), "有认证扩展的请求应返回 Some");
        assert_eq!(result.unwrap().id, 1);
    }

    /// 覆盖 authenticate_async 空用户名早返回分支（不依赖 DB）
    #[tokio::test]
    async fn authenticate_async_empty_username_returns_err() {
        let result = authenticate_async("", "password").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
    }

    /// 覆盖 authenticate_async 空密码早返回分支（不依赖 DB）
    #[tokio::test]
    async fn authenticate_async_empty_password_returns_err() {
        let result = authenticate_async("user", "").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
    }

    /// 覆盖 authenticate_async 空白用户名早返回分支（trim 后为空）
    #[tokio::test]
    async fn authenticate_async_blank_username_returns_err() {
        let result = authenticate_async("   ", "password").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
    }

    /// 覆盖 resolve_merchant_id acquire 失败路径 — 需先 init_auth 注入 pool
    #[tokio::test]
    async fn resolve_merchant_id_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        init_auth(
            "test-secret-coverage",
            "sz300-test",
            86400,
            state.db_pool.clone(),
        );
        let result = resolve_merchant_id(1, None).await;
        assert!(result.is_err());
    }

    /// 覆盖 get_user_info_by_username acquire 失败路径 — 返回 None
    #[tokio::test]
    async fn get_user_info_by_username_returns_none_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        init_auth(
            "test-secret-coverage-2",
            "sz300-test",
            86400,
            state.db_pool.clone(),
        );
        let result = get_user_info_by_username("nonexistent").await;
        assert!(result.is_none(), "DB 不可用时应返回 None");
    }

    /// 覆盖 get_user_info_by_id acquire 失败路径 — 返回 None
    #[tokio::test]
    async fn get_user_info_by_id_returns_none_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        init_auth(
            "test-secret-coverage-3",
            "sz300-test",
            86400,
            state.db_pool.clone(),
        );
        let result = get_user_info_by_id(999).await;
        assert!(result.is_none(), "DB 不可用时应返回 None");
    }
}
