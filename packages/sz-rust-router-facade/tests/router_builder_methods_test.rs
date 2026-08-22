//! 任务组 13.2：RouterBuilder patch/head/options 路由分发测试

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_router_facade::router::RouterBuilder;
use tower::ServiceExt;

#[tokio::test]
async fn patch_route_dispatches_to_handler() {
    let router = RouterBuilder::new()
        .patch("/update", || async { "patched" })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/update")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"patched");
}

#[tokio::test]
async fn head_route_dispatches_to_handler() {
    let router = RouterBuilder::new()
        .head("/meta", || async { "head-response" })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn options_route_dispatches_to_handler() {
    let router = RouterBuilder::new()
        .options("/cors", || async { "options-response" })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/cors")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"options-response");
}

#[tokio::test]
async fn patch_route_returns_404_for_wrong_method() {
    let router = RouterBuilder::new()
        .patch("/update", || async { "patched" })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/update")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn all_http_methods_coexist() {
    let router = RouterBuilder::new()
        .get("/g", || async { "get" })
        .post("/p", || async { "post" })
        .put("/pu", || async { "put" })
        .delete("/d", || async { "delete" })
        .patch("/pa", || async { "patch" })
        .head("/h", || async { "head" })
        .options("/o", || async { "options" })
        .build();

    for (method, path, expected) in [
        ("GET", "/g", "get"),
        ("POST", "/p", "post"),
        ("PUT", "/pu", "put"),
        ("DELETE", "/d", "delete"),
        ("PATCH", "/pa", "patch"),
        ("OPTIONS", "/o", "options"),
    ] {
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK, "method {method} on {path}");
        if method != "HEAD" {
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], expected.as_bytes(), "body for {method} {path}");
        }
    }
}
