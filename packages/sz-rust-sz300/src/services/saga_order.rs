// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 Saga 分布式事务接入订单业务
//!
//! 对应 tasks.md §5.1-5.4：定义订单 Saga 步骤与补偿 Action、抽象 StockService/PaymentService trait、
//! 实现 DbTxLogStore 持久化、OrderService::create_with_saga。
//!
//! ## Saga 流程
//!
//! 1. CreateOrderAction（正向：插入订单+订单项；补偿：删除订单+订单项）
//! 2. DeductStockAction（正向：扣减商品库存；补偿：恢复库存）
//! 3. InitiatePaymentAction（正向：更新订单状态为已支付；补偿：更新为已取消）

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_core::orm::{Pool, Value as DbValue};
use sz_rust_distributed_tx::{
    DtxError, InMemoryTxLogStore, SagaAction, SagaOrchestrator, SagaStep, StepStatus, TxLogEntry,
    TxLogStore, TxState, TxType,
};

use crate::models::order::Order;
use crate::models::order_item::OrderItem;

// ============================================================================
// 任务 5.2: StockService 与 PaymentService trait + DB 实现
// ============================================================================

/// 库存服务 trait（v1.9.0 Saga 扣减/恢复库存）
#[async_trait]
pub trait StockService: Send + Sync + 'static {
    /// 扣减库存（正向操作）
    async fn deduct(&self, product_id: i64, quantity: i32) -> Result<(), DtxError>;
    /// 恢复库存（补偿操作）
    async fn restore(&self, product_id: i64, quantity: i32) -> Result<(), DtxError>;
}

/// 支付服务 trait（v1.9.0 Saga 发起/取消支付）
#[async_trait]
pub trait PaymentService: Send + Sync + 'static {
    /// 发起支付（正向操作）— 返回支付 ID
    async fn initiate(&self, order_id: i64, amount_fen: i64) -> Result<String, DtxError>;
    /// 取消支付（补偿操作）
    async fn cancel(&self, payment_id: &str) -> Result<(), DtxError>;
}

/// DB 库存服务实现 — 操作 `product` 表的 `stock` 字段
pub struct DbStockService {
    pool: Arc<Pool>,
}

impl DbStockService {
    /// 创建实例
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StockService for DbStockService {
    async fn deduct(&self, product_id: i64, quantity: i32) -> Result<(), DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;
        let sql = "UPDATE product SET stock = stock - ? WHERE product_id = ? AND stock >= ?";
        let params = [
            DbValue::I32(quantity),
            DbValue::I64(product_id),
            DbValue::I32(quantity),
        ];
        let affected = conn
            .execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Generic(format!("扣减库存失败: {}", e)))?;
        if affected == 0 {
            return Err(DtxError::Generic(format!(
                "库存不足或商品不存在: product_id={}, need={}",
                product_id, quantity
            )));
        }
        Ok(())
    }

    async fn restore(&self, product_id: i64, quantity: i32) -> Result<(), DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;
        let sql = "UPDATE product SET stock = stock + ? WHERE product_id = ?";
        let params = [DbValue::I32(quantity), DbValue::I64(product_id)];
        conn.execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Generic(format!("恢复库存失败: {}", e)))?;
        Ok(())
    }
}

/// DB 支付服务实现 — v1.9.0 模拟支付（更新订单状态 + 生成支付 ID）
pub struct DbPaymentService {
    pool: Arc<Pool>,
}

impl DbPaymentService {
    /// 创建实例
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PaymentService for DbPaymentService {
    async fn initiate(&self, order_id: i64, amount_fen: i64) -> Result<String, DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;
        let sql = "UPDATE `order` SET status = 2, pay_at = NOW(), updated_at = NOW() \
                   WHERE order_id = ? AND status = 1";
        let params = [DbValue::I64(order_id)];
        let affected = conn
            .execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Generic(format!("发起支付失败: {}", e)))?;
        if affected == 0 {
            return Err(DtxError::Generic(format!(
                "订单不存在或状态非待支付: order_id={}",
                order_id
            )));
        }
        Ok(format!("pay-{}-{}", order_id, amount_fen))
    }

    async fn cancel(&self, payment_id: &str) -> Result<(), DtxError> {
        let order_id: i64 = payment_id
            .strip_prefix("pay-")
            .and_then(|s| s.split('-').next())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| DtxError::Generic(format!("无效的 payment_id: {}", payment_id)))?;

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;
        let sql = "UPDATE `order` SET status = 0, updated_at = NOW() WHERE order_id = ?";
        let params = [DbValue::I64(order_id)];
        conn.execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Generic(format!("取消支付失败: {}", e)))?;
        Ok(())
    }
}

// ============================================================================
// 任务 5.1: 订单 Saga 步骤与补偿 Action
// ============================================================================

/// CreateOrderAction — 正向：插入订单 + 订单项；补偿：删除订单 + 订单项
pub struct CreateOrderAction {
    pool: Arc<Pool>,
}

impl CreateOrderAction {
    /// 创建实例
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SagaAction for CreateOrderAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        let order: Order = serde_json::from_value(
            payload
                .get("order")
                .cloned()
                .ok_or_else(|| DtxError::Generic("payload 缺少 order".into()))?,
        )
        .map_err(|e| DtxError::Serialization(format!("反序列化 order 失败: {}", e)))?;
        let items: Vec<OrderItem> = serde_json::from_value(
            payload
                .get("items")
                .cloned()
                .ok_or_else(|| DtxError::Generic("payload 缺少 items".into()))?,
        )
        .map_err(|e| DtxError::Serialization(format!("反序列化 items 失败: {}", e)))?;

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;

        let order_sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, \
                         total_weight_g, item_count, status, pay_method, offline_seq, \
                         created_at, updated_at) \
                         VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, NOW(), NOW())";
        let order_params = [
            DbValue::String(order.order_no.clone()),
            DbValue::I64(order.merchant_id),
            DbValue::I64(order.device_id),
            DbValue::I64(order.total_fen),
            DbValue::I64(order.total_weight_g),
            DbValue::I32(order.item_count),
            DbValue::I8(order.pay_method),
            DbValue::String(order.offline_seq.clone()),
        ];
        conn.execute_with_params(order_sql, &order_params)
            .await
            .map_err(|e| DtxError::Generic(format!("插入订单失败: {}", e)))?;

        let rows = conn
            .query("SELECT LAST_INSERT_ID() as order_id")
            .await
            .map_err(|e| DtxError::Generic(format!("获取订单 ID 失败: {}", e)))?;
        let order_id = rows
            .first()
            .and_then(|r| r.get("order_id"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| DtxError::Generic("获取 LAST_INSERT_ID 失败".into()))?;

        for item in &items {
            let item_sql = "INSERT INTO order_item (order_id, good_id, good_name, price_fen, \
                            weight_g, total_fen, quantity) VALUES (?, ?, ?, ?, ?, ?, ?)";
            let item_params = [
                DbValue::I64(order_id),
                DbValue::I64(item.good_id),
                DbValue::String(item.good_name.clone()),
                DbValue::I64(item.price_fen),
                DbValue::I64(item.weight_g),
                DbValue::I64(item.total_fen),
                DbValue::I32(item.quantity),
            ];
            conn.execute_with_params(item_sql, &item_params)
                .await
                .map_err(|e| DtxError::Generic(format!("插入订单项失败: {}", e)))?;
        }

        let mut new_payload = payload.clone();
        if let Some(obj) = new_payload.as_object_mut() {
            obj.insert("order_id".to_string(), json!(order_id));
        }
        Ok(new_payload)
    }
}

/// CancelOrderAction — 补偿：删除订单 + 订单项
pub struct CancelOrderAction {
    pool: Arc<Pool>,
}

impl CancelOrderAction {
    /// 创建实例
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SagaAction for CancelOrderAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        let order_id = payload
            .get("order_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| DtxError::Generic("payload 缺少 order_id".into()))?;

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Generic(format!("获取连接失败: {}", e)))?;

        conn.execute_with_params(
            "DELETE FROM order_item WHERE order_id = ?",
            &[DbValue::I64(order_id)],
        )
        .await
        .map_err(|e| DtxError::Generic(format!("删除订单项失败: {}", e)))?;

        conn.execute_with_params(
            "DELETE FROM `order` WHERE order_id = ?",
            &[DbValue::I64(order_id)],
        )
        .await
        .map_err(|e| DtxError::Generic(format!("删除订单失败: {}", e)))?;

        Ok(payload.clone())
    }
}

/// DeductStockAction — 正向：扣减所有订单项的库存
pub struct DeductStockAction {
    stock_service: Arc<dyn StockService>,
}

impl DeductStockAction {
    /// 创建实例
    pub fn new(stock_service: Arc<dyn StockService>) -> Self {
        Self { stock_service }
    }
}

#[async_trait]
impl SagaAction for DeductStockAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        let items: Vec<OrderItem> = serde_json::from_value(
            payload
                .get("items")
                .cloned()
                .ok_or_else(|| DtxError::Generic("payload 缺少 items".into()))?,
        )
        .map_err(|e| DtxError::Serialization(format!("反序列化 items 失败: {}", e)))?;

        for item in &items {
            self.stock_service
                .deduct(item.good_id, item.quantity)
                .await?;
        }

        let mut new_payload = payload.clone();
        if let Some(obj) = new_payload.as_object_mut() {
            obj.insert("stock_deducted".to_string(), json!(true));
        }
        Ok(new_payload)
    }
}

/// RestoreStockAction — 补偿：恢复所有订单项的库存
pub struct RestoreStockAction {
    stock_service: Arc<dyn StockService>,
}

impl RestoreStockAction {
    /// 创建实例
    pub fn new(stock_service: Arc<dyn StockService>) -> Self {
        Self { stock_service }
    }
}

#[async_trait]
impl SagaAction for RestoreStockAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        let items: Vec<OrderItem> = serde_json::from_value(
            payload
                .get("items")
                .cloned()
                .ok_or_else(|| DtxError::Generic("payload 缺少 items".into()))?,
        )
        .map_err(|e| DtxError::Serialization(format!("反序列化 items 失败: {}", e)))?;

        for item in &items {
            self.stock_service
                .restore(item.good_id, item.quantity)
                .await?;
        }
        Ok(payload.clone())
    }
}

/// InitiatePaymentAction — 正向：发起支付
pub struct InitiatePaymentAction {
    payment_service: Arc<dyn PaymentService>,
}

impl InitiatePaymentAction {
    /// 创建实例
    pub fn new(payment_service: Arc<dyn PaymentService>) -> Self {
        Self { payment_service }
    }
}

#[async_trait]
impl SagaAction for InitiatePaymentAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        let order_id = payload
            .get("order_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| DtxError::Generic("payload 缺少 order_id".into()))?;
        let amount_fen = payload
            .get("order")
            .and_then(|o| o.get("total_fen"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let payment_id = self.payment_service.initiate(order_id, amount_fen).await?;

        let mut new_payload = payload.clone();
        if let Some(obj) = new_payload.as_object_mut() {
            obj.insert("payment_id".to_string(), json!(payment_id));
        }
        Ok(new_payload)
    }
}

/// CancelPaymentAction — 补偿：取消支付
pub struct CancelPaymentAction {
    payment_service: Arc<dyn PaymentService>,
}

impl CancelPaymentAction {
    /// 创建实例
    pub fn new(payment_service: Arc<dyn PaymentService>) -> Self {
        Self { payment_service }
    }
}

#[async_trait]
impl SagaAction for CancelPaymentAction {
    async fn execute(&self, payload: &Value) -> Result<Value, DtxError> {
        if let Some(payment_id) = payload.get("payment_id").and_then(|v| v.as_str()) {
            self.payment_service.cancel(payment_id).await?;
        }
        Ok(payload.clone())
    }
}

// ============================================================================
// 任务 5.3: DbTxLogStore 持久化事务状态
// ============================================================================

/// DB 事务日志存储 — 持久化到 `sz_dtx_log` 表
pub struct DbTxLogStore {
    pool: Arc<Pool>,
}

impl DbTxLogStore {
    /// 创建实例
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }
}

fn state_to_str(state: TxState) -> &'static str {
    match state {
        TxState::Running => "running",
        TxState::Completed => "completed",
        TxState::Compensated => "compensated",
        TxState::Cancelled => "cancelled",
        TxState::Failed => "failed",
        TxState::Timeout => "timeout",
    }
}

fn str_to_state(s: &str) -> TxState {
    match s {
        "completed" => TxState::Completed,
        "compensated" => TxState::Compensated,
        "cancelled" => TxState::Cancelled,
        "failed" => TxState::Failed,
        "timeout" => TxState::Timeout,
        _ => TxState::Running,
    }
}

#[async_trait]
impl TxLogStore for DbTxLogStore {
    async fn save(&self, entry: &TxLogEntry) -> Result<(), DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Persistence(format!("获取连接失败: {}", e)))?;
        let steps_json = serde_json::to_string(&entry.steps)
            .map_err(|e| DtxError::Serialization(e.to_string()))?;
        let payload_json = serde_json::to_string(&entry.payload)
            .map_err(|e| DtxError::Serialization(e.to_string()))?;
        let tx_type = match entry.tx_type {
            TxType::Saga => "saga",
            TxType::Tcc => "tcc",
        };
        let state = state_to_str(entry.state);
        let sql = "INSERT INTO sz_dtx_log (tx_id, tx_type, state, steps, payload, \
                   created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) \
                   ON DUPLICATE KEY UPDATE state = ?, payload = ?, updated_at = ?";
        let params = [
            DbValue::String(entry.tx_id.clone()),
            DbValue::String(tx_type.to_string()),
            DbValue::String(state.to_string()),
            DbValue::String(steps_json),
            DbValue::String(payload_json.clone()),
            DbValue::I64(entry.created_at),
            DbValue::I64(entry.updated_at),
            DbValue::String(state.to_string()),
            DbValue::String(payload_json),
            DbValue::I64(entry.updated_at),
        ];
        conn.execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Persistence(format!("保存事务日志失败: {}", e)))?;
        Ok(())
    }

    async fn update_state(
        &self,
        tx_id: &str,
        state: TxState,
        payload: &Value,
    ) -> Result<(), DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Persistence(format!("获取连接失败: {}", e)))?;
        let state_str = state_to_str(state);
        let payload_json =
            serde_json::to_string(payload).map_err(|e| DtxError::Serialization(e.to_string()))?;
        let now = chrono::Utc::now().timestamp_millis();
        let sql = "UPDATE sz_dtx_log SET state = ?, payload = ?, updated_at = ? WHERE tx_id = ?";
        let params = [
            DbValue::String(state_str.to_string()),
            DbValue::String(payload_json),
            DbValue::I64(now),
            DbValue::String(tx_id.to_string()),
        ];
        conn.execute_with_params(sql, &params)
            .await
            .map_err(|e| DtxError::Persistence(format!("更新事务状态失败: {}", e)))?;
        Ok(())
    }

    async fn get(&self, tx_id: &str) -> Result<Option<TxLogEntry>, DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Persistence(format!("获取连接失败: {}", e)))?;
        let sql = "SELECT tx_id, tx_type, state, steps, payload, created_at, updated_at \
                   FROM sz_dtx_log WHERE tx_id = ?";
        let rows = conn
            .query_with_params(sql, &[DbValue::String(tx_id.to_string())])
            .await
            .map_err(|e| DtxError::Persistence(format!("查询事务日志失败: {}", e)))?;
        match rows.into_iter().next() {
            Some(row) => parse_tx_log_entry(&row).map(Some),
            None => Ok(None),
        }
    }

    async fn list_running(&self) -> Result<Vec<TxLogEntry>, DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Persistence(format!("获取连接失败: {}", e)))?;
        let sql = "SELECT tx_id, tx_type, state, steps, payload, created_at, updated_at \
                   FROM sz_dtx_log WHERE state = 'running' ORDER BY created_at";
        let rows = conn
            .query(sql)
            .await
            .map_err(|e| DtxError::Persistence(format!("查询进行中事务失败: {}", e)))?;
        rows.iter().map(parse_tx_log_entry).collect()
    }

    async fn delete(&self, tx_id: &str) -> Result<(), DtxError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| DtxError::Persistence(format!("获取连接失败: {}", e)))?;
        conn.execute_with_params(
            "DELETE FROM sz_dtx_log WHERE tx_id = ?",
            &[DbValue::String(tx_id.to_string())],
        )
        .await
        .map_err(|e| DtxError::Persistence(format!("删除事务日志失败: {}", e)))?;
        Ok(())
    }
}

fn parse_tx_log_entry(
    row: &std::collections::HashMap<String, DbValue>,
) -> Result<TxLogEntry, DtxError> {
    let tx_id = row
        .get("tx_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DtxError::Persistence("缺少 tx_id".into()))?
        .to_string();
    let tx_type = match row.get("tx_type").and_then(|v| v.as_str()) {
        Some("tcc") => TxType::Tcc,
        _ => TxType::Saga,
    };
    let state = row
        .get("state")
        .and_then(|v| v.as_str())
        .map(str_to_state)
        .unwrap_or(TxState::Running);
    let steps: Value = row
        .get("steps")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let payload: Value = row
        .get("payload")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);
    Ok(TxLogEntry {
        tx_id,
        tx_type,
        state,
        steps,
        payload,
        created_at,
        updated_at,
    })
}

// ============================================================================
// 任务 5.4: create_with_saga
// ============================================================================

/// Saga 订单创建结果
#[derive(Debug, Clone)]
pub enum OrderCreateResult {
    /// 三步全部成功
    Success {
        /// 新创建的订单 ID
        order_id: i64,
        /// Saga 事务 ID
        tx_id: String,
    },
    /// 补偿成功（订单已回滚）
    Compensated {
        /// Saga 事务 ID
        tx_id: String,
        /// 失败的步骤 ID
        failed_step: String,
    },
    /// 补偿失败，待人工干预
    ManualIntervention {
        /// Saga 事务 ID
        tx_id: String,
        /// 失败的步骤 ID
        failed_step: String,
    },
}

/// 使用 Saga 分布式事务创建订单
///
/// ## 流程
///
/// 1. CreateOrderAction — 插入订单 + 订单项
/// 2. DeductStockAction — 扣减库存
/// 3. InitiatePaymentAction — 发起支付
///
/// 任一步失败逆序补偿；补偿失败进入待人工干预。
pub async fn create_with_saga(
    pool: Arc<Pool>,
    order: &Order,
    items: &[OrderItem],
    stock_svc: Arc<dyn StockService>,
    payment_svc: Arc<dyn PaymentService>,
    tx_log: Option<Arc<dyn TxLogStore>>,
) -> Result<OrderCreateResult, DtxError> {
    let orchestrator = SagaOrchestrator::new().with_compensate_retries(3);
    let payload = json!({ "order": order, "items": items });

    let steps = vec![
        SagaStep::new(
            "create_order",
            Arc::new(CreateOrderAction::new(pool.clone())),
            Arc::new(CancelOrderAction::new(pool.clone())),
        ),
        SagaStep::new(
            "deduct_stock",
            Arc::new(DeductStockAction::new(stock_svc.clone())),
            Arc::new(RestoreStockAction::new(stock_svc)),
        ),
        SagaStep::new(
            "initiate_payment",
            Arc::new(InitiatePaymentAction::new(payment_svc.clone())),
            Arc::new(CancelPaymentAction::new(payment_svc)),
        ),
    ];

    let result = orchestrator.execute(steps, payload).await?;

    // 持久化事务状态（若提供了 tx_log store）
    if let Some(ref store) = tx_log {
        let final_state = if result.success {
            TxState::Completed
        } else {
            let has_compensate_failure = result
                .steps
                .iter()
                .any(|s| s.status == StepStatus::CompensateFailed);
            if has_compensate_failure {
                TxState::Failed
            } else {
                TxState::Compensated
            }
        };
        let _ = store
            .update_state(&result.tx_id, final_state, &result.final_payload)
            .await;
    }

    if result.success {
        let order_id = result
            .final_payload
            .get("order_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(OrderCreateResult::Success {
            order_id,
            tx_id: result.tx_id,
        })
    } else {
        let failed_step = result.failed_step.unwrap_or_else(|| "unknown".into());
        let has_compensate_failure = result
            .steps
            .iter()
            .any(|s| s.status == StepStatus::CompensateFailed);
        if has_compensate_failure {
            Ok(OrderCreateResult::ManualIntervention {
                tx_id: result.tx_id,
                failed_step,
            })
        } else {
            Ok(OrderCreateResult::Compensated {
                tx_id: result.tx_id,
                failed_step,
            })
        }
    }
}

/// 构造订单 Saga 步骤（供测试/外部调用使用）
pub fn build_order_saga_steps(
    pool: Arc<Pool>,
    stock_svc: Arc<dyn StockService>,
    payment_svc: Arc<dyn PaymentService>,
) -> Vec<SagaStep> {
    vec![
        SagaStep::new(
            "create_order",
            Arc::new(CreateOrderAction::new(pool.clone())),
            Arc::new(CancelOrderAction::new(pool)),
        ),
        SagaStep::new(
            "deduct_stock",
            Arc::new(DeductStockAction::new(stock_svc.clone())),
            Arc::new(RestoreStockAction::new(stock_svc)),
        ),
        SagaStep::new(
            "initiate_payment",
            Arc::new(InitiatePaymentAction::new(payment_svc.clone())),
            Arc::new(CancelPaymentAction::new(payment_svc)),
        ),
    ]
}

/// 复用 InMemoryTxLogStore（测试/降级用）
pub fn in_memory_tx_log_store() -> InMemoryTxLogStore {
    InMemoryTxLogStore::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock 库存服务（测试用）
    struct MockStockService {
        fail: bool,
    }

    #[async_trait]
    impl StockService for MockStockService {
        async fn deduct(&self, _product_id: i64, _quantity: i32) -> Result<(), DtxError> {
            if self.fail {
                Err(DtxError::Generic("mock 扣减库存失败".into()))
            } else {
                Ok(())
            }
        }
        async fn restore(&self, _product_id: i64, _quantity: i32) -> Result<(), DtxError> {
            Ok(())
        }
    }

    /// Mock 支付服务（测试用）
    struct MockPaymentService {
        fail: bool,
    }

    #[async_trait]
    impl PaymentService for MockPaymentService {
        async fn initiate(&self, order_id: i64, amount_fen: i64) -> Result<String, DtxError> {
            if self.fail {
                Err(DtxError::Generic("mock 发起支付失败".into()))
            } else {
                Ok(format!("pay-{}-{}", order_id, amount_fen))
            }
        }
        async fn cancel(&self, _payment_id: &str) -> Result<(), DtxError> {
            Ok(())
        }
    }

    #[test]
    fn test_order_create_result_variants() {
        let success = OrderCreateResult::Success {
            order_id: 42,
            tx_id: "saga-1".into(),
        };
        match success {
            OrderCreateResult::Success { order_id, .. } => assert_eq!(order_id, 42),
            _ => panic!("应为 Success"),
        }

        let compensated = OrderCreateResult::Compensated {
            tx_id: "saga-2".into(),
            failed_step: "deduct_stock".into(),
        };
        match compensated {
            OrderCreateResult::Compensated { failed_step, .. } => {
                assert_eq!(failed_step, "deduct_stock")
            }
            _ => panic!("应为 Compensated"),
        }

        let manual = OrderCreateResult::ManualIntervention {
            tx_id: "saga-3".into(),
            failed_step: "initiate_payment".into(),
        };
        match manual {
            OrderCreateResult::ManualIntervention { failed_step, .. } => {
                assert_eq!(failed_step, "initiate_payment")
            }
            _ => panic!("应为 ManualIntervention"),
        }
    }

    #[test]
    fn test_state_to_str_round_trip() {
        assert_eq!(state_to_str(TxState::Running), "running");
        assert_eq!(state_to_str(TxState::Completed), "completed");
        assert_eq!(state_to_str(TxState::Compensated), "compensated");
        assert_eq!(state_to_str(TxState::Cancelled), "cancelled");
        assert_eq!(state_to_str(TxState::Failed), "failed");
        assert_eq!(state_to_str(TxState::Timeout), "timeout");

        assert_eq!(str_to_state("running"), TxState::Running);
        assert_eq!(str_to_state("completed"), TxState::Completed);
        assert_eq!(str_to_state("failed"), TxState::Failed);
        assert_eq!(str_to_state("unknown"), TxState::Running);
    }

    #[tokio::test]
    async fn test_mock_stock_service_success() {
        let svc = MockStockService { fail: false };
        assert!(svc.deduct(1, 10).await.is_ok());
        assert!(svc.restore(1, 10).await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_stock_service_failure() {
        let svc = MockStockService { fail: true };
        assert!(svc.deduct(1, 10).await.is_err());
        // restore 不受 fail 控制
        assert!(svc.restore(1, 10).await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_payment_service_success() {
        let svc = MockPaymentService { fail: false };
        let pay_id = svc.initiate(100, 5000).await.unwrap();
        assert_eq!(pay_id, "pay-100-5000");
        assert!(svc.cancel("pay-100-5000").await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_payment_service_failure() {
        let svc = MockPaymentService { fail: true };
        assert!(svc.initiate(100, 5000).await.is_err());
        // cancel 不受 fail 控制
        assert!(svc.cancel("pay-100-5000").await.is_ok());
    }

    #[test]
    fn test_in_memory_tx_log_store_creation() {
        let store = in_memory_tx_log_store();
        assert_eq!(store.len(), 0);
    }
}
