// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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

use sz_rust_core::orm::ModelExt;
use sz_rust_core::orm::{Pool, Value};

use crate::db::acquire_cancel_safe;
use crate::models::order::Order;
use crate::models::order_item::OrderItem;
use crate::query_cache::QueryCache;

/// 订单列表筛选条件
#[derive(Debug)]
pub struct OrderFilters {
    /// 商户 ID
    pub merchant_id: Option<i64>,
    /// 设备 ID
    pub device_id: Option<i64>,
    /// 订单状态
    pub status: Option<i64>,
    /// 起始日期（created_at >=，闭区间）
    pub start_date: Option<String>,
    /// 截止日期（按日期解析为次日 0 点的开区间，避免漏掉当天带时间的订单）
    pub end_date: Option<String>,
    /// keyset 游标：只返回 order_id 小于该值的订单（深度翻页用，避免 OFFSET 扫描）
    pub before_order_id: Option<i64>,
}

impl OrderFilters {
    /// 构造仅含归属过滤的筛选条件（无日期/状态/游标）
    pub fn new(merchant_id: Option<i64>, device_id: Option<i64>, status: Option<i64>) -> Self {
        Self {
            merchant_id,
            device_id,
            status,
            start_date: None,
            end_date: None,
            before_order_id: None,
        }
    }
}

/// 订单分页结果
pub struct OrderPage {
    /// 订单列表（原始行数据）
    pub list: Vec<HashMap<String, Value>>,
    /// 总数（与筛选条件一致；keyset 模式下仍返回，便于前端展示总量）
    pub total: i64,
    /// 下一页游标（keyset 模式下本页已满时返回最后一行的 order_id）
    pub next_cursor: Option<i64>,
}

/// 订单详情（含订单项）
pub struct OrderDetail {
    /// 订单主表行数据
    pub order: HashMap<String, Value>,
    /// 订单项列表
    pub items: Vec<HashMap<String, Value>>,
}

/// 订单 COUNT 缓存键：WHERE 子句 + 参数化值（serde_json 确定性序列化）。
///
/// 同一筛选条件必须命中同一键；参数化值参与键构造，避免不同筛选值
/// （同一 where_clause 下 merchant_id 不同）共享同一缓存项。
fn count_cache_key(where_clause: &str, params: &[Value]) -> String {
    let params_json = serde_json::to_string(params).unwrap_or_default();
    format!("order:count:{}|{}", where_clause, params_json)
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
    /// - `cache`：查询缓存（`None` 时总是执行 COUNT；`Some` 时 COUNT 结果按筛选键缓存）
    /// - `page`：页码（从 1 开始）
    /// - `page_size`：每页条数（1..=100）
    /// - `filters`：筛选条件
    ///
    /// # 返回
    ///
    /// 成功返回 [`OrderPage`]，失败返回错误描述字符串（不泄露 DB 内部信息）。
    #[tracing::instrument(skip(pool, cache))]
    pub async fn list(
        pool: &Pool,
        cache: Option<&QueryCache>,
        page: i64,
        page_size: i64,
        filters: OrderFilters,
    ) -> Result<OrderPage, String> {
        // keyset 模式（提供游标）不使用 OFFSET：大偏移量会让 MySQL 扫描并丢弃前 N 行
        let keyset = filters.before_order_id.is_some();
        let offset = if keyset {
            0
        } else {
            (page - 1).max(0) * page_size
        };

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
                // 半开区间 [start, end+1d)：created_at 是 DATETIME，用 `<= '2026-10-28'`
                // 会退化成 `<= '2026-10-28 00:00:00'`，漏掉当天 00:00 之后的所有订单。
                match chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
                    Ok(date) => {
                        let next = date
                            .succ_opt()
                            .ok_or_else(|| "截止日期超出可表示范围".to_string())?;
                        conditions.push("created_at < ?");
                        params.push(Value::String(next.format("%Y-%m-%d").to_string()));
                    }
                    // 非纯日期（含时间部分）保持闭区间语义
                    Err(_) => {
                        conditions.push("created_at <= ?");
                        params.push(Value::String(trimmed.to_string()));
                    }
                }
            }
        }
        if let Some(cursor) = filters.before_order_id {
            conditions.push("order_id < ?");
            params.push(Value::I64(cursor));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "订单列表获取 DB 连接失败");
            "数据库连接失败".to_string()
        })?;

        // 总数查询 — COUNT 结果缓存（LRU+TTL 60s）。
        //
        // 订单写路径（OrderService::create 提交成功）会清空整个查询缓存，保证
        // list 的 COUNT 不残留过期值。GraphQL/MQTT/SAGA 等旁路写路径不经过本
        // 服务，依赖 TTL 自然过期（≤60s 最终一致），见 ADR-048。
        let total: i64 = {
            let count_sql = format!("SELECT COUNT(*) as total FROM `order` {}", where_clause);
            let run_count = async {
                let rows = conn
                    .query_with_params(&count_sql, &params)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "订单列表 COUNT 查询失败");
                        "查询失败".to_string()
                    })?;
                Ok::<i64, String>(
                    rows.first()
                        .and_then(|row| row.get("total"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                )
            };

            if let Some(c) = cache {
                let key = count_cache_key(&where_clause, &params);
                match c.get(&key).and_then(|b| b.as_slice().try_into().ok()) {
                    Some(bytes) => i64::from_le_bytes(bytes),
                    None => {
                        let total = run_count.await?;
                        c.set(&key, total.to_le_bytes().to_vec(), None);
                        total
                    }
                }
            } else {
                run_count.await?
            }
        };

        // 列表查询 — 追加分页参数（keyset 模式不生成 OFFSET 子句）
        //
        // keyset 深翻页固定 `ORDER BY order_id DESC`：优化器会优先选覆盖索引
        // `idx_merchant_created (merchant_id, created_at)`，再对整段商户订单做 filesort
        // （实测 1000 行/商户时 0.53ms vs 0.017ms）。显式指定 `idx_merchant`，其二级索引
        // 尾部带主键，反向扫描可直接产出有序的 20 行。仅在带 merchant_id 时提示，
        // 否则纯设备游标场景会退化为全索引扫描。
        let list_from = if keyset && filters.merchant_id.is_some() {
            "`order` FORCE INDEX (idx_merchant)"
        } else {
            "`order`"
        };
        let list_sql = if keyset {
            format!(
                "SELECT {} FROM {} {} ORDER BY order_id DESC LIMIT ?",
                Order::columns().join(", "),
                list_from,
                where_clause
            )
        } else {
            format!(
                "SELECT {} FROM `order` {} ORDER BY order_id DESC LIMIT ? OFFSET ?",
                Order::columns().join(", "),
                where_clause
            )
        };
        let mut list_params = params.clone();
        list_params.push(Value::I64(page_size));
        if !keyset {
            list_params.push(Value::I64(offset));
        }
        let rows = conn
            .query_with_params(&list_sql, &list_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单列表查询失败");
                "查询失败".to_string()
            })?;

        // 本页已满才可能存在下一页；用最后一行 order_id 作为游标（ORDER BY order_id DESC）
        let next_cursor = if keyset && rows.len() as i64 == page_size {
            rows.last()
                .and_then(|row| row.get("order_id"))
                .and_then(|v| v.as_i64())
        } else {
            None
        };

        Ok(OrderPage {
            list: rows,
            total,
            next_cursor,
        })
    }

    /// 根据 order_id 查询订单详情（含订单项）
    ///
    /// # 返回
    ///
    /// - `Ok(Some(detail))`：订单存在，detail 包含订单主表与订单项
    /// - `Ok(None)`：订单不存在
    /// - `Err(msg)`：DB 错误（msg 不泄露内部信息）
    #[tracing::instrument(skip(pool))]
    pub async fn get_with_items(pool: &Pool, order_id: i64) -> Result<Option<OrderDetail>, String> {
        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "订单详情获取 DB 连接失败: order_id={}", order_id);
            "数据库连接失败".to_string()
        })?;

        // 查询订单主表
        let order_sql = format!(
            "SELECT {} FROM `order` WHERE order_id = ?",
            Order::columns().join(", ")
        );
        let order_params = [Value::I64(order_id)];
        let order_rows = conn
            .query_with_params(&order_sql, &order_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "订单详情查询失败: order_id={}", order_id);
                "查询失败".to_string()
            })?;

        let order_row = match order_rows.into_iter().next() {
            Some(row) => row,
            None => return Ok(None),
        };

        // 查询订单项
        let items_sql = format!(
            "SELECT {} FROM order_item WHERE order_id = ?",
            OrderItem::columns().join(", ")
        );
        let items_rows = conn
            .query_with_params(&items_sql, &order_params)
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
    ///
    /// # 事务语义（2026-10-28 修复 P0：下单非原子）
    ///
    /// 订单头 + 全部订单项在同一事务内提交；任一步失败整体回滚，
    /// 不再出现"头已写入、行部分丢失"的半单。订单项使用多值 INSERT 批量写入。
    ///
    /// # 幂等语义（2026-10-28 修复：离线重放）
    ///
    /// `offline_seq` 非空时受 `uk_device_offline_seq (device_id, offline_seq)` 约束保护
    /// （`005_order_idempotency.sql`）。设备断网重传命中该唯一键时，回滚本次事务并
    /// 返回**已存在订单的 order_id**，而不是报"订单号冲突"——重放对设备是成功，不是失败。
    #[tracing::instrument(skip(pool, cache, order, items))]
    pub async fn create(
        pool: &Pool,
        cache: Option<&QueryCache>,
        order: &Order,
        items: &[OrderItem],
    ) -> Result<i64, String> {
        enum CreateOutcome {
            Created(i64),
            DuplicateOfflineSeq,
        }

        /// 单条 INSERT 的最大行数（7 列 × 500 行 = 3500 参数，远低于 MySQL 上限）
        const MAX_ROWS_PER_INSERT: usize = 500;

        let mut conn = acquire_cancel_safe(pool).await.map_err(|e| {
            tracing::error!(error = %e, "创建订单获取 DB 连接失败: order_no={}", order.order_no);
            "数据库连接失败".to_string()
        })?;

        conn.begin_transaction().await.map_err(|e| {
            tracing::error!(error = %e, "创建订单开启事务失败: order_no={}", order.order_no);
            "创建失败".to_string()
        })?;

        let result: Result<CreateOutcome, String> = async {
            // 插入订单主表（status 固定为 1=待支付；item_count 以实际明细数为准）
            //
            // offline_seq 用 `NULLIF(?, '')`：005 迁移把该列改为可空并建立
            // `uk_device_offline_seq (device_id, offline_seq)`；MySQL 唯一索引允许多个 NULL，
            // 若在线订单写 ''，同设备的第二单会与第一单在 (device_id, '') 上冲突。
            let order_sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, total_weight_g, item_count, status, pay_method, offline_seq, created_at, updated_at) \
                             VALUES (?, ?, ?, ?, ?, ?, 1, ?, NULLIF(?, ''), NOW(), NOW())";
            let order_params = [
                Value::String(order.order_no.clone()),
                Value::I64(order.merchant_id),
                Value::I64(order.device_id),
                Value::I64(order.total_fen),
                Value::I64(order.total_weight_g),
                Value::I32(items.len().min(i32::MAX as usize) as i32),
                Value::I8(order.pay_method),
                Value::String(order.offline_seq.clone()),
            ];
            if let Err(e) = conn.execute_with_params(order_sql, &order_params).await {
                let msg = e.to_string();
                tracing::error!(error = %msg, "创建订单失败: order_no={}", order.order_no);
                let duplicate = msg.contains("Duplicate") || msg.contains("duplicate");
                // 只有带离线序列号的订单才可能命中幂等键；(device_id, offline_seq) 已在 005 迁移建唯一索引。
                // 命中 order_no 等其他唯一键说明不是重放，必须报错而不是返回别人的单。
                if duplicate && !order.offline_seq.trim().is_empty() {
                    return Ok(CreateOutcome::DuplicateOfflineSeq);
                }
                return Err(if duplicate {
                    "订单号冲突，请重试".to_string()
                } else {
                    "创建失败".to_string()
                });
            }

            // 获取新订单 ID（同一事务同一连接）
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
            if new_order_id <= 0 {
                tracing::error!("获取订单 ID 失败: LAST_INSERT_ID() 返回 {new_order_id}");
                return Err("创建失败".to_string());
            }

            // 插入订单项 — 多值 INSERT 批量写入
            for chunk in items.chunks(MAX_ROWS_PER_INSERT) {
                let placeholders = vec!["(?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ");
                let item_sql = format!(
                    "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) VALUES {placeholders}"
                );
                let mut item_params = Vec::with_capacity(chunk.len() * 7);
                for item in chunk {
                    item_params.push(Value::I64(new_order_id));
                    item_params.push(Value::I64(item.good_id));
                    item_params.push(Value::String(item.good_name.clone()));
                    item_params.push(Value::I64(item.price_fen));
                    item_params.push(Value::I64(item.weight_g));
                    item_params.push(Value::I64(item.total_fen));
                    item_params.push(Value::I32(item.quantity));
                }
                conn.execute_with_params(&item_sql, &item_params)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "创建订单项失败: order_id={}", new_order_id);
                        "创建失败".to_string()
                    })?;
            }

            // 事务性 Outbox：订单与 order.created 事件同事务提交（006_outbox.sql）
            let event_payload = serde_json::json!({
                "order_id": new_order_id,
                "order_no": order.order_no,
                "merchant_id": order.merchant_id,
                "device_id": order.device_id,
                "total_fen": order.total_fen,
                "item_count": items.len(),
                "status": 1,
            })
            .to_string();
            let outbox_sql = "INSERT INTO outbox_event (aggregate_type, aggregate_id, event_type, payload) \
                              VALUES ('order', ?, 'order.created', ?)";
            conn.execute_with_params(
                outbox_sql,
                &[
                    Value::String(new_order_id.to_string()),
                    Value::String(event_payload),
                ],
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "写入 outbox 事件失败: order_id={}", new_order_id);
                "创建失败".to_string()
            })?;

            Ok(CreateOutcome::Created(new_order_id))
        }
        .await;

        match result {
            Ok(CreateOutcome::Created(order_id)) => {
                conn.commit().await.map_err(|e| {
                    tracing::error!(error = %e, "提交订单事务失败: order_id={}", order_id);
                    "创建失败".to_string()
                })?;
                // 新订单会改变所有筛选口径下的 COUNT：整体清空最安全。
                // QueryCache 不支持前缀批量失效；旁路写路径依赖 TTL，见模块注释。
                if let Some(c) = cache {
                    c.clear();
                }
                Ok(order_id)
            }
            // 离线重放：本次事务未提交任何数据，回滚后按幂等键查回原单
            Ok(CreateOutcome::DuplicateOfflineSeq) => {
                if let Err(e) = conn.rollback().await {
                    tracing::error!(error = %e, "回滚重放事务失败: order_no={}", order.order_no);
                }
                let rows = conn
                    .query_with_params(
                        "SELECT order_id FROM `order` WHERE device_id = ? AND offline_seq = ? LIMIT 1",
                        &[
                            Value::I64(order.device_id),
                            Value::String(order.offline_seq.clone()),
                        ],
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            "重放查回原单失败: device_id={}, offline_seq={}",
                            order.device_id,
                            order.offline_seq
                        );
                        "创建失败".to_string()
                    })?;
                let existing_id = rows
                    .first()
                    .and_then(|row| row.get("order_id"))
                    .and_then(|v| v.as_i64())
                    .filter(|id| *id > 0);
                match existing_id {
                    Some(id) => {
                        tracing::info!(
                            "离线订单重放命中幂等键: device_id={}, offline_seq={}, order_id={}",
                            order.device_id,
                            order.offline_seq,
                            id
                        );
                        Ok(id)
                    }
                    None => Err("订单号冲突，请重试".to_string()),
                }
            }
            Err(msg) => {
                if let Err(e) = conn.rollback().await {
                    tracing::error!(error = %e, "回滚订单事务失败: order_no={}", order.order_no);
                }
                Err(msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_cache_key_is_stable_for_same_filters() {
        let where_clause = "WHERE merchant_id = ? AND status = ?";
        let params = [Value::I64(1), Value::I64(2)];
        let k1 = count_cache_key(where_clause, &params);
        let k2 = count_cache_key(where_clause, &params);
        assert_eq!(k1, k2, "相同筛选条件必须生成相同缓存键");
        assert!(k1.starts_with("order:count:"), "缓存键应带领域前缀");
    }

    #[test]
    fn count_cache_key_differs_by_param_value() {
        let where_clause = "WHERE merchant_id = ?";
        let k1 = count_cache_key(where_clause, &[Value::I64(1)]);
        let k2 = count_cache_key(where_clause, &[Value::I64(2)]);
        assert_ne!(k1, k2, "不同商户必须使用不同缓存键");
    }

    #[test]
    fn count_cache_key_differs_by_where_clause() {
        let k1 = count_cache_key("WHERE merchant_id = ?", &[Value::I64(1)]);
        let k2 = count_cache_key("WHERE device_id = ?", &[Value::I64(1)]);
        assert_ne!(k1, k2, "不同筛选列必须使用不同缓存键");
    }

    #[test]
    fn count_cache_key_handles_empty_where() {
        let key = count_cache_key("", &[]);
        assert_eq!(key, "order:count:|[]");
    }
}

// 注：`row_to_json` 已提取至 `services/mod.rs`（消除 DRY 重复，2026-07-26）
