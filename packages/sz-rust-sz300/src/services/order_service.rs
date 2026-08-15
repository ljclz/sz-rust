//! 订单服务层 — 封装订单相关 SQL 操作（含订单项）
//!
//! 2026-07-25 新增（修复 P0-4：控制器分层违反）。
//!
//! ## 设计
//!
//! - 控制器层（`controllers/order.rs`）仅负责：解析请求参数、调用服务、格式化响应
//! - 服务层（本模块）负责：构建 SQL、执行查询、返回领域数据
//! - 模型层（`models/order.rs`、`models/order_item.rs`）负责：定义实体结构与 ORM 映射
//!
//! ## 安全
//!
//! 所有 SQL 均使用 `?` 占位符 + `query_with_params` / `execute_with_params` 参数化查询，杜绝 SQL 注入。
//! DB 错误信息仅记录日志，不返回调用方（调用方收到通用错误描述）。

use std::collections::HashMap;

use sz_rust_core::orm::{Pool, Value};

use crate::models::order::Order;
use crate::models::order_item::OrderItem;

/// 订单列表筛选条件
#[derive(Debug)]
pub struct OrderFilters {
    /// 商户 ID
    pub merchant_id: Option<i64>,
    /// 设备 ID
    pub device_id: Option<i64>,
    /// 订单状态
    pub status: Option<i64>,
    /// 起始日期（created_at >=）
    pub start_date: Option<String>,
    /// 截止日期（created_at <=）
    pub end_date: Option<String>,
}

/// 订单分页结果
pub struct OrderPage {
    /// 订单列表（原始行数据）
    pub list: Vec<HashMap<String, Value>>,
    /// 总数
    pub total: i64,
}

/// 订单详情（含订单项）
pub struct OrderDetail {
    /// 订单主表行数据
    pub order: HashMap<String, Value>,
    /// 订单项列表
    pub items: Vec<HashMap<String, Value>>,
}

/// 订单服务 — 封装订单相关 DB 操作
pub struct OrderService;

impl OrderService {
    /// 分页查询订单列表
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
    /// 成功返回 [`OrderPage`]，失败返回错误描述字符串（不泄露 DB 内部信息）。
    #[tracing::instrument(skip(pool))]
    pub async fn list(
        pool: &Pool,
        page: i64,
        page_size: i64,
        filters: OrderFilters,
    ) -> Result<OrderPage, String> {
        let offset = (page - 1).max(0) * page_size;

        // 构建 WHERE 条件（参数化，杜绝 SQL 注入）
        let mut conditions: Vec<&'static str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if let Some(mid) = filters.merchant_id {
            conditions.push("merchant_id = ?");
            params.push(Value::I64(mid));
        }
        if let Some(did) = filters.device_id {
            conditions.push("device_id = ?");
            params.push(Value::I64(did));
        }
        if let Some(st) = filters.status {
            conditions.push("status = ?");
            params.push(Value::I64(st));
        }
        if let Some(sd) = &filters.start_date {
            let trimmed = sd.trim();
            if !trimmed.is_empty() {
                conditions.push("created_at >= ?");
                params.push(Value::String(trimmed.to_string()));
            }
        }
        if let Some(ed) = &filters.end_date {
            let trimmed = ed.trim();
            if !trimmed.is_empty() {
                conditions.push("created_at <= ?");
                params.push(Value::String(trimmed.to_string()));
            }
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "订单列表获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        // 总数查询
        let count_sql = format!("SELECT COUNT(*) as total FROM `order` {}", where_clause);
        let count_rows = conn
            .query_with_params(&count_sql, &params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单列表 COUNT 查询失败");
                "查询失败".to_string()
            })?;
        let total: i64 = count_rows
            .first()
            .and_then(|row| row.get("total"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 列表查询 — 追加分页参数（显式列投影，铁律：禁 SELECT *）
        let list_sql = format!(
            "SELECT order_id, order_no, merchant_id, device_id, total_fen, total_weight_g, item_count, status, pay_method, pay_at, offline_seq, created_at, updated_at FROM `order` {} ORDER BY order_id DESC LIMIT ? OFFSET ?",
            where_clause
        );
        let mut list_params = params.clone();
        list_params.push(Value::I64(page_size));
        list_params.push(Value::I64(offset));
        let rows = conn
            .query_with_params(&list_sql, &list_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单列表查询失败");
                "查询失败".to_string()
            })?;

        Ok(OrderPage { list: rows, total })
    }

    /// 根据 order_id 查询订单详情（含订单项）
    ///
    /// # 返回
    ///
    /// - `Ok(Some(detail))`：订单存在，detail 包含订单主表与订单项
    /// - `Ok(None)`：订单不存在
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    /// 根据 order_id 查询订单详情（含订单项）
    ///
    /// 安全修复 H-1：`merchant_id` 为服务端强制校验的数据边界，
    /// 订单必须同时匹配 order_id 与 merchant_id 才会返回（防越权）。
    pub async fn get_with_items(
        pool: &Pool,
        order_id: i64,
        merchant_id: i64,
    ) -> Result<Option<OrderDetail>, String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "订单详情获取 DB 连接失败: order_id={}", order_id);
            "数据库连接失败".to_string()
        })?;

        // 查询订单主表（强制商户边界；显式列投影）
        let order_sql = "SELECT order_id, order_no, merchant_id, device_id, total_fen, total_weight_g, item_count, status, pay_method, pay_at, offline_seq, created_at, updated_at FROM `order` WHERE order_id = ? AND merchant_id = ?";
        let order_params = [Value::I64(order_id), Value::I64(merchant_id)];
        let order_rows = conn
            .query_with_params(order_sql, &order_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单详情查询失败: order_id={}", order_id);
                "查询失败".to_string()
            })?;

        let order_row = match order_rows.into_iter().next() {
            Some(row) => row,
            None => return Ok(None),
        };

        // 查询订单项（显式列投影；2026-08-14 修复参数数量不匹配：order_item 仅 1 个占位符，不得复用 order_params）
        let items_sql = "SELECT item_id, order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity FROM order_item WHERE order_id = ?";
        let items_params = [Value::I64(order_id)];
        let items_rows = conn
            .query_with_params(items_sql, &items_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单项查询失败: order_id={}", order_id);
                "查询失败".to_string()
            })?;

        Ok(Some(OrderDetail {
            order: order_row,
            items: items_rows,
        }))
    }

    /// 创建订单（含订单项）— 返回新订单 order_id
    ///
    /// # 参数
    ///
    /// - `pool`：数据库连接池
    /// - `order`：订单实体（`order_id` 字段被忽略，由 DB 自增生成；`status` 字段被忽略，固定为 1=待支付）
    /// - `items`：订单项列表
    #[tracing::instrument(skip(pool, order, items))]
    pub async fn create(pool: &Pool, order: &Order, items: &[OrderItem]) -> Result<i64, String> {
        let mut conn = pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "创建订单获取 DB 连接失败: order_no={}", order.order_no);
            "数据库连接失败".to_string()
        })?;

        // 插入订单主表（status 固定为 1=待支付）
        let order_sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, total_weight_g, item_count, status, pay_method, offline_seq, created_at, updated_at) \
                         VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, NOW(), NOW())";
        let order_params = [
            Value::String(order.order_no.clone()),
            Value::I64(order.merchant_id),
            Value::I64(order.device_id),
            Value::I64(order.total_fen),
            Value::I64(order.total_weight_g),
            Value::I32(order.item_count),
            Value::I8(order.pay_method),
            Value::String(order.offline_seq.clone()),
        ];
        conn.execute_with_params(order_sql, &order_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "创建订单失败: order_no={}", order.order_no);
                "创建失败".to_string()
            })?;

        // 获取新订单 ID
        let id_rows = conn
            .query("SELECT LAST_INSERT_ID() as order_id")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "获取订单 ID 失败: order_no={}", order.order_no);
                "创建失败".to_string()
            })?;
        let new_order_id = id_rows
            .first()
            .and_then(|row| row.get("order_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 插入订单项
        for item in items {
            let item_sql = "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) \
                            VALUES (?, ?, ?, ?, ?, ?, ?)";
            let item_params = [
                Value::I64(new_order_id),
                Value::I64(item.good_id),
                Value::String(item.good_name.clone()),
                Value::I64(item.price_fen),
                Value::I64(item.weight_g),
                Value::I64(item.total_fen),
                Value::I32(item.quantity),
            ];
            if let Err(e) = conn.execute_with_params(item_sql, &item_params).await {
                // 不中断整体流程，继续插入其他订单项
                tracing::error!(error = %e, "创建订单项失败: order_id={}", new_order_id);
            }
        }

        Ok(new_order_id)
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）
