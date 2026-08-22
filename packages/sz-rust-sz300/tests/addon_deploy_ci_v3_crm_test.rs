//! addon_deploy_ci_v3 — CRM addon 接线端到端测试
//!
//! 验证 sz300 router.rs 中 CRM register_routes 接线正确：
//! - /api/crm/contacts CRUD 路由可达（非 404）
//! - /api/crm/leads CRUD 路由可达
//! - /api/crm/deals CRUD 路由可达
//! - CRM 路由参数使用 axum 0.8 `{id}` 格式（不 panic）

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_addons_crm::{register_routes, CrmState};
use sz_rust_core::router::RouterBuilder;
use tower::ServiceExt;

fn build_crm_router() -> axum::Router {
    let state = CrmState::default();
    let builder = RouterBuilder::new();
    let builder = register_routes(builder, state);
    builder.build()
}

async fn get_status(router: axum::Router, req: Request<Body>) -> StatusCode {
    router.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn crm_contacts_list_reachable() {
    let router = build_crm_router();
    let req = Request::builder()
        .uri("/api/crm/contacts?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CRM contacts list 路由应可达");
}

#[tokio::test]
async fn crm_leads_list_reachable() {
    let router = build_crm_router();
    let req = Request::builder()
        .uri("/api/crm/leads?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CRM leads list 路由应可达");
}

#[tokio::test]
async fn crm_deals_list_reachable() {
    let router = build_crm_router();
    let req = Request::builder()
        .uri("/api/crm/deals?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CRM deals list 路由应可达");
}

#[tokio::test]
async fn crm_contact_detail_reachable() {
    let router = build_crm_router();
    let req = Request::builder()
        .uri("/api/crm/contacts/1")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "CRM contact detail 路由应可达（{{id}} 格式不 panic）"
    );
}
