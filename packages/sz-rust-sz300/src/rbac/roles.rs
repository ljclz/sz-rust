// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz300 角色定义与权限矩阵
//!
//! ## 角色体系
//!
//! | 角色 | 说明 | 继承 |
//! |------|------|------|
//! | `admin` | 超级管理员，拥有全部权限 | merchant:write, merchant:read, device |
//! | `merchant:write` | 商户写权限（CRUD 商户/商品/订单） | merchant:read |
//! | `merchant:read` | 商户读权限（查询商户/商品/订单） | — |
//! | `device` | 设备管理权限 | — |
//!
//! ## 权限矩阵
//!
//! | 资源 \ 角色 | admin | merchant:write | merchant:read | device |
//! |-------------|-------|----------------|---------------|--------|
//! | merchant:create | ✓ | ✓ | ✗ | ✗ |
//! | merchant:read | ✓ | ✓ | ✓ | ✗ |
//! | merchant:update | ✓ | ✓ | ✗ | ✗ |
//! | merchant:delete | ✓ | ✓ | ✗ | ✗ |
//! | product:create | ✓ | ✓ | ✗ | ✗ |
//! | product:read | ✓ | ✓ | ✓ | ✗ |
//! | product:update | ✓ | ✓ | ✗ | ✗ |
//! | product:delete | ✓ | ✓ | ✗ | ✗ |
//! | device:bind | ✓ | ✗ | ✗ | ✓ |
//! | device:read | ✓ | ✓ | ✓ | ✓ |
//! | device:ota | ✓ | ✗ | ✗ | ✓ |
//! | order:create | ✓ | ✓ | ✗ | ✗ |
//! | order:read | ✓ | ✓ | ✓ | ✗ |
//! | file:upload | ✓ | ✓ | ✗ | ✗ |

use sz_rust_auth_facade::{RbacEngine, Resource, RoleId};

/// 角色常量
pub mod role {
    use super::RoleId;

    /// 超级管理员
    pub fn admin() -> RoleId {
        RoleId::new("admin")
    }
    /// 商户写权限
    pub fn merchant_write() -> RoleId {
        RoleId::new("merchant:write")
    }
    /// 商户读权限
    pub fn merchant_read() -> RoleId {
        RoleId::new("merchant:read")
    }
    /// 设备管理
    pub fn device() -> RoleId {
        RoleId::new("device")
    }
}

/// 资源常量（resource_type, operation）
pub mod perm {
    use super::Resource;

    /// 商户资源权限
    pub fn merchant(op: &str) -> Resource {
        Resource::new("merchant", "*", op)
    }
    /// 商品资源权限
    pub fn product(op: &str) -> Resource {
        Resource::new("product", "*", op)
    }
    /// 设备资源权限
    pub fn device(op: &str) -> Resource {
        Resource::new("device", "*", op)
    }
    /// 订单资源权限
    pub fn order(op: &str) -> Resource {
        Resource::new("order", "*", op)
    }
    /// 文件资源权限
    pub fn file(op: &str) -> Resource {
        Resource::new("file", "*", op)
    }
}

/// 创建并初始化 sz300 默认 RBAC 引擎
///
/// 填充角色继承关系与权限矩阵，`max_depth = 16` 防止循环继承。
pub fn init_rbac_engine() -> RbacEngine {
    let engine = RbacEngine::new(16);

    // 角色继承：admin → merchant:write → merchant:read
    engine.add_inheritance(role::admin(), role::merchant_write());
    engine.add_inheritance(role::merchant_write(), role::merchant_read());
    engine.add_inheritance(role::admin(), role::device());

    // merchant:read 权限
    engine.grant_permission(role::merchant_read(), perm::merchant("read"));
    engine.grant_permission(role::merchant_read(), perm::product("read"));
    engine.grant_permission(role::merchant_read(), perm::order("read"));
    engine.grant_permission(role::merchant_read(), perm::device("read"));

    // merchant:write 权限
    for op in ["create", "update", "delete"] {
        engine.grant_permission(role::merchant_write(), perm::merchant(op));
        engine.grant_permission(role::merchant_write(), perm::product(op));
    }
    engine.grant_permission(role::merchant_write(), perm::order("create"));
    engine.grant_permission(role::merchant_write(), perm::file("upload"));

    // device 权限
    for op in ["bind", "unbind", "ota", "status_report"] {
        engine.grant_permission(role::device(), perm::device(op));
    }

    // admin 直接权限（补充 device:read，其余通过继承获得）
    engine.grant_permission(role::admin(), perm::device("read"));

    engine
}
