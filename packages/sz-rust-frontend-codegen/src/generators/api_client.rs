//! API 客户端生成器

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::GenerationConfig;
use crate::error::FrontendCodegenError;
use crate::report::GeneratedFile;
use crate::template_engine::CodegenTemplateEngine;

/// TypeScript interface 定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsInterfaceDefinition {
    /// 接口名
    pub name: String,
    /// 字段列表
    pub fields: Vec<TsField>,
}

/// TypeScript 字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsField {
    /// 字段名
    pub name: String,
    /// 类型
    pub ts_type: String,
    /// 是否可选
    pub optional: bool,
}

/// OpenAPI Schema 提取器
pub struct OpenApiSchemaExtractor;

impl OpenApiSchemaExtractor {
    /// 提取请求 Schema
    pub fn extract_request_schema(
        spec: &serde_json::Value,
        path: &str,
        method: &str,
    ) -> Option<TsInterfaceDefinition> {
        let path_item = spec.get("paths")?.get(path)?;
        let operation = path_item.get(method.to_lowercase().as_str())?;
        let body = operation.get("requestBody")?;
        let schema = body
            .get("content")?
            .get("application/json")?
            .get("schema")?;
        Self::schema_to_ts_interface(
            schema,
            &format!("{}{}Request", capitalize(method), capitalize(path)),
        )
    }

    /// 提取响应 Schema
    pub fn extract_response_schema(
        spec: &serde_json::Value,
        path: &str,
        method: &str,
    ) -> Option<TsInterfaceDefinition> {
        let path_item = spec.get("paths")?.get(path)?;
        let operation = path_item.get(method.to_lowercase().as_str())?;
        let responses = operation.get("responses")?;
        let ok = responses.get("200").or_else(|| responses.get("default"))?;
        let schema = ok.get("content")?.get("application/json")?.get("schema")?;
        Self::schema_to_ts_interface(
            schema,
            &format!("{}{}Response", capitalize(method), capitalize(path)),
        )
    }

    /// Schema 转 TypeScript interface
    pub fn schema_to_ts_interface(
        schema: &serde_json::Value,
        name: &str,
    ) -> Option<TsInterfaceDefinition> {
        let props = schema.get("properties")?;
        let required: Vec<&str> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let mut fields = Vec::new();
        for (key, val) in props.as_object()? {
            let ts_type = match val.get("type").and_then(|t| t.as_str()) {
                Some("string") => "string".to_string(),
                Some("integer") | Some("number") => "number".to_string(),
                Some("boolean") => "boolean".to_string(),
                Some("array") => {
                    let item_type = val
                        .get("items")
                        .and_then(|i| i.get("type"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("any");
                    format!("{item_type}[]")
                }
                _ => "any".to_string(),
            };
            fields.push(TsField {
                name: key.clone(),
                ts_type,
                optional: !required.contains(&key.as_str()),
            });
        }
        Some(TsInterfaceDefinition {
            name: name.to_string(),
            fields,
        })
    }
}

/// API 客户端生成器
pub struct ApiClientGenerator<'a> {
    engine: &'a CodegenTemplateEngine,
}

impl<'a> ApiClientGenerator<'a> {
    /// 创建生成器
    pub fn new(engine: &'a CodegenTemplateEngine) -> Self {
        Self { engine }
    }

    /// 生成 API 客户端
    pub async fn generate(
        &self,
        openapi_spec: &serde_json::Value,
        config: &GenerationConfig,
    ) -> Result<Vec<GeneratedFile>, FrontendCodegenError> {
        let paths = openapi_spec.get("paths").and_then(|p| p.as_object());
        let mut modules: BTreeMap<String, Vec<ApiEndpoint>> = BTreeMap::new();

        if let Some(paths) = paths {
            for (path, methods) in paths {
                let module = path.split('/').nth(1).unwrap_or("api").to_string();
                if let Some(obj) = methods.as_object() {
                    for (method, _op) in obj {
                        let upper = method.to_uppercase();
                        if matches!(upper.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
                            modules
                                .entry(module.clone())
                                .or_default()
                                .push(ApiEndpoint {
                                    method: upper,
                                    path: path.clone(),
                                });
                        }
                    }
                }
            }
        }

        let mut files = Vec::new();
        for (module, endpoints) in &modules {
            let mut context = tera::Context::new();
            context.insert("module", module);
            context.insert("endpoints", endpoints);
            context.insert("with_interceptors", &config.with_interceptors);

            let api_content = self.engine.render("api/module.ts.tera", &context)?;
            files.push(GeneratedFile {
                path: std::path::PathBuf::from(format!("src/api/{module}.ts")),
                size_bytes: api_content.len() as u64,
                source_model: module.clone(),
                source_template: "api/module.ts.tera".to_string(),
                is_overwritten: false,
            });

            let types_content = self.engine.render("types/module.ts.tera", &context)?;
            files.push(GeneratedFile {
                path: std::path::PathBuf::from(format!("src/types/{module}.ts")),
                size_bytes: types_content.len() as u64,
                source_model: module.clone(),
                source_template: "types/module.ts.tera".to_string(),
                is_overwritten: false,
            });
        }

        if config.with_interceptors {
            let context = tera::Context::new();
            let content = self.engine.render("utils/request.ts.tera", &context)?;
            files.push(GeneratedFile {
                path: std::path::PathBuf::from("src/utils/request.ts"),
                size_bytes: content.len() as u64,
                source_model: "request".to_string(),
                source_template: "utils/request.ts.tera".to_string(),
                is_overwritten: false,
            });
        }

        Ok(files)
    }
}

/// API 端点
#[derive(Debug, Clone, Serialize)]
pub struct ApiEndpoint {
    /// HTTP 方法
    pub method: String,
    /// 路径
    pub path: String,
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
