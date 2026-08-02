//! `#[controller]` 宏集成测试
//!
//! 验证宏生成的 `SzController` trait 实现是否正确。

use sz_rust_macros::controller;

/// 测试用控制器
#[controller]
pub struct TestController;

#[test]
fn test_controller_implements_sz_controller() {
    // 如果 #[controller] 宏正确生成了 impl SzController，则可以调用 trait 方法
    let ctrl = TestController;
    let json =
        sz_rust_core::controller::SzController::render_json(&ctrl, 1, "ok", serde_json::json!({}));
    assert_eq!(json["code"], 1);
    assert_eq!(json["msg"], "ok");
}

#[test]
fn test_controller_render_json() {
    let ctrl = TestController;
    let json = sz_rust_core::controller::SzController::render_json(
        &ctrl,
        0,
        "error",
        serde_json::json!({"detail": "not found"}),
    );
    assert_eq!(json["code"], 0);
    assert_eq!(json["msg"], "error");
    assert_eq!(json["data"]["detail"], "not found");
}
