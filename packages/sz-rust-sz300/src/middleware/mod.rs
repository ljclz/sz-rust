/// 认证中间件模块
pub mod auth_middleware;
/// metrics 端点访问控制中间件（T7）
pub mod metrics_auth;
/// 角色鉴权中间件（admin feature 门控）
#[cfg(feature = "admin")]
pub mod role_guard;
