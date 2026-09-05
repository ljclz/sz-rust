// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据库连接池采集器
//!
//! 提供连接池实时状态的采集能力，供 `GET /api/admin/db/pool` 端点使用。
//!
//! ## 设计说明
//!
//! `sz-orm-core` 的 `Pool` 类型封装了内部连接池实现，不直接暴露 `size()` / `available()` 等方法。
//! 本模块通过 [`DbPoolStats`] trait 将应用层的 `Pool` 适配为统一的采集接口：
//!
//! - 应用层（`sz-rust-sz300`）在注入 `AppState` 时同时提供一个实现了 `DbPoolStats` 的适配器
//! - 采集器调用 `stats()` 获取 `PoolInfo` 并序列化返回
//!
//! ## 字段说明
//!
//! | 字段 | 说明 | 来源 |
//! |------|------|------|
//! | `active` | 当前活跃连接数（已借出） | `Pool::status().active` |
//! | `idle` | 当前空闲连接数 | `Pool::status().idle` |
//! | `max` | 最大连接数上限 | `Pool::max_size()` |
//! | `usage_percent` | 连接池使用率（active/max × 100） | 计算值 |

use serde::Serialize;

/// 连接池实时信息（`GET /api/admin/db/pool` 响应体 data 字段）
#[derive(Debug, Clone, Serialize)]
pub struct PoolInfo {
    /// 当前活跃连接数（已从池中借出）
    pub active: u32,
    /// 当前空闲连接数（等待被借出）
    pub idle: u32,
    /// 最大连接数上限
    pub max: u32,
    /// 连接池使用率（0-100）
    pub usage_percent: f32,
}

/// 连接池状态采集 trait
///
/// 由应用层实现，将具体的 `Pool` 类型适配为采集器可理解的接口。
///
/// ## 实现要求
///
/// - `stats()` 应为 O(1) 操作（读取原子计数器），不应阻塞
/// - 实现必须 `Send + Sync + 'static`，可安全跨线程共享
pub trait DbPoolStats: Send + Sync {
    /// 采集当前连接池状态
    fn stats(&self) -> PoolInfo;
}

// ============================================================================
// 单元测试（mock 实现验证序列化与计算逻辑）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟连接池（用于单元测试）
    struct MockPool {
        active: u32,
        idle: u32,
        max: u32,
    }

    impl DbPoolStats for MockPool {
        fn stats(&self) -> PoolInfo {
            let usage = if self.max == 0 {
                0.0
            } else {
                self.active as f32 / self.max as f32 * 100.0
            };
            PoolInfo {
                active: self.active,
                idle: self.idle,
                max: self.max,
                usage_percent: usage,
            }
        }
    }

    #[test]
    fn test_pool_info_serializes_correctly() {
        let pool = MockPool {
            active: 5,
            idle: 3,
            max: 10,
        };
        let info = pool.stats();
        let json = serde_json::to_string(&info).unwrap();

        assert!(json.contains("\"active\":5"));
        assert!(json.contains("\"idle\":3"));
        assert!(json.contains("\"max\":10"));
        assert!(json.contains("\"usage_percent\":50"));
    }

    #[test]
    fn test_usage_percent_calculation() {
        let pool = MockPool {
            active: 7,
            idle: 3,
            max: 10,
        };
        let info = pool.stats();
        assert!((info.usage_percent - 70.0).abs() < 0.01);
    }

    #[test]
    fn test_usage_percent_zero_max_guard() {
        let pool = MockPool {
            active: 5,
            idle: 0,
            max: 0,
        };
        let info = pool.stats();
        assert_eq!(info.usage_percent, 0.0);
    }

    #[test]
    fn test_pool_info_all_fields_present() {
        let pool = MockPool {
            active: 2,
            idle: 8,
            max: 10,
        };
        let info = pool.stats();
        let json = serde_json::to_value(&info).unwrap();

        assert_eq!(json["active"], 2);
        assert_eq!(json["idle"], 8);
        assert_eq!(json["max"], 10);
        assert!(json["usage_percent"].is_f64());
    }
}
