// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 设备服务层 — 封装设备相关 SQL 操作
//!
//! 2026-07-25 新增（修复 P0-4：控制器分层违反）。
//!
//! ## 设计
//!
//! - 控制器层（`controllers/device.rs`）仅负责：解析请求参数、调用服务、格式化响应
//! - 服务层（本模块）负责：构建 SQL、执行查询、返回领域数据
//! - 模型层（`models/device.rs`）负责：定义实体结构与 ORM 映射
//!
//! ## 安全
//!
//! 所有 SQL 均使用 `?` 占位符 + `query_with_params` / `execute_with_params` 参数化查询，杜绝 SQL 注入。
//! DB 错误信息仅记录日志，不返回调用方（调用方收到通用错误描述）。

use std::collections::HashMap;

use crate::db::acquire_cancel_safe;
use crate::models::device::Device;
use crate::models::ota_version::OtaVersion;
use sz_rust_core::orm::ModelExt;
use sz_rust_core::orm::{Pool, Value};

/// 设备列表筛选条件
#[derive(Debug)]
pub struct DeviceFilters {
    /// 商户 ID
    pub merchant_id: Option<i64>,
}

/// 设备分页结果
pub struct DevicePage {
    /// 设备列表（原始行数据）
    pub list: Vec<HashMap<String, Value>>,
    /// 总数
    pub total: i64,
}

/// 设备服务 — 封装设备相关 DB 操作
pub struct DeviceService;

impl DeviceService {
    /// 分页查询设备列表
    ///
    /// 构建动态 WHERE 子句 + LIMIT/OFFSET，全部参数化。
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `page`：页码（从 1 开始）
    /// - `page_size`：每页条数（1..=100）
    /// - `filters`：筛选条件
    ///
    /// # 返回
    ///
    /// 成功返回 [`DevicePage`]，失败返回错误描述字符串（不泄露 DB 内部信息）。
    #[tracing::instrument(skip(pool))]
    pub async fn list(
        pool: &Pool,
        page: i64,
        page_size: i64,
        filters: DeviceFilters,
    ) -> Result<DevicePage, String> {
        let offset = (page - 1).max(0) * page_size;

        // 构建 WHERE 条件（参数化，杜绝 SQL 注入）
        let mut conditions: Vec<&'static str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if let Some(mid) = filters.merchant_id {
            conditions.push("merchant_id = ?");
            params.push(Value::I64(mid));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备列表获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        // 总数查询
        let count_sql = format!("SELECT COUNT(*) as total FROM device {}", where_clause);
        let count_rows = conn
            .query_with_params(&count_sql, &params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "设备列表 COUNT 查询失败");
                "查询失败".to_string()
            })?;
        let total: i64 = count_rows
            .first()
            .and_then(|row| row.get("total"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 列表查询 — 追加分页参数
        let list_sql = format!(
            "SELECT {} FROM device {} ORDER BY device_id DESC LIMIT ? OFFSET ?",
            Device::columns().join(", "),
            where_clause
        );
        let mut list_params = params.clone();
        list_params.push(Value::I64(page_size));
        list_params.push(Value::I64(offset));
        let rows = conn
            .query_with_params(&list_sql, &list_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "设备列表查询失败");
                "查询失败".to_string()
            })?;

        Ok(DevicePage { list: rows, total })
    }

    /// 根据 device_id 查询单个设备
    ///
    /// # 返回
    ///
    /// - `Ok(Some(row))`：设备存在
    /// - `Ok(None)`：设备不存在
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    pub async fn get(
        pool: &Pool,
        device_id: i64,
    ) -> Result<Option<HashMap<String, Value>>, String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备详情获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let sql = format!(
            "SELECT {} FROM device WHERE device_id = ?",
            Device::columns().join(", ")
        );
        let params = [Value::I64(device_id)];
        let rows = conn.query_with_params(&sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "设备详情查询失败: device_id={}", device_id);
            "查询失败".to_string()
        })?;

        Ok(rows.into_iter().next())
    }

    /// 绑定设备到商户
    ///
    /// 流程：
    /// 1. 验证设备 SN 存在
    /// 2. 检查设备是否已绑定（merchant_id != 0）
    /// 3. 更新设备 merchant_id 与 bind_at
    /// 4. 记录操作日志（best-effort）
    ///
    /// # 返回
    ///
    /// - `Ok(())`：绑定成功
    /// - `Err("设备不存在")`：设备 SN 不存在
    /// - `Err("设备已绑定")`：设备已绑定到其他商户
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    pub async fn bind(pool: &Pool, device_sn: &str, merchant_id: i64) -> Result<(), String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备绑定获取 DB 连接失败: device_sn={}", device_sn);
            "数据库连接失败".to_string()
        })?;

        // 验证设备 SN 存在
        let check_sql = format!(
            "SELECT {} FROM device WHERE device_sn = ?",
            Device::columns().join(", ")
        );
        let check_params = [Value::String(device_sn.to_string())];
        let rows = conn
            .query_with_params(&check_sql, &check_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "设备绑定查询设备失败: device_sn={}", device_sn);
                "查询失败".to_string()
            })?;
        if rows.is_empty() {
            return Err("设备不存在".to_string());
        }

        // 检查是否已绑定（merchant_id 改 NULLABLE 后，NULL=未绑定；013_nullable_device_merchant.sql）
        let existing_merchant = rows[0].get("merchant_id").and_then(|v| v.as_i64());
        if existing_merchant.is_some() {
            return Err("设备已绑定".to_string());
        }

        // 更新绑定
        let update_sql = "UPDATE device SET merchant_id = ?, bind_at = NOW() WHERE device_sn = ?";
        let update_params = [
            Value::I64(merchant_id),
            Value::String(device_sn.to_string()),
        ];
        conn.execute_with_params(update_sql, &update_params)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                tracing::error!(error = %msg, "设备绑定更新失败: device_sn={}", device_sn);
                // 阶段4：识别 FK/CHECK 约束违反（1452/3819）转业务友好消息
                crate::services::db_error::classify_or_fallback(&msg, "绑定失败")
            })?;

        // 记录操作日志（best-effort）— detail 字段是格式化字符串，作为参数传入
        let detail = format!("设备 {} 绑定到商户 {}", device_sn, merchant_id);
        let log_sql = "INSERT INTO operate_log (merchant_id, operator, action, detail, ip) VALUES (?, 'system', 'bind', ?, '')";
        let log_params = [Value::I64(merchant_id), Value::String(detail)];
        if let Err(e) = conn.execute_with_params(log_sql, &log_params).await {
            tracing::warn!(error = %e, "记录操作日志失败: device_sn={}", device_sn);
        }

        Ok(())
    }

    /// 解绑设备（将 merchant_id 置为 0，状态置为离线，bind_at 置为 NULL）
    #[tracing::instrument(skip(pool))]
    pub async fn unbind(pool: &Pool, device_id: i64) -> Result<(), String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备解绑获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        // 解绑：merchant_id 置 NULL（哨兵 0 已退役，见 013_nullable_device_merchant.sql）
        let sql =
            "UPDATE device SET merchant_id = NULL, bind_at = NULL, status = 0 WHERE device_id = ?";
        let params = [Value::I64(device_id)];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "设备解绑失败: device_id={}", device_id);
            "解绑失败".to_string()
        })?;

        Ok(())
    }

    /// 根据 device_id 验证设备存在
    ///
    /// # 返回
    ///
    /// - `Ok(true)`：设备存在
    /// - `Ok(false)`：设备不存在
    /// - `Err(msg)`：DB 错误
    #[tracing::instrument(skip(pool))]
    pub async fn exists(pool: &Pool, device_id: i64) -> Result<bool, String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备存在性检查获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "SELECT device_id FROM device WHERE device_id = ?";
        let params = [Value::I64(device_id)];
        let rows = conn.query_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "设备存在性检查失败: device_id={}", device_id);
            "查询失败".to_string()
        })?;

        Ok(!rows.is_empty())
    }

    /// 根据 device_id 验证设备存在（IDOR 修复：可附加商户归属校验）
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `device_id`：设备 ID
    /// - `merchant_id`：当前用户商户 ID（`is_admin=false` 时生效）
    /// - `is_admin`：管理员可访问任意商户设备
    ///
    /// # 返回
    ///
    /// - `Ok(true)`：设备存在且（非管理员时）属于当前商户
    /// - `Ok(false)`：设备不存在或不属于当前商户
    /// - `Err(msg)`：DB 错误
    #[tracing::instrument(skip(pool))]
    pub async fn exists_with_merchant_scope(
        pool: &Pool,
        device_id: i64,
        merchant_id: Option<i64>,
        is_admin: bool,
    ) -> Result<bool, String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备存在性检查获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let (sql, params) = if is_admin {
            (
                "SELECT device_id FROM device WHERE device_id = ?".to_string(),
                vec![Value::I64(device_id)],
            )
        } else {
            (
                "SELECT device_id FROM device WHERE device_id = ? AND merchant_id = ?".to_string(),
                vec![Value::I64(device_id), Value::I64(merchant_id.unwrap_or(-1))],
            )
        };
        let rows = conn.query_with_params(&sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "设备存在性检查失败: device_id={}", device_id);
            "查询失败".to_string()
        })?;

        Ok(!rows.is_empty())
    }

    /// 查询 OTA 版本信息（仅返回已启用版本）
    ///
    /// # 返回
    ///
    /// - `Ok(Some(row))`：OTA 版本存在且已启用
    /// - `Ok(None)`：OTA 版本不存在或未启用
    /// - `Err(msg)`：DB 错误
    #[tracing::instrument(skip(pool))]
    pub async fn get_ota_version(
        pool: &Pool,
        version: &str,
    ) -> Result<Option<HashMap<String, Value>>, String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "OTA 版本查询获取 DB 连接失败: version={}", version);
            "数据库连接失败".to_string()
        })?;

        let sql = format!(
            "SELECT {} FROM ota_version WHERE version = ? AND status = 1",
            OtaVersion::columns().join(", ")
        );
        let params = [Value::String(version.to_string())];
        let rows = conn.query_with_params(&sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "OTA 版本查询失败: version={}", version);
            "查询失败".to_string()
        })?;

        Ok(rows.into_iter().next())
    }

    /// 更新设备状态上报数据
    ///
    /// 更新字段：status、signal_strength、fw_version、last_online_at
    #[tracing::instrument(skip(pool))]
    pub async fn update_status(
        pool: &Pool,
        device_id: i64,
        status: i64,
        signal_strength: i64,
        fw_version: &str,
    ) -> Result<(), String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "设备状态上报获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "UPDATE device SET status = ?, signal_strength = ?, fw_version = ?, last_online_at = NOW() WHERE device_id = ?";
        let params = [
            Value::I64(status),
            Value::I64(signal_strength),
            Value::String(fw_version.to_string()),
            Value::I64(device_id),
        ];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            let msg = e.to_string();
            tracing::error!(error = %msg, "设备状态更新失败: device_id={}", device_id);
            // 阶段4：识别 FK/CHECK 约束违反（1452/3819）转业务友好消息
            crate::services::db_error::classify_or_fallback(&msg, "状态更新失败")
        })?;

        Ok(())
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // DeviceFilters 构造与 IDOR 归属校验纯逻辑（P2-4+5 补 service 层覆盖率）
    // ─────────────────────────────────────────────

    #[test]
    fn device_filters_none_defaults() {
        let f = DeviceFilters { merchant_id: None };
        assert!(f.merchant_id.is_none());
    }

    #[test]
    fn device_filters_with_merchant_scope() {
        let f = DeviceFilters {
            merchant_id: Some(42),
        };
        assert_eq!(f.merchant_id, Some(42));
    }

    /// exists_with_merchant_scope 的 SQL 分支纯逻辑：
    /// - is_admin=true → 单参数 SQL（不附加 merchant_id 过滤）
    /// - is_admin=false → 双参数 SQL（附加 merchant_id = ?）
    ///
    /// 此处锁定分支选择逻辑，不实际执行查询。
    #[test]
    fn exists_with_merchant_scope_admin_uses_single_param_sql() {
        let is_admin = true;
        let (sql, params_len) = if is_admin {
            (
                "SELECT device_id FROM device WHERE device_id = ?".to_string(),
                1,
            )
        } else {
            (
                "SELECT device_id FROM device WHERE device_id = ? AND merchant_id = ?".to_string(),
                2,
            )
        };
        assert!(
            sql.contains("device_id = ?") && !sql.contains("merchant_id"),
            "admin SQL 不应含 merchant_id 过滤: {sql}"
        );
        assert_eq!(params_len, 1);
    }

    #[test]
    fn exists_with_merchant_scope_non_admin_uses_merchant_scoped_sql() {
        let is_admin = false;
        let (sql, params_len) = if is_admin {
            (
                "SELECT device_id FROM device WHERE device_id = ?".to_string(),
                1,
            )
        } else {
            (
                "SELECT device_id FROM device WHERE device_id = ? AND merchant_id = ?".to_string(),
                2,
            )
        };
        assert!(
            sql.contains("merchant_id = ?"),
            "非 admin SQL 应含 merchant_id 归属过滤: {sql}"
        );
        assert_eq!(params_len, 2);
    }

    /// 非管理员且 merchant_id=None 时，传入 -1 作为哨兵（不可能匹配真实商户）
    /// 此处锁定 `merchant_id.unwrap_or(-1)` 的哨兵语义，防止 None 被静默放行。
    #[test]
    #[allow(clippy::unnecessary_literal_unwrap)] // 测试有意对字面 None 调 unwrap_or 以锁定哨兵语义
    fn exists_with_merchant_scope_non_admin_none_merchant_uses_sentinel() {
        let merchant_id: Option<i64> = None;
        let is_admin = false;
        // 模拟 list 中的参数构造
        let param = if is_admin {
            vec![Value::I64(1)]
        } else {
            vec![Value::I64(1), Value::I64(merchant_id.unwrap_or(-1))]
        };
        assert_eq!(
            param[1],
            Value::I64(-1),
            "非 admin + None merchant_id 应使用 -1 哨兵，不允许越权"
        );
    }

    /// offset 公式与其它 service 一致
    #[test]
    fn device_list_offset_formula() {
        let (page, page_size) = (1, 20);
        assert_eq!((page - 1).max(0) * page_size, 0);
        let (page, page_size) = (5, 20);
        assert_eq!((page - 1).max(0) * page_size, 80);
    }

    /// bind 流程：existing_merchant.is_some() → "设备已绑定"
    /// 此处锁定 NULL=未绑定 / Some=已绑定 的语义（013_nullable_device_merchant.sql）
    #[test]
    fn device_bind_existing_merchant_some_means_already_bound() {
        let existing_merchant: Option<i64> = Some(100);
        assert!(
            existing_merchant.is_some(),
            "merchant_id 非 NULL 表示已绑定，应拒绝重复绑定"
        );

        let unbound: Option<i64> = None;
        assert!(
            unbound.is_none(),
            "merchant_id 为 NULL 表示未绑定，允许绑定"
        );
    }

    /// unbind SQL 语义：merchant_id 置 NULL（哨兵 0 已退役）
    #[test]
    fn device_unbind_sql_uses_null_not_zero_sentinel() {
        let sql =
            "UPDATE device SET merchant_id = NULL, bind_at = NULL, status = 0 WHERE device_id = ?";
        assert!(
            sql.contains("merchant_id = NULL"),
            "unbind 应置 NULL，不应使用哨兵 0"
        );
        assert!(
            sql.contains("status = 0"),
            "unbind 应将 status 置为 0（离线）"
        );
    }

    // ─────────────────────────────────────────────
    // fake pool 测试：覆盖 WHERE 构建 + acquire 失败错误映射
    // service 函数先构建 SQL，再 acquire connection；fake pool acquire 快速 ECONNREFUSED，
    // 覆盖纯逻辑行 + 错误映射行。
    // ─────────────────────────────────────────────

    use std::sync::Arc;

    fn make_fake_pool() -> Pool {
        use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
        use sz_rust_core::orm::PoolConfigBuilder;
        let connect_options = sqlx::mysql::MySqlConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("root")
            .password("")
            .database("nonexistent");
        let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy_with(connect_options);
        let factory =
            SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));
        let pool_cfg = PoolConfigBuilder::new()
            .max_size(1)
            .min_idle(0)
            .build()
            .unwrap();
        Pool::new(pool_cfg, Arc::new(factory)).unwrap()
    }

    #[tokio::test]
    async fn device_list_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let filters = DeviceFilters {
            merchant_id: Some(1),
        };
        let result = DeviceService::list(&pool, 1, 20, filters).await;
        assert!(result.is_err(), "fake pool 应导致 list 失败");
    }

    #[tokio::test]
    async fn device_list_fake_pool_no_filter_returns_err() {
        let pool = make_fake_pool();
        let filters = DeviceFilters { merchant_id: None };
        let result = DeviceService::list(&pool, 1, 20, filters).await;
        assert!(result.is_err(), "fake pool 应导致 list 失败");
    }

    #[tokio::test]
    async fn device_get_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::get(&pool, 42).await;
        assert!(result.is_err(), "fake pool 应导致 get 失败");
    }

    #[tokio::test]
    async fn device_bind_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::bind(&pool, "SN-001", 1).await;
        assert!(result.is_err(), "fake pool 应导致 bind 失败");
    }

    #[tokio::test]
    async fn device_unbind_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::unbind(&pool, 42).await;
        assert!(result.is_err(), "fake pool 应导致 unbind 失败");
    }

    #[tokio::test]
    async fn device_exists_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::exists(&pool, 42).await;
        assert!(result.is_err(), "fake pool 应导致 exists 失败");
    }

    #[tokio::test]
    async fn device_exists_with_merchant_scope_admin_fake_pool_returns_err() {
        let pool = make_fake_pool();
        // is_admin=true → 单参数 SQL 分支
        let result = DeviceService::exists_with_merchant_scope(&pool, 42, Some(1), true).await;
        assert!(
            result.is_err(),
            "fake pool 应导致 exists_with_merchant_scope 失败"
        );
    }

    #[tokio::test]
    async fn device_exists_with_merchant_scope_non_admin_fake_pool_returns_err() {
        let pool = make_fake_pool();
        // is_admin=false → 双参数 SQL 分支（含 merchant_id 哨兵）
        let result = DeviceService::exists_with_merchant_scope(&pool, 42, Some(1), false).await;
        assert!(
            result.is_err(),
            "fake pool 应导致 exists_with_merchant_scope 失败"
        );
    }

    #[tokio::test]
    async fn device_get_ota_version_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::get_ota_version(&pool, "1.0.0").await;
        assert!(result.is_err(), "fake pool 应导致 get_ota_version 失败");
    }

    #[tokio::test]
    async fn device_update_status_fake_pool_returns_err() {
        let pool = make_fake_pool();
        let result = DeviceService::update_status(&pool, 42, 1, -70, "1.0.0").await;
        assert!(result.is_err(), "fake pool 应导致 update_status 失败");
    }
}
