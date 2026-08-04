//! P3-FACADE 系列：P3 四簇 facade 集成测试
//!
//! 验证 sz-rust-orm-ext-facade / sz-rust-router-facade /
//! sz-rust-middleware-facade / sz-rust-mvc-facade 的协作，
//! 以及 core 向后兼容 re-export 的类型同一性。

use sz_rust_middleware_facade::auth::is_route_allowed;
use sz_rust_router_facade::router::{parse_path, RouterBuilder};

/// P3-FACADE-01：路由解析（router-facade）三层规则
#[test]
fn router_parse_path_three_layer_rules() {
    // 对齐 PHP auto_multi_app 解析规则
    let root = parse_path("/");
    assert_eq!(root.app, "index", "P3-FACADE-01: / → index 应用");
    assert_eq!(root.controller, "Index", "P3-FACADE-01: / → Index 控制器");
    assert_eq!(root.action, "index", "P3-FACADE-01: / → index 操作");

    let two = parse_path("/product/detail?id=3");
    assert_eq!(two.app, "index", "P3-FACADE-01: 两级路径 → 默认应用");
    assert_eq!(two.controller, "Product", "P3-FACADE-01: 首段 → 控制器");
    assert_eq!(two.action, "detail", "P3-FACADE-01: 次段 → 操作");
    // 非 app_map 应用时 3 段路径按 {controller}/{action} 消费前 2 段（第 3 段忽略）
    assert_eq!(
        parse_path("/a/b/c").action,
        "b",
        "P3-FACADE-01: 非多应用模式 3 段路径 action 取第 2 段"
    );
    assert_eq!(
        parse_path("/a/b/c").controller,
        "A",
        "P3-FACADE-01: 非多应用模式 3 段路径 controller 取第 1 段"
    );
}

/// P3-FACADE-02：middleware-facade 白名单 + mvc 视图渲染联动（路由鉴权上下文）
#[test]
fn middleware_allowlist_and_mvc_guard_context() {
    // auth 中间件的白名单判断（middleware-facade）
    let allow_list = vec!["index/login".to_string(), "index/register".to_string()];
    assert!(
        is_route_allowed("index/login", &allow_list),
        "P3-FACADE-02: 白名单内放行"
    );
    assert!(
        !is_route_allowed("order/create", &allow_list),
        "P3-FACADE-02: 白名单外拒绝"
    );
    // * 通配支持
    let wildcard = vec!["public/*".to_string()];
    assert!(
        is_route_allowed("public/avatar", &wildcard),
        "P3-FACADE-02: * 通配匹配"
    );
}

/// P3-FACADE-03：RouterBuilder 构建 axum Router（router-facade 独立可用）
#[test]
fn router_builder_builds_axum_router() {
    let builder = RouterBuilder::<()>::new();
    let _router = builder.build(); // 不触发任何 http 请求，仅验证类型可用
}

/// P3-FACADE-04：core 向后兼容 re-export 类型同一性（编译期强校验）
///
/// P3 提取后 core 的 `sz_rust_core::{model,hooks,relation,router,routing,websocket_route,openapi,
/// middleware,log,controller,guard,view}` 均为 re-export。以下赋值若类型不同将无法编译。
#[test]
fn p3_core_reexports_are_identical_types() {
    // middleware / log（AuthenticatedUser 字段直接构造）
    let user = sz_rust_core::middleware::auth::AuthenticatedUser { user_id: 9527 };
    let _user2: sz_rust_middleware_facade::auth::AuthenticatedUser = user;
    // router / routing（parse_path 返回 ParsedPath）
    let p1 = sz_rust_core::router::parse_path("/x/y");
    let p2: sz_rust_router_facade::router::ParsedPath = p1;
    assert_eq!(p2.action, "y");
    // orm-ext 簇：BaseModel trait 约束函数定义即编译期验证（trait 路径可达）
    #[allow(dead_code)] // 编译期验证用：仅定义即证明 trait 可从两条路径引用
    fn assert_base_model<T: sz_rust_core::model::BaseModel>() {}
    #[allow(dead_code)]
    fn assert_base_model_facade<T: sz_rust_orm_ext_facade::model::BaseModel>() {}
    // 运行时验证 hooks 类型同一性
    let recorder: sz_rust_core::hooks::HookExecutionRecorder =
        sz_rust_orm_ext_facade::hooks::HookExecutionRecorder::new();
    let _recorder2: sz_rust_orm_ext_facade::hooks::HookExecutionRecorder = recorder;
    // view 错误类型
    let err: sz_rust_core::view::ViewError =
        sz_rust_mvc_facade::view::ViewError::TemplateNotFound("missing.tpl".to_string());
    assert!(matches!(
        err,
        sz_rust_core::view::ViewError::TemplateNotFound(_)
    ));
}
