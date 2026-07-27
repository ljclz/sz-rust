//! ORM Facade — 统一访问 sz-orm 全家桶
//!
//! ## 设计目标
//!
//! 减少 `sz-rust-sz300` 等业务包对 `sz-orm-*` 子包的穿透依赖。
//! 业务包应通过 `sz_rust_core::orm::*` 访问 ORM 功能，而非直接依赖 `sz-orm-core`、
//! `sz-orm-auth`、`sz-orm-mqtt`、`sz-orm-scheduler`、`sz-orm-logger`。
//!
//! ## 使用示例
//!
//! ```ignore
//! // 旧方式（穿透依赖）
//! use sz_orm_core::{Pool, Value, Model, ModelExt};
//! use sz_orm_auth::{Authorizer, JwtAuthenticator};
//! use sz_orm_mqtt::{MqttConfig, QoS};
//!
//! // 新方式（facade）
//! use sz_rust_core::orm::{Pool, Value, Model, ModelExt, Authorizer, JwtAuthenticator, MqttConfig, QoS};
//! ```
//!
//! ## 依赖关系
//!
//! | 业务包 | 旧依赖数 | 新依赖数 | 减少 |
//! |--------|---------|---------|------|
//! | sz-rust-sz300 | 8 个 sz-orm-* | 3 个 sz-orm-* | 5 个 |
//!
//! 业务包保留的直接依赖：
//! - `sz-orm-sqlx`（SQLx 后端实现，sz-rust-core 不提供）
//! - `sz-orm-config`（配置管理，sz-rust-core 不提供）
//! - `sz-orm-macros`（过程宏，需直接依赖）

// ============================================================================
// sz-orm-core：连接池 + 模型 trait + 值类型
// ============================================================================
pub use sz_orm_core::{
    value_to_json, Model, ModelExt, Pool, PoolConfig, PoolConfigBuilder, PoolError, RelationLoader,
    TimestampFields, Value,
};

// ============================================================================
// sz-orm-auth：认证 + 授权
// ============================================================================
pub use sz_orm_auth::{Authorizer, JwtAuthenticator, RbacAuthorizer};

/// 认证子模块（Credentials / User / Claims 等数据结构）
pub mod auth {
    pub use sz_orm_auth::auth::{Claims, Credentials, User};
}

/// JWT 子模块（JwtEncoder / JwtClaims 等编码工具）
pub mod jwt {
    pub use sz_orm_auth::jwt::{JwtClaims, JwtEncoder};
}

// ============================================================================
// sz-orm-mqtt：MQTT 消息队列
// ============================================================================
pub use sz_orm_mqtt::{MqttConfig, MqttMessage, MqttTopic, QoS};

// ============================================================================
// sz-orm-scheduler：定时任务调度器
// ============================================================================
pub use sz_orm_scheduler as scheduler;

// ============================================================================
// sz-orm-logger：日志适配器
// ============================================================================
pub use sz_orm_logger as logger;

// ============================================================================
// 便捷 re-export：常用 API 顶层访问
// ============================================================================
/// 认证凭据（ Convenience re-export ）
pub use sz_orm_auth::auth::Credentials;
/// 用户信息（ Convenience re-export ）
pub use sz_orm_auth::auth::User;
