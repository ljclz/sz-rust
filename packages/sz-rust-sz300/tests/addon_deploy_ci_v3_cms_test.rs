//! addon_deploy_ci_v3 — CMS addon 接线端到端测试
//!
//! 验证 sz300 router.rs 中 CMS register_routes 接线正确：
//! - /api/cms/articles CRUD 路由可达（非 404）
//! - /api/cms/categories CRUD 路由可达
//! - /api/cms/tags CRUD 路由可达

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_addons_cms::{register_routes, CmsState};
use sz_rust_core::router::RouterBuilder;
use tower::ServiceExt;

fn build_cms_router() -> axum::Router {
    let state = CmsState::default();
    let builder = RouterBuilder::new();
    let builder = register_routes(builder, state);
    builder.build()
}

async fn get_status(router: axum::Router, req: Request<Body>) -> StatusCode {
    router.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn cms_articles_list_reachable() {
    let router = build_cms_router();
    let req = Request::builder()
        .uri("/api/cms/articles?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CMS articles list 路由应可达");
}

#[tokio::test]
async fn cms_categories_list_reachable() {
    let router = build_cms_router();
    let req = Request::builder()
        .uri("/api/cms/categories?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CMS categories list 路由应可达");
}

#[tokio::test]
async fn cms_tags_list_reachable() {
    let router = build_cms_router();
    let req = Request::builder()
        .uri("/api/cms/tags?page=1&page_size=20")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CMS tags list 路由应可达");
}

#[tokio::test]
async fn cms_article_detail_reachable() {
    let router = build_cms_router();
    let req = Request::builder()
        .uri("/api/cms/articles/1")
        .body(Body::empty())
        .unwrap();
    let status = get_status(router, req).await;
    assert_eq!(status, StatusCode::OK, "CMS article detail 路由应可达");
}
