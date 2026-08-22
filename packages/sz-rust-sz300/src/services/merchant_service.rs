//! 商户服务层 — 封装商户相关 SQL 操作
//!
//! 2026-07-25 新增（修复 P0-4：控制器分层违反）。
//!
//! ## 设计
//!
//! - 控制器层（`controllers/merchant.rs`）仅负责：解析请求参数、调用服务、格式化响应
//! - 服务层（本模块）负责：构建 SQL、执行查询、返回领域数据
//! - 模型层（`models/merchant.rs`）负责：定义实体结构与 ORM 映射
//!
//! ## 安全
//!
//! 所有 SQL 均使用 `?` 占位符 + `query_with_params` / `execute_with_params` 参数化查询，杜绝 SQL 注入。
//! DB 错误信息仅记录日志，不返回调用方（调用方收到通用错误描述）。

use std::collections::HashMap;

use sz_rust_core::orm::{Pool, Value};

use crate::models::merchant::Merchant;

/// 商户分页结果
pub struct MerchantPage {
    /// 商户列表（原始行数据）
    pub list: Vec<HashMap<String, Value>>,
    /// 总数
    pub total: i64,
}

/// 商户服务 — 封装商户相关 DB 操作
pub struct MerchantService;

impl MerchantService {
    /// 分页查询商户列表
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `page`：页码（从 1 开始）
    /// - `page_size`：每页条数（1..=100）
    ///
    /// # 返回
    ///
    /// 成功返回 [`MerchantPage`]，失败返回错误描述字符串（不泄露 DB 内部信息）。
    #[tracing::instrument(skip(pool))]
    pub async fn list(pool: &Pool, page: i64, page_size: i64) -> Result<MerchantPage, String> {
        let offset = (page - 1).max(0) * page_size;

        // P1-SEC-06: 外部 IO 超时保护（默认 5s）
        sz_rust_core::runtime::spawn::with_timeout(async {
            let mut conn = pool.acquire().await.map_err(|e| {
                tracing::error!(error = %e, "商户列表获取 DB 连接失败");
                "数据库连接失败".to_string()
            })?;

            // 总数查询
            let count_sql = "SELECT COUNT(*) as total FROM merchant";
            let count_rows = conn.query(count_sql).await.map_err(|e| {
                tracing::error!(error = %e, "商户列表 COUNT 查询失败");
                "查询失败".to_string()
            })?;
            let total: i64 = count_rows
                .first()
                .and_then(|row| row.get("total"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            // 列表查询 — LIMIT/OFFSET 参数化，显式列投影（铁律：禁 SELECT *；2026-08-14 修复 merchant_name 不存在列）
            let list_sql = "SELECT merchant_id, market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name, created_at, updated_at FROM merchant ORDER BY merchant_id DESC LIMIT ? OFFSET ?";
            let list_params = [Value::I64(page_size), Value::I64(offset)];
            let rows = conn
                .query_with_params(list_sql, &list_params)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "商户列表查询失败");
                    "查询失败".to_string()
                })?;

            Ok(MerchantPage { list: rows, total })
        })
        .await
        .map_err(|_| {
            tracing::error!("商户列表查询超时（>5s）");
            "服务暂时不可用".to_string()
        })?
    }

    /// 根据 merchant_id 查询单个商户
    ///
    /// # 返回
    ///
    /// - `Ok(Some(row))`：商户存在
    /// - `Ok(None)`：商户不存在
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    pub async fn get(
        pool: &Pool,
        merchant_id: i64,
    ) -> Result<Option<HashMap<String, Value>>, String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "商户详情获取 DB 连接失败: merchant_id={}", merchant_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "SELECT merchant_id, market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name, created_at, updated_at FROM merchant WHERE merchant_id = ?";
        let params = [Value::I64(merchant_id)];
        let rows = conn.query_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "商户详情查询失败: merchant_id={}", merchant_id);
            "查询失败".to_string()
        })?;

        Ok(rows.into_iter().next())
    }

    /// 创建商户
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `merchant`：商户实体（`merchant_id` 字段被忽略，由 DB 自增生成）
    #[tracing::instrument(skip(pool, merchant))]
    pub async fn create(pool: &Pool, merchant: &Merchant) -> Result<(), String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "创建商户获取 DB 连接失败: name={}", merchant.name);
            "数据库连接失败".to_string()
        })?;

        let sql = "INSERT INTO merchant (market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name, created_at, updated_at) \
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())";
        let params = [
            Value::I64(merchant.market_id),
            Value::String(merchant.name.clone()),
            Value::String(merchant.stall_no.clone()),
            Value::String(merchant.contact_phone.clone()),
            Value::String(merchant.category.clone()),
            Value::I64(merchant.status as i64),
            Value::String(merchant.bank_account.clone()),
            Value::String(merchant.bank_name.clone()),
        ];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "创建商户失败: name={}", merchant.name);
            "创建失败".to_string()
        })?;

        Ok(())
    }

    /// 更新商户 — 动态 SET 子句（仅更新提供的字段）
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `merchant_id`：商户 ID
    /// - `fields`：待更新字段（key=列名，value=新值）
    ///
    /// # 返回
    ///
    /// - `Ok(())`：更新成功
    /// - `Err(msg)`：更新失败（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool, fields))]
    pub async fn update(
        pool: &Pool,
        merchant_id: i64,
        fields: HashMap<String, Value>,
    ) -> Result<(), String> {
        if fields.is_empty() {
            return Err("未提供需要更新的字段".to_string());
        }

        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "更新商户获取 DB 连接失败: merchant_id={}", merchant_id);
            "数据库连接失败".to_string()
        })?;

        // 构建动态 SET 子句 — 列名通过白名单校验，杜绝 SQL 注入
        let allowed_columns: &[&str] = &[
            "market_id",
            "name",
            "stall_no",
            "contact_phone",
            "category",
            "status",
            "bank_account",
            "bank_name",
        ];

        let mut set_clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        for col in allowed_columns {
            if let Some(val) = fields.get(*col) {
                set_clauses.push(format!("{} = ?", col));
                params.push(val.clone());
            }
        }

        if set_clauses.is_empty() {
            return Err("未提供需要更新的字段".to_string());
        }

        set_clauses.push("updated_at = NOW()".to_string());
        let sql = format!(
            "UPDATE merchant SET {} WHERE merchant_id = ?",
            set_clauses.join(", ")
        );
        params.push(Value::I64(merchant_id));

        conn.execute_with_params(&sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "更新商户失败: merchant_id={}", merchant_id);
            "更新失败".to_string()
        })?;

        Ok(())
    }

    /// 删除商户（软删除 — 将 status 置为 0）
    #[tracing::instrument(skip(pool))]
    pub async fn delete(pool: &Pool, merchant_id: i64) -> Result<(), String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "删除商户获取 DB 连接失败: merchant_id={}", merchant_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "UPDATE merchant SET status = 0, updated_at = NOW() WHERE merchant_id = ?";
        let params = [Value::I64(merchant_id)];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "删除商户失败: merchant_id={}", merchant_id);
            "删除失败".to_string()
        })?;

        Ok(())
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 覆盖 update 空字段早返回分支（不依赖 DB）
    #[tokio::test]
    async fn update_empty_fields_returns_err() {
        let state = crate::state::mock_app_state();
        let fields = HashMap::new();
        let result = MerchantService::update(&state.db_pool, 1, fields).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "未提供需要更新的字段");
    }

    /// 覆盖 list acquire 失败路径 — mock_app_state 连接假地址，acquire 返回 Err
    #[tokio::test]
    async fn list_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let result = MerchantService::list(&state.db_pool, 1, 15).await;
        assert!(result.is_err());
    }

    /// 覆盖 get acquire 失败路径
    #[tokio::test]
    async fn get_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let result = MerchantService::get(&state.db_pool, 1).await;
        assert!(matches!(result, Err(ref e) if e == "数据库连接失败"));
    }

    /// 覆盖 create acquire 失败路径
    #[tokio::test]
    async fn create_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let merchant = crate::models::merchant::Merchant {
            merchant_id: None,
            market_id: 0,
            name: "test".to_string(),
            stall_no: "".to_string(),
            contact_phone: "13800000000".to_string(),
            category: "".to_string(),
            status: 1,
            bank_account: "".to_string(),
            bank_name: "".to_string(),
            created_at: None,
            updated_at: None,
        };
        let result = MerchantService::create(&state.db_pool, &merchant).await;
        assert!(matches!(result, Err(ref e) if e == "数据库连接失败"));
    }

    /// 覆盖 delete acquire 失败路径
    #[tokio::test]
    async fn delete_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let result = MerchantService::delete(&state.db_pool, 1).await;
        assert!(matches!(result, Err(ref e) if e == "数据库连接失败"));
    }
}
