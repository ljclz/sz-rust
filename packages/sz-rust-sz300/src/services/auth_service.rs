use sz_orm_auth::{Authorizer, JwtAuthenticator, RbacAuthorizer};
use sz_orm_auth::auth::{User, PasswordVerifier, Credentials};
use sz_orm_auth::AuthError;
use sz_orm_core::Pool;
use sz_orm_core::Value;
use std::sync::{Arc, OnceLock};

static AUTH: OnceLock<JwtAuthenticator> = OnceLock::new();
static RBAC: OnceLock<RbacAuthorizer> = OnceLock::new();

/// 基于数据库的密码验证器
struct DbPasswordVerifier {
    pool: Arc<Pool>,
}

impl PasswordVerifier for DbPasswordVerifier {
    fn verify_password(&self, username: &str, password: &str) -> Result<i64, AuthError> {
        // 使用 tokio block_on 在同步上下文中执行异步 DB 查询
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut conn = self.pool.acquire().await
                    .map_err(|e| AuthError::InvalidCredentials(format!("DB错误: {}", e)))?;

                let sql = format!(
                    "SELECT user_id, password_hash as password FROM merchant_user WHERE username = '{}'",
                    sql_escape(username)
                );

                let rows = conn.query(&sql).await
                    .map_err(|e| AuthError::InvalidCredentials(format!("查询失败: {}", e)))?;

                let row = rows.first().ok_or_else(|| {
                    AuthError::InvalidCredentials("用户不存在".to_string())
                })?;

                let stored_hash = row.get("password")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| AuthError::InvalidCredentials("无法读取密码".to_string()))?;

                let user_id = row.get("user_id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| AuthError::InvalidCredentials("无法读取用户ID".to_string()))?;

                // bcrypt 验证密码
                if bcrypt::verify(password, stored_hash).unwrap_or(false) {
                    // 更新最后登录时间
                    let _ = conn.execute(
                        &format!("UPDATE merchant_user SET last_login_at = NOW() WHERE user_id = {}", user_id)
                    ).await;
                    Ok(user_id)
                } else {
                    Err(AuthError::InvalidCredentials("密码错误".to_string()))
                }
            })
        });
        result
    }
}

fn sql_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

pub fn init_auth(secret: &str, issuer: &str, expiry: u64, pool: Arc<Pool>) {
    let verifier = Arc::new(DbPasswordVerifier { pool });
    let auth = JwtAuthenticator::new(secret, issuer, expiry)
        .with_password_verifier(verifier);
    let _ = AUTH.set(auth);
    let _ = RBAC.set(RbacAuthorizer::new());
}

pub fn get_auth() -> &'static JwtAuthenticator {
    AUTH.get().expect("auth not initialized")
}

pub fn get_rbac() -> &'static RbacAuthorizer {
    RBAC.get().expect("rbac not initialized")
}

pub fn authenticate(username: &str, password: &str) -> Result<String, String> {
    let creds = Credentials::new(username, password);
    let token = get_auth()
        .authenticate(&creds)
        .map_err(|e| format!("认证失败: {}", e))?;
    Ok(token.access_token)
}

pub fn verify_token(token: &str) -> Result<User, String> {
    let user = get_auth()
        .verify_token(token)
        .map_err(|e| format!("token验证失败: {}", e))?;
    Ok(user)
}

pub fn check_permission(user: &User, permission: &str, resource: &str) -> bool {
    get_rbac().can(user, permission, resource).unwrap_or(false)
}
