//! 电商示例 — 商品/订单/支付 + OpenAPI 自动生成（4.3 竞争力深化：完整示例项目）
//!
//! 演示 sz-rust 框架的多 facade 协作：
//! - orm facade：参数化查询构建（防注入，显式列投影）
//! - pay facade：支付流程（PayOrder → MemoryPayProvider → PayResult）
//! - router facade：OpenAPI spec 从路由配置自动生成（`spec_from_route_config`）
//! - http facade：统一 API 响应
//!
//! ## 端点
//!
//! | 方法 | 路径 | 说明 |
//! |------|------|------|
//! | GET  | /product/list | 商品列表（参数化查询） |
//! | POST | /order/create | 下单（body: {"product_id","quantity"}）|
//! | POST | /order/pay/{order_no} | 支付订单 |
//! | GET  | /order/query/{order_no} | 查询订单（参数化 WHERE）|
//! | GET  | /openapi.json | 自动生成的 OpenAPI spec |
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin ecommerce_demo
//! ```
//!
//! 使用内存存储 + MemoryPayProvider，无需数据库/支付网关。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sz_rust_core::openapi::spec_from_route_config;
use sz_rust_core::openapi::OpenApiBuilder;
use sz_rust_core::orm::{DbType, SelectQuery};
use sz_rust_core::pay::{MemoryPayProvider, PayOrder, PayProvider};
use sz_rust_core::routing::{HttpMethod, RouteConfig, RouteRule};

// ============================================================================
// 模型层
// ============================================================================

/// 商品
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Product {
    id: i64,
    name: String,
    price: i64, // 单位：分
}

/// 订单
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Order {
    order_no: String,
    product_id: i64,
    quantity: i64,
    total_amount: i64,
    status: String, // pending / paid
}

// ============================================================================
// 共享状态
// ============================================================================

struct AppState {
    products: Mutex<Vec<Product>>,
    orders: Mutex<Vec<Order>>,
    next_id: AtomicI64,
    pay_provider: MemoryPayProvider,
}

impl AppState {
    fn new() -> Self {
        Self {
            products: Mutex::new(vec![
                Product {
                    id: 1,
                    name: "鲜视达礼盒".to_string(),
                    price: 8800_i64,
                },
                Product {
                    id: 2,
                    name: "Rust 实战教程".to_string(),
                    price: 6600_i64,
                },
            ]),
            orders: Mutex::new(Vec::new()),
            next_id: AtomicI64::new(1000),
            pay_provider: MemoryPayProvider::new(),
        }
    }

    fn create_order(&self, product_id: i64, quantity: i64) -> Option<Order> {
        let product = self
            .products
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|p| p.id == product_id)
            .cloned()?;
        let seq = self.next_id.fetch_add(1, Ordering::SeqCst);
        let order = Order {
            order_no: format!("E{seq}"),
            product_id,
            quantity,
            total_amount: product.price * quantity,
            status: "pending".to_string(),
        };
        self.orders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(order.clone());
        Some(order)
    }

    fn pay_order(&self, order_no: &str) -> Option<Order> {
        let order = self
            .orders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|o| o.order_no == order_no)
            .cloned()?;
        // pay facade：构建支付订单并调用支付 provider
        let pay_order = PayOrder::new()
            .out_trade_no(order.order_no.clone())
            .total_amount(order.total_amount)
            .subject("电商示例订单")
            .notify_url("https://example.com/notify");
        let result = self.pay_provider.pay(pay_order).ok()?;

        let mut orders = self.orders.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(o) = orders.iter_mut().find(|o| o.order_no == order_no) {
            o.status = if result.trade_status == "WAIT_BUYER_PAY" {
                "paid"
            } else {
                "failed"
            }
            .to_string();
            return Some(o.clone());
        }
        None
    }

    fn find_order(&self, order_no: &str) -> Option<Order> {
        self.orders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|o| o.order_no == order_no)
            .cloned()
    }
}

// ============================================================================
// 处理器
// ============================================================================

async fn list_products(State(state): State<Arc<AppState>>) -> axum::response::Response {
    // orm facade：显式列投影（禁止 SELECT *）
    let sql = SelectQuery::new()
        .columns(&["id", "name", "price"])
        .from("products")
        .build(DbType::MySQL);
    tracing::info!("参数化查询：{sql}");
    let products = state
        .products
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    sz_rust_core::response::render_success(json!(products), "ok")
}

async fn create_order(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> axum::response::Response {
    let product_id = payload["product_id"].as_i64().unwrap_or(0);
    let quantity = payload["quantity"].as_i64().unwrap_or(1);
    match state.create_order(product_id, quantity) {
        Some(order) => sz_rust_core::response::render_success(json!(order), "下单成功"),
        None => sz_rust_core::response::render_error("商品不存在"),
    }
}

async fn pay_order(
    State(state): State<Arc<AppState>>,
    Path(order_no): Path<String>,
) -> axum::response::Response {
    match state.pay_order(&order_no) {
        Some(order) => sz_rust_core::response::render_success(json!(order), "支付处理完成"),
        None => sz_rust_core::response::render_error("订单不存在"),
    }
}

async fn query_order(
    State(state): State<Arc<AppState>>,
    Path(order_no): Path<String>,
) -> axum::response::Response {
    // orm facade：参数化 WHERE（防注入）
    let sql = SelectQuery::new()
        .columns(&["order_no", "total_amount", "status"])
        .from("orders")
        .where_clause("order_no = ?")
        .build(DbType::MySQL);
    tracing::info!("参数化查询：{sql}");
    match state.find_order(&order_no) {
        Some(order) => sz_rust_core::response::render_success(json!(order), "ok"),
        None => sz_rust_core::response::render_error("订单不存在"),
    }
}

async fn openapi_spec() -> axum::response::Response {
    // router facade：从路由配置自动生成 OpenAPI spec
    let mut config = RouteConfig::new();
    config.add_route(RouteRule::new(
        HttpMethod::GET,
        "/product/list",
        "Product@list",
    ));
    config.add_route(RouteRule::new(
        HttpMethod::POST,
        "/order/create",
        "Order@create",
    ));
    config.add_route(RouteRule::new(
        HttpMethod::POST,
        "/order/pay/{order_no}",
        "Order@pay",
    ));
    config.add_route(RouteRule::new(
        HttpMethod::GET,
        "/order/query/{order_no}",
        "Order@query",
    ));

    let spec = spec_from_route_config(
        OpenApiBuilder::new("鲜视达电商 API", "1.0.0")
            .description("电商示例自动生成的 OpenAPI 文档")
            .bearer_auth("BearerAuth"),
        &config,
    );
    axum::response::Response::builder()
        .header("content-type", "application/json")
        .body(axum::body::Body::from(spec))
        .expect("响应体构造失败")
}

// ============================================================================
// 入口
// ============================================================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let state = Arc::new(AppState::new());
    let app = Router::new()
        .route("/product/list", get(list_products))
        .route("/order/create", post(create_order))
        .route("/order/pay/{order_no}", post(pay_order))
        .route("/order/query/{order_no}", get(query_order))
        .route("/openapi.json", get(openapi_spec))
        .with_state(state);

    let addr = "127.0.0.1:8082";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定监听地址失败");
    tracing::info!(
        "电商示例运行于 http://{addr} （/product/list /order/create /order/pay/{{no}} /openapi.json）"
    );
    axum::serve(listener, app).await.expect("HTTP 服务启动失败");
}
