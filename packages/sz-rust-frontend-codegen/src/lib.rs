// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 前端代码生成器 — 根据 ORM 模型自动生成 Vue/React 组件、路由、权限、API 客户端
//!
//! ## 功能概述
//!
//! - 解析 Rust ORM 模型（`#[derive(Model)]`）提取字段、关系、验证规则元信息
//! - 根据模型元信息生成 Vue 3 / React 18 组件（列表/详情/新建/编辑 4 页面）
//! - 根据后端路由定义生成前端路由（Vue Router / React Router v6）
//! - 根据权限定义生成前端权限控制（路由守卫、v-permission 指令、usePermission 组合式函数）
//! - 根据 OpenAPI spec 生成 API 客户端（请求函数 + TypeScript 类型定义）
//! - 支持自定义 Tera 模板覆盖内置模板
//! - CLI 集成：`sz-rust make:frontend`
//!
//! ## 模块结构
//!
//! | 模块 | 功能 |
//! |------|------|
//! | `error` | 错误类型 `FrontendCodegenError`（17 变体） |
//! | `config` | 生成配置 `GenerationConfig` 与配置文件加载 |
//! | `report` | 生成报告 `GenerationReport` 与 CLI 表格输出 |
//! | `model_parser` | ORM 模型解析器（syn AST 解析） |
//! | `metadata` | 模型元信息结构（Field/Relation/Validation/Model） |
//! | `ui_adapter` | UI 库适配器（ElementPlus / AntDesignVue） |
//! | `template_engine` | 模板引擎封装（Tera + 自定义过滤器） |
//! | `filters` | Tera 自定义过滤器（7 个） |
//! | `path_guard` | 路径穿越防护 |
//! | `file_writer` | 原子文件写入与覆盖策略 |
//! | `generators` | 组件生成器（Vue/React/Route/Permission/ApiClient） |
//! | `service` | 核心服务编排 `CodegenService` |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod file_writer;
pub mod filters;
pub mod generators;
pub mod metadata;
pub mod model_parser;
pub mod path_guard;
pub mod report;
pub mod service;
pub mod template_engine;
pub mod ui_adapter;

pub use config::{Framework, GenerationConfig, OverrideStrategy, UiLibrary};
pub use error::FrontendCodegenError;
pub use metadata::ModelMetadata;
pub use report::GenerationReport;
pub use service::CodegenService;
