//! ORM Facade — 统一访问 sz-orm 全家桶
//!
//! ## 设计目标
//!
//! 减少 `sz-rust-addons-operate`、`sz-rust-sz300` 等业务包对 `sz-orm-*` 子包的穿透依赖。
//! 业务包应通过 `sz_rust_core::orm::*` 访问 ORM 功能，而非直接依赖 `sz-orm-core`、
//! `sz-orm-auth`、`sz-orm-mqtt`、`sz-orm-scheduler`、`sz-orm-logger`。
//!
//! ## 使用示例
//!
//! ```ignore
//! // 业务包统一通过 facade 访问（推荐）
//! use sz_rust_core::orm::{Pool, Value, Model, ModelExt, Authorizer, JwtAuthenticator, MqttConfig, QoS};
//! use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
//! use sz_rust_core::orm::scheduler::{CronScheduler, JobHandler, ScheduledTask, Scheduler};
//! ```
//!
//! ## 依赖关系（基于 Cargo.toml 实际测量）
//!
//! | 业务包 | 迁移前 sz-orm-* 依赖数 | 迁移后 sz-orm-* 依赖数 | 减少 |
//! |--------|----------------------|----------------------|------|
//! | sz-rust-addons-operate | 2 个（sz-orm-core + sz-rust-core） | 0 个 | 2 个 |
//! | sz-rust-sz300 | 8 个 | 1 个（sz-orm-sqlx） | 7 个 |
//!
//! 业务包保留的直接依赖（仅 sz-rust-sz300）：
//! - `sz-orm-sqlx`（SQLx 后端实现，sz-rust-core 不提供后端绑定）

// ============================================================================
// sz-orm-core：连接池 + 模型 trait + 值类型 + 仓储层 + 错误类型 + 迁移系统
// ============================================================================
pub use sz_orm_core::{
    value_to_json, BelongsTo, BelongsToMany, Cache, CacheError, Connection, ConnectionFactory,
    DbError, DbType, HasMany, HasOne, MemoryCache, Model, ModelExt, MorphMany, MorphTo, Pool,
    PoolConfig, PoolConfigBuilder, PoolError, Relation, RelationAccess, RelationError,
    RelationLoader, TimestampFields, Value,
    // NOTE: WithRelation intentionally NOT re-exported at facade root — it is
    // exported by both sz_orm_model and sz_orm_query via glob re-exports in
    // sz-orm-core, which is ambiguous under Rust 1.96+'s deny(ambiguous_glob_imports).
    // Use `crate::orm::find_with_related::WithRelation` instead.
};

/// 仓储子模块（Repository trait + WhereCondition/WhereOp 条件构造）
///
/// 业务层应通过 `sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp}` 访问，
/// 而非直接依赖 `sz_orm_core::repository`，以保持 facade 收口。
pub mod repository {
    pub use sz_orm_core::repository::{
        BatchUpdateResult, EntityAttributes, EntityKey, GenericKeyRepository, InMemoryRepository,
        PageResult, Repository, RepositoryError, RepositoryResult, WhereCondition, WhereOp,
    };
}

/// 迁移子模块（schema migration）
///
/// 业务层应通过 `sz_rust_core::orm::migration::*` 访问，
/// 而非直接依赖 `sz_orm_core::migration`。
pub mod migration {
    pub use sz_orm_core::migration::{
        FileMigrationResolver, Migration, MigrationContext, MigrationResolver, Migrator,
    };
}

// ============================================================================
// sz-orm-auth：认证 + 授权
// ============================================================================
pub use sz_orm_auth::{Authorizer, JwtAuthenticator, RbacAuthorizer};

/// 认证子模块（Credentials / User / Claims 等数据结构）
///
/// 业务层应通过 `sz_rust_core::orm::auth::{Credentials, User, Claims}` 访问，
/// 而非直接依赖 `sz_orm_auth::auth`。
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
pub use sz_orm_mqtt::{MqttConfig, MqttError, MqttMessage, MqttPlugin, MqttTopic, QoS};

// ============================================================================
// sz-orm-scheduler：定时任务调度器
// ============================================================================
/// 调度器子模块（CronScheduler / JobHandler / ScheduledTask 等）
///
/// 业务层应通过 `sz_rust_core::orm::scheduler::*` 访问，
/// 而非直接依赖 `sz_orm_scheduler`。
pub mod scheduler {
    pub use sz_orm_scheduler::{
        CounterJobHandler, CronExpr, CronScheduler, JobHandler, RecordingJobHandler, ScheduledTask,
        Scheduler, SchedulerError,
    };
}

/// 调度器根级 re-export（供 core 内部 dogfood 使用）
pub use sz_orm_scheduler::{
    CronExpr, CronScheduler, JobHandler, RecordingJobHandler, ScheduledTask, Scheduler,
    SchedulerError,
};

// ============================================================================
// sz-orm-logger：日志适配器
// ============================================================================
/// 日志子模块（Logger / LogEntry / LogLevel 等）
///
/// 业务层应通过 `sz_rust_core::orm::logger::*` 访问，
/// 而非直接依赖 `sz_orm_logger`。
pub mod logger {
    pub use sz_orm_logger::{
        LogEntry, LogLevel, Logger, LoggerFactory, Metrics, MetricsSnapshot, StructuredLogger,
    };
}

// ============================================================================
// sz-orm-query-builder：链式 SQL 构造器（sea-query 风格，独立于 Model）
// ============================================================================
pub use sz_orm_query_builder::{
    BuiltQuery, DeleteQuery, InsertQuery, Query, SelectQuery, UpdateQuery,
};

// ============================================================================
// sz-orm-macros：编译时 SQL 宏（sql_string! / query! 等）
// ============================================================================
pub use sz_orm_macros::{query, sql_string};

// ============================================================================
// sz-orm-core 补充子模块（core 内部 dogfood 所需）
// ============================================================================

/// Hooks 子模块（生命周期钩子）
///
/// 业务层应通过 `sz_rust_core::orm::hooks::*` 访问，
/// 而非直接依赖 `sz_orm_core::hooks`。
pub mod hooks {
    pub use sz_orm_core::hooks::*;
}

/// L2 缓存子模块（二级缓存共享层）
///
/// 业务层应通过 `sz_rust_core::orm::l2_cache::*` 访问，
/// 而非直接依赖 `sz_orm_core::l2_cache`。
pub mod l2_cache {
    pub use sz_orm_core::l2_cache::*;
}

/// 关联查询子模块（find_with_related  eager/join/subquery 策略）
///
/// 业务层应通过 `sz_rust_core::orm::find_with_related::*` 访问，
/// 而非直接依赖 `sz_orm_core::find_with_related`。
pub mod find_with_related {
    pub use sz_orm_core::find_with_related::{
        find_with_related_eager_sql, find_with_related_join, find_with_related_subquery,
        inspect_relation, FindWithRelated, WithRelation, WithRelation as FindWithRelation,
    };
}

// ============================================================================
// sz-orm-limit：限流器（速率限制中间件依赖）
// ============================================================================
pub use sz_orm_limit::{
    DistributedRateLimiter, FixedWindowRateLimiter, InMemoryBackend, RateLimitError,
    RateLimitHeaders, RateLimitResult, RateLimitResponseStrategy, SlidingWindowRateLimiter,
    TokenBucketRateLimiter,
};

/// RateLimiter trait re-export（限流中间件核心 trait）
pub use sz_orm_limit::RateLimiter;

// ============================================================================
// sz-orm-tracing：分布式追踪（trace 中间件依赖）
// ============================================================================
pub use sz_orm_tracing::{
    Alert, AlertHook, AlertLevel, ErrorBudget, ErrorRateCounter, LatencyHistogram, LogAlertHook,
    OtelTracer, SaturationGauge, SlaMonitor, SlaReport, Span, SpanLog, SzTracer, Tracer,
    TracingError,
};

// ============================================================================
// sz-orm-queue：消息队列（runtime::queue 依赖）
// ============================================================================
pub use sz_orm_queue::{
    ActiveConfig, BackpressurePolicy, InMemoryQueue, KafkaConfig, Message, MessageQueue, MqError,
    MqProvider, NatsConfig, OverflowStrategy, PulsarConfig, QueueConfig,
};

// ============================================================================
// sz-orm-websocket：WebSocket 服务（runtime::websocket 依赖）
// ============================================================================
pub use sz_orm_websocket::{
    DefaultWebSocketHandler, WebSocketHandler, WsError, WsServer,
};

// ============================================================================
// sz-orm-storage：云存储驱动（upload/storage 依赖）
// ============================================================================
pub use sz_orm_storage::{
    AliyunOssStorage, HuaweiObsStorage, LocalStorage, StorageBuilder, StorageConfig,
    StorageError, StorageProvider, StorageWrapper,
};

// ============================================================================
// sz-orm-sql-validator：SQL 安全校验（SQL 注入防护）
// ============================================================================
pub use sz_orm_sql_validator::{
    detect_statement_type, validate, validate_delete, validate_insert, validate_parameter_count,
    validate_select, validate_sql, validate_table_name, validate_column_name, validate_update,
    SqlComplexityScore, SqlStatementType, SqlToken, SqlValidationError,
    ValidationResult, WhitelistValidator, ComplexityLevel,
};

// ============================================================================
// P2: sz-orm-graphql — GraphQL 集成（可选 feature: graphql）
// ============================================================================
#[cfg(feature = "graphql")]
pub mod graphql;

// ============================================================================
// P2: sz-orm-grpc — gRPC 支持（可选 feature: grpc）
// ============================================================================
#[cfg(feature = "grpc")]
pub mod grpc;
