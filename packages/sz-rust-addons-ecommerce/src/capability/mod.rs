//! 电商插件能力实现模块。
//!
//! 提供 6 个 Capability 实现（3 订单 + 3 购物车），对齐 design.md 2.2.2.6 节。

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_capability::{CapError, CapResult, Capability, CapabilitySource};

use crate::controller::cart::CartController;
use crate::controller::order::OrderController;
use crate::model::order::Order;
use crate::model::order_item::OrderItem;
use crate::EcommerceState;
use sz_rust_core::orm::repository::Repository;

fn controller_result_to_cap_result(value: Value) -> CapResult<Value> {
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 {
        Ok(value)
    } else {
        let msg = value
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string();
        if code == 404 {
            Err(CapError::NotFound(msg))
        } else if code == 422 || code == 400 {
            Err(CapError::ValidationError(msg))
        } else {
            Err(CapError::ExecutionError(msg))
        }
    }
}

// ============================================================================
// 订单类 Capability（3 个）
// ============================================================================

/// 创建订单能力。校验 items 非空，自动生成订单号，计算总金额。
pub struct CreateOrderCapability {
    state: EcommerceState,
}
impl CreateOrderCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for CreateOrderCapability {
    fn name(&self) -> &'static str {
        "ecommerce.create_order"
    }
    fn description(&self) -> &'static str {
        "创建订单，校验 items 非空，自动生成订单号，计算总金额"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_id": { "type": "integer" },
                "shipping_address": { "type": "string" },
                "remark": { "type": "string" },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "product_id": { "type": "integer" },
                            "product_name": { "type": "string" },
                            "unit_price": { "type": "number" },
                            "quantity": { "type": "integer", "minimum": 1 }
                        }
                    }
                }
            },
            "required": ["user_id", "items"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "order", "create", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let user_id = args
            .get("user_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("user_id is required".to_string()))?;
        let items = args
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CapError::ValidationError("items is required".to_string()))?;
        if items.is_empty() {
            return Err(CapError::ValidationError("订单项列表不可为空".to_string()));
        }
        let shipping_address = args
            .get("shipping_address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let remark = args
            .get("remark")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let now = chrono::Utc::now().timestamp();
        let order_no = format!("ORD-{}-{}", now, user_id);

        let mut total_amount: f64 = 0.0;
        let mut order_items: Vec<OrderItem> = Vec::with_capacity(items.len());
        for (idx, item) in items.iter().enumerate() {
            let product_id = item.get("product_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let product_name = item
                .get("product_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let unit_price = item
                .get("unit_price")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(0);
            let subtotal = unit_price * quantity as f64;
            total_amount += subtotal;
            order_items.push(OrderItem {
                id: (idx as i64) + 1,
                order_id: 0,
                product_id,
                product_name,
                unit_price,
                quantity,
                subtotal,
                created_at: now,
                updated_at: now,
            });
        }

        let order = Order {
            id: 0,
            order_no: order_no.clone(),
            user_id,
            total_amount,
            paid_amount: 0.0,
            status: "pending".to_string(),
            shipping_address,
            remark,
            created_at: now,
            updated_at: now,
        };
        let order_repo = &*self.state.orders;
        let item_repo = &*self.state.order_items;
        match order_repo.save(order) {
            Ok(saved_order) => {
                let order_id = saved_order.id;
                for mut oi in order_items {
                    oi.order_id = order_id;
                    let _ = item_repo.save(oi);
                }
                Ok(json!({
                    "code": 0,
                    "msg": "created",
                    "data": {
                        "order": saved_order,
                        "items_count": items.len()
                    }
                }))
            }
            Err(e) => Err(CapError::ExecutionError(e.to_string())),
        }
    }
}

/// 搜索订单能力。支持状态过滤与分页。
pub struct SearchOrderCapability {
    state: EcommerceState,
}
impl SearchOrderCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for SearchOrderCapability {
    fn name(&self) -> &'static str {
        "ecommerce.search_order"
    }
    fn description(&self) -> &'static str {
        "搜索订单列表，支持状态过滤与分页"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            }
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "order", "search", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let result = OrderController::list(&*self.state.orders, page, page_size, status).await;
        controller_result_to_cap_result(result)
    }
}

/// 取消订单能力。需要人工确认。
pub struct CancelOrderCapability {
    state: EcommerceState,
}
impl CancelOrderCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for CancelOrderCapability {
    fn name(&self) -> &'static str {
        "ecommerce.cancel_order"
    }
    fn description(&self) -> &'static str {
        "取消订单，仅 pending/paid 状态可取消"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" }
            },
            "required": ["id"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "order", "cancel", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("id is required".to_string()))?;
        let result = OrderController::cancel(&*self.state.orders, id).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 购物车类 Capability（3 个）
// ============================================================================

/// 查询购物车能力。返回指定用户全部购物车项。
pub struct QueryCartCapability {
    state: EcommerceState,
}
impl QueryCartCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for QueryCartCapability {
    fn name(&self) -> &'static str {
        "ecommerce.query_cart"
    }
    fn description(&self) -> &'static str {
        "查询指定用户购物车列表"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_id": { "type": "integer" }
            },
            "required": ["user_id"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "cart", "query", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let user_id = args
            .get("user_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("user_id is required".to_string()))?;
        let result = CartController::list(&*self.state.carts, user_id).await;
        controller_result_to_cap_result(result)
    }
}

/// 添加购物车能力。校验 quantity > 0，调用累加版 CartController::add。
pub struct AddToCartCapability {
    state: EcommerceState,
}
impl AddToCartCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for AddToCartCapability {
    fn name(&self) -> &'static str {
        "ecommerce.add_to_cart"
    }
    fn description(&self) -> &'static str {
        "添加商品到购物车，同用户同商品数量累加"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_id": { "type": "integer" },
                "product_id": { "type": "integer" },
                "quantity": { "type": "integer", "minimum": 1 },
                "selected": { "type": "boolean" }
            },
            "required": ["user_id", "product_id", "quantity"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "cart", "add", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let quantity = args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(0);
        if quantity <= 0 {
            return Err(CapError::ValidationError("商品数量必须大于 0".to_string()));
        }
        let mut body = args;
        if body.get("id").is_none() {
            body["id"] = json!(0);
        }
        if body.get("selected").is_none() {
            body["selected"] = json!(false);
        }
        if body.get("created_at").is_none() {
            body["created_at"] = json!(0);
        }
        if body.get("updated_at").is_none() {
            body["updated_at"] = json!(0);
        }
        let result = CartController::add(&*self.state.carts, body).await;
        controller_result_to_cap_result(result)
    }
}

/// 清空购物车能力。需要人工确认。
pub struct ClearCartCapability {
    state: EcommerceState,
}
impl ClearCartCapability {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for ClearCartCapability {
    fn name(&self) -> &'static str {
        "ecommerce.clear_cart"
    }
    fn description(&self) -> &'static str {
        "清空指定用户全部购物车项"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_id": { "type": "integer" }
            },
            "required": ["user_id"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["ecommerce", "cart", "clear", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let user_id = args
            .get("user_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("user_id is required".to_string()))?;
        let result = CartController::clear(&*self.state.carts, user_id).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// EcommercePlugin — CapabilityHook 实现
// ============================================================================

use std::sync::Arc;
use sz_rust_addons_loader::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;

/// 电商插件 CapabilityHook 实现。
pub struct EcommercePlugin {
    state: EcommerceState,
}
impl EcommercePlugin {
    pub fn new(state: EcommerceState) -> Self {
        Self { state }
    }
}

pub const ECOMMERCE_CAPABILITY_NAMES: [&str; 6] = [
    "ecommerce.create_order",
    "ecommerce.search_order",
    "ecommerce.cancel_order",
    "ecommerce.query_cart",
    "ecommerce.add_to_cart",
    "ecommerce.clear_cart",
];

impl CapabilityHook for EcommercePlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(CreateOrderCapability::new(self.state.clone())),
            Arc::new(SearchOrderCapability::new(self.state.clone())),
            Arc::new(CancelOrderCapability::new(self.state.clone())),
            Arc::new(QueryCartCapability::new(self.state.clone())),
            Arc::new(AddToCartCapability::new(self.state.clone())),
            Arc::new(ClearCartCapability::new(self.state.clone())),
        ];
        let mut names = Vec::with_capacity(caps.len());
        for cap in caps {
            let name = cap.name().to_string();
            registry.register(cap);
            names.push(name);
        }
        Ok(names)
    }

    fn capability_names(&self) -> Vec<String> {
        ECOMMERCE_CAPABILITY_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::cart::CartItem;
    use sz_rust_core::orm::repository::Repository;

    fn test_state() -> EcommerceState {
        EcommerceState::default()
    }

    // --- 订单类测试 ---

    #[tokio::test]
    async fn create_order_capability_validates_empty_items() {
        let state = test_state();
        let cap = CreateOrderCapability::new(state);
        let result = cap.call(json!({"user_id": 1, "items": []})).await;
        assert!(result.is_err());
        match result {
            Err(CapError::ValidationError(msg)) => assert!(msg.contains("订单项列表不可为空")),
            _ => panic!("expected ValidationError"),
        }
    }

    #[tokio::test]
    async fn create_order_capability_creates_order_and_items() {
        let state = test_state();
        let cap = CreateOrderCapability::new(state.clone());
        let result = cap
            .call(json!({
                "user_id": 1,
                "shipping_address": "北京",
                "items": [
                    {"product_id": 10, "product_name": "手机", "unit_price": 2999.0, "quantity": 2},
                    {"product_id": 20, "product_name": "耳机", "unit_price": 99.0, "quantity": 1}
                ]
            }))
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["order"]["status"], "pending");
        assert_eq!(result["data"]["order"]["total_amount"], 6097.0);
        assert_eq!(result["data"]["order"]["paid_amount"], 0.0);
        assert_eq!(result["data"]["items_count"], 2);
        assert!(result["data"]["order"]["order_no"]
            .as_str()
            .unwrap()
            .starts_with("ORD-"));
    }

    #[tokio::test]
    async fn search_order_capability_returns_results() {
        let state = test_state();
        state
            .orders
            .save(Order {
                id: 1,
                order_no: "ORD001".to_string(),
                user_id: 1,
                total_amount: 100.0,
                paid_amount: 0.0,
                status: "pending".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchOrderCapability::new(state.clone());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn search_order_capability_filters_by_status() {
        let state = test_state();
        state
            .orders
            .save(Order {
                id: 1,
                order_no: "ORD001".to_string(),
                user_id: 1,
                status: "pending".to_string(),
                ..Default::default()
            })
            .unwrap();
        state
            .orders
            .save(Order {
                id: 2,
                order_no: "ORD002".to_string(),
                user_id: 1,
                status: "paid".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchOrderCapability::new(state.clone());
        let result = cap.call(json!({"status": "paid"})).await.unwrap();
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn cancel_order_capability_requires_confirmation() {
        let state = test_state();
        let cap = CancelOrderCapability::new(state);
        assert!(cap.requires_confirmation());
    }

    #[tokio::test]
    async fn cancel_order_capability_cancels_pending() {
        let state = test_state();
        state
            .orders
            .save(Order {
                id: 1,
                order_no: "ORD001".to_string(),
                user_id: 1,
                status: "pending".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = CancelOrderCapability::new(state.clone());
        let result = cap.call(json!({"id": 1})).await.unwrap();
        assert_eq!(result["data"]["status"], "cancelled");
    }

    #[tokio::test]
    async fn cancel_order_capability_not_found() {
        let state = test_state();
        let cap = CancelOrderCapability::new(state);
        let result = cap.call(json!({"id": 999})).await;
        assert!(matches!(result, Err(CapError::NotFound(_))));
    }

    // --- 购物车类测试 ---

    #[tokio::test]
    async fn query_cart_capability_returns_user_items() {
        let state = test_state();
        state
            .carts
            .save(CartItem {
                id: 1,
                user_id: 10,
                product_id: 100,
                quantity: 2,
                ..Default::default()
            })
            .unwrap();
        let cap = QueryCartCapability::new(state.clone());
        let result = cap.call(json!({"user_id": 10})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total_count"], 1);
    }

    #[tokio::test]
    async fn add_to_cart_capability_validates_quantity() {
        let state = test_state();
        let cap = AddToCartCapability::new(state);
        let result = cap
            .call(json!({"user_id": 1, "product_id": 10, "quantity": 0}))
            .await;
        assert!(matches!(result, Err(CapError::ValidationError(_))));
    }

    #[tokio::test]
    async fn add_to_cart_capability_adds_new_item() {
        let state = test_state();
        let cap = AddToCartCapability::new(state.clone());
        let result = cap
            .call(json!({"user_id": 1, "product_id": 10, "quantity": 3}))
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["quantity"], 3);
    }

    #[tokio::test]
    async fn add_to_cart_capability_merges_same_item() {
        let state = test_state();
        let cap = AddToCartCapability::new(state.clone());
        cap.call(json!({"user_id": 1, "product_id": 10, "quantity": 2}))
            .await
            .unwrap();
        let result = cap
            .call(json!({"user_id": 1, "product_id": 10, "quantity": 3}))
            .await
            .unwrap();
        assert_eq!(result["msg"], "merged");
        assert_eq!(result["data"]["quantity"], 5);
    }

    #[tokio::test]
    async fn clear_cart_capability_requires_confirmation() {
        let state = test_state();
        let cap = ClearCartCapability::new(state);
        assert!(cap.requires_confirmation());
    }

    #[tokio::test]
    async fn clear_cart_capability_clears_user_cart() {
        let state = test_state();
        state
            .carts
            .save(CartItem {
                id: 1,
                user_id: 5,
                product_id: 10,
                quantity: 1,
                ..Default::default()
            })
            .unwrap();
        state
            .carts
            .save(CartItem {
                id: 2,
                user_id: 5,
                product_id: 20,
                quantity: 1,
                ..Default::default()
            })
            .unwrap();
        let cap = ClearCartCapability::new(state.clone());
        let result = cap.call(json!({"user_id": 5})).await.unwrap();
        assert_eq!(result["data"]["rows"], 2);
    }

    // --- Hook 测试 ---

    #[tokio::test]
    async fn ecommerce_plugin_registers_6_capabilities() {
        let state = test_state();
        let plugin = EcommercePlugin::new(state);
        let registry = CapabilityRegistry::new();
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"ecommerce.create_order".to_string()));
        assert!(names.contains(&"ecommerce.clear_cart".to_string()));
    }

    #[tokio::test]
    async fn ecommerce_plugin_capability_names() {
        let state = test_state();
        let plugin = EcommercePlugin::new(state);
        let names = plugin.capability_names();
        assert_eq!(names.len(), 6);
        assert_eq!(names[0], "ecommerce.create_order");
        assert_eq!(names[5], "ecommerce.clear_cart");
    }
}
