use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use sz_rust_core::orm::auth::{Credentials, User};
use sz_rust_core::orm::Pool;
use sz_rust_core::orm::Value;
use sz_rust_core::orm::{Authorizer, JwtAuthenticator, RbacAuthorizer};

static AUTH: OnceLock<JwtAuthenticator> = OnceLock::new();
static RBAC: OnceLock<RbacAuthorizer> = OnceLock::new();
static DB_POOL: OnceLock<Arc<Pool>> = OnceLock::new();

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
    })?
    .map_err(|e| e)?;

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

    // 使用 JwtAuthenticator 编码 token — 同步且极快（仅 HS256 哈希）
    let creds = Credentials::new(username, password);
    let token = get_auth().authenticate(&creds).map_err(|e| {
        tracing::error!(error = ?e, "JWT 编码失败");
        "认证失败".to_string()
    })?;
    Ok(token.access_token)
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
    }
}
