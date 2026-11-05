// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P2-5 RBAC / IDOR 专项测试（2026-11-05 新增）
//!
//! 覆盖两条水平越权防线：
//!
//! 1. **controllers 层 IDOR 归属校验**（`controllers::common` 纯逻辑，无 feature gate）：
//!    商户 A 的用户上下文访问商户 B 的资源 → `can_access_merchant_resource_from_user` 返回 false。
//!    这是 sz300 防水平越权的**主防线**——RBAC guard 只校验角色权限（merchant:read 等），
//!    不区分商户归属；真正的"商户 A 不能读商户 B 的订单"由本层校验提供。
//!
//! 2. **RBAC guard 角色权限校验**（`rbac::guard::rbac_guard`，feature `v18-rbac`）：
//!    无 token / 无效 token → 401；有效 token 但无角色权限 → 403。
//!    此处补充"跨商户语义文档化"用例：商户 A 的 token 持 merchant:read 角色 → guard allow
//!    （guard 不区分商户），跨商户拒绝由 controllers 层 IDOR 校验提供。
//!
//! ## 测试设计
//!
//! - 纯逻辑测试（无 `#[ignore]`）：直接调用 `controllers::common` 的 pub fn，
//!   构造 `CurrentUser` 模拟商户 A/B/管理员，断言归属校验结果。
//! - RBAC guard 测试（feature `v18-rbac`）：mock `RbacEngine` + 真实 JWT 签发，
//!   通过 axum `oneshot` 跑 guard 中间件，断言 401/403/200。
//! - 不依赖 DB，不标记 `#[ignore]`，不接入 db-integration.yml。

#![forbid(unsafe_code)]

use sz_rust_sz300::controllers::common::{
    can_access_merchant_resource_from_user, can_create_merchant_from_user,
    effective_merchant_scope_from_user,
};
use sz_rust_sz300::middleware::auth_middleware::CurrentUser;

// ============================================================================
// 测试辅助
// ============================================================================

/// 构造当前用户上下文
fn user(user_id: i64, username: &str, merchant_id: Option<i64>, is_admin: bool) -> CurrentUser {
    CurrentUser {
        user_id,
        username: username.to_string(),
        merchant_id,
        is_admin,
    }
}

/// 商户 A（merchant_id=1，普通用户）
fn merchant_a() -> CurrentUser {
    user(10, "merchant_a", Some(1), false)
}

/// 商户 B（merchant_id=2，普通用户）
fn merchant_b() -> CurrentUser {
    user(20, "merchant_b", Some(2), false)
}

/// 管理员（merchant_id=None，is_admin=true）
fn admin_user() -> CurrentUser {
    user(1, "admin", None, true)
}

// ============================================================================
// Part 2-1: IDOR 水平越权 — 商户 A 访问商户 B 的资源（纯逻辑，主防线）
// ============================================================================

/// 商户 A 访问自己商户的资源 → 允许
#[test]
fn idor_merchant_a_access_own_resource_allowed() {
    let user = merchant_a();
    let resource_merchant_id = Some(1); // 资源属于商户 A
    assert!(
        can_access_merchant_resource_from_user(Some(&user), resource_merchant_id),
        "商户 A 访问自己商户的资源应允许"
    );
}

/// 商户 A 访问商户 B 的资源 → 拒绝（水平越权防护）
#[test]
fn idor_merchant_a_access_merchant_b_resource_denied() {
    let user = merchant_a();
    let resource_merchant_id = Some(2); // 资源属于商户 B
    assert!(
        !can_access_merchant_resource_from_user(Some(&user), resource_merchant_id),
        "商户 A 访问商户 B 的资源必须拒绝（水平越权）"
    );
}

/// 商户 B 访问商户 A 的资源 → 拒绝（对称）
#[test]
fn idor_merchant_b_access_merchant_a_resource_denied() {
    let user = merchant_b();
    let resource_merchant_id = Some(1);
    assert!(
        !can_access_merchant_resource_from_user(Some(&user), resource_merchant_id),
        "商户 B 访问商户 A 的资源必须拒绝（水平越权对称）"
    );
}

/// 管理员访问任意商户资源 → 允许
#[test]
fn idor_admin_access_any_merchant_allowed() {
    let admin = admin_user();
    for mid in [1, 2, 999, -1] {
        assert!(
            can_access_merchant_resource_from_user(Some(&admin), Some(mid)),
            "管理员访问商户 {mid} 的资源应允许"
        );
    }
    // 资源 merchant_id 为 None（未归属）也允许
    assert!(
        can_access_merchant_resource_from_user(Some(&admin), None),
        "管理员访问未归属资源应允许"
    );
}

/// 未认证请求（user=None）→ 拒绝（fail-closed）
#[test]
fn idor_unauthenticated_request_denied() {
    assert!(
        !can_access_merchant_resource_from_user(None, Some(1)),
        "未认证请求必须拒绝（fail-closed）"
    );
    assert!(
        !can_access_merchant_resource_from_user(None, None),
        "未认证请求对未归属资源也必须拒绝"
    );
}

/// 普通用户 merchant_id=None（未归属商户）→ 拒绝访问任何已归属资源
/// 防止"未归属用户"绕过校验
#[test]
fn idor_user_with_none_merchant_denied_for_owned_resource() {
    let unscoped_user = user(30, "unscoped", None, false);
    assert!(
        !can_access_merchant_resource_from_user(Some(&unscoped_user), Some(1)),
        "未归属商户的用户不得访问任何已归属资源"
    );
    assert!(
        !can_access_merchant_resource_from_user(Some(&unscoped_user), Some(2)),
        "未归属商户的用户不得访问商户 B 资源"
    );
}

// ============================================================================
// Part 2-2: IDOR 直接 resource ID 越权（模拟用 other_merchant 的 order_id 访问）
// ============================================================================

/// 模拟 IDOR：攻击者直接用商户 B 的 order_id 访问 /api/order/{id}
/// 流程：从 DB 取该 order 的 merchant_id=2 → can_access 校验 → 拒绝
/// 此处纯逻辑模拟"取回的 resource_merchant_id=2"，断言商户 A 被拒绝。
#[test]
fn idor_direct_resource_id_access_denied_cross_merchant() {
    let attacker = merchant_a();
    // 模拟从 DB 查回的 order 行：merchant_id=2（属于商户 B）
    let fetched_order_merchant_id: Option<i64> = Some(2);
    assert!(
        !can_access_merchant_resource_from_user(Some(&attacker), fetched_order_merchant_id),
        "商户 A 用商户 B 的 order_id 直接访问必须拒绝（IDOR）"
    );
}

/// IDOR：商户 A 用自己的 order_id 访问 → 允许
#[test]
fn idor_direct_resource_id_access_allowed_own_merchant() {
    let user = merchant_a();
    let fetched_order_merchant_id: Option<i64> = Some(1);
    assert!(
        can_access_merchant_resource_from_user(Some(&user), fetched_order_merchant_id),
        "商户 A 用自己商户的 order_id 访问应允许"
    );
}

/// IDOR：跨商户 product/device 资源访问同样被拒（覆盖 order/product/device 三类资源）
#[test]
fn idor_cross_merchant_denied_for_all_resource_types() {
    let attacker = merchant_a();
    // 模拟从 DB 查回的 resource 行的 merchant_id
    for (resource_type, resource_merchant_id) in
        [("order", Some(2)), ("product", Some(2)), ("device", Some(2))]
    {
        assert!(
            !can_access_merchant_resource_from_user(Some(&attacker), resource_merchant_id),
            "商户 A 跨商户访问 {resource_type} 资源必须拒绝"
        );
    }
}

// ============================================================================
// Part 2-3: effective_merchant_scope — 列表查询的归属过滤
// ============================================================================

/// 商户 A 请求 merchant_id=2（试图越权列表查询）→ 实际查询被强制限定到自己的 merchant_id=1
/// 这是列表查询的 IDOR 防护：忽略请求参数，使用用户上下文的 merchant_id。
#[test]
fn idor_list_query_scope_forced_to_own_merchant() {
    let user = merchant_a();
    let requested = Some(2); // 请求参数试图指定商户 B
    let (effective, allowed) = effective_merchant_scope_from_user(Some(&user), requested);
    assert!(allowed, "已认证用户应允许列表查询");
    assert_eq!(
        effective,
        Some(1),
        "商户 A 的列表查询范围必须强制为 merchant_id=1，忽略请求参数中的 2"
    );
}

/// 管理员请求 merchant_id=2 → 允许查询指定商户（管理员可跨商户）
#[test]
fn idor_list_query_scope_admin_honors_request() {
    let admin = admin_user();
    let requested = Some(2);
    let (effective, allowed) = effective_merchant_scope_from_user(Some(&admin), requested);
    assert!(allowed);
    assert_eq!(
        effective, Some(2),
        "管理员列表查询应使用请求参数中的 merchant_id"
    );
}

/// 管理员请求 merchant_id=None → 查询全部商户
#[test]
fn idor_list_query_scope_admin_no_filter_queries_all() {
    let admin = admin_user();
    let (effective, allowed) = effective_merchant_scope_from_user(Some(&admin), None);
    assert!(allowed);
    assert!(
        effective.is_none(),
        "管理员 + 无请求参数 → 查询全部商户（无归属过滤）"
    );
}

/// 未认证请求列表 → 拒绝
#[test]
fn idor_list_query_scope_unauthenticated_denied() {
    let (effective, allowed) = effective_merchant_scope_from_user(None, Some(1));
    assert!(!allowed, "未认证请求必须拒绝");
    assert!(
        effective.is_none(),
        "拒绝时 effective 应为 None，调用方不应使用"
    );
}

// ============================================================================
// Part 2-4: 创建商户越权防护
// ============================================================================

/// 普通用户不得创建商户（管理动作）
#[test]
fn idor_create_merchant_denied_for_non_admin() {
    assert!(
        !can_create_merchant_from_user(Some(&merchant_a())),
        "普通商户用户不得创建商户"
    );
    assert!(
        !can_create_merchant_from_user(Some(&merchant_b())),
        "普通商户用户不得创建商户"
    );
}

/// 管理员可以创建商户
#[test]
fn idor_create_merchant_allowed_for_admin() {
    assert!(
        can_create_merchant_from_user(Some(&admin_user())),
        "管理员可以创建商户"
    );
}

/// 未认证不得创建商户
#[test]
fn idor_create_merchant_denied_for_unauthenticated() {
    assert!(
        !can_create_merchant_from_user(None),
        "未认证请求不得创建商户"
    );
}

// ============================================================================
// Part 2-5: RBAC guard 跨商户语义文档化（feature v18-rbac）
// ============================================================================
//
// RBAC guard 校验**角色权限**（merchant:read 等），不区分商户归属。
// "商户 A 的 token 持 merchant:read 角色 → guard allow"是设计语义：
// guard allow 不代表业务层放行，controllers 层 IDOR 校验会拒绝跨商户访问。
//
// 此模块文档化该语义，并验证 guard 对未授权/无权限的拒绝（401/403）。

#[cfg(feature = "v18-rbac")]
mod rbac_guard_cross_merchant {

    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use sz_rust_auth_facade::{PermissionDecision, RbacEngine};
    use sz_rust_sz300::rbac::{guard::rbac_guard, roles};
    use tower::ServiceExt;

    const TEST_SECRET: &str = "test-rbac-idor-secret-2026";

    fn init_test_auth() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            sz_rust_sz300::services::auth_service::init_auth_test_only(TEST_SECRET);
        });
    }

    fn issue_token(username: &str, user_id: i64, roles: Vec<&str>) -> String {
        init_test_auth();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_core::orm::jwt::JwtClaims::new(username, exp)
            .with_issuer("sz300-test")
            .with_roles(roles.into_iter().map(String::from).collect())
            .with_user_id(user_id);
        sz_rust_core::orm::jwt::JwtEncoder::new(TEST_SECRET)
            .encode(&claims)
            .unwrap()
    }

    fn bearer_request(token: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/protected")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap()
    }

    fn no_token_request() -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/protected")
            .body(Body::empty())
            .unwrap()
    }

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn build_router(engine: Arc<RbacEngine>, resource: sz_rust_auth_facade::Resource) -> Router {
        Router::new()
            .route("/protected", post(ok_handler))
            .layer(middleware::from_fn_with_state(
                (engine, resource),
                rbac_guard,
            ))
    }

    async fn fetch_body(resp: axum::http::Response<Body>) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// 无 token → 401（未提供认证令牌）
    #[tokio::test]
    async fn guard_no_token_returns_401() {
        let engine = Arc::new(roles::init_rbac_engine());
        let router = build_router(engine, roles::perm::merchant("read"));
        let resp = router.oneshot(no_token_request()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = fetch_body(resp).await;
        assert!(body.contains("未提供认证令牌"));
    }

    /// 无效 token → 401
    #[tokio::test]
    async fn guard_invalid_token_returns_401() {
        let engine = Arc::new(roles::init_rbac_engine());
        let router = build_router(engine, roles::perm::merchant("read"));
        let resp = router
            .oneshot(bearer_request("invalid-token-xyz"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// 商户 A 的 token（merchant:read 角色）访问 merchant:read 资源 → guard allow（200）
    ///
    /// 这是 RBAC guard 的语义：guard allow 只代表**角色有权限**，不代表**商户归属匹配**。
    /// 跨商户拒绝由 controllers 层 IDOR 校验提供（见本文件 Part 2-1/2-2 用例）。
    #[tokio::test]
    async fn guard_merchant_a_with_read_role_allowed_for_merchant_read() {
        let engine = Arc::new(roles::init_rbac_engine());
        engine.assign_role("10", roles::role::merchant_read());
        let token = issue_token("merchant_a", 10, vec!["merchant:read"]);
        let router = build_router(engine, roles::perm::merchant("read"));
        let resp = router.oneshot(bearer_request(&token)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "guard 对持 merchant:read 角色的 token 应 allow（角色权限通过）"
        );
    }

    /// 商户 A 的 token（仅 merchant:read）访问 merchant:create 资源 → guard deny（403）
    ///
    /// 这是 RBAC guard 的垂直越权防护：角色无权限 → 403。
    /// 与 IDOR（水平越权）互补：guard 防垂直，controllers 层防水平。
    #[tokio::test]
    async fn guard_merchant_a_without_write_role_denied_for_create() {
        let engine = Arc::new(roles::init_rbac_engine());
        engine.assign_role("10", roles::role::merchant_read());
        let token = issue_token("merchant_a", 10, vec!["merchant:read"]);
        let router = build_router(engine, roles::perm::merchant("create"));
        let resp = router.oneshot(bearer_request(&token)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = fetch_body(resp).await;
        assert!(body.contains("无权访问"), "应返回 403 无权访问: {body}");
    }

    /// 商户 A 与商户 B 持相同角色 → guard 对两者 allow 一致（guard 不区分商户）
    ///
    /// 文档化：guard 无法区分"商户 A 的 token"与"商户 B 的 token"——
    /// 两者都持 merchant:read 角色，guard 都 allow。跨商户拒绝由 controllers 层提供。
    #[tokio::test]
    async fn guard_treats_merchant_a_and_b_identically_by_role() {
        let engine_a = Arc::new(roles::init_rbac_engine());
        engine_a.assign_role("10", roles::role::merchant_read());
        let token_a = issue_token("merchant_a", 10, vec!["merchant:read"]);
        let router_a = build_router(engine_a, roles::perm::merchant("read"));
        let resp_a = router_a.oneshot(bearer_request(&token_a)).await.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);

        let engine_b = Arc::new(roles::init_rbac_engine());
        engine_b.assign_role("20", roles::role::merchant_read());
        let token_b = issue_token("merchant_b", 20, vec!["merchant:read"]);
        let router_b = build_router(engine_b, roles::perm::merchant("read"));
        let resp_b = router_b.oneshot(bearer_request(&token_b)).await.unwrap();
        assert_eq!(resp_b.status(), StatusCode::OK);

        // guard 对两者一致 allow；跨商户拒绝不在 guard 层，在 controllers 层 IDOR 校验。
    }

    /// 管理员 token 访问任意资源 → guard allow
    #[tokio::test]
    async fn guard_admin_allowed_for_all_resources() {
        let engine = Arc::new(roles::init_rbac_engine());
        engine.assign_role("1", roles::role::admin());
        let token = issue_token("admin", 1, vec!["admin"]);
        for resource in [
            roles::perm::merchant("read"),
            roles::perm::merchant("create"),
            roles::perm::product("delete"),
            roles::perm::device("bind"),
            roles::perm::order("create"),
        ] {
            let router = build_router(engine.clone(), resource);
            let resp = router.oneshot(bearer_request(&token)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "管理员应对所有资源 allow"
            );
        }
    }

    /// 未预指派角色的 token → guard 自动同步 merchant:read（无 DB 环境）
    ///
    /// 文档化 guard.rs:68-83 的 F-3 修复行为：engine 中无该用户角色时，
    /// guard 调用 `auth_context(user.id)`，无 DB → None → is_admin=false →
    /// 指派 `merchant:read`。对 `merchant("read")` 资源 → Allow → 200。
    /// 与 `rbac_test::test_guard_syncs_role_when_unassigned` 行为一致。
    #[tokio::test]
    async fn guard_unassigned_role_syncs_to_merchant_read() {
        let engine = Arc::new(roles::init_rbac_engine());
        // 不 assign_role，模拟"engine 无角色记录"场景
        let token = issue_token("ghost", 999, vec![]);
        let router = build_router(engine, roles::perm::merchant("read"));
        let resp = router.oneshot(bearer_request(&token)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "未预指派角色 + 无 DB → guard 同步 merchant:read → read 资源 allow"
        );
    }

    /// RBAC 引擎权限矩阵：merchant:read 角色对 order:read allow，对 order:create deny
    /// 此处锁定角色权限矩阵，防止 roles.rs 误改导致越权。
    #[tokio::test]
    async fn rbac_matrix_merchant_read_can_read_but_not_write() {
        let engine = roles::init_rbac_engine();
        engine.assign_role("10", roles::role::merchant_read());
        // read 系列允许
        for op in ["read"] {
            assert_eq!(
                engine.check("10", &roles::perm::merchant(op)).await.unwrap(),
                PermissionDecision::Allow
            );
            assert_eq!(
                engine.check("10", &roles::perm::product(op)).await.unwrap(),
                PermissionDecision::Allow
            );
            assert_eq!(
                engine.check("10", &roles::perm::order(op)).await.unwrap(),
                PermissionDecision::Allow
            );
        }
        // write 系列拒绝
        for op in ["create", "update", "delete"] {
            assert_eq!(
                engine.check("10", &roles::perm::merchant(op)).await.unwrap(),
                PermissionDecision::Deny
            );
            assert_eq!(
                engine.check("10", &roles::perm::product(op)).await.unwrap(),
                PermissionDecision::Deny
            );
        }
        assert_eq!(
            engine.check("10", &roles::perm::order("create")).await.unwrap(),
            PermissionDecision::Deny
        );
    }
}