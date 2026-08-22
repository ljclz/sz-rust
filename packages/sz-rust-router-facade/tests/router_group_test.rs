//! 任务组 21.2：路由分组 group/nest 链式调用测试

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_router_facade::router::RouterBuilder;
use tower::ServiceExt;

#[tokio::test]
async fn group_prefixes_routes_correctly() {
    let router = RouterBuilder::new()
        .group("/api", |group| {
            group.get("/users", || async { "list users" })
        })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"list users");
}

#[tokio::test]
async fn group_with_multiple_routes() {
    let router = RouterBuilder::new()
        .group("/api", |group| {
            group
                .get("/users", || async { "list" })
                .post("/users", || async { "create" })
                .get("/items", || async { "items" })
        })
        .build();

    for (method, path, expected) in [
        ("GET", "/api/users", "list"),
        ("POST", "/api/users", "create"),
        ("GET", "/api/items", "items"),
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

        assert_eq!(resp.status(), StatusCode::OK, "{method} {path}");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], expected.as_bytes(), "{method} {path}");
    }
}

#[tokio::test]
async fn nest_embeds_router_at_prefix() {
    let sub_router = RouterBuilder::new()
        .get("/health", || async { "ok" })
        .build();

    let router = RouterBuilder::new().nest("/api", sub_router).build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn group_chained_with_top_level_routes() {
    let router = RouterBuilder::new()
        .get("/health", || async { "health" })
        .group("/api", |group| group.get("/users", || async { "users" }))
        .build();

    let resp1 = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    let resp2 = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
}

#[tokio::test]
async fn nested_groups() {
    let router = RouterBuilder::new()
        .group("/api", |api| {
            api.group("/v1", |v1| v1.get("/users", || async { "v1 users" }))
        })
        .build();

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"v1 users");
}
