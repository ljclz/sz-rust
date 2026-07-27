//! 商品服务层 — 封装商品相关 SQL 操作
//!
//! 2026-07-25 新增（修复 P0-4：控制器分层违反）。
//!
//! ## 设计
//!
//! - 控制器层（`controllers/product.rs`）仅负责：解析请求参数、调用服务、格式化响应
//! - 服务层（本模块）负责：构建 SQL、执行查询、返回领域数据
//! - 模型层（`models/product.rs`）负责：定义实体结构与 ORM 映射
//!
//! ## 安全
//!
//! 所有 SQL 均使用 `?` 占位符 + `query_with_params` / `execute_with_params` 参数化查询，杜绝 SQL 注入。
//! DB 错误信息仅记录日志，不返回调用方（调用方收到通用错误描述）。

use std::collections::HashMap;

use sz_rust_core::orm::{Pool, Value};

use crate::models::product::Product;

/// 商品列表筛选条件
#[derive(Debug)]
pub struct ProductFilters {
    /// 商户 ID
    pub merchant_id: Option<i64>,
    /// 类目 ID
    pub cat_id: Option<i64>,
    /// 名称关键字（模糊匹配）
    pub keyword: Option<String>,
}

/// 商品分页结果
pub struct ProductPage {
    /// 商品列表（原始行数据）
    pub list: Vec<HashMap<String, Value>>,
    /// 总数
    pub total: i64,
}

/// 商品服务 — 封装商品相关 DB 操作
pub struct ProductService;

impl ProductService {
    /// 分页查询商品列表
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
    /// 成功返回 [`ProductPage`]，失败返回错误描述字符串（不泄露 DB 内部信息）。
    #[tracing::instrument(skip(pool))]
    pub async fn list(
        pool: &Pool,
        page: i64,
        page_size: i64,
        filters: ProductFilters,
    ) -> Result<ProductPage, String> {
        let offset = (page - 1).max(0) * page_size;

        // 构建 WHERE 条件（参数化，杜绝 SQL 注入）
        let mut conditions: Vec<&'static str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if let Some(mid) = filters.merchant_id {
            conditions.push("merchant_id = ?");
            params.push(Value::I64(mid));
        }
        if let Some(cid) = filters.cat_id {
            conditions.push("cat_id = ?");
            params.push(Value::I64(cid));
        }
        if let Some(kw) = &filters.keyword {
            let trimmed = kw.trim();
            if !trimmed.is_empty() {
                conditions.push("name LIKE ?");
                params.push(Value::String(format!("%{}%", trimmed)));
            }
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "商品列表获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        // 总数查询
        let count_sql = format!("SELECT COUNT(*) as total FROM good {}", where_clause);
        let count_rows = conn
            .query_with_params(&count_sql, &params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "商品列表 COUNT 查询失败");
                "查询失败".to_string()
            })?;
        let total: i64 = count_rows
            .first()
            .and_then(|row| row.get("total"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 列表查询 — 追加分页参数
        let list_sql = format!(
            "SELECT * FROM good {} ORDER BY good_id DESC LIMIT ? OFFSET ?",
            where_clause
        );
        let mut list_params = params.clone();
        list_params.push(Value::I64(page_size));
        list_params.push(Value::I64(offset));
        let rows = conn
            .query_with_params(&list_sql, &list_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "商品列表查询失败");
                "查询失败".to_string()
            })?;

        Ok(ProductPage { list: rows, total })
    }

    /// 根据 good_id 查询单个商品
    ///
    /// # 返回
    ///
    /// - `Ok(Some(row))`：商品存在
    /// - `Ok(None)`：商品不存在
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    pub async fn get(pool: &Pool, good_id: i64) -> Result<Option<HashMap<String, Value>>, String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "商品详情获取 DB 连接失败: good_id={}", good_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "SELECT * FROM good WHERE good_id = ?";
        let params = [Value::I64(good_id)];
        let rows = conn.query_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "商品详情查询失败: good_id={}", good_id);
            "查询失败".to_string()
        })?;

        Ok(rows.into_iter().next())
    }

    /// 创建商品 — 返回新商品 good_id
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `product`：商品实体（`good_id` 字段被忽略，由 DB 自增生成）
    #[tracing::instrument(skip(pool, product))]
    pub async fn create(pool: &Pool, product: &Product) -> Result<i64, String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "创建商品获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        let sql = "INSERT INTO good (merchant_id, cat_id, name, barcode, price, unit, ai_class_id, image, status, created_at, updated_at) \
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())";
        let params = [
            Value::I64(product.merchant_id),
            Value::I64(product.cat_id),
            Value::String(product.name.clone()),
            Value::String(product.barcode.clone()),
            Value::I64(product.price),
            Value::String(product.unit.clone()),
            Value::I64(product.ai_class_id),
            Value::String(product.image.clone()),
            Value::I32(product.status as i32),
        ];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "创建商品失败: name={}", product.name);
            "创建失败".to_string()
        })?;

        // 获取自增主键
        let id_rows = conn
            .query("SELECT LAST_INSERT_ID() as good_id")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "获取商品 ID 失败: name={}", product.name);
                "创建失败".to_string()
            })?;
        let new_id = id_rows
            .first()
            .and_then(|row| row.get("good_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        Ok(new_id)
    }

    /// 更新商品 — 动态 SET 子句（仅更新提供的字段）
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `good_id`：商品 ID
    /// - `fields`：待更新字段（key=列名，value=新值）
    ///
    /// # 返回
    ///
    /// - `Ok(())`：更新成功
    /// - `Err(msg)`：更新失败（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool, fields))]
    pub async fn update(
        pool: &Pool,
        good_id: i64,
        fields: HashMap<String, Value>,
    ) -> Result<(), String> {
        if fields.is_empty() {
            return Err("未提供需要更新的字段".to_string());
        }

        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "更新商品获取 DB 连接失败: good_id={}", good_id);
            "数据库连接失败".to_string()
        })?;

        // 构建动态 SET 子句 — 列名通过白名单校验，杜绝 SQL 注入
        let allowed_columns: &[&str] = &[
            "merchant_id",
            "cat_id",
            "name",
            "barcode",
            "price",
            "unit",
            "ai_class_id",
            "image",
            "status",
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
            "UPDATE good SET {} WHERE good_id = ?",
            set_clauses.join(", ")
        );
        params.push(Value::I64(good_id));

        conn.execute_with_params(&sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "更新商品失败: good_id={}", good_id);
            "更新失败".to_string()
        })?;

        Ok(())
    }

    /// 删除商品（软删除 — 将 status 置为 0）
    #[tracing::instrument(skip(pool))]
    pub async fn delete(pool: &Pool, good_id: i64) -> Result<(), String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "删除商品获取 DB 连接失败: good_id={}", good_id);
            "数据库连接失败".to_string()
        })?;

        let sql = "UPDATE good SET status = 0, updated_at = NOW() WHERE good_id = ?";
        let params = [Value::I64(good_id)];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "删除商品失败: good_id={}", good_id);
            "删除失败".to_string()
        })?;

        Ok(())
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）
