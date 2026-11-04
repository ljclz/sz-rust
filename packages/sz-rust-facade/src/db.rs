// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Db 静态门面
//!
//! 对齐 PHP `think\facade\Db`，委托全局 `sz_orm_core::Pool` 单例。

use std::sync::{Arc, OnceLock};

use sz_rust_orm_facade::Pool;

use crate::FacadeError;

static DB_POOL: OnceLock<Arc<Pool>> = OnceLock::new();

/// Db 静态门面（对齐 PHP `think\facade\Db`）
///
/// 委托全局 `OnceCell<Arc<Pool>>` 单例。需调用 [`Db::init`] 初始化连接池。
pub struct Db;

impl Db {
    /// 初始化全局数据库连接池
    ///
    /// 必须在调用其他方法前调用。重复调用不会覆盖已有实例。
    pub fn init(pool: Arc<Pool>) {
        let _ = DB_POOL.set(pool);
    }

    /// 获取全局连接池引用
    fn pool() -> Result<&'static Arc<Pool>, FacadeError> {
        DB_POOL
            .get()
            .ok_or(FacadeError::NotInitialized("Db::init(pool)"))
    }

    /// 获取连接池引用（公开 API，供下游直接使用 Pool）
    pub fn pool_ref() -> Result<&'static Arc<Pool>, FacadeError> {
        Self::pool()
    }

    /// 是否已初始化
    pub fn is_initialized() -> bool {
        DB_POOL.get().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_not_initialized() {
        assert!(!Db::is_initialized());
        let result = Db::pool();
        assert!(result.is_err());
    }
}
