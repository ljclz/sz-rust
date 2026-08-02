//! SZ-Rust Core — 主框架包
//!
//! 对标 ThinkPHP 8 的 Rust Web 框架核心，基于 axum 0.8 + SZ-ORM。
//!
//! ## 模块结构
//!
//! 所有模块均已实现并通过测试。`addons` 重导出 `sz-rust-addons-loader`，
//! `macros` 重导出 `sz-rust-macros` 的过程宏。
//!
//! | 模块 | 对齐 PHP | 状态 |
//! |------|---------|------|
//! | `controller` | `app\SzController` / `app\BaseController` | ✅ |
//! | `model` | `think\Model` | ✅ |
//! | `schema_cache` | `think\db\Fetch` 字段缓存（SchemaCache + TableSchema + ColumnDefinition） | ✅ |
//! | `relation` | `think\Model` 关联关系（HasMany/BelongsTo/HasOne/BelongsToMany/Morph） | ✅ |
//! | `request` | `$this->request->post/get` | ✅ |
//! | `response` | `renderJson/renderSuccess/renderError` | ✅ |
//! | `middleware` | CORS/Auth/Log/RateLimit/Trace | ✅ |
//! | `guard` | NestJS Guard + Spring Security（sz-rust 自研） | ✅ |
//! | `hooks` | think-orm Model 钩子（HookDispatcher 16 事件） | ✅ |
//! | `multi_app` | `auto_multi_app` | ✅ |
//! | `health` | 健康检查端点（K8s liveness/readiness） | ✅ |
//! | `static_files` | 静态文件路由（`tower-http::ServeDir`） | ✅ |
//! | `error_handler` | 404/500 标准化 JSON 响应 | ✅ |
//! | `h2` | HTTP/2 + TLS（`think-swoole` SSL） | ✅ |
//! | `routing` | 三层路由机制（属性宏/配置式/约定式） | ✅ |
//! | `addons` | `addons/` 插件 | ✅ 重导出 `sz-rust-addons-loader` |
//! | `router` | `with_route` | ✅ |
//! | `container` | `app()` 容器 | ✅ |
//! | `error` | `BaseException` | ✅ |
//! | `macros` | `compact()` | ✅ 重导出 `sz-rust-macros` |
//! | `config` | `config/app.php` / `database.php` | ✅ |
//! | `log` | `think-logger` | ✅ |
//! | `server` | `think-swoole` / `think-worker` 启动入口 | ✅ |
//! | `validate` | `think\Validate` 数据验证器 | ✅ |
//! | `upload` | `think\File` + `think\file\UploadedFile` 文件上传 | ✅ |
//! | `cache` | `think\facade\Cache` 缓存 facade | ✅ |
//! | `session` | `think\facade\Session` 会话管理（SessionStore trait + MemorySessionStore） | ✅ |
//! | `cookie` | `think\Cookie` Cookie 管理（CookieJar + CookieOptions） | ✅ |
//! | `event` | `think\Event` 事件系统（Listener/Subscriber/Observer） | ✅ |
//! | `env` | `think\facade\Env` 环境变量管理 | ✅ |
//! | `i18n` | `think\facade\Lang` 多语言国际化 | ✅ |
//! | `mail` | `think\facade\Mail` 邮件抽象（Mailer trait + MemoryMailer） | ✅ |
//! | `notify` | `think\facade\Notify` 通知抽象（Notifier trait + MemoryNotifier + SlackNotifier） | ✅ |
//! | `oauth` | Laravel Socialite OAuth2 客户端（OAuth2Provider trait + GenericOAuth2Provider） | ✅ |
//! | `pay` | `yansongda/pay` 支付聚合（PayProvider trait + MemoryPayProvider + PayHttpTransport） | ✅ |
//! | `migration_history` | `think migrate` 迁移历史表（多方言 DDL + CRUD SQL 生成） | ✅ |
//! | `api_version` | API 版本管理（URL/Header/Query 多策略协商） | ✅ |
//! | `cache_warmer` | 缓存预热管道（部署/启动时预热，串行/并行+超时控制） | ✅ |
//! | `debug_page` | Whoops-style 调试页（开发环境 HTML + 生产环境简洁页） | ✅ |
//! | `openapi` | OpenAPI 3.0.3 规范构建器 + Swagger UI / Redoc 渲染 | ✅ |
//! | `qr_code` | `endroid/qr-code` 二维码生成（PNG/SVG/矩阵） | ✅ |
//! | `wechat` | `overtrue/wechat` / `EasyWeChat` 微信 SDK（公众号/小程序/开放平台/企业微信） | ✅ |
//! | `gateway` | `GatewayWorker\Gateway` WebSocket 客户端管理（Gateway API 抽象 + GatewayTransport trait + MemoryGatewayTransport） | ✅ |

#![forbid(unsafe_code)]
// v0.2.0：启用 missing_docs 警告，要求所有公开项必须有文档注释
#![warn(missing_docs)]
// 文档构建时将 missing_docs 作为错误（CI 中 RUSTDOCFLAGS="-D warnings" 会强制）
#![cfg_attr(doctest, warn(missing_docs))]

pub mod addons;
pub mod api_version;
pub mod cache;
pub mod cache_warmer;
pub mod config;
pub mod container;
pub mod controller;
pub mod cookie;
pub mod debug_page;
pub mod env;
pub mod error;
pub mod error_handler;
pub mod event;
pub mod gateway;
pub mod guard;
pub mod h2;
pub mod health;
pub mod hooks;
pub mod i18n;
pub mod log;
pub mod macros;
pub mod mail;
pub mod middleware;
pub mod migration_history;
pub mod model;
pub mod multi_app;
pub mod notify;
pub mod oauth;
pub mod openapi;
pub mod orm;
pub mod pay;
pub mod qr_code;
pub mod relation;
pub mod request;
pub mod response;
pub mod router;
pub mod routing;
pub mod runtime;
pub mod schema_cache;
pub mod seed;
pub mod server;
pub mod session;
pub mod static_files;
pub mod upload;
pub mod validate;
pub mod view;
pub mod websocket_route;
pub mod wechat;

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
pub use crate::orm::sql_string;

/// 编译时 SQL 校验 + 可选真实 DB 验证宏 — 复用自 `sz-orm-macros`
///
/// 与 [`sql_string!`] 行为一致，额外支持在 `db-verify` feature 启用且
/// `SZ_ORM_QUERY_VERIFY=1` 环境变量设置时，连接 `DATABASE_URL` 指向的
/// 数据库执行 `EXPLAIN` 进行真实 schema 校验。
pub use crate::orm::query;

// ============================================================================
// 运行时 SQL 校验 — 复用自 sz-orm-sql-validator
// ============================================================================

pub use crate::orm::{
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
