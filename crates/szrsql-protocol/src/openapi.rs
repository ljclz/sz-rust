//! Phase 7d.17 — OpenAPI 3.0 规范 + Swagger UI 页面。
//!
//! 提供两个公开端点（无需鉴权）：
//! - `GET /api/v1/openapi.json` — 返回 OpenAPI 3.0 规范 JSON
//! - `GET /api/v1/swagger` — 返回 Swagger UI HTML 页面
//!
//! # 设计
//!
//! - **零外部依赖**：使用 `serde_json::Value` 手动构建 OpenAPI 3.0 文档
//! - **端点全覆盖**：自动覆盖 http.rs 中所有 7 个 REST 端点
//! - **Bearer 鉴权**：在 components.securitySchemes 中声明 Bearer token 方案
//! - **Swagger UI**：单文件 HTML，通过 CDN 加载 Swagger UI JavaScript
//!
//! # OpenAPI 3.0 结构
//!
//! - `openapi`: 固定 "3.0.3"
//! - `info`: 标题/版本/描述
//! - `servers`: 服务器列表（默认 http://127.0.0.1:{port}）
//! - `paths`: 端点路径与操作
//! - `components`: 可复用组件（安全方案/Schema）
//! - `security`: 全局安全要求

use serde_json::{json, Value};

/// 返回 SzRSQL 版本号（与 crate version 一致）。
fn crate_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// =====================================================================
//  OpenAPI 3.0 规范生成
// =====================================================================

/// 生成完整的 OpenAPI 3.0 规范 JSON。
///
/// 覆盖以下 7 个 REST 端点：
/// - `GET /healthz` — 存活探针
/// - `GET /readyz` — 就绪探针
/// - `GET /metrics` — Prometheus 指标
/// - `GET /api/v1/sessions` — 列出活跃会话（需鉴权）
/// - `POST /api/v1/cancel/{pid}` — 取消指定会话（需鉴权）
/// - `POST /api/v1/backup` — 触发备份（需鉴权）
/// - `POST /api/v1/config/reload` — 触发配置热重载（需鉴权）
pub fn generate_openapi_spec() -> Value {
    let version = crate_version();

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "SzRSQL Management API",
            "version": version,
            "description": "SzRSQL 数据库管理端点 REST API 文档。\n\n提供健康检查、Prometheus 指标、会话管理、备份触发和配置重载等管理功能。",
            "license": {
                "name": "MIT OR Apache-2.0",
                "url": "https://github.com/ljclz/szrsql"
            }
        },
        "servers": [
            {
                "url": "http://127.0.0.1:8080",
                "description": "默认本地管理端口（实际端口由 http_port 配置）"
            }
        ],
        "tags": [
            { "name": "health", "description": "健康检查与就绪探针" },
            { "name": "metrics", "description": "Prometheus 指标导出" },
            { "name": "sessions", "description": "会话管理（需鉴权）" },
            { "name": "admin", "description": "管理操作（需鉴权）" }
        ],
        "paths": {
            "/healthz": {
                "get": {
                    "tags": ["health"],
                    "summary": "存活探针",
                    "description": "始终返回 200 + `{\"status\":\"ok\"}`，用于 Kubernetes liveness probe。",
                    "operationId": "healthz",
                    "responses": {
                        "200": {
                            "description": "服务器存活",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "example": "ok" }
                                        },
                                        "required": ["status"]
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/readyz": {
                "get": {
                    "tags": ["health"],
                    "summary": "就绪探针",
                    "description": "服务器 Running 时返回 200，Draining/Closed 时返回 503。用于 Kubernetes readiness probe。",
                    "operationId": "readyz",
                    "responses": {
                        "200": {
                            "description": "服务器就绪",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "enum": ["ready"] }
                                        },
                                        "required": ["status"]
                                    }
                                }
                            }
                        },
                        "503": {
                            "description": "服务器未就绪（draining 或 closed）",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "enum": ["draining", "closed"] }
                                        },
                                        "required": ["status"]
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/metrics": {
                "get": {
                    "tags": ["metrics"],
                    "summary": "Prometheus 指标",
                    "description": "返回 Prometheus 文本格式指标，包括 connections_total/queries_total/active_connections/wal_lsn。",
                    "operationId": "metrics",
                    "responses": {
                        "200": {
                            "description": "Prometheus 文本格式指标",
                            "content": {
                                "text/plain": {
                                    "schema": { "type": "string" },
                                    "example": "# HELP szrsql_connections_total Total number of connections accepted since startup.\n# TYPE szrsql_connections_total counter\nszrsql_connections_total 0\n"
                                }
                            }
                        }
                    }
                }
            },
            "/api/v1/sessions": {
                "get": {
                    "tags": ["sessions"],
                    "summary": "列出活跃会话",
                    "description": "返回当前所有活跃数据库会话列表（需 Bearer token 鉴权）。",
                    "operationId": "listSessions",
                    "security": [{ "bearerAuth": [] }],
                    "responses": {
                        "200": {
                            "description": "活跃会话列表",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "sessions": {
                                                "type": "array",
                                                "items": {
                                                    "type": "object",
                                                    "properties": {
                                                        "pid": { "type": "integer", "format": "int32", "description": "会话进程 ID" },
                                                        "user": { "type": "string", "description": "连接用户" },
                                                        "database": { "type": "string", "description": "连接数据库" },
                                                        "state": { "type": "string", "enum": ["active", "idle"], "description": "会话状态" },
                                                        "query": { "type": "string", "description": "当前查询（idle 时为空）" },
                                                        "duration_ms": { "type": "integer", "description": "当前查询已执行毫秒数" }
                                                    }
                                                }
                                            }
                                        },
                                        "required": ["sessions"]
                                    }
                                }
                            }
                        },
                        "401": {
                            "description": "鉴权失败",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" }
                                }
                            }
                        }
                    }
                }
            },
            "/api/v1/cancel/{pid}": {
                "post": {
                    "tags": ["sessions"],
                    "summary": "取消指定会话",
                    "description": "取消指定 PID 的数据库会话当前正在执行的查询（需 Bearer token 鉴权）。",
                    "operationId": "cancelSession",
                    "security": [{ "bearerAuth": [] }],
                    "parameters": [
                        {
                            "name": "pid",
                            "in": "path",
                            "required": true,
                            "description": "会话进程 ID",
                            "schema": { "type": "integer", "format": "int32", "minimum": 0 }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "取消成功",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "cancelled": { "type": "integer", "description": "被取消的 PID" }
                                        },
                                        "required": ["cancelled"]
                                    }
                                }
                            }
                        },
                        "400": {
                            "description": "PID 格式错误",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" }
                                }
                            }
                        },
                        "401": {
                            "description": "鉴权失败",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" }
                                }
                            }
                        }
                    }
                }
            },
            "/api/v1/backup": {
                "post": {
                    "tags": ["admin"],
                    "summary": "触发备份",
                    "description": "触发数据库物理备份（需 Bearer token 鉴权）。\n\n当前为占位实现，待 Phase 5 持久化层完成后接入真实备份机制。",
                    "operationId": "triggerBackup",
                    "security": [{ "bearerAuth": [] }],
                    "responses": {
                        "200": {
                            "description": "备份成功（当前为 stub）",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "example": "backup completed (stub)" }
                                        },
                                        "required": ["status"]
                                    }
                                }
                            }
                        },
                        "401": {
                            "description": "鉴权失败",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" }
                                }
                            }
                        }
                    }
                }
            },
            "/api/v1/config/reload": {
                "post": {
                    "tags": ["admin"],
                    "summary": "触发配置热重载",
                    "description": "触发服务器配置热重载（需 Bearer token 鉴权）。\n\n当前为占位实现，待后续集成 SIGHUP 机制后接入真实配置重载。",
                    "operationId": "reloadConfig",
                    "security": [{ "bearerAuth": [] }],
                    "responses": {
                        "200": {
                            "description": "配置重载成功（当前为 stub）",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "status": { "type": "string", "example": "config reloaded (stub)" }
                                        },
                                        "required": ["status"]
                                    }
                                }
                            }
                        },
                        "401": {
                            "description": "鉴权失败",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Error" }
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "simple token",
                    "description": "在 Authorization header 中传递 `Bearer <token>`，token 由 http_config.auth_token 配置。"
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string", "description": "错误描述" }
                    },
                    "required": ["error"]
                }
            }
        }
    })
}

/// 将 OpenAPI 规范序列化为格式化的 JSON 字符串。
///
/// 使用 2 空格缩进，便于人类阅读和工具解析。
pub fn openapi_spec_to_json() -> String {
    serde_json::to_string_pretty(&generate_openapi_spec())
        .expect("OpenAPI spec serialization should never fail")
}

// =====================================================================
//  Swagger UI 页面
// =====================================================================

/// 渲染 Swagger UI HTML 页面。
///
/// 单文件 HTML，通过 CDN 加载 Swagger UI 5.x JavaScript/CSS，
/// 指向 `/api/v1/openapi.json` 加载 OpenAPI 规范。
///
/// # 设计
///
/// - 完全自包含：除 CDN 资源外无任何本地依赖
/// - 深色主题：与终端用户偏好一致
/// - 不暴露任何敏感信息：仅加载 openapi.json
pub fn render_swagger_ui() -> String {
    let version = crate_version();

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>SzRSQL Management API - Swagger UI</title>
    <link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.18.2/swagger-ui.css">
    <style>
        html {{
            box-sizing: border-box;
            overflow: -moz-scrollbars-vertical;
            overflow-y: scroll;
        }}
        *, *:before, *:after {{
            box-sizing: inherit;
        }}
        body {{
            margin: 0;
            background: #fafafa;
        }}
        .topbar {{
            background-color: #1a1a1a;
            color: #fff;
            padding: 12px 24px;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            font-size: 14px;
        }}
        .topbar .title {{
            font-weight: 600;
            font-size: 16px;
        }}
        .topbar .version {{
            margin-left: 12px;
            color: #999;
            font-size: 12px;
        }}
    </style>
</head>
<body>
    <div class="topbar">
        <span class="title">SzRSQL Management API</span>
        <span class="version">v{version}</span>
    </div>
    <div id="swagger-ui"></div>

    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.18.2/swagger-ui-bundle.js" charset="UTF-8"></script>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.18.2/swagger-ui-standalone-preset.js" charset="UTF-8"></script>
    <script>
        window.onload = function() {{
            window.ui = SwaggerUIBundle({{
                url: "/api/v1/openapi.json",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "StandaloneLayout",
                docExpansion: "none",
                defaultModelsExpandDepth: 1,
                defaultModelExpandDepth: 1,
                filter: true,
                tryItOutEnabled: true,
                persistAuthorization: true
            }});
        }};
    </script>
</body>
</html>"#
    )
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== OpenAPI 规范生成 ====================

    #[test]
    fn test_openapi_spec_has_valid_version() {
        let spec = generate_openapi_spec();
        assert_eq!(spec["openapi"], "3.0.3");
    }

    #[test]
    fn test_openapi_spec_info_fields() {
        let spec = generate_openapi_spec();
        let info = &spec["info"];
        assert_eq!(info["title"], "SzRSQL Management API");
        assert_eq!(info["version"], crate_version());
        assert!(info["description"].as_str().unwrap().contains("SzRSQL"));
        assert!(info["license"]["name"].as_str().unwrap().contains("MIT"));
    }

    #[test]
    fn test_openapi_spec_has_servers() {
        let spec = generate_openapi_spec();
        let servers = spec["servers"].as_array().unwrap();
        assert!(!servers.is_empty());
        assert!(servers[0]["url"].as_str().unwrap().contains("127.0.0.1"));
    }

    #[test]
    fn test_openapi_spec_has_all_tags() {
        let spec = generate_openapi_spec();
        let tags = spec["tags"].as_array().unwrap();
        let tag_names: Vec<&str> = tags.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(tag_names.contains(&"health"));
        assert!(tag_names.contains(&"metrics"));
        assert!(tag_names.contains(&"sessions"));
        assert!(tag_names.contains(&"admin"));
        assert_eq!(tag_names.len(), 4);
    }

    // ==================== 端点覆盖完整性 ====================

    #[test]
    fn test_openapi_spec_covers_all_7_endpoints() {
        let spec = generate_openapi_spec();
        let paths = spec["paths"].as_object().unwrap();

        // 公开端点（3 个）
        assert!(paths.contains_key("/healthz"));
        assert!(paths.contains_key("/readyz"));
        assert!(paths.contains_key("/metrics"));

        // 鉴权端点（4 个）
        assert!(paths.contains_key("/api/v1/sessions"));
        assert!(paths.contains_key("/api/v1/cancel/{pid}"));
        assert!(paths.contains_key("/api/v1/backup"));
        assert!(paths.contains_key("/api/v1/config/reload"));

        assert_eq!(
            paths.len(),
            7,
            "OpenAPI spec must cover all 7 REST endpoints"
        );
    }

    #[test]
    fn test_healthz_endpoint_spec() {
        let spec = generate_openapi_spec();
        let healthz = &spec["paths"]["/healthz"]["get"];
        assert_eq!(healthz["operationId"], "healthz");
        assert_eq!(healthz["summary"], "存活探针");
        assert!(healthz["responses"].get("200").is_some());
        // 健康端点无需鉴权
        assert!(healthz.get("security").is_none());
    }

    #[test]
    fn test_readyz_endpoint_has_503_response() {
        let spec = generate_openapi_spec();
        let readyz = &spec["paths"]["/readyz"]["get"];
        assert_eq!(readyz["operationId"], "readyz");
        assert!(readyz["responses"].get("200").is_some());
        assert!(readyz["responses"].get("503").is_some());
    }

    #[test]
    fn test_metrics_endpoint_returns_text_plain() {
        let spec = generate_openapi_spec();
        let metrics = &spec["paths"]["/metrics"]["get"];
        let content_type = &metrics["responses"]["200"]["content"]["text/plain"]["schema"]["type"];
        assert_eq!(content_type, "string");
    }

    #[test]
    fn test_sessions_endpoint_requires_auth() {
        let spec = generate_openapi_spec();
        let sessions = &spec["paths"]["/api/v1/sessions"]["get"];
        let security = sessions["security"].as_array().unwrap();
        assert!(security[0].get("bearerAuth").is_some());
    }

    #[test]
    fn test_cancel_endpoint_has_pid_parameter() {
        let spec = generate_openapi_spec();
        let cancel = &spec["paths"]["/api/v1/cancel/{pid}"]["post"];
        let params = cancel["parameters"].as_array().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "pid");
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["format"], "int32");
    }

    #[test]
    fn test_backup_endpoint_spec() {
        let spec = generate_openapi_spec();
        let backup = &spec["paths"]["/api/v1/backup"]["post"];
        assert_eq!(backup["operationId"], "triggerBackup");
        let security = backup["security"].as_array().unwrap();
        assert!(security[0].get("bearerAuth").is_some());
    }

    #[test]
    fn test_config_reload_endpoint_spec() {
        let spec = generate_openapi_spec();
        let reload = &spec["paths"]["/api/v1/config/reload"]["post"];
        assert_eq!(reload["operationId"], "reloadConfig");
        let security = reload["security"].as_array().unwrap();
        assert!(security[0].get("bearerAuth").is_some());
    }

    // ==================== 安全方案 ====================

    #[test]
    fn test_openapi_spec_has_bearer_auth_scheme() {
        let spec = generate_openapi_spec();
        let schemes = &spec["components"]["securitySchemes"];
        let bearer = &schemes["bearerAuth"];
        assert_eq!(bearer["type"], "http");
        assert_eq!(bearer["scheme"], "bearer");
    }

    #[test]
    fn test_openapi_spec_has_error_schema() {
        let spec = generate_openapi_spec();
        let schemas = &spec["components"]["schemas"];
        let error = &schemas["Error"];
        assert_eq!(error["type"], "object");
        assert!(error["properties"].get("error").is_some());
    }

    // ==================== JSON 序列化 ====================

    #[test]
    fn test_openapi_spec_to_json_is_valid_json() {
        let json_str = openapi_spec_to_json();
        // 应可被 serde_json 重新解析
        let parsed: Value = serde_json::from_str(&json_str).expect("spec JSON should be valid");
        assert_eq!(parsed["openapi"], "3.0.3");
    }

    #[test]
    fn test_openapi_spec_to_json_is_pretty() {
        let json_str = openapi_spec_to_json();
        // 格式化 JSON 应包含换行符
        assert!(
            json_str.contains('\n'),
            "pretty JSON should contain newlines"
        );
        assert!(
            json_str.contains("  "),
            "pretty JSON should contain 2-space indentation"
        );
    }

    // ==================== Swagger UI 页面 ====================

    #[test]
    fn test_swagger_ui_has_doctype_and_html() {
        let html = render_swagger_ui();
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn test_swagger_ui_has_correct_title() {
        let html = render_swagger_ui();
        assert!(html.contains("<title>SzRSQL Management API - Swagger UI</title>"));
    }

    #[test]
    fn test_swagger_ui_points_to_openapi_json() {
        let html = render_swagger_ui();
        assert!(
            html.contains(r#"url: "/api/v1/openapi.json""#),
            "Swagger UI must point to /api/v1/openapi.json"
        );
    }

    #[test]
    fn test_swagger_ui_loads_swagger_ui_bundle() {
        let html = render_swagger_ui();
        assert!(
            html.contains("swagger-ui-bundle.js"),
            "Swagger UI must load swagger-ui-bundle.js"
        );
        assert!(
            html.contains("swagger-ui.css"),
            "Swagger UI must load swagger-ui.css"
        );
    }

    #[test]
    fn test_swagger_ui_initializes_swaggeruibundle() {
        let html = render_swagger_ui();
        assert!(
            html.contains("SwaggerUIBundle({"),
            "Swagger UI must call SwaggerUIBundle()"
        );
    }

    #[test]
    fn test_swagger_ui_contains_version() {
        let html = render_swagger_ui();
        let version = crate_version();
        assert!(
            html.contains(&format!("v{version}")),
            "Swagger UI should display version {version}"
        );
    }

    #[test]
    fn test_swagger_ui_is_self_contained_html() {
        let html = render_swagger_ui();
        // 应包含完整的 HTML 结构
        assert!(html.contains("<head>"));
        assert!(html.contains("<body>"));
        assert!(html.contains("</head>"));
        assert!(html.contains("</body>"));
    }

    #[test]
    fn test_swagger_ui_has_standalone_layout() {
        let html = render_swagger_ui();
        assert!(
            html.contains("StandaloneLayout"),
            "Swagger UI should use StandaloneLayout"
        );
        assert!(
            html.contains("SwaggerUIStandalonePreset"),
            "Swagger UI should load StandalonePreset"
        );
    }
}
