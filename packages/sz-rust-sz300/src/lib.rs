//! 鲜视达 SZ-300 后端服务 — 端到端集成示例
//!
//! 基于 sz-rust-core 框架实现的完整业务应用，对齐 PHP `鲜视达` 项目后端。
//!
//! ## 模块结构
//!
//! | 模块 | 职责 |
//! |------|------|
//! | [`config`] | 环境变量驱动的配置加载 |
//! | [`controllers`] | HTTP 路由处理器（对齐 PHP controller） |
//! | [`db`] | 数据库连接池初始化（MySQL + PostgreSQL） |
//! | [`middleware`] | 应用中间件（JWT 认证、日志等） |
//! | [`models`] | 数据模型（对齐 PHP model） |
//! | [`router`] | 路由注册 |
//! | [`services`] | 业务服务层（MQTT、认证、文件等） |
//! | [`state`] | 应用共享状态 |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// 环境变量驱动的配置加载
pub mod config;
/// HTTP 路由处理器（对齐 PHP controller）
pub mod controllers;
/// 数据库连接池初始化（MySQL + PostgreSQL）
pub mod db;
pub mod i18n_error;
/// 应用中间件（JWT 认证、日志等）
pub mod middleware;
/// 数据模型（对齐 PHP model）
pub mod models;
/// OpenAPI 规范构建与 API 文档端点
pub mod openapi;
/// 路由注册
pub mod router;
/// 业务服务层（MQTT、认证、文件等）
pub mod services;
/// 应用共享状态
pub mod state;
