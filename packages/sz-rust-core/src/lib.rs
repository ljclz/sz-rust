//! SZ-Rust Core — 主框架包
//!
//! 对标 ThinkPHP 8 的 Rust Web 框架核心，基于 axum 0.8 + SZ-ORM。
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 实现阶段 |
//! |------|---------|---------|
//! | `controller` | `app\SzController` / `app\BaseController` | Phase 2 |
//! | `model` | `think\Model` | Phase 2 + Phase 4 |
//! | `relation` | `think\Model` 关联关系（HasMany/BelongsTo/HasOne/BelongsToMany/Morph） | Phase 4 |
//! | `request` | `$this->request->post/get` | Phase 1 + Phase 5 |
//! | `response` | `renderJson/renderSuccess/renderError` | Phase 1 + Phase 2 |
//! | `middleware` | CORS/Auth/Log/RateLimit/Trace | Phase 1 + Phase 3 |
//! | `guard` | NestJS Guard + Spring Security（sz-rust 自研） | Phase 3.7 |
//! | `hooks` | think-orm Model 钩子（HookDispatcher 16 事件） | Phase 3.8 |
//! | `multi_app` | `auto_multi_app` | Phase 1 |
//! | `health` | 健康检查端点（K8s liveness/readiness） | Phase 1.7 |
//! | `static_files` | 静态文件路由（`tower-http::ServeDir`） | Phase 1.8 |
//! | `error_handler` | 404/500 标准化 JSON 响应 | Phase 1.9 |
//! | `h2` | HTTP/2 + TLS（`think-swoole` SSL） | Phase 1.10 |
//! | `routing` | 三层路由机制（属性宏/配置式/约定式） | Phase 1.11 |
//! | `addons` | `addons/` 插件 | Phase 10 |
//! | `router` | `with_route` | Phase 1 |
//! | `container` | `app()` 容器 | Phase 0.6 |
//! | `error` | `BaseException` | Phase 0.5 |
//! | `macros` | `compact()` | Phase 2 |
//! | `config` | `config/app.php` / `database.php` | Phase 0.4 |
//! | `log` | `think-logger` | Phase 0.7 |
//! | `server` | `think-swoole` / `think-worker` 启动入口 | Phase 1.1 |
//! | `validate` | `think\Validate` 数据验证器 | Phase 5 |
//! | `upload` | `think\File` + `think\file\UploadedFile` 文件上传 | Phase 5.5 |
//! | `cache` | `think\facade\Cache` 缓存 facade | Phase 6 |
//! | `session` | `think\facade\Session` 会话管理（SessionStore trait + MemorySessionStore） | Phase P2-10 |
//! | `cookie` | `think\Cookie` Cookie 管理（CookieJar + CookieOptions） | Phase P2-11 |
//! | `event` | `think\Event` 事件系统（Listener/Subscriber/Observer） | Phase 6.6 |

#![forbid(unsafe_code)]
// v0.2.0：启用 missing_docs 警告，要求所有公开项必须有文档注释
#![warn(missing_docs)]
// 文档构建时将 missing_docs 作为错误（CI 中 RUSTDOCFLAGS="-D warnings" 会强制）
#![cfg_attr(doctest, warn(missing_docs))]

pub mod addons;
pub mod cache;
pub mod config;
pub mod container;
pub mod controller;
pub mod cookie;
pub mod error;
pub mod error_handler;
pub mod event;
pub mod guard;
pub mod h2;
pub mod health;
pub mod hooks;
pub mod log;
pub mod macros;
pub mod middleware;
pub mod model;
pub mod multi_app;
pub mod relation;
pub mod request;
pub mod response;
pub mod router;
pub mod routing;
pub mod runtime;
pub mod server;
pub mod session;
pub mod static_files;
pub mod upload;
pub mod validate;
pub mod view;

// ============================================================================
// 过程宏重导出
// ============================================================================

/// 编译时 SQL 校验宏 — 复用自 `sz-orm-macros`
///
/// 在编译期对 SQL 字符串字面量进行语法和注入模式校验，校验通过后
/// 将 SQL 作为 `&'static str` 发出到调用处。任何校验失败都会触发
/// `compile_error!`，二进制无法构建。
///
/// ## 校验规则
///
/// - SELECT 必须包含 FROM
/// - INSERT 必须包含 INTO 和 VALUES
/// - UPDATE 必须包含 SET
/// - DELETE 必须包含 FROM
/// - 括号必须平衡
/// - 字符串字面量必须闭合
/// - 禁止 SQL 注入模式（`; DROP TABLE` / `OR 1=1` / `UNION SELECT` / `--` / `/*` 等）
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::sql_string;
///
/// // 基础用法：校验通过后返回 &str
/// let sql = sql_string!("SELECT * FROM users WHERE id = 1");
///
/// // 带参数数量校验
/// let sql = sql_string!("SELECT * FROM users WHERE id = ?"; params: 1);
///
/// // ❌ 编译错误：SELECT 缺少 FROM
/// // let sql = sql_string!("SELECT * users");
///
/// // ❌ 编译错误：检测到 SQL 注入模式
/// // let sql = sql_string!("SELECT * FROM users WHERE name = 'x' OR '1'='1'");
/// ```
pub use sz_orm_macros::sql_string;

/// 编译时 SQL 校验 + 可选真实 DB 验证宏 — 复用自 `sz-orm-macros`
///
/// 与 [`sql_string!`] 行为一致，额外支持在 `db-verify` feature 启用且
/// `SZ_ORM_QUERY_VERIFY=1` 环境变量设置时，连接 `DATABASE_URL` 指向的
/// 数据库执行 `EXPLAIN` 进行真实 schema 校验。
pub use sz_orm_macros::query;

// ============================================================================
// 运行时 SQL 校验 — 复用自 sz-orm-sql-validator
// ============================================================================

pub use sz_orm_sql_validator::{
    detect_statement_type, validate, validate_column_name, validate_delete, validate_insert,
    validate_parameter_count, validate_select, validate_sql, validate_table_name, validate_update,
    SqlStatementType, SqlValidationError, ValidationResult,
};

/// 运行时 SQL 校验便捷函数
///
/// 对 [`validate_sql`] 的薄包装，返回 `Result<(), String>` 以便上层不依赖
/// `SqlValidationError` 类型也能处理错误。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::validate_sql_runtime;
///
/// // 合法 SQL
/// assert!(validate_sql_runtime("SELECT * FROM users WHERE id = 1").is_ok());
///
/// // 非法 SQL（缺少 FROM）
/// assert!(validate_sql_runtime("SELECT * users").is_err());
///
/// // SQL 注入
/// assert!(validate_sql_runtime("SELECT * FROM users WHERE name = 'x' OR '1'='1'").is_err());
/// ```
pub fn validate_sql_runtime(sql: &str) -> Result<(), String> {
    validate_sql(sql).map_err(|e| e.to_string())
}
