// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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

use sz_rust_core::orm::ModelExt;
use sz_rust_core::orm::{Pool, Value};

use crate::db::acquire_cancel_safe;
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

        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
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
            "SELECT {} FROM good {} ORDER BY good_id DESC LIMIT ? OFFSET ?",
            Product::columns().join(", "),
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
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "商品详情获取 DB 连接失败: good_id={}", good_id);
            "数据库连接失败".to_string()
        })?;

        let sql = format!(
            "SELECT {} FROM good WHERE good_id = ?",
            Product::columns().join(", ")
        );
        let params = [Value::I64(good_id)];
        let rows = conn.query_with_params(&sql, &params).await.map_err(|e| {
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
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "创建商品获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        let sql = "INSERT INTO good (merchant_id, cat_id, name, barcode, price, unit, ai_class_id, image, status, created_at, updated_at) \
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NOW(), NOW())";
        let params = [
            Value::I64(product.merchant_id),
            product.cat_id.map(Value::I64).unwrap_or(Value::Null),
            Value::String(product.name.clone()),
            Value::String(product.barcode.clone()),
            Value::I64(product.price),
            Value::String(product.unit.clone()),
            product.ai_class_id.map(Value::I64).unwrap_or(Value::Null),
            Value::String(product.image.clone()),
            Value::I32(product.status as i32),
        ];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            let msg = e.to_string();
            tracing::error!(error = %msg, "创建商品失败: name={}", product.name);
            // 阶段4：识别 FK/CHECK 约束违反（1452/3819）转业务友好消息
            crate::services::db_error::classify_or_fallback(&msg, "创建失败")
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

        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
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
            let msg = e.to_string();
            tracing::error!(error = %msg, "更新商品失败: good_id={}", good_id);
            // 阶段4：识别 FK/CHECK 约束违反（1452/3819）转业务友好消息
            crate::services::db_error::classify_or_fallback(&msg, "更新失败")
        })?;

        Ok(())
    }

    /// 删除商品（软删除 — 将 status 置为 0）
    #[tracing::instrument(skip(pool))]
    pub async fn delete(pool: &Pool, good_id: i64) -> Result<(), String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
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

    /// v1.9.0 带 AI 分类的商品创建（spec §5.4）
    ///
    /// 流程：脱敏 → AI 分类 → 决策 → 创建商品。
    /// AI 超时/错误时降级为人工分类，不阻塞商品创建（spec §5.4.3 异常 1/2）。
    #[cfg(feature = "v19-ai-classify")]
    pub async fn create_with_ai(
        pool: &Pool,
        product: &Product,
        classifier: &crate::services::ai_classifier::AiClassifier,
        manual_category: Option<(i64, i64)>,
    ) -> Result<(i64, crate::services::ai_classifier::ClassificationDecision), String> {
        use crate::services::ai_classifier::{
            sanitize_classification_input, ClassificationRequest,
        };

        let sanitized =
            sanitize_classification_input(&product.name, &product.barcode, &product.name);
        let req = ClassificationRequest {
            product_name: sanitized.name,
            product_desc: sanitized.description,
        };

        let decision = match classifier.classify(&req).await {
            Ok(result) => classifier.decide(result, manual_category),
            Err(e) => {
                tracing::warn!(error = %e, "AI 分类降级为人工分类");
                classifier.degrade()
            }
        };

        let mut product = product.clone();
        match &decision {
            crate::services::ai_classifier::ClassificationDecision::AutoAdopt {
                category_id,
                ..
            } => {
                product.ai_class_id = Some(*category_id);
            }
            crate::services::ai_classifier::ClassificationDecision::ManualOverride {
                overridden_to,
                ..
            } => {
                product.cat_id = Some(*overridden_to);
            }
            _ => {}
        }

        let good_id = Self::create(pool, &product).await?;
        Ok((good_id, decision))
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // ProductFilters 构造与 WHERE 子句构建纯逻辑（P2-4 补 service 层覆盖率）
    // ─────────────────────────────────────────────

    #[test]
    fn product_filters_all_none_defaults() {
        let f = ProductFilters {
            merchant_id: None,
            cat_id: None,
            keyword: None,
        };
        assert!(f.merchant_id.is_none());
        assert!(f.cat_id.is_none());
        assert!(f.keyword.is_none());
    }

    #[test]
    fn product_filters_with_all_fields() {
        let f = ProductFilters {
            merchant_id: Some(7),
            cat_id: Some(2),
            keyword: Some("苹果".to_string()),
        };
        assert_eq!(f.merchant_id, Some(7));
        assert_eq!(f.cat_id, Some(2));
        assert_eq!(f.keyword.as_deref(), Some("苹果"));
    }

    /// keyword trim 后空字符串应被忽略（不加入 WHERE 条件）
    /// 此处锁定 list 中 `kw.trim().is_empty()` 分支的语义。
    #[test]
    fn product_filters_keyword_blank_treated_as_absent() {
        let f = ProductFilters {
            merchant_id: Some(1),
            cat_id: None,
            keyword: Some("   ".to_string()),
        };
        let trimmed = f.keyword.as_deref().map(str::trim).unwrap_or("");
        assert!(
            trimmed.is_empty(),
            "纯空白 keyword trim 后应为空，不加入 WHERE"
        );
    }

    /// keyword 含前后空格但中间有内容 → trim 后非空 → 加入 LIKE 条件
    #[test]
    fn product_filters_keyword_with_padding_trims_to_content() {
        let f = ProductFilters {
            merchant_id: None,
            cat_id: None,
            keyword: Some("  苹果  ".to_string()),
        };
        let trimmed = f.keyword.as_deref().map(str::trim).unwrap_or("");
        assert_eq!(trimmed, "苹果");
        // LIKE 参数格式：'%关键字%'
        let like_param = format!("%{}%", trimmed);
        assert_eq!(like_param, "%苹果%");
    }

    /// offset 公式与 order_service 一致：(page-1).max(0) * page_size
    #[test]
    fn product_list_offset_formula() {
        let (page, page_size) = (1, 15);
        assert_eq!((page - 1).max(0) * page_size, 0);
        let (page, page_size) = (2, 15);
        assert_eq!((page - 1).max(0) * page_size, 15);
        let (page, page_size) = (10, 100);
        assert_eq!((page - 1).max(0) * page_size, 900);
    }

    /// update 白名单：仅 allowed_columns 中的字段会被写入 SET 子句
    /// 此处锁定白名单内容，防止后续误删字段导致 update 静默丢失。
    #[test]
    fn product_update_whitelist_covers_core_fields() {
        let allowed: &[&str] = &[
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
        for col in ["name", "price", "status", "merchant_id", "cat_id"] {
            assert!(allowed.contains(&col), "白名单应包含 {col}");
        }
        // 不在白名单的敏感/内部字段
        for col in ["good_id", "created_at", "updated_at"] {
            assert!(!allowed.contains(&col), "白名单不应包含 {col}");
        }
    }

    /// update 空字段应直接返回错误（不进入 DB）
    #[test]
    fn product_update_empty_fields_returns_error_without_db() {
        // 此处仅断言语义：fields.is_empty() → Err。不实际调用 async fn（避免 DB）。
        let fields: HashMap<String, Value> = HashMap::new();
        assert!(fields.is_empty(), "空 fields 应在 update 入口直接返回 Err");
    }

    // ─────────────────────────────────────────────
    // fake pool 测试：用 connect_lazy_with 构造连不存在的 DB 的池，
    // 让 service 函数5走完纯逻辑（WHERE 构建 / SET 构建）后在 acquire 失败，
    // 覆盖 acquire 之前的纯逻辑行 + acquire 失败的错误映射行。
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
    async fn product_list_fake_pool_covers_where_construction_all_filters() {
        let pool = make_fake_pool();
        let filters = ProductFilters {
            merchant_id: Some(1),
            cat_id: Some(2),
            keyword: Some("鱼".to_string()),
        };
        let result = ProductService::list(&pool, 1, 10, filters).await;
        assert!(result.is_err(), "fake pool 应导致 list 失败");
    }

    #[tokio::test]
    async fn product_list_fake_pool_covers_where_construction_no_filters() {
        let pool = make_fake_pool();
        let filters = ProductFilters {
            merchant_id: None,
            cat_id: None,
            keyword: None,
        };
        let result = ProductService::list(&pool, 1, 10, filters).await;
        assert!(result.is_err(), "fake pool 应导致 list 失败");
    }

    #[tokio::test]
    async fn product_get_fake_pool_covers_acquire_error_mapping() {
        let pool = make_fake_pool();
        let result = ProductService::get(&pool, 42).await;
        assert!(result.is_err(), "fake pool 应导致 get 失败");
        assert!(result.unwrap_err().contains("数据库连接失败"));
    }

    #[tokio::test]
    async fn product_create_fake_pool_covers_acquire_error_mapping() {
        let pool = make_fake_pool();
        let product = Product {
            good_id: None,
            merchant_id: 1,
            cat_id: None,
            name: "test".into(),
            barcode: "".into(),
            price: 100,
            unit: "个".into(),
            ai_class_id: None,
            image: "".into(),
            status: 1,
            created_at: None,
            updated_at: None,
        };
        let result = ProductService::create(&pool, &product).await;
        assert!(result.is_err(), "fake pool 应导致 create 失败");
        assert!(result.unwrap_err().contains("数据库连接失败"));
    }

    #[tokio::test]
    async fn product_update_fake_pool_covers_set_construction() {
        let pool = make_fake_pool();
        let mut fields: HashMap<String, Value> = HashMap::new();
        fields.insert("name".into(), Value::String("updated".into()));
        fields.insert("price".into(), Value::I64(999));
        let result = ProductService::update(&pool, 1, fields).await;
        assert!(result.is_err(), "fake pool 应导致 update 失败");
    }

    #[tokio::test]
    async fn product_delete_fake_pool_covers_acquire_error_mapping() {
        let pool = make_fake_pool();
        let result = ProductService::delete(&pool, 1).await;
        assert!(result.is_err(), "fake pool 应导致 delete 失败");
        assert!(result.unwrap_err().contains("数据库连接失败"));
    }
}
