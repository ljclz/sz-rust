// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 管理监控数据采集（Admin Monitor API）
//!
//! 提供系统信息、数据库连接池、Redis 状态的采集能力，
//! 供 `/api/admin/*` 端点使用。
//!
//! ## 模块划分
//!
//! - [`sysinfo_collector`]：CPU / 内存 / 磁盘 / 负载 / 进程启动时间
//! - [`db_pool_collector`]：数据库连接池实时状态（需应用层实现 [`db_pool_collector::DbPoolStats`]）
//! - [`redis_collector`]：Redis 服务器信息（PING 探活降级）
//!
//! ## Feature 门控
//!
//! 本模块由 `admin` feature 门控，默认不编译。
//! 启用后额外依赖 `sysinfo` 与 `redis` crate。

#[cfg(feature = "admin")]
pub mod db_pool_collector;
#[cfg(feature = "admin")]
pub mod redis_collector;
#[cfg(feature = "admin")]
pub mod sysinfo_collector;

#[cfg(feature = "admin")]
pub use db_pool_collector::{DbPoolStats, PoolInfo};
#[cfg(feature = "admin")]
pub use redis_collector::{RedisCollectError, RedisInfo, RedisStats, RedisVariable};
#[cfg(feature = "admin")]
pub use sysinfo_collector::{collect_server_info, LoadAvg, ServerInfo};
