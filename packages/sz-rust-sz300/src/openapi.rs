//! SZ-300 业务 API OpenAPI 规范构建与文档端点
//!
//! 基于 `sz_rust_core::openapi::OpenApiBuilder` 编程式注册所有业务端点，
//! 提供 Swagger UI / Redoc / 原始 JSON 三种文档访问方式。
//!
//! ## 端点
//!
//! - `GET /api-docs` — Swagger UI 页面
//! - `GET /api-docs/redoc` — Redoc 页面
//! - `GET /api-docs/openapi.json` — OpenAPI 3.0.3 规范 JSON
//!
//! ## 对齐
//!
//! 对齐 PHP ThinkPHP 8 + `zircote/swagger`（PHP-DI OpenAPI 注解）的文档生成能力，
//! 但采用编程式构建器（无需 derive 宏），业务代码通过链式 API 注册端点。

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use once_cell::sync::Lazy;
use serde_json::Value;
use sz_rust_core::openapi::{redoc_html, swagger_ui_html, HttpMethod, OpenApiBuilder};

/// OpenAPI 规范（全局静态，启动时构建一次）
static OPENAPI_SPEC: Lazy<Value> = Lazy::new(build_openapi_spec);

/// Swagger UI HTML（全局静态，启动时构建一次）
static SWAGGER_HTML: Lazy<String> = Lazy::new(|| {
    let spec_json = serde_json::to_string(&*OPENAPI_SPEC).unwrap_or_else(|_| "{}".to_string());
    swagger_ui_html(&spec_json)
});

/// Redoc HTML（全局静态，启动时构建一次）
static REDOC_HTML: Lazy<String> = Lazy::new(|| {
    let spec_json = serde_json::to_string(&*OPENAPI_SPEC).unwrap_or_else(|_| "{}".to_string());
    redoc_html(&spec_json)
});

/// 构建 SZ-300 业务 API 的 OpenAPI 3.0.3 规范
///
/// 编程式注册所有业务端点，包括认证、商户、商品、设备、订单、文件上传等模块。
fn build_openapi_spec() -> Value {
    OpenApiBuilder::new("SZ-300 业务 API", env!("CARGO_PKG_VERSION"))
        .description("鲜视达 SZ-300 后端服务 API 文档 — 基于 sz-rust 框架")
        .tag("认证", "用户登录、令牌刷新、当前用户信息、退出登录")
        .tag("商户管理", "商户的增删改查")
        .tag("商品管理", "商品的增删改查")
        .tag("设备管理", "设备绑定、解绑、OTA 升级、状态上报")
        .tag("订单管理", "订单查询与创建")
        .tag("文件上传", "文件与图片上传")
        .tag("健康检查", "存活、就绪、启动探针与 Prometheus 指标")
        .bearer_auth("BearerAuth")
        // ===== 健康检查 =====
        .path("/health", HttpMethod::Get, |op| {
            op.summary("存活检查（liveness）").tag("健康检查").response(
                200,
                "服务存活",
                "application/json",
            );
        })
        .path("/health/ready", HttpMethod::Get, |op| {
            op.summary("就绪检查（readiness）")
                .description("检查数据库连接是否正常，失败返回 503")
                .tag("健康检查")
                .response(200, "服务就绪", "application/json")
                .response(503, "服务未就绪", "application/json");
        })
        .path("/health/startup", HttpMethod::Get, |op| {
            op.summary("启动检查（startup）")
                .description("验证应用启动初期的依赖可用性，不依赖 DB")
                .tag("健康检查")
                .response(200, "已启动", "application/json")
                .response(503, "启动中", "application/json");
        })
        .path("/metrics", HttpMethod::Get, |op| {
            op.summary("Prometheus 指标端点")
                .description("输出 Prometheus 文本格式指标")
                .tag("健康检查")
                .response(200, "Prometheus 文本格式", "text/plain");
        })
        // ===== 认证 =====
        .path("/api/v1/auth/login", HttpMethod::Post, |op| {
            op.summary("用户登录")
                .description("通过用户名密码获取 JWT Token，成功后下发 CSRF Cookie")
                .tag("认证")
                .parameter("username", "query", "用户名", true, "string")
                .parameter("password", "query", "密码", true, "string")
                .response(200, "登录成功，返回 token 与用户信息", "application/json")
                .response(400, "参数错误（用户名或密码为空）", "application/json")
                .response(401, "认证失败（用户名或密码错误）", "application/json");
        })
        .path("/api/v1/auth/refresh", HttpMethod::Post, |op| {
            op.summary("刷新登录令牌")
                .tag("认证")
                .response(200, "刷新成功", "application/json");
        })
        .path("/api/v1/auth/me", HttpMethod::Post, |op| {
            op.summary("获取当前登录用户信息")
                .description("通过 Authorization: Bearer <token> 解析当前用户")
                .tag("认证")
                .response(200, "返回当前用户完整信息", "application/json")
                .response(401, "未提供认证令牌或令牌无效", "application/json");
        })
        .path("/api/v1/auth/logout", HttpMethod::Post, |op| {
            op.summary("退出登录")
                .description("清除客户端 CSRF Cookie")
                .tag("认证")
                .response(200, "已退出登录", "application/json");
        })
        // ===== 商户管理 =====
        .path("/api/v1/merchant/list", HttpMethod::Post, |op| {
            op.summary("获取商户列表").tag("商户管理").response(
                200,
                "返回商户列表",
                "application/json",
            );
        })
        .path("/api/v1/merchant/info", HttpMethod::Post, |op| {
            op.summary("获取商户详情")
                .tag("商户管理")
                .parameter("merchant_id", "query", "商户 ID", true, "integer")
                .response(200, "返回商户详情", "application/json")
                .response(404, "商户不存在", "application/json");
        })
        .path("/api/v1/merchant/create", HttpMethod::Post, |op| {
            op.summary("创建商户")
                .tag("商户管理")
                .response(200, "创建成功", "application/json")
                .response(400, "参数错误", "application/json");
        })
        .path("/api/v1/merchant/update", HttpMethod::Post, |op| {
            op.summary("更新商户信息")
                .tag("商户管理")
                .parameter("merchant_id", "query", "商户 ID", true, "integer")
                .response(200, "更新成功", "application/json")
                .response(404, "商户不存在", "application/json");
        })
        .path("/api/v1/merchant/delete", HttpMethod::Post, |op| {
            op.summary("删除商户")
                .tag("商户管理")
                .parameter("merchant_id", "query", "商户 ID", true, "integer")
                .response(200, "删除成功", "application/json")
                .response(404, "商户不存在", "application/json");
        })
        // ===== 商品管理 =====
        .path("/api/v1/product/list", HttpMethod::Post, |op| {
            op.summary("获取商品列表").tag("商品管理").response(
                200,
                "返回商品列表",
                "application/json",
            );
        })
        .path("/api/v1/product/info", HttpMethod::Post, |op| {
            op.summary("获取商品详情")
                .tag("商品管理")
                .parameter("product_id", "query", "商品 ID", true, "integer")
                .response(200, "返回商品详情", "application/json")
                .response(404, "商品不存在", "application/json");
        })
        .path("/api/v1/product/create", HttpMethod::Post, |op| {
            op.summary("创建商品")
                .tag("商品管理")
                .response(200, "创建成功", "application/json")
                .response(400, "参数错误", "application/json");
        })
        .path("/api/v1/product/update", HttpMethod::Post, |op| {
            op.summary("更新商品信息")
                .tag("商品管理")
                .parameter("product_id", "query", "商品 ID", true, "integer")
                .response(200, "更新成功", "application/json")
                .response(404, "商品不存在", "application/json");
        })
        .path("/api/v1/product/delete", HttpMethod::Post, |op| {
            op.summary("删除商品")
                .tag("商品管理")
                .parameter("product_id", "query", "商品 ID", true, "integer")
                .response(200, "删除成功", "application/json")
                .response(404, "商品不存在", "application/json");
        })
        // ===== 设备管理 =====
        .path("/api/v1/device/list", HttpMethod::Post, |op| {
            op.summary("获取设备列表").tag("设备管理").response(
                200,
                "返回设备列表",
                "application/json",
            );
        })
        .path("/api/v1/device/info", HttpMethod::Post, |op| {
            op.summary("获取设备详情")
                .tag("设备管理")
                .parameter("device_id", "query", "设备 ID", true, "string")
                .response(200, "返回设备详情", "application/json")
                .response(404, "设备不存在", "application/json");
        })
        .path("/api/v1/device/bind", HttpMethod::Post, |op| {
            op.summary("绑定设备到商户")
                .tag("设备管理")
                .response(200, "绑定成功", "application/json")
                .response(400, "参数错误或设备已被绑定", "application/json");
        })
        .path("/api/v1/device/unbind", HttpMethod::Post, |op| {
            op.summary("解绑设备")
                .tag("设备管理")
                .response(200, "解绑成功", "application/json");
        })
        .path("/api/v1/device/ota", HttpMethod::Post, |op| {
            op.summary("触发设备 OTA 升级").tag("设备管理").response(
                200,
                "OTA 升级任务已触发",
                "application/json",
            );
        })
        .path("/api/v1/device/status_report", HttpMethod::Post, |op| {
            op.summary("设备状态上报")
                .description("设备端上报运行状态，服务端记录并分析")
                .tag("设备管理")
                .response(200, "上报成功", "application/json");
        })
        // ===== 订单管理 =====
        .path("/api/v1/order/list", HttpMethod::Post, |op| {
            op.summary("获取订单列表").tag("订单管理").response(
                200,
                "返回订单列表",
                "application/json",
            );
        })
        .path("/api/v1/order/info", HttpMethod::Post, |op| {
            op.summary("获取订单详情")
                .tag("订单管理")
                .parameter("order_id", "query", "订单 ID", true, "integer")
                .response(200, "返回订单详情", "application/json")
                .response(404, "订单不存在", "application/json");
        })
        .path("/api/v1/order/create", HttpMethod::Post, |op| {
            op.summary("创建订单")
                .tag("订单管理")
                .response(200, "创建成功", "application/json")
                .response(400, "参数错误", "application/json");
        })
        // ===== 文件上传 =====
        .path("/api/v1/file/upload", HttpMethod::Post, |op| {
            op.summary("文件上传（Base64）")
                .description("通过 Base64 编码上传文件")
                .tag("文件上传")
                .response(200, "上传成功，返回文件 URL", "application/json")
                .response(400, "参数错误或文件类型不允许", "application/json");
        })
        .path("/api/v1/file/upload_multipart", HttpMethod::Post, |op| {
            op.summary("文件上传（Multipart）")
                .description("通过 multipart/form-data 上传文件")
                .tag("文件上传")
                .response(200, "上传成功，返回文件 URL", "application/json")
                .response(400, "参数错误或文件类型不允许", "application/json");
        })
        .build()
}

/// Swagger UI 文档端点 — `GET /api-docs`
pub async fn swagger_ui() -> impl IntoResponse {
    Html(SWAGGER_HTML.clone())
}

/// Redoc 文档端点 — `GET /api-docs/redoc`
pub async fn redoc() -> impl IntoResponse {
    Html(REDOC_HTML.clone())
}

/// OpenAPI 规范 JSON 端点 — `GET /api-docs/openapi.json`
pub async fn openapi_json() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json; charset=utf-8")],
        Json(OPENAPI_SPEC.clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_has_basic_info() {
        let spec = &*OPENAPI_SPEC;
        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "SZ-300 业务 API");
        assert!(!spec["paths"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_spec_has_tags() {
        let spec = &*OPENAPI_SPEC;
        let tags = spec["tags"].as_array().expect("tags 应为数组");
        assert!(tags.len() >= 7, "应至少有 7 个标签");
    }

    #[test]
    fn test_spec_has_bearer_auth() {
        let spec = &*OPENAPI_SPEC;
        assert_eq!(
            spec["components"]["securitySchemes"]["BearerAuth"]["scheme"],
            "bearer"
        );
    }

    #[test]
    fn test_spec_includes_auth_endpoints() {
        let spec = &*OPENAPI_SPEC;
        assert!(spec["paths"]["/api/v1/auth/login"]["post"].is_object());
        assert!(spec["paths"]["/api/v1/auth/me"]["post"].is_object());
        assert!(spec["paths"]["/api/v1/auth/refresh"]["post"].is_object());
        assert!(spec["paths"]["/api/v1/auth/logout"]["post"].is_object());
    }

    #[test]
    fn test_spec_includes_health_endpoints() {
        let spec = &*OPENAPI_SPEC;
        assert!(spec["paths"]["/health"]["get"].is_object());
        assert!(spec["paths"]["/health/ready"]["get"].is_object());
        assert!(spec["paths"]["/health/startup"]["get"].is_object());
        assert!(spec["paths"]["/metrics"]["get"].is_object());
    }

    #[test]
    fn test_spec_includes_merchant_endpoints() {
        let spec = &*OPENAPI_SPEC;
        for ep in &[
            "/api/v1/merchant/list",
            "/api/v1/merchant/info",
            "/api/v1/merchant/create",
            "/api/v1/merchant/update",
            "/api/v1/merchant/delete",
        ] {
            assert!(spec["paths"][ep]["post"].is_object(), "缺少端点: {}", ep);
        }
    }

    #[test]
    fn test_spec_includes_product_endpoints() {
        let spec = &*OPENAPI_SPEC;
        for ep in &[
            "/api/v1/product/list",
            "/api/v1/product/info",
            "/api/v1/product/create",
            "/api/v1/product/update",
            "/api/v1/product/delete",
        ] {
            assert!(spec["paths"][ep]["post"].is_object(), "缺少端点: {}", ep);
        }
    }

    #[test]
    fn test_spec_includes_device_endpoints() {
        let spec = &*OPENAPI_SPEC;
        for ep in &[
            "/api/v1/device/list",
            "/api/v1/device/info",
            "/api/v1/device/bind",
            "/api/v1/device/unbind",
            "/api/v1/device/ota",
            "/api/v1/device/status_report",
        ] {
            assert!(spec["paths"][ep]["post"].is_object(), "缺少端点: {}", ep);
        }
    }

    #[test]
    fn test_spec_includes_order_endpoints() {
        let spec = &*OPENAPI_SPEC;
        for ep in &[
            "/api/v1/order/list",
            "/api/v1/order/info",
            "/api/v1/order/create",
        ] {
            assert!(spec["paths"][ep]["post"].is_object(), "缺少端点: {}", ep);
        }
    }

    #[test]
    fn test_spec_includes_file_upload_endpoints() {
        let spec = &*OPENAPI_SPEC;
        assert!(spec["paths"]["/api/v1/file/upload"]["post"].is_object());
        assert!(spec["paths"]["/api/v1/file/upload_multipart"]["post"].is_object());
    }

    #[test]
    fn test_swagger_html_is_valid() {
        let html = &*SWAGGER_HTML;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("swagger-ui"));
    }

    #[test]
    fn test_redoc_html_is_valid() {
        let html = &*REDOC_HTML;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("redoc"));
    }

    #[test]
    fn test_spec_total_endpoint_count() {
        let spec = &*OPENAPI_SPEC;
        let paths = spec["paths"].as_object().expect("paths 应为对象");
        let mut count = 0;
        for (_, methods) in paths.iter() {
            for method in ["get", "post", "put", "delete", "patch", "options", "head"] {
                if methods.get(method).is_some() {
                    count += 1;
                }
            }
        }
        // 健康检查 4 + 认证 4 + 商户 5 + 商品 5 + 设备 6 + 订单 3 + 文件上传 2 = 29
        assert_eq!(count, 29, "端点总数应为 29，实际: {}", count);
    }
}
