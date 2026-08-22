//! ecommerce 插件接入 sz300 端到端测试
//!
//! 验证 sz300 router.rs 中 register_routes 接线正确：
//! - /api/ecommerce/orders CRUD 路由可达
//! - /api/ecommerce/cart 路由可达
//! - EcommerceState 通过闭包捕获传递，不依赖 AppState

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_addons_ecommerce::{register_routes, EcommerceState};
use sz_rust_core::router::RouterBuilder;
use tower::ServiceExt;

fn build_ecommerce_router() -> axum::Router {
    let ec_state = EcommerceState::default();
    let builder = RouterBuilder::new();
    let builder = register_routes(builder, ec_state);
    builder.build()
}

async fn get_status(router: axum::Router, req: Request<Body>) -> StatusCode {
    router.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn ecommerce_orders_list_reachable() {
    let router = build_ecommerce_router();
    let req = Request::builder()
        .uri("/api/ecommerce/orders?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ecommerce_orders_create_reachable() {
    let router = build_ecommerce_router();
    let body = serde_json::json!({
        "id": 0,
        "order_no": "TEST-001",
        "user_id": 1,
        "merchant_id": 1,
        "total_amount": 100.0,
        "status": "pending",
        "created_at": 0,
        "updated_at": 0
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/ecommerce/orders")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ecommerce_cart_list_reachable() {
    let router = build_ecommerce_router();
    let req = Request::builder()
        .uri("/api/ecommerce/cart?user_id=1")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ecommerce_order_items_list_reachable() {
    let router = build_ecommerce_router();
    let req = Request::builder()
        .uri("/api/ecommerce/order_items?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ecommerce_orders_cancel_reachable() {
    let router = build_ecommerce_router();
    let req = Request::builder()
        .method("POST")
        .uri("/api/ecommerce/orders/1/cancel")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ecommerce_orders_pay_reachable() {
    let router = build_ecommerce_router();
    let req = Request::builder()
        .method("POST")
        .uri("/api/ecommerce/orders/1/pay")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK);
}
