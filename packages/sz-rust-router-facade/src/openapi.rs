// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! OpenAPI 文档 — 对齐 Swagger / OpenAPI 3.0.3
//!
//! 提供编程式 OpenAPI 规范构建器，支持生成 JSON/YAML 规范文件并提供
//! Swagger UI 渲染端点。无需 derive 宏，业务代码通过链式 API 注册端点。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_router_facade::openapi::{OpenApiBuilder, HttpMethod};
//!
//! let spec = OpenApiBuilder::new("SZ-Rust API", "1.0.0")
//!     .description("SZ-Rust 框架 API 文档")
//!     .path("/api/v1/users", HttpMethod::Get, |op| {
//!         op.summary("获取用户列表")
//!           .tag("用户")
//!           .response(200, "成功", "application/json")
//!     })
//!     .path("/api/v1/users/{id}", HttpMethod::Get, |op| {
//!         op.summary("获取用户详情")
//!           .tag("用户")
//!           .parameter("id", "path", "用户 ID", true, "integer")
//!           .response(200, "成功", "application/json")
//!           .response(404, "用户不存在", "application/json")
//!     })
//!     .build();
//!
//! // spec 为 serde_json::Value，可直接序列化为 JSON
//! let json = serde_json::to_string_pretty(&spec).unwrap();
//! ```
//!
//! ## Swagger UI 集成
//!
//! 通过 [`swagger_ui_html`] 获取 Swagger UI HTML 页面，挂载到 axum 路由：
//!
//! ```ignore
//! use sz_rust_router_facade::openapi::{OpenApiBuilder, swagger_ui_html};
//!
//! let spec_json = serde_json::to_string(&builder.build()).unwrap();
//! let html = swagger_ui_html(&spec_json);
//! ```

use serde_json::{json, Map, Value};

/// 从路由配置自动扫描生成 OpenAPI 路径
///
/// 将 [`crate::routing::RouteConfig`] 中全部路由规则（含分组展平后的规则）
/// 自动转换为 OpenAPI 操作，无需手工逐条注册。
///
/// ## 自动生成规则
///
/// - `summary`：取 handler 引用（如 `User@list`）
/// - `tag`：取路径第一段（如 `/api/v1/users` → `users`），用于分组展示
/// - `parameters`：自动识别路径中的 `{param}` 模板变量，生成必填 path 参数
/// - `responses`：默认注册 200（成功）与 404（资源不存在）
pub fn routes_to_spec(routes: &[crate::routing::RouteRule], spec: &mut Value) {
    // 先收集待追加的 tag（路径首段去重），避免对 spec 的双重可变借用
    let mut tags_to_add: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = spec
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    for rule in routes {
        let tag = rule
            .path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("default")
            .to_string();
        if seen.insert(tag.clone()) {
            tags_to_add.push((tag.clone(), format!("{} 相关接口", tag)));
        }
    }

    // 路径生成（单一可变借用区间）
    {
        let paths = spec
            .get_mut("paths")
            .and_then(|p| p.as_object_mut())
            .expect("spec must contain a paths object");

        for rule in routes {
            let method = match rule.method {
                crate::routing::HttpMethod::GET => "get",
                crate::routing::HttpMethod::POST => "post",
                crate::routing::HttpMethod::PUT => "put",
                crate::routing::HttpMethod::DELETE => "delete",
                crate::routing::HttpMethod::PATCH => "patch",
                crate::routing::HttpMethod::OPTIONS => "options",
            };

            // 从路径首段派生 tag（如 /api/v1/users → users）
            let tag = rule
                .path
                .trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or("default")
                .to_string();

            // 自动识别路径模板变量 → path 参数
            let mut parameters = Vec::new();
            for segment in rule.path.split('/') {
                if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                    parameters.push(json!({
                        "name": name,
                        "in": "path",
                        "required": true,
                        "description": format!("路径参数 {}", name),
                        "schema": { "type": "string" },
                    }));
                }
            }

            let mut op = json!({
                "summary": rule.handler,
                "tags": [tag],
                "responses": {
                    "200": { "description": "成功" },
                    "404": { "description": "资源不存在" }
                },
            });
            if !parameters.is_empty() {
                op["parameters"] = Value::Array(parameters);
            }
            if !rule.middleware.is_empty() {
                op["description"] = json!(format!("中间件: {}", rule.middleware.join(", ")));
            }

            let path_entry = paths.entry(rule.path.clone()).or_insert_with(|| json!({}));
            if let Value::Object(ref mut obj) = path_entry {
                obj.insert(method.to_string(), op);
            }
        }
    }

    // 追加收集到的 tag（与手工注册的 tag 合并；spec 无 tags 时创建）
    if !tags_to_add.is_empty() {
        if spec.get("tags").is_none() {
            spec["tags"] = Value::Array(Vec::new());
        }
        let tag_arr = spec
            .get_mut("tags")
            .and_then(|t| t.as_array_mut())
            .expect("tags must be an array after creation");
        for (name, desc) in tags_to_add {
            tag_arr.push(json!({ "name": name, "description": desc }));
        }
    }
}

/// HTTP 方法枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    /// HTTP GET
    Get,
    /// HTTP POST
    Post,
    /// HTTP PUT
    Put,
    /// HTTP DELETE
    Delete,
    /// HTTP PATCH
    Patch,
    /// HTTP OPTIONS
    Options,
    /// HTTP HEAD
    Head,
}

impl HttpMethod {
    /// 转为 OpenAPI 规范的小写字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Delete => "delete",
            HttpMethod::Patch => "patch",
            HttpMethod::Options => "options",
            HttpMethod::Head => "head",
        }
    }
}

/// OpenAPI 规范构建器
///
/// 对齐 OpenAPI 3.0.3 规范，通过链式 API 构建 spec。
pub struct OpenApiBuilder {
    title: String,
    version: String,
    description: Option<String>,
    paths: Map<String, Value>,
    tags: Vec<Value>,
    security_schemes: Map<String, Value>,
}

impl OpenApiBuilder {
    /// 创建新的构建器
    ///
    /// # 参数
    ///
    /// - `title`：API 标题
    /// - `version`：API 版本
    pub fn new(title: &str, version: &str) -> Self {
        Self {
            title: title.to_string(),
            version: version.to_string(),
            description: None,
            paths: Map::new(),
            tags: Vec::new(),
            security_schemes: Map::new(),
        }
    }

    /// 设置 API 描述
    pub fn description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    /// 添加标签（用于分组）
    pub fn tag(mut self, name: &str, description: &str) -> Self {
        self.tags.push(json!({
            "name": name,
            "description": description,
        }));
        self
    }

    /// 添加 API 端点
    ///
    /// # 参数
    ///
    /// - `path`：路径（如 `/api/v1/users/{id}`）
    /// - `method`：HTTP 方法
    /// - `config`：操作配置闭包
    pub fn path<F>(mut self, path: &str, method: HttpMethod, config: F) -> Self
    where
        F: FnOnce(&mut OperationBuilder),
    {
        let mut op = OperationBuilder::new();
        config(&mut op);

        let path_entry = self
            .paths
            .entry(path.to_string())
            .or_insert_with(|| json!({}));
        if let Value::Object(ref mut obj) = path_entry {
            obj.insert(method.as_str().to_string(), op.build());
        }
        self
    }

    /// 添加 Bearer Token 安全方案
    pub fn bearer_auth(mut self, scheme_name: &str) -> Self {
        self.security_schemes.insert(
            scheme_name.to_string(),
            json!({
                "type": "http",
                "scheme": "bearer",
                "bearerFormat": "JWT",
            }),
        );
        self
    }

    /// 添加 API Key 安全方案
    pub fn api_key_auth(mut self, scheme_name: &str, header_name: &str) -> Self {
        self.security_schemes.insert(
            scheme_name.to_string(),
            json!({
                "type": "apiKey",
                "in": "header",
                "name": header_name,
            }),
        );
        self
    }

    /// 构建 OpenAPI spec（`serde_json::Value`）
    pub fn build(self) -> Value {
        let mut info = json!({
            "title": self.title,
            "version": self.version,
        });
        if let Some(desc) = self.description {
            info["description"] = json!(desc);
        }

        let mut spec = json!({
            "openapi": "3.0.3",
            "info": info,
            "paths": self.paths,
        });

        let mut components = Map::new();
        if !self.security_schemes.is_empty() {
            components.insert(
                "securitySchemes".to_string(),
                Value::Object(self.security_schemes),
            );
        }
        if !components.is_empty() {
            spec["components"] = Value::Object(components);
        }

        if !self.tags.is_empty() {
            spec["tags"] = Value::Array(self.tags);
        }

        spec
    }

    /// 构建 JSON 字符串（美化格式）
    pub fn to_json_string(self) -> String {
        serde_json::to_string_pretty(&self.build()).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 从路由配置自动扫描生成完整 OpenAPI spec
///
/// 扫描 [`crate::routing::RouteConfig`] 的全部路由（含分组展平后的规则），
/// 自动为每个端点生成 operation（summary、tag、path 参数、默认响应）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_router_facade::openapi::{OpenApiBuilder, spec_from_route_config};
/// use sz_rust_router_facade::routing::load_routes_from_json_str;
///
/// let cfg = load_routes_from_json_str(r#"{"routes": [...]}"#).unwrap();
/// let spec = OpenApiBuilder::new("SZ-Rust API", "1.0.0")
///     .description("自动扫描路由生成的文档")
///     .bearer_auth("BearerAuth");
/// let json = spec_from_route_config(spec, &cfg);
/// ```
pub fn spec_from_route_config(
    builder: OpenApiBuilder,
    config: &crate::routing::RouteConfig,
) -> String {
    let mut spec = builder.build();
    let routes = config.flatten();
    routes_to_spec(&routes, &mut spec);
    serde_json::to_string_pretty(&spec).unwrap_or_else(|_| "{}".to_string())
}

/// 操作构建器 — 描述单个 API 端点的元数据
pub struct OperationBuilder {
    summary: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    parameters: Vec<Value>,
    responses: Map<String, Value>,
    deprecated: bool,
}

impl OperationBuilder {
    /// 创建新的操作构建器
    pub fn new() -> Self {
        Self {
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: Vec::new(),
            responses: Map::new(),
            deprecated: false,
        }
    }

    /// 设置摘要
    pub fn summary(&mut self, summary: &str) -> &mut Self {
        self.summary = Some(summary.to_string());
        self
    }

    /// 设置详细描述
    pub fn description(&mut self, desc: &str) -> &mut Self {
        self.description = Some(desc.to_string());
        self
    }

    /// 添加标签（用于分组）
    pub fn tag(&mut self, tag: &str) -> &mut Self {
        self.tags.push(tag.to_string());
        self
    }

    /// 添加参数
    ///
    /// # 参数
    ///
    /// - `name`：参数名
    /// - `location`：参数位置（`path` / `query` / `header` / `cookie`）
    /// - `desc`：参数描述
    /// - `required`：是否必填
    /// - `schema_type`：数据类型（`string` / `integer` / `number` / `boolean` / `array`）
    pub fn parameter(
        &mut self,
        name: &str,
        location: &str,
        desc: &str,
        required: bool,
        schema_type: &str,
    ) -> &mut Self {
        self.parameters.push(json!({
            "name": name,
            "in": location,
            "description": desc,
            "required": required,
            "schema": {
                "type": schema_type
            }
        }));
        self
    }

    /// 添加响应
    ///
    /// # 参数
    ///
    /// - `status`：HTTP 状态码（如 `200`、`404`）
    /// - `desc`：响应描述
    /// - `content_type`：内容类型（如 `"application/json"`）
    pub fn response(&mut self, status: u16, desc: &str, content_type: &str) -> &mut Self {
        self.responses.insert(
            status.to_string(),
            json!({
                "description": desc,
                "content": {
                    content_type: {
                        "schema": {
                            "type": "object"
                        }
                    }
                }
            }),
        );
        self
    }

    /// 添加响应（带 schema 引用）
    ///
    /// # 参数
    ///
    /// - `status`：HTTP 状态码
    /// - `desc`：响应描述
    /// - `content_type`：内容类型
    /// - `schema_ref`：schema 引用名（如 `"#/components/schemas/User"`）
    pub fn response_with_schema(
        &mut self,
        status: u16,
        desc: &str,
        content_type: &str,
        schema_ref: &str,
    ) -> &mut Self {
        self.responses.insert(
            status.to_string(),
            json!({
                "description": desc,
                "content": {
                    content_type: {
                        "schema": {
                            "$ref": schema_ref
                        }
                    }
                }
            }),
        );
        self
    }

    /// 标记为已弃用
    pub fn deprecated(&mut self) -> &mut Self {
        self.deprecated = true;
        self
    }

    /// 构建操作 JSON
    fn build(self) -> Value {
        let mut op = Map::new();
        if let Some(s) = self.summary {
            op.insert("summary".to_string(), json!(s));
        }
        if let Some(d) = self.description {
            op.insert("description".to_string(), json!(d));
        }
        if !self.tags.is_empty() {
            op.insert("tags".to_string(), json!(self.tags));
        }
        if !self.parameters.is_empty() {
            op.insert("parameters".to_string(), json!(self.parameters));
        }
        if !self.responses.is_empty() {
            op.insert("responses".to_string(), Value::Object(self.responses));
        } else {
            // 默认响应
            op.insert(
                "responses".to_string(),
                json!({
                    "200": {
                        "description": "成功"
                    }
                }),
            );
        }
        if self.deprecated {
            op.insert("deprecated".to_string(), json!(true));
        }
        Value::Object(op)
    }
}

impl Default for OperationBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 生成 Swagger UI HTML 页面
///
/// 通过 CDN 加载 Swagger UI，将 OpenAPI JSON 内嵌到页面中。
///
/// # 参数
///
/// - `spec_json`：OpenAPI 规范 JSON 字符串
pub fn swagger_ui_html(spec_json: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>API 文档 - Swagger UI</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
    <style>
        body {{ margin: 0; }}
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script>
        window.onload = function() {{
            const spec = {spec_json};
            SwaggerUIBundle({{
                spec: spec,
                dom_id: '#swagger-ui',
                presets: [SwaggerUIBundle.presets.apis],
                layout: 'BaseLayout',
            }});
        }};
    </script>
</body>
</html>"#
    )
}

/// 生成 Redoc HTML 页面（替代 Swagger UI 的轻量文档查看器）
///
/// # 参数
///
/// - `spec_json`：OpenAPI 规范 JSON 字符串
pub fn redoc_html(spec_json: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>API 文档 - Redoc</title>
    <style>
        body {{ margin: 0; padding: 0; }}
    </style>
</head>
<body>
    <redoc spec-json='{spec_json}'></redoc>
    <script src="https://cdn.jsdelivr.net/npm/redoc@next/bundles/redoc.standalone.js"></script>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "get");
        assert_eq!(HttpMethod::Post.as_str(), "post");
        assert_eq!(HttpMethod::Put.as_str(), "put");
        assert_eq!(HttpMethod::Delete.as_str(), "delete");
        assert_eq!(HttpMethod::Patch.as_str(), "patch");
        assert_eq!(HttpMethod::Options.as_str(), "options");
        assert_eq!(HttpMethod::Head.as_str(), "head");
    }

    #[test]
    fn test_openapi_builder_basic() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .description("A test API")
            .build();

        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "Test API");
        assert_eq!(spec["info"]["version"], "1.0.0");
        assert_eq!(spec["info"]["description"], "A test API");
        assert!(spec["paths"].is_object());
    }

    #[test]
    fn test_openapi_builder_with_path() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/users", HttpMethod::Get, |op| {
                op.summary("获取用户列表")
                    .tag("用户")
                    .response(200, "成功", "application/json");
            })
            .build();

        let path = &spec["paths"]["/api/v1/users"]["get"];
        assert_eq!(path["summary"], "获取用户列表");
        assert_eq!(path["tags"][0], "用户");
        assert_eq!(path["responses"]["200"]["description"], "成功");
    }

    #[test]
    fn test_openapi_builder_with_parameter() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/users/{id}", HttpMethod::Get, |op| {
                op.summary("获取用户详情")
                    .parameter("id", "path", "用户 ID", true, "integer")
                    .response(200, "成功", "application/json")
                    .response(404, "用户不存在", "application/json");
            })
            .build();

        let path = &spec["paths"]["/api/v1/users/{id}"]["get"];
        assert_eq!(path["parameters"][0]["name"], "id");
        assert_eq!(path["parameters"][0]["in"], "path");
        assert_eq!(path["parameters"][0]["required"], true);
        assert_eq!(path["parameters"][0]["schema"]["type"], "integer");
        assert_eq!(path["responses"]["404"]["description"], "用户不存在");
    }

    #[test]
    fn test_openapi_builder_with_tags() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .tag("用户", "用户管理接口")
            .tag("订单", "订单管理接口")
            .build();

        assert_eq!(spec["tags"][0]["name"], "用户");
        assert_eq!(spec["tags"][0]["description"], "用户管理接口");
        assert_eq!(spec["tags"][1]["name"], "订单");
    }

    #[test]
    fn test_openapi_builder_bearer_auth() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .bearer_auth("BearerAuth")
            .build();

        let scheme = &spec["components"]["securitySchemes"]["BearerAuth"];
        assert_eq!(scheme["type"], "http");
        assert_eq!(scheme["scheme"], "bearer");
        assert_eq!(scheme["bearerFormat"], "JWT");
    }

    #[test]
    fn test_openapi_builder_api_key_auth() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .api_key_auth("ApiKeyAuth", "X-API-Key")
            .build();

        let scheme = &spec["components"]["securitySchemes"]["ApiKeyAuth"];
        assert_eq!(scheme["type"], "apiKey");
        assert_eq!(scheme["in"], "header");
        assert_eq!(scheme["name"], "X-API-Key");
    }

    #[test]
    fn test_openapi_builder_default_response() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/health", HttpMethod::Get, |op| {
                op.summary("健康检查");
            })
            .build();

        let path = &spec["paths"]["/api/v1/health"]["get"];
        // 未指定响应时应有默认 200 响应
        assert_eq!(path["responses"]["200"]["description"], "成功");
    }

    #[test]
    fn test_openapi_builder_deprecated() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/old", HttpMethod::Get, |op| {
                op.summary("旧接口").deprecated();
            })
            .build();

        let path = &spec["paths"]["/api/v1/old"]["get"];
        assert_eq!(path["deprecated"], true);
    }

    #[test]
    fn test_openapi_builder_multiple_methods_same_path() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/users", HttpMethod::Get, |op| {
                op.summary("获取列表");
            })
            .path("/api/v1/users", HttpMethod::Post, |op| {
                op.summary("创建用户");
            })
            .build();

        let path = &spec["paths"]["/api/v1/users"];
        assert!(path["get"].is_object());
        assert!(path["post"].is_object());
        assert_eq!(path["get"]["summary"], "获取列表");
        assert_eq!(path["post"]["summary"], "创建用户");
    }

    #[test]
    fn test_openapi_builder_to_json_string() {
        let builder = OpenApiBuilder::new("Test API", "1.0.0");
        let json = builder.to_json_string();
        assert!(json.contains("\"openapi\": \"3.0.3\""));
        assert!(json.contains("\"title\": \"Test API\""));
        assert!(json.contains("\"version\": \"1.0.0\""));
    }

    #[test]
    fn test_openapi_builder_response_with_schema() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0")
            .path("/api/v1/users/{id}", HttpMethod::Get, |op| {
                op.response_with_schema(
                    200,
                    "成功",
                    "application/json",
                    "#/components/schemas/User",
                );
            })
            .build();

        let schema_ref = &spec["paths"]["/api/v1/users/{id}"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/User");
    }

    #[test]
    fn test_swagger_ui_html_contains_spec() {
        let spec_json = r#"{"openapi":"3.0.3","info":{"title":"Test"}}"#;
        let html = swagger_ui_html(spec_json);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("swagger-ui"));
        assert!(html.contains(spec_json));
    }

    #[test]
    fn test_redoc_html_contains_spec() {
        let spec_json = r#"{"openapi":"3.0.3","info":{"title":"Test"}}"#;
        let html = redoc_html(spec_json);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("redoc"));
        assert!(html.contains(spec_json));
    }

    #[test]
    fn test_operation_builder_default() {
        let op = OperationBuilder::default();
        assert!(op.summary.is_none());
        assert!(op.description.is_none());
        assert!(op.tags.is_empty());
        assert!(op.parameters.is_empty());
        assert!(op.responses.is_empty());
        assert!(!op.deprecated);
    }

    #[test]
    fn test_openapi_builder_no_description() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0").build();
        assert!(spec["info"]["description"].is_null());
    }

    #[test]
    fn test_openapi_builder_no_security_schemes() {
        let spec = OpenApiBuilder::new("Test API", "1.0.0").build();
        // 无安全方案时不应有 components
        assert!(spec.get("components").is_none() || spec["components"].is_null());
    }

    #[test]
    fn test_openapi_builder_full_spec() {
        let spec = OpenApiBuilder::new("SZ-Rust API", "1.0.0")
            .description("全栈 API 文档")
            .tag("用户", "用户管理")
            .tag("认证", "认证授权")
            .bearer_auth("BearerAuth")
            .path("/api/v1/auth/login", HttpMethod::Post, |op| {
                op.summary("用户登录")
                    .description("通过用户名密码获取 JWT Token")
                    .tag("认证")
                    .parameter("username", "query", "用户名", true, "string")
                    .parameter("password", "query", "密码", true, "string")
                    .response(200, "登录成功", "application/json")
                    .response(401, "认证失败", "application/json");
            })
            .path("/api/v1/users", HttpMethod::Get, |op| {
                op.summary("获取用户列表").tag("用户").response_with_schema(
                    200,
                    "成功",
                    "application/json",
                    "#/components/schemas/UserList",
                );
            })
            .path("/api/v1/users/{id}", HttpMethod::Delete, |op| {
                op.summary("删除用户")
                    .tag("用户")
                    .parameter("id", "path", "用户 ID", true, "integer")
                    .response(204, "删除成功", "application/json")
                    .response(404, "用户不存在", "application/json");
            })
            .build();

        // 验证基本结构
        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "SZ-Rust API");
        assert_eq!(spec["info"]["description"], "全栈 API 文档");

        // 验证路径
        assert_eq!(spec["paths"].as_object().unwrap().len(), 3);

        // 验证标签
        assert_eq!(spec["tags"].as_array().unwrap().len(), 2);

        // 验证安全方案
        assert_eq!(
            spec["components"]["securitySchemes"]["BearerAuth"]["scheme"],
            "bearer"
        );

        // 验证 login 端点
        let login = &spec["paths"]["/api/v1/auth/login"]["post"];
        assert_eq!(login["summary"], "用户登录");
        assert_eq!(login["description"], "通过用户名密码获取 JWT Token");
        assert_eq!(login["tags"][0], "认证");
        assert_eq!(login["parameters"].as_array().unwrap().len(), 2);
        assert_eq!(login["responses"]["401"]["description"], "认证失败");

        // 验证 delete 端点
        let delete = &spec["paths"]["/api/v1/users/{id}"]["delete"];
        assert_eq!(delete["summary"], "删除用户");
        assert_eq!(delete["responses"]["204"]["description"], "删除成功");
    }

    // ---- 路由自动扫描 ----

    fn sample_route_config() -> crate::routing::RouteConfig {
        use crate::routing::{HttpMethod, RouteConfig, RouteRule};
        let mut cfg = RouteConfig::new();
        cfg.add_route(RouteRule::new(
            HttpMethod::GET,
            "/api/v1/users",
            "User@list",
        ));
        cfg.add_route(RouteRule::new(
            HttpMethod::POST,
            "/api/v1/users",
            "User@create",
        ));
        cfg.add_route(RouteRule::new(
            HttpMethod::GET,
            "/api/v1/users/{id}",
            "User@detail",
        ));
        cfg.add_route(RouteRule::new(
            HttpMethod::DELETE,
            "/api/v1/users/{id}",
            "User@delete",
        ));
        cfg
    }

    #[test]
    fn test_routes_to_spec_generates_paths() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let paths = spec["paths"].as_object().unwrap();
        assert_eq!(paths.len(), 2, "应有 2 个路径（/users 与 /users/{{id}}）");
        assert!(paths.contains_key("/api/v1/users"));
        assert!(paths.contains_key("/api/v1/users/{id}"));
    }

    #[test]
    fn test_routes_to_spec_method_mapping() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let users = &spec["paths"]["/api/v1/users"];
        assert!(users["get"].is_object(), "GET 应存在");
        assert!(users["post"].is_object(), "POST 应存在");
        assert_eq!(users["get"]["summary"], "User@list");
        assert_eq!(users["post"]["summary"], "User@create");
    }

    #[test]
    fn test_routes_to_spec_auto_detect_path_params() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let detail = &spec["paths"]["/api/v1/users/{id}"]["get"];
        let params = detail["parameters"].as_array().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "id");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
    }

    #[test]
    fn test_routes_to_spec_tag_from_path() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let users = &spec["paths"]["/api/v1/users"]["get"];
        assert_eq!(
            users["tags"][0], "api",
            "tag 应取路径首段（/api/v1/users → api）"
        );
    }

    #[test]
    fn test_routes_to_spec_tags_collected_and_deduped() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let tags = spec["tags"].as_array().unwrap();
        let names: Vec<&str> = tags.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, vec!["api"], "同一 tag 不应重复收集");
    }

    #[test]
    fn test_routes_to_spec_middleware_in_description() {
        use crate::routing::{HttpMethod, RouteRule};
        let mut cfg = crate::routing::RouteConfig::new();
        cfg.add_route(
            RouteRule::new(HttpMethod::GET, "/api/v1/admin", "Admin@index").with_middleware("auth"),
        );
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let op = &spec["paths"]["/api/v1/admin"]["get"];
        assert_eq!(op["description"], "中间件: auth");
    }

    #[test]
    fn test_routes_to_spec_default_responses() {
        let cfg = sample_route_config();
        let mut spec = OpenApiBuilder::new("Scan Test", "1.0.0").build();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let detail = &spec["paths"]["/api/v1/users/{id}"]["get"];
        assert_eq!(detail["responses"]["200"]["description"], "成功");
        assert_eq!(detail["responses"]["404"]["description"], "资源不存在");
    }

    #[test]
    fn test_spec_from_route_config_full_flow() {
        let cfg = sample_route_config();
        let builder = OpenApiBuilder::new("Scan Full", "1.0.0")
            .description("自动扫描")
            .bearer_auth("BearerAuth");
        let json = spec_from_route_config(builder, &cfg);

        let spec: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "Scan Full");
        assert_eq!(spec["paths"].as_object().unwrap().len(), 2);
        assert_eq!(
            spec["components"]["securitySchemes"]["BearerAuth"]["scheme"],
            "bearer"
        );
    }

    #[test]
    fn test_routes_to_spec_merges_with_existing_paths() {
        // 手工注册的路径与扫描生成的路径应共存，不互相覆盖
        let mut spec = OpenApiBuilder::new("Merge Test", "1.0.0")
            .path("/api/v1/health", HttpMethod::Get, |op| {
                op.summary("健康检查");
            })
            .build();

        let cfg = sample_route_config();
        routes_to_spec(&cfg.flatten(), &mut spec);

        let paths = spec["paths"].as_object().unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths.contains_key("/api/v1/health"));
        assert_eq!(paths["/api/v1/health"]["get"]["summary"], "健康检查");
    }
}
