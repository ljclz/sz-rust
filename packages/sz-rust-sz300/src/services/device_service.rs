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
    /// 构建设备列表动态 WHERE 子句 + 参数（纯函数，2026-08-16 抽取自 `list`）
    ///
    /// 返回 `(where_clause, params)`：无条件时 where_clause 为空字符串。
    /// 所有条件参数化绑定，杜绝 SQL 注入。
    pub fn build_list_where(filters: &DeviceFilters) -> (String, Vec<Value>) {
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
        (where_clause, params)
    }

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

        let (where_clause, params) = Self::build_list_where(&filters);

        let mut conn = pool.acquire().await.map_err(|e| {
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

        // 列表查询 — 追加分页参数（显式列投影，铁律：禁 SELECT *）
        let list_sql = format!(
            "SELECT device_id, merchant_id, device_sn, device_model, fw_version, status, signal_strength, bind_at, last_online_at, created_at, updated_at FROM device {} ORDER BY device_id DESC LIMIT ? OFFSET ?",
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
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "设备详情获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "SELECT device_id, merchant_id, device_sn, device_model, fw_version, status, signal_strength, bind_at, last_online_at, created_at, updated_at FROM device WHERE device_id = ?";
        let params = [Value::I64(device_id)];
        let rows = conn.query_with_params(sql, &params).await.map_err(|e| {
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
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "设备绑定获取 DB 连接失败: device_sn={}", device_sn);
            "数据库连接失败".to_string()
        })?;

        // 验证设备 SN 存在（bind 仅需 device_id/merchant_id 两列，显式投影）
        let check_sql = "SELECT device_id, merchant_id FROM device WHERE device_sn = ?";
        let check_params = [Value::String(device_sn.to_string())];
        let rows = conn
            .query_with_params(check_sql, &check_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "设备绑定查询设备失败: device_sn={}", device_sn);
                "查询失败".to_string()
            })?;
        if rows.is_empty() {
            return Err("设备不存在".to_string());
        }

        // 检查是否已绑定
        let existing_merchant = rows[0]
            .get("merchant_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if existing_merchant != 0 {
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
                tracing::error!(error = %e, "设备绑定更新失败: device_sn={}", device_sn);
                "绑定失败".to_string()
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
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "设备解绑获取 DB 连接失败: device_id={}", device_id);
            "数据库连接失败".to_string()
        })?;

        let sql =
            "UPDATE device SET merchant_id = 0, bind_at = NULL, status = 0 WHERE device_id = ?";
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
        let mut conn = pool.acquire().await.map_err(|e| {
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
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "OTA 版本查询获取 DB 连接失败: version={}", version);
            "数据库连接失败".to_string()
        })?;

        let sql = "SELECT ota_id, version, device_model, url, md5, changelog, size, forced, status, created_at FROM ota_version WHERE version = ? AND status = 1";
        let params = [Value::String(version.to_string())];
        let rows = conn.query_with_params(sql, &params).await.map_err(|e| {
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
        let mut conn = pool.acquire().await.map_err(|e| {
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
            tracing::error!(error = %e, "设备状态更新失败: device_id={}", device_id);
            "状态更新失败".to_string()
        })?;

        Ok(())
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_list_where_no_filters_returns_empty_clause() {
        let (clause, params) =
            DeviceService::build_list_where(&DeviceFilters { merchant_id: None });
        assert_eq!(clause, "");
        assert!(params.is_empty());
    }

    #[test]
    fn build_list_where_merchant_id_parameterized() {
        let (clause, params) = DeviceService::build_list_where(&DeviceFilters {
            merchant_id: Some(7),
        });
        assert_eq!(clause, "WHERE merchant_id = ?");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], Value::I64(7));
    }
}
