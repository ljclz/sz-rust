// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 集成测试：模型解析、模板渲染、组件生成、覆盖策略

use std::path::PathBuf;

use sz_rust_frontend_codegen::file_writer::FileWriter;
use sz_rust_frontend_codegen::generators::api_client::{
    ApiClientGenerator, OpenApiSchemaExtractor,
};
use sz_rust_frontend_codegen::generators::permission::{PermissionConfig, PermissionGenerator};
use sz_rust_frontend_codegen::generators::react::ReactComponentGenerator;
use sz_rust_frontend_codegen::generators::route::{FrontendRoute, RouteGenerator, RouteMeta};
use sz_rust_frontend_codegen::generators::vue::VueComponentGenerator;
use sz_rust_frontend_codegen::metadata::ModelMetadata;
use sz_rust_frontend_codegen::model_parser::ModelParser;
use sz_rust_frontend_codegen::template_engine::CodegenTemplateEngine;
use sz_rust_frontend_codegen::ui_adapter::UiAdapter;
use sz_rust_frontend_codegen::{
    CodegenService, Framework, GenerationConfig, OverrideStrategy, UiLibrary,
};

// ── 模型解析集成测试 ──

#[tokio::test]
async fn test_parse_model_with_derive() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: Option<String>,
    pub password: String,
    pub created_at: DateTime,
}
"#,
    )
    .await
    .unwrap();

    let result = ModelParser::parse_dir(temp.path()).await;
    assert!(result.is_ok());
    let models = result.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].name, "User");
    assert_eq!(models[0].module_name, "user");
    assert_eq!(models[0].fields.len(), 5);
}

#[tokio::test]
async fn test_parse_model_without_derive_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("plain.rs");
    tokio::fs::write(
        &model_file,
        r#"
pub struct PlainStruct {
    pub id: i32,
}
"#,
    )
    .await
    .unwrap();

    let result = ModelParser::parse_dir(temp.path()).await;
    assert!(result.is_ok());
    let models = result.unwrap();
    assert!(models.is_empty());
}

#[tokio::test]
async fn test_parse_model_field_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: Option<String>,
}
"#,
    )
    .await
    .unwrap();

    let models = ModelParser::parse_dir(temp.path()).await.unwrap();
    let model = &models[0];

    let id_field = model.fields.iter().find(|f| f.name == "id").unwrap();
    assert!(id_field.is_primary_key);
    assert!(!id_field.is_nullable);

    let email_field = model.fields.iter().find(|f| f.name == "email").unwrap();
    assert!(email_field.is_nullable);
}

#[tokio::test]
async fn test_parse_model_sensitive_field() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub password: String,
    pub token: String,
}
"#,
    )
    .await
    .unwrap();

    let models = ModelParser::parse_dir(temp.path()).await.unwrap();
    let model = &models[0];

    let password_field = model.fields.iter().find(|f| f.name == "password").unwrap();
    assert!(password_field.is_sensitive);

    let token_field = model.fields.iter().find(|f| f.name == "token").unwrap();
    assert!(token_field.is_sensitive);
}

#[tokio::test]
async fn test_parse_model_auto_timestamp() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
"#,
    )
    .await
    .unwrap();

    let models = ModelParser::parse_dir(temp.path()).await.unwrap();
    let model = &models[0];

    let created = model
        .fields
        .iter()
        .find(|f| f.name == "created_at")
        .unwrap();
    assert!(created.is_auto_timestamp);

    let updated = model
        .fields
        .iter()
        .find(|f| f.name == "updated_at")
        .unwrap();
    assert!(updated.is_auto_timestamp);
}

// ── 模板引擎集成测试 ──

#[tokio::test]
async fn test_template_engine_init_builtin() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let result = CodegenTemplateEngine::init(&builtin_dir, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_template_engine_render_vue_list() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let mut context = tera::Context::new();
    context.insert("module_name", "user");
    context.insert("fields", &Vec::<serde_json::Value>::new());
    context.insert("writable_fields", &Vec::<serde_json::Value>::new());
    context.insert("relations", &Vec::<serde_json::Value>::new());

    let ui_tags = serde_json::json!({
        "table": "el-table",
        "table_column": "el-table-column",
        "form": "el-form",
        "form_item": "el-form-item",
        "input": "el-input",
        "button": "el-button",
        "pagination": "el-pagination",
        "descriptions": "el-descriptions",
        "descriptions_item": "el-descriptions-item",
        "select": "el-select",
        "date_picker": "el-date-picker",
    });
    context.insert("ui_tags", &ui_tags);
    context.insert("with_tests", &false);

    let result = engine.render("vue/list.vue.tera", &context);
    assert!(result.is_ok());
    let content = result.unwrap();
    assert!(content.contains("el-table"));
}

// ── Vue 组件生成集成测试 ──

#[tokio::test]
async fn test_vue_component_generator() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let model = create_test_model();
    let ui_adapted = UiAdapter::adapt(&model, UiLibrary::ElementPlus);
    let config = GenerationConfig::default();

    let generator = VueComponentGenerator::new(&engine);
    let result = generator.generate(&model, &ui_adapted, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    assert_eq!(files.len(), 4);

    let names: Vec<&str> = files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(names.contains(&"Index.vue"));
    assert!(names.contains(&"Show.vue"));
    assert!(names.contains(&"Create.vue"));
    assert!(names.contains(&"Edit.vue"));
}

// ── React 组件生成集成测试 ──

#[tokio::test]
async fn test_react_component_generator() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let model = create_test_model();
    let config = GenerationConfig {
        framework: Framework::React,
        ..Default::default()
    };

    let generator = ReactComponentGenerator::new(&engine);
    let result = generator.generate(&model, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    assert_eq!(files.len(), 4);

    let names: Vec<&str> = files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(names.contains(&"Index.tsx"));
    assert!(names.contains(&"Show.tsx"));
    assert!(names.contains(&"Create.tsx"));
    assert!(names.contains(&"Edit.tsx"));
}

// ── 路由生成集成测试 ──

#[tokio::test]
async fn test_route_generator_vue() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let routes = vec![FrontendRoute {
        path: "/user".to_string(),
        name: "user".to_string(),
        component: "user/Index.vue".to_string(),
        meta: RouteMeta {
            title: Some("用户管理".to_string()),
            permission: Some("user:list".to_string()),
            lazy: true,
        },
        children: vec![],
    }];

    let config = GenerationConfig::default();
    let generator = RouteGenerator::new(&engine);
    let result = generator.generate(&routes, &config).await;
    assert!(result.is_ok());
    let file = result.unwrap();
    assert!(file.path.to_string_lossy().contains("routes.ts"));
}

// ── 权限生成集成测试 ──

#[tokio::test]
async fn test_permission_generator() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let perm_config = PermissionConfig {
        permissions: vec!["user:list".to_string(), "user:create".to_string()],
        roles: vec!["admin".to_string()],
        login_path: "/login".to_string(),
        forbidden_path: "/403".to_string(),
    };

    let config = GenerationConfig::default();
    let generator = PermissionGenerator::new(&engine);
    let result = generator.generate(&perm_config, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    assert_eq!(files.len(), 4);
}

// ── API 客户端生成集成测试 ──

#[tokio::test]
async fn test_api_client_generator() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let openapi_spec = serde_json::json!({
        "paths": {
            "/user": {
                "get": { "summary": "List users" },
                "post": { "summary": "Create user" }
            },
            "/user/{id}": {
                "get": { "summary": "Show user" },
                "put": { "summary": "Update user" },
                "delete": { "summary": "Delete user" }
            }
        }
    });

    let config = GenerationConfig::default();
    let generator = ApiClientGenerator::new(&engine);
    let result = generator.generate(&openapi_spec, &config).await;
    assert!(
        result.is_ok(),
        "API client generation failed: {:?}",
        result.err()
    );
    let files = result.unwrap();
    assert!(!files.is_empty());
}

// ── OpenAPI Schema 提取测试 ──

#[test]
fn test_openapi_schema_extractor_request() {
    let spec = serde_json::json!({
        "paths": {
            "/user": {
                "post": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "age": { "type": "integer" }
                                    },
                                    "required": ["name"]
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let result = OpenApiSchemaExtractor::extract_request_schema(&spec, "/user", "post");
    assert!(result.is_some());
    let def = result.unwrap();
    assert_eq!(def.fields.len(), 2);

    let name_field = def.fields.iter().find(|f| f.name == "name").unwrap();
    assert!(!name_field.optional);

    let age_field = def.fields.iter().find(|f| f.name == "age").unwrap();
    assert!(age_field.optional);
}

#[test]
fn test_openapi_schema_extractor_response() {
    let spec = serde_json::json!({
        "paths": {
            "/user/{id}": {
                "get": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "id": { "type": "integer" },
                                            "name": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let result = OpenApiSchemaExtractor::extract_response_schema(&spec, "/user/{id}", "get");
    assert!(result.is_some());
    let def = result.unwrap();
    assert_eq!(def.fields.len(), 2);
}

// ── 文件写入集成测试 ──

#[tokio::test]
async fn test_file_writer_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.txt");
    FileWriter::write_atomic(&path, "hello world")
        .await
        .unwrap();
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(content, "hello world");
}

#[tokio::test]
async fn test_file_writer_batch_skip() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let existing_file = output_dir.join("existing.txt");
    tokio::fs::write(&existing_file, "old").await.unwrap();

    let files = vec![
        (PathBuf::from("existing.txt"), "new".to_string()),
        (PathBuf::from("new.txt"), "content".to_string()),
    ];

    let result = FileWriter::write_batch(files, &output_dir, OverrideStrategy::Skip).await;
    assert!(result.is_ok());
    let write_result = result.unwrap();
    assert_eq!(write_result.skipped.len(), 1);
    assert_eq!(write_result.success.len(), 1);

    let existing_content = tokio::fs::read_to_string(&existing_file).await.unwrap();
    assert_eq!(existing_content, "old");
}

#[tokio::test]
async fn test_file_writer_batch_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let existing_file = output_dir.join("existing.txt");
    tokio::fs::write(&existing_file, "old").await.unwrap();

    let files = vec![(PathBuf::from("existing.txt"), "new".to_string())];

    let result = FileWriter::write_batch(files, &output_dir, OverrideStrategy::Overwrite).await;
    assert!(result.is_ok());
    let write_result = result.unwrap();
    assert_eq!(write_result.success.len(), 1);
    assert!(write_result.success[0].is_overwritten);

    let content = tokio::fs::read_to_string(&existing_file).await.unwrap();
    assert_eq!(content, "new");
}

// ── CodegenService 集成测试 ──

#[tokio::test]
async fn test_codegen_service_missing_model() {
    let service = CodegenService::new();
    let config = GenerationConfig {
        models: vec![],
        ..Default::default()
    };
    let result = service.generate(config).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_MISSING");
}

#[tokio::test]
async fn test_codegen_service_model_dir_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let service = CodegenService::new();
    let config = GenerationConfig {
        models: vec!["User".to_string()],
        model_dir: temp.path().join("nonexistent"),
        output_dir: temp.path().join("output"),
        ..Default::default()
    };
    let result = service.generate(config).await;
    assert!(result.is_err());
}

// ── 端到端生产接线测试 ──

#[tokio::test]
async fn test_e2e_cli_produces_vue_project() {
    let model_temp = tempfile::tempdir().unwrap();
    let model_file = model_temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: Option<String>,
    pub created_at: DateTime,
}
"#,
    )
    .await
    .unwrap();

    let out_temp = tempfile::tempdir().unwrap();
    let output_dir = out_temp.path().to_path_buf();

    let config = GenerationConfig {
        models: vec!["User".to_string()],
        model_dir: model_temp.path().to_path_buf(),
        framework: Framework::Vue,
        ui_library: UiLibrary::ElementPlus,
        output_dir: output_dir.clone(),
        template_dir: None,
        override_strategy: OverrideStrategy::Overwrite,
        with_tests: false,
        with_interceptors: false,
        lazy_load: true,
        force: true,
    };

    let service = CodegenService::new();
    let report = service.generate(config).await.unwrap();
    assert!(
        report.failures.is_empty(),
        "生成不应有失败: {:?}",
        report.failures
    );
    assert!(!report.generated_files.is_empty(), "应生成至少一个文件");

    let expected_files = [
        "src/views/user/Index.vue",
        "src/views/user/Show.vue",
        "src/views/user/Create.vue",
        "src/views/user/Edit.vue",
    ];

    for expected in &expected_files {
        let full_path = output_dir.join(expected);
        assert!(
            full_path.exists(),
            "预期文件未生成: {}",
            full_path.display()
        );
        let content = tokio::fs::read_to_string(&full_path)
            .await
            .unwrap_or_default();
        assert!(
            !content.trim().is_empty(),
            "文件内容不应为空: {}",
            full_path.display()
        );
    }

    let index_content = tokio::fs::read_to_string(output_dir.join("src/views/user/Index.vue"))
        .await
        .unwrap();
    assert!(
        index_content.contains("el-table") || index_content.contains("<template"),
        "Index.vue 应含 Vue 模板内容"
    );
}

// ── 报告完整性测试 ──

#[tokio::test]
async fn test_report_config_snapshot() {
    let model_temp = tempfile::tempdir().unwrap();
    let model_file = model_temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
}
"#,
    )
    .await
    .unwrap();

    let out_temp = tempfile::tempdir().unwrap();
    let output_dir = out_temp.path().to_path_buf();

    let config = GenerationConfig {
        models: vec!["User".to_string()],
        model_dir: model_temp.path().to_path_buf(),
        framework: Framework::React,
        ui_library: UiLibrary::AntDesignVue,
        output_dir: output_dir.clone(),
        template_dir: None,
        override_strategy: OverrideStrategy::Overwrite,
        with_tests: true,
        with_interceptors: true,
        lazy_load: false,
        force: true,
    };

    let service = CodegenService::new();
    let report = service.generate(config.clone()).await.unwrap();

    let snap = report.config.as_ref().expect("config 快照应存在");
    assert_eq!(snap.models, config.models);
    assert_eq!(snap.framework, config.framework);
    assert_eq!(snap.ui_library, config.ui_library);
    assert_eq!(snap.with_tests, config.with_tests);

    let json = serde_json::to_string(&report).unwrap();
    assert!(
        !json.contains(output_dir.to_str().unwrap()),
        "序列化 JSON 不应含 output_dir 绝对路径（脱敏）"
    );
    assert!(
        !json.contains("model_dir"),
        "序列化 JSON 不应含 model_dir 字段（config 已 skip_serializing）"
    );
}

// ── 路径穿越防护测试 ──

#[tokio::test]
async fn test_path_traversal_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let malicious_files = vec![
        (PathBuf::from("../../etc/passwd"), "malicious".to_string()),
        (PathBuf::from("../../../tmp/evil.txt"), "evil".to_string()),
    ];

    let result =
        FileWriter::write_batch(malicious_files, &output_dir, OverrideStrategy::Overwrite).await;
    assert!(result.is_ok(), "write_batch 应返回 Ok，越界文件记入 failed");
    let write_result = result.unwrap();
    assert_eq!(write_result.failed.len(), 2, "两个路径穿越文件均应被拒绝");

    for (path, _msg) in &write_result.failed {
        assert!(
            path.to_string_lossy().contains(".."),
            "被拒绝的路径应含 ..: {:?}",
            path
        );
    }

    let passwd_path = temp.path().join("etc/passwd");
    assert!(
        !passwd_path.exists(),
        "越界文件不应被创建: {:?}",
        passwd_path
    );

    let evil_path = std::path::Path::new("/tmp/evil.txt");
    let _ = evil_path;
}

// ── 确定性生成测试 ──

#[tokio::test]
async fn test_deterministic_generation() {
    let model_temp = tempfile::tempdir().unwrap();
    let model_file = model_temp.path().join("user.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: Option<String>,
    pub created_at: DateTime,
}
"#,
    )
    .await
    .unwrap();

    let out1 = tempfile::tempdir().unwrap();
    let out2 = tempfile::tempdir().unwrap();

    let make_config = |output_dir: PathBuf| GenerationConfig {
        models: vec!["User".to_string()],
        model_dir: model_temp.path().to_path_buf(),
        framework: Framework::Vue,
        ui_library: UiLibrary::ElementPlus,
        output_dir,
        template_dir: None,
        override_strategy: OverrideStrategy::Overwrite,
        with_tests: false,
        with_interceptors: false,
        lazy_load: true,
        force: true,
    };

    let service = CodegenService::new();
    let report1 = service
        .generate(make_config(out1.path().to_path_buf()))
        .await
        .unwrap();
    let report2 = service
        .generate(make_config(out2.path().to_path_buf()))
        .await
        .unwrap();

    assert_eq!(
        report1.generated_files.len(),
        report2.generated_files.len(),
        "两次生成的文件数量应相同"
    );

    for (f1, f2) in report1
        .generated_files
        .iter()
        .zip(report2.generated_files.iter())
    {
        assert_eq!(f1.path, f2.path, "文件路径应相同（确定性排序）");
        assert_eq!(
            f1.size_bytes, f2.size_bytes,
            "文件大小应相同（确定性生成）: {:?}",
            f1.path
        );

        let abs1 = out1.path().join(&f1.path);
        let abs2 = out2.path().join(&f2.path);
        let content1 = tokio::fs::read(&abs1).await.unwrap();
        let content2 = tokio::fs::read(&abs2).await.unwrap();
        assert_eq!(
            content1, content2,
            "文件内容应字节级相同（确定性生成）: {:?}",
            f1.path
        );
    }

    let paths1: Vec<_> = report1.generated_files.iter().map(|f| &f.path).collect();
    let mut paths1_sorted = paths1.clone();
    paths1_sorted.sort();
    assert_eq!(
        paths1, paths1_sorted,
        "generated_files 应按 path 字典序排序"
    );
}

// ── 辅助函数 ──

fn create_test_model() -> ModelMetadata {
    use sz_rust_frontend_codegen::metadata::FieldMetadata;

    ModelMetadata {
        name: "User".to_string(),
        table_name: "user".to_string(),
        module_name: "user".to_string(),
        fields: vec![
            FieldMetadata {
                name: "id".to_string(),
                rust_type: "i32".to_string(),
                ts_type: "number".to_string(),
                sql_type: "INTEGER".to_string(),
                is_nullable: false,
                is_primary_key: true,
                is_indexed: false,
                is_sensitive: false,
                is_auto_timestamp: false,
                validation_rules: vec![],
                relation: None,
                doc_comment: None,
            },
            FieldMetadata {
                name: "name".to_string(),
                rust_type: "String".to_string(),
                ts_type: "string".to_string(),
                sql_type: "VARCHAR".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_indexed: false,
                is_sensitive: false,
                is_auto_timestamp: false,
                validation_rules: vec![],
                relation: None,
                doc_comment: None,
            },
            FieldMetadata {
                name: "email".to_string(),
                rust_type: "String".to_string(),
                ts_type: "string".to_string(),
                sql_type: "VARCHAR".to_string(),
                is_nullable: true,
                is_primary_key: false,
                is_indexed: true,
                is_sensitive: false,
                is_auto_timestamp: false,
                validation_rules: vec![],
                relation: None,
                doc_comment: None,
            },
        ],
        relations: vec![],
        validations: vec![],
        doc_comment: None,
    }
}
