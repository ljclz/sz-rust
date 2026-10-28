// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 文件上传/分片（multipart + 分片上传 + 断点续传 + 存储抽象）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate（根包 `api-upload`）控制，默认不启用。
//!
//! # 组成
//!
//! - [`engine`]：完整上传引擎 re-export（来自 `sz-rust-infra-facade::upload`）
//! - [`chunk`]：分片上传 + 断点续传引擎（sha256 校验 + 磁盘持久化 + 幂等去重）
//! - [`multipart`]：Multipart 表单文件上传（spec §5.23 规则 1）
//! - [`vscan`]：病毒扫描钩子（spec §5.23 规则 4）
//! - [`storage`]：存储抽象（Local/S3/MinIO/OSS，spec §5.23 规则 5）
//! - [`error`]：本 crate 错误类型（错误码 17150 起）

#![forbid(unsafe_code)]

pub mod chunk;
pub mod error;
pub mod multipart;
pub mod storage;
pub mod vscan;

pub use error::*;

/// 标准上传引擎（对齐 `think\File` / `think\file\UploadedFile`）
pub use sz_rust_infra_facade::upload as engine;
