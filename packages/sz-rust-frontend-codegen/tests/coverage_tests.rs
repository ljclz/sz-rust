//! 覆盖率补充测试：config、filters、report、path_guard、service、template_engine、model_parser 等

use std::path::{Path, PathBuf};

use sz_rust_frontend_codegen::config::{
    load_config_file, merge_config, Framework, GenerationConfig, OverrideStrategy, UiLibrary,
};
use sz_rust_frontend_codegen::error::FrontendCodegenError;
use sz_rust_frontend_codegen::file_writer::FileWriter;
use sz_rust_frontend_codegen::filters::register_filters;
use sz_rust_frontend_codegen::generators::api_client::{
    ApiClientGenerator, OpenApiSchemaExtractor,
};
use sz_rust_frontend_codegen::generators::route::{
    group_by_prefix, FrontendRoute, RouteGenerator, RouteMeta,
};
use sz_rust_frontend_codegen::generators::vue::VueComponentGenerator;
use sz_rust_frontend_codegen::metadata::{FieldMetadata, ModelMetadata};
use sz_rust_frontend_codegen::model_parser::ModelParser;
use sz_rust_frontend_codegen::path_guard::PathGuard;
use sz_rust_frontend_codegen::report::{GeneratedFile, GenerationReport, SkippedFile, Warning};
use sz_rust_frontend_codegen::service::{parse_models, validate_templates, CodegenService};
use sz_rust_frontend_codegen::template_engine::CodegenTemplateEngine;
use sz_rust_frontend_codegen::ui_adapter::UiAdapter;
use tera::{Context, Tera, Value};

// ── config.rs: load_config_file / merge_config ──

#[tokio::test]
async fn test_load_config_file_valid() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(".codegen.toml");
    let content = r#"
models = ["User", "Order"]
model_dir = "src/model/"
framework = "react"
ui_library = "ant_design_vue"
output_dir = "./out/"
override_strategy = "overwrite"
with_tests = true
with_interceptors = true
lazy_load = false
force = true
"#;
    tokio::fs::write(&config_path, content).await.unwrap();

    let result = load_config_file(&config_path).await;
    assert!(
        result.is_ok(),
        "load_config_file failed: {:?}",
        result.err()
    );
    let config = result.unwrap();
    assert_eq!(config.models, vec!["User".to_string(), "Order".to_string()]);
    assert_eq!(config.framework, Framework::React);
    assert_eq!(config.ui_library, UiLibrary::AntDesignVue);
    assert_eq!(config.override_strategy, OverrideStrategy::Overwrite);
    assert!(config.with_tests);
    assert!(config.with_interceptors);
    assert!(!config.lazy_load);
    assert!(config.force);
}

#[tokio::test]
async fn test_load_config_file_not_found() {
    let result = load_config_file(Path::new("/nonexistent/path/config.toml")).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_CONFIG_PARSE_ERROR");
}

#[tokio::test]
async fn test_load_config_file_invalid_toml() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("bad.toml");
    tokio::fs::write(&config_path, "this is not valid toml = = =")
        .await
        .unwrap();

    let result = load_config_file(&config_path).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_CONFIG_PARSE_ERROR");
}

#[test]
fn test_merge_config_cli_overrides_file() {
    let file_config = GenerationConfig {
        models: vec!["FromFile".to_string()],
        model_dir: PathBuf::from("file/model/"),
        framework: Framework::Vue,
        ui_library: UiLibrary::ElementPlus,
        output_dir: PathBuf::from("./file_out/"),
        template_dir: Some(PathBuf::from("./file_tmpl/")),
        override_strategy: OverrideStrategy::Skip,
        with_tests: true,
        with_interceptors: true,
        lazy_load: false,
        force: true,
    };
    // CLI 传入非默认值以覆盖 file_config
    let cli_args = GenerationConfig {
        models: vec!["FromCli".to_string()],
        model_dir: PathBuf::from("cli/model/"),
        framework: Framework::React,
        ui_library: UiLibrary::AntDesignVue,
        output_dir: PathBuf::from("./cli_out/"),
        template_dir: Some(PathBuf::from("./cli_tmpl/")),
        override_strategy: OverrideStrategy::Merge,
        with_tests: false,
        with_interceptors: false,
        lazy_load: true,
        force: false,
    };

    let merged = merge_config(file_config, cli_args);
    assert_eq!(merged.models, vec!["FromCli".to_string()]);
    assert_eq!(merged.model_dir, PathBuf::from("cli/model/"));
    assert_eq!(merged.framework, Framework::React);
    assert_eq!(merged.ui_library, UiLibrary::AntDesignVue);
    assert_eq!(merged.output_dir, PathBuf::from("./cli_out/"));
    assert_eq!(merged.template_dir, Some(PathBuf::from("./cli_tmpl/")));
    assert_eq!(merged.override_strategy, OverrideStrategy::Merge);
    // with_tests: cli(false) || file(true) = true
    assert!(merged.with_tests);
    // with_interceptors: cli(false) || file(true) = true
    assert!(merged.with_interceptors);
    // lazy_load: cli(true) && file(false) = false
    assert!(!merged.lazy_load);
    // force: cli(false) || file(true) = true
    assert!(merged.force);
}

#[test]
fn test_merge_config_cli_defaults_use_file_values() {
    let file_config = GenerationConfig {
        models: vec!["FromFile".to_string()],
        model_dir: PathBuf::from("file/model/"),
        framework: Framework::React,
        ui_library: UiLibrary::AntDesignVue,
        output_dir: PathBuf::from("./file_out/"),
        template_dir: Some(PathBuf::from("./file_tmpl/")),
        override_strategy: OverrideStrategy::Overwrite,
        with_tests: true,
        with_interceptors: true,
        lazy_load: false,
        force: true,
    };
    let cli_args = GenerationConfig::default();

    let merged = merge_config(file_config, cli_args);
    assert_eq!(merged.models, vec!["FromFile".to_string()]);
    assert_eq!(merged.model_dir, PathBuf::from("file/model/"));
    assert_eq!(merged.framework, Framework::React);
    assert_eq!(merged.ui_library, UiLibrary::AntDesignVue);
    assert_eq!(merged.output_dir, PathBuf::from("./file_out/"));
    assert_eq!(merged.template_dir, Some(PathBuf::from("./file_tmpl/")));
    assert_eq!(merged.override_strategy, OverrideStrategy::Overwrite);
    assert!(merged.with_tests);
    assert!(merged.with_interceptors);
    assert!(!merged.lazy_load);
    assert!(merged.force);
}

#[test]
fn test_merge_config_template_dir_cli_priority() {
    let file_config = GenerationConfig {
        template_dir: Some(PathBuf::from("./file_tmpl/")),
        ..Default::default()
    };
    let cli_args = GenerationConfig::default();
    let merged = merge_config(file_config, cli_args);
    assert_eq!(merged.template_dir, Some(PathBuf::from("./file_tmpl/")));
}

// ── filters.rs: 通过 Tera 模板调用所有过滤器 ──

fn make_tera_with_filters() -> Tera {
    let mut tera = Tera::default();
    register_filters(&mut tera);
    tera.add_raw_template("t.txt", "{{ s | rust_to_ts_type }}")
        .unwrap();
    tera.add_raw_template("p.txt", "{{ s | snake_to_pascal }}")
        .unwrap();
    tera.add_raw_template("k.txt", "{{ s | pascal_to_kebab }}")
        .unwrap();
    tera.add_raw_template("c.txt", "{{ s | snake_to_camel }}")
        .unwrap();
    tera.add_raw_template("sens.txt", "{{ s | is_sensitive }}")
        .unwrap();
    tera.add_raw_template("pl.txt", "{{ s | pluralize }}")
        .unwrap();
    tera.add_raw_template("sg.txt", "{{ s | singularize }}")
        .unwrap();
    tera.add_raw_template("cap.txt", "{{ s | capitalize }}")
        .unwrap();
    tera
}

#[test]
fn test_filter_rust_to_ts_type_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "i32");
    let out = tera.render("t.txt", &ctx).unwrap();
    assert_eq!(out, "number");
}

#[test]
fn test_filter_snake_to_pascal_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "user_order");
    let out = tera.render("p.txt", &ctx).unwrap();
    assert_eq!(out, "UserOrder");
}

#[test]
fn test_filter_snake_to_pascal_empty_part() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "");
    let out = tera.render("p.txt", &ctx).unwrap();
    assert_eq!(out, "");
}

#[test]
fn test_filter_pascal_to_kebab_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "UserOrder");
    let out = tera.render("k.txt", &ctx).unwrap();
    assert_eq!(out, "user-order");
}

#[test]
fn test_filter_pascal_to_kebab_single_char() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "A");
    let out = tera.render("k.txt", &ctx).unwrap();
    assert_eq!(out, "a");
}

#[test]
fn test_filter_snake_to_camel_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "user_order");
    let out = tera.render("c.txt", &ctx).unwrap();
    assert_eq!(out, "userOrder");
}

#[test]
fn test_filter_snake_to_camel_single_part() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "user");
    let out = tera.render("c.txt", &ctx).unwrap();
    assert_eq!(out, "user");
}

#[test]
fn test_filter_snake_to_camel_empty() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "");
    let out = tera.render("c.txt", &ctx).unwrap();
    assert_eq!(out, "");
}

#[test]
fn test_filter_is_sensitive_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "password");
    let out = tera.render("sens.txt", &ctx).unwrap();
    assert_eq!(out, "true");
}

#[test]
fn test_filter_is_sensitive_false_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "name");
    let out = tera.render("sens.txt", &ctx).unwrap();
    assert_eq!(out, "false");
}

#[test]
fn test_filter_pluralize_y_suffix() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "category");
    let out = tera.render("pl.txt", &ctx).unwrap();
    assert_eq!(out, "categories");
}

#[test]
fn test_filter_pluralize_s_suffix() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "users");
    let out = tera.render("pl.txt", &ctx).unwrap();
    assert_eq!(out, "users");
}

#[test]
fn test_filter_pluralize_normal() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "order");
    let out = tera.render("pl.txt", &ctx).unwrap();
    assert_eq!(out, "orders");
}

#[test]
fn test_filter_singularize_ies_suffix() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "categories");
    let out = tera.render("sg.txt", &ctx).unwrap();
    assert_eq!(out, "category");
}

#[test]
fn test_filter_singularize_s_suffix() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "orders");
    let out = tera.render("sg.txt", &ctx).unwrap();
    assert_eq!(out, "order");
}

#[test]
fn test_filter_singularize_ss_suffix() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "class");
    let out = tera.render("sg.txt", &ctx).unwrap();
    assert_eq!(out, "class");
}

#[test]
fn test_filter_singularize_no_change() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "user");
    let out = tera.render("sg.txt", &ctx).unwrap();
    assert_eq!(out, "user");
}

#[test]
fn test_filter_capitalize_via_tera() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "user");
    let out = tera.render("cap.txt", &ctx).unwrap();
    assert_eq!(out, "User");
}

#[test]
fn test_filter_capitalize_empty() {
    let tera = make_tera_with_filters();
    let mut ctx = Context::new();
    ctx.insert("s", "");
    let out = tera.render("cap.txt", &ctx).unwrap();
    assert_eq!(out, "");
}

#[test]
fn test_filter_non_string_value() {
    // 测试过滤器对非字符串值的处理（unwrap_or 兜底）
    let mut tera = Tera::default();
    register_filters(&mut tera);
    tera.add_raw_template("n.txt", "{{ v | capitalize }}")
        .unwrap();
    let mut ctx = Context::new();
    ctx.insert("v", &Value::Null);
    let out = tera.render("n.txt", &ctx).unwrap();
    assert_eq!(out, "");
}

// ── report.rs: format_cli 各分支 ──

#[test]
fn test_report_format_cli_with_generated_files() {
    let mut report = GenerationReport::new();
    report.generated_files.push(GeneratedFile {
        path: PathBuf::from("src/views/user/Index.vue"),
        size_bytes: 1024,
        source_model: "User".to_string(),
        source_template: "vue/list.vue.tera".to_string(),
        is_overwritten: false,
    });
    report.generated_files.push(GeneratedFile {
        path: PathBuf::from("src/views/user/Show.vue"),
        size_bytes: 512,
        source_model: "User".to_string(),
        source_template: "vue/show.vue.tera".to_string(),
        is_overwritten: true,
    });
    let output = report.format_cli();
    assert!(output.contains("✓ 生成文件"));
    assert!(output.contains("Index.vue"));
    assert!(output.contains("Show.vue"));
    assert!(output.contains("(覆盖)"));
}

#[test]
fn test_report_format_cli_with_skipped_files() {
    let mut report = GenerationReport::new();
    report.skipped_files.push(SkippedFile {
        path: PathBuf::from("src/old.txt"),
        reason: "文件已存在".to_string(),
    });
    let output = report.format_cli();
    assert!(output.contains("⊘ 跳过文件"));
    assert!(output.contains("old.txt"));
    assert!(output.contains("文件已存在"));
}

#[test]
fn test_report_format_cli_with_warnings() {
    let mut report = GenerationReport::new();
    report.warnings.push(Warning {
        code: "W001".to_string(),
        message: "字段类型未知".to_string(),
        related_file: None,
    });
    let output = report.format_cli();
    assert!(output.contains("⚠ 警告"));
    assert!(output.contains("W001"));
    assert!(output.contains("字段类型未知"));
}

#[test]
fn test_report_format_cli_with_failures() {
    use sz_rust_frontend_codegen::report::Failure;
    let mut report = GenerationReport::new();
    report.failures.push(Failure {
        code: "E001".to_string(),
        message: "渲染失败".to_string(),
        source_model: Some("User".to_string()),
        source_template: Some("vue/list.vue.tera".to_string()),
    });
    let output = report.format_cli();
    assert!(output.contains("✗ 失败"));
    assert!(output.contains("E001"));
    assert!(output.contains("渲染失败"));
}

#[test]
fn test_report_default_equals_new() {
    let r1 = GenerationReport::default();
    let r2 = GenerationReport::new();
    assert!(!r1.trace_id.is_empty());
    assert!(!r2.trace_id.is_empty());
}

// ── path_guard.rs: validate 各错误分支 ──

#[test]
fn test_path_guard_valid_relative() {
    let result = PathGuard::validate(Path::new("src/views/user/Index.vue"), Path::new("."));
    assert!(result.is_ok());
}

#[test]
fn test_path_guard_rejects_null_byte() {
    let path = Path::new("src\0evil");
    let result = PathGuard::validate(path, Path::new("."));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
}

#[test]
fn test_path_guard_rejects_absolute_path() {
    let result = PathGuard::validate(Path::new("/etc/passwd"), Path::new("."));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
}

#[test]
fn test_path_guard_rejects_parent_dir() {
    let result = PathGuard::validate(Path::new("../etc/passwd"), Path::new("."));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
}

#[test]
fn test_path_guard_rejects_nested_parent_dir() {
    let result = PathGuard::validate(Path::new("src/../../etc/passwd"), Path::new("."));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
}

// ── error.rs: 未覆盖的 error_code 分支 ──

#[test]
fn test_error_code_model_parse_error() {
    let err = FrontendCodegenError::ModelParseError("parse fail".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_PARSE_ERROR");
}

#[test]
fn test_error_code_template_dir_not_found() {
    let err = FrontendCodegenError::TemplateDirNotFound("/tmpl".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND");
}

#[test]
fn test_error_code_template_missing() {
    let err = FrontendCodegenError::TemplateMissing("list.vue.tera".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_MISSING");
}

#[test]
fn test_error_code_template_syntax_error() {
    let err = FrontendCodegenError::TemplateSyntaxError("syntax".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_SYNTAX_ERROR");
}

#[test]
fn test_error_code_template_render_error() {
    let err = FrontendCodegenError::TemplateRenderError("render".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_RENDER_ERROR");
}

#[test]
fn test_error_code_template_inheritance_cycle() {
    let err = FrontendCodegenError::TemplateInheritanceCycle("cycle".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_INHERITANCE_CYCLE");
}

#[test]
fn test_error_code_unknown_filter() {
    let err = FrontendCodegenError::UnknownFilter("foo".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_FILTER_UNKNOWN");
}

#[test]
fn test_error_code_file_write_error() {
    let err = FrontendCodegenError::FileWriteError("write fail".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_FILE_WRITE_ERROR");
}

#[test]
fn test_error_code_output_dir_not_empty() {
    let err = FrontendCodegenError::OutputDirNotEmpty("/out".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_OUTPUT_DIR_NOT_EMPTY");
}

#[test]
fn test_error_code_generic() {
    let err = FrontendCodegenError::Generic("generic".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_GENERIC");
}

#[test]
fn test_error_from_tera_error() {
    // 构造一个 tera::Error 并通过 From 转换
    let tera_err = tera::Error::msg("template not found");
    let err: FrontendCodegenError = tera_err.into();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_RENDER_ERROR");
    assert!(err.to_string().contains("template not found"));
}

// ── generators/route.rs: group_by_prefix / React 框架 ──

#[test]
fn test_group_by_prefix_multiple_groups() {
    let routes = vec![
        FrontendRoute {
            path: "/user/list".to_string(),
            name: "user_list".to_string(),
            component: "user/Index.vue".to_string(),
            meta: RouteMeta {
                title: None,
                permission: None,
                lazy: false,
            },
            children: vec![],
        },
        FrontendRoute {
            path: "/order/list".to_string(),
            name: "order_list".to_string(),
            component: "order/Index.vue".to_string(),
            meta: RouteMeta {
                title: None,
                permission: None,
                lazy: false,
            },
            children: vec![],
        },
        FrontendRoute {
            path: "/user/detail".to_string(),
            name: "user_detail".to_string(),
            component: "user/Show.vue".to_string(),
            meta: RouteMeta {
                title: None,
                permission: None,
                lazy: false,
            },
            children: vec![],
        },
    ];
    let groups = group_by_prefix(&routes);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups.get("user").unwrap().len(), 2);
    assert_eq!(groups.get("order").unwrap().len(), 1);
}

#[test]
fn test_group_by_prefix_empty_path() {
    let routes = vec![FrontendRoute {
        path: "noslash".to_string(),
        name: "test".to_string(),
        component: "test.vue".to_string(),
        meta: RouteMeta {
            title: None,
            permission: None,
            lazy: false,
        },
        children: vec![],
    }];
    let groups = group_by_prefix(&routes);
    assert_eq!(groups.len(), 1);
    assert!(groups.contains_key(""));
}

#[tokio::test]
async fn test_route_generator_react_framework() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let routes = vec![FrontendRoute {
        path: "/user".to_string(),
        name: "user".to_string(),
        component: "user/Index.tsx".to_string(),
        meta: RouteMeta {
            title: Some("用户".to_string()),
            permission: None,
            lazy: true,
        },
        children: vec![],
    }];

    let config = GenerationConfig {
        framework: Framework::React,
        ..Default::default()
    };
    let generator = RouteGenerator::new(&engine);
    let result = generator.generate(&routes, &config).await;
    assert!(result.is_ok());
    let file = result.unwrap();
    assert!(file.path.to_string_lossy().contains("routes.tsx"));
}

// ── generators/vue.rs: with_tests 分支 ──

#[tokio::test]
async fn test_vue_component_generator_with_tests() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let model = create_test_model();
    let ui_adapted = UiAdapter::adapt(&model, UiLibrary::ElementPlus);
    let config = GenerationConfig {
        with_tests: true,
        ..Default::default()
    };

    let generator = VueComponentGenerator::new(&engine);
    let result = generator.generate(&model, &ui_adapted, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    // 4 个组件 + 4 个测试骨架
    assert_eq!(files.len(), 8);

    let has_spec = files
        .iter()
        .any(|f| f.path.to_string_lossy().contains(".spec.ts"));
    assert!(has_spec);
}

// ── generators/api_client.rs: schema_to_ts_interface 各类型分支 + with_interceptors ──

#[test]
fn test_schema_to_ts_interface_boolean_type() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "active": { "type": "boolean" }
        }
    });
    let result = OpenApiSchemaExtractor::schema_to_ts_interface(&schema, "Test");
    assert!(result.is_some());
    let def = result.unwrap();
    let field = def.fields.iter().find(|f| f.name == "active").unwrap();
    assert_eq!(field.ts_type, "boolean");
}

#[test]
fn test_schema_to_ts_interface_array_type() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "tags": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    });
    let result = OpenApiSchemaExtractor::schema_to_ts_interface(&schema, "Test");
    assert!(result.is_some());
    let def = result.unwrap();
    let field = def.fields.iter().find(|f| f.name == "tags").unwrap();
    assert_eq!(field.ts_type, "string[]");
}

#[test]
fn test_schema_to_ts_interface_array_no_item_type() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "items": { "type": "array" }
        }
    });
    let result = OpenApiSchemaExtractor::schema_to_ts_interface(&schema, "Test");
    assert!(result.is_some());
    let def = result.unwrap();
    let field = def.fields.iter().find(|f| f.name == "items").unwrap();
    assert_eq!(field.ts_type, "any[]");
}

#[test]
fn test_schema_to_ts_interface_unknown_type() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "custom": { "type": "object" }
        }
    });
    let result = OpenApiSchemaExtractor::schema_to_ts_interface(&schema, "Test");
    assert!(result.is_some());
    let def = result.unwrap();
    let field = def.fields.iter().find(|f| f.name == "custom").unwrap();
    assert_eq!(field.ts_type, "any");
}

#[test]
fn test_schema_to_ts_interface_no_properties() {
    let schema = serde_json::json!({ "type": "object" });
    let result = OpenApiSchemaExtractor::schema_to_ts_interface(&schema, "Test");
    assert!(result.is_none());
}

#[test]
fn test_extract_request_schema_missing_path() {
    let spec = serde_json::json!({ "paths": {} });
    let result = OpenApiSchemaExtractor::extract_request_schema(&spec, "/missing", "post");
    assert!(result.is_none());
}

#[test]
fn test_extract_request_schema_missing_method() {
    let spec = serde_json::json!({
        "paths": { "/user": {} }
    });
    let result = OpenApiSchemaExtractor::extract_request_schema(&spec, "/user", "post");
    assert!(result.is_none());
}

#[test]
fn test_extract_response_schema_default_response() {
    let spec = serde_json::json!({
        "paths": {
            "/user": {
                "get": {
                    "responses": {
                        "default": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "msg": { "type": "string" }
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
    let result = OpenApiSchemaExtractor::extract_response_schema(&spec, "/user", "get");
    assert!(result.is_some());
    let def = result.unwrap();
    assert_eq!(def.fields.len(), 1);
}

#[test]
fn test_extract_response_schema_missing_responses() {
    let spec = serde_json::json!({
        "paths": {
            "/user": {
                "get": {}
            }
        }
    });
    let result = OpenApiSchemaExtractor::extract_response_schema(&spec, "/user", "get");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_api_client_generator_with_interceptors() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let openapi_spec = serde_json::json!({
        "paths": {
            "/user": {
                "get": { "summary": "List" },
                "post": { "summary": "Create" }
            }
        }
    });

    let config = GenerationConfig {
        with_interceptors: true,
        ..Default::default()
    };
    let generator = ApiClientGenerator::new(&engine);
    let result = generator.generate(&openapi_spec, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    // 应包含 request.ts 拦截器
    let has_request = files
        .iter()
        .any(|f| f.path.to_string_lossy().contains("request.ts"));
    assert!(has_request);
}

#[tokio::test]
async fn test_api_client_generator_empty_paths() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    let openapi_spec = serde_json::json!({});
    let config = GenerationConfig::default();
    let generator = ApiClientGenerator::new(&engine);
    let result = generator.generate(&openapi_spec, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    assert!(files.is_empty());
}

#[tokio::test]
async fn test_api_client_generator_unsupported_method_filtered() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();

    // HEAD/OPTIONS 等非主流方法应被过滤
    let openapi_spec = serde_json::json!({
        "paths": {
            "/user": {
                "head": { "summary": "Head" },
                "options": { "summary": "Options" },
                "get": { "summary": "List" }
            }
        }
    });
    let config = GenerationConfig::default();
    let generator = ApiClientGenerator::new(&engine);
    let result = generator.generate(&openapi_spec, &config).await;
    assert!(result.is_ok());
    let files = result.unwrap();
    // 只有 GET 被处理，生成 api/user.ts + types/user.ts
    assert_eq!(files.len(), 2);
}

// ── template_engine.rs: custom_dir 加载 ──

#[tokio::test]
async fn test_template_engine_with_custom_dir() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let temp = tempfile::tempdir().unwrap();
    let custom_dir = temp.path().join("custom_templates");
    tokio::fs::create_dir_all(&custom_dir).await.unwrap();

    // 创建一个自定义模板覆盖内置模板
    let custom_template = custom_dir.join("vue").join("list.vue.tera");
    tokio::fs::create_dir_all(custom_template.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &custom_template,
        "<!-- custom template -->\n<div>Custom List</div>\n",
    )
    .await
    .unwrap();

    let result = CodegenTemplateEngine::init(&builtin_dir, Some(&custom_dir)).await;
    assert!(
        result.is_ok(),
        "init with custom dir failed: {:?}",
        result.err()
    );
    let engine = result.unwrap();

    let ctx = Context::new();
    let rendered = engine.render("vue/list.vue.tera", &ctx).unwrap();
    assert!(rendered.contains("Custom List"));
}

#[tokio::test]
async fn test_template_engine_custom_dir_not_found() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let result = CodegenTemplateEngine::init(
        &builtin_dir,
        Some(Path::new("/nonexistent/custom/templates")),
    )
    .await;
    match result {
        Err(err) => assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND"),
        Ok(_) => panic!("expected error but got Ok"),
    }
}

#[tokio::test]
async fn test_template_engine_builtin_dir_not_found() {
    let result =
        CodegenTemplateEngine::init(Path::new("/nonexistent/builtin/templates"), None).await;
    match result {
        Err(err) => assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND"),
        Ok(_) => panic!("expected error but got Ok"),
    }
}

#[tokio::test]
async fn test_template_engine_builtin_dir_accessor() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();
    let returned = engine.builtin_dir();
    assert_eq!(returned, builtin_dir.as_path());
}

#[tokio::test]
async fn test_template_engine_render_missing_template() {
    let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
    let engine = CodegenTemplateEngine::init(&builtin_dir, None)
        .await
        .unwrap();
    let ctx = Context::new();
    let result = engine.render("nonexistent/template.tera", &ctx);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_RENDER_ERROR");
}

// ── model_parser.rs: parse_file / extract_metadata / 文档注释 / field 属性 ──

#[tokio::test]
async fn test_parse_file_with_model_derive() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("order.rs");
    tokio::fs::write(
        &model_file,
        r#"
/// 订单模型
#[derive(Model)]
pub struct Order {
    /// 订单 ID
    pub id: i32,
    /// 订单名称
    pub name: String,
}
"#,
    )
    .await
    .unwrap();

    let result = ModelParser::parse_file(&model_file).await;
    assert!(result.is_ok());
    let opt = result.unwrap();
    assert!(opt.is_some());
    let model = opt.unwrap();
    assert_eq!(model.name, "Order");
    assert_eq!(model.module_name, "order");
    assert_eq!(model.doc_comment, Some("订单模型".to_string()));

    let id_field = model.fields.iter().find(|f| f.name == "id").unwrap();
    assert_eq!(id_field.doc_comment, Some("订单 ID".to_string()));
}

#[tokio::test]
async fn test_parse_file_without_model_derive() {
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

    let result = ModelParser::parse_file(&model_file).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn test_parse_file_with_entity_derive() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("entity.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Entity)]
pub struct Product {
    pub id: i32,
    pub title: String,
}
"#,
    )
    .await
    .unwrap();

    let result = ModelParser::parse_file(&model_file).await;
    assert!(result.is_ok());
    let opt = result.unwrap();
    assert!(opt.is_some());
    assert_eq!(opt.unwrap().name, "Product");
}

#[tokio::test]
async fn test_parse_file_with_field_attrs() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("with_attrs.rs");
    tokio::fs::write(
        &model_file,
        r#"
#[derive(Model)]
pub struct Item {
    #[field(pk)]
    pub item_id: i32,
    #[field(index)]
    pub code: String,
    pub name: String,
}
"#,
    )
    .await
    .unwrap();

    let result = ModelParser::parse_file(&model_file).await;
    assert!(result.is_ok());
    let model = result.unwrap().unwrap();

    let id_field = model.fields.iter().find(|f| f.name == "item_id").unwrap();
    assert!(id_field.is_primary_key);

    let code_field = model.fields.iter().find(|f| f.name == "code").unwrap();
    assert!(code_field.is_indexed);
}

#[tokio::test]
async fn test_parse_file_invalid_rust_syntax() {
    let temp = tempfile::tempdir().unwrap();
    let model_file = temp.path().join("bad.rs");
    tokio::fs::write(&model_file, "this is not rust code !!!")
        .await
        .unwrap();

    let result = ModelParser::parse_file(&model_file).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_PARSE_ERROR");
}

#[tokio::test]
async fn test_parse_dir_multiple_models() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::write(
        temp.path().join("user.rs"),
        r#"
#[derive(Model)]
pub struct User { pub id: i32, pub name: String }
"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        temp.path().join("order.rs"),
        r#"
#[derive(Model)]
pub struct Order { pub id: i32, pub total: f64 }
"#,
    )
    .await
    .unwrap();
    // 非 .rs 文件应被跳过
    tokio::fs::write(temp.path().join("readme.md"), "# README")
        .await
        .unwrap();

    let result = ModelParser::parse_dir(temp.path()).await;
    assert!(result.is_ok());
    let models = result.unwrap();
    assert_eq!(models.len(), 2);
}

// ── file_writer.rs: failed 分支 ──

#[tokio::test]
async fn test_file_writer_batch_with_failure() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    // 使用路径穿越触发 PathGuard 失败
    let files = vec![(PathBuf::from("../malicious.txt"), "content".to_string())];

    let result = FileWriter::write_batch(files, &output_dir, OverrideStrategy::Skip).await;
    match result {
        Err(err) => assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL"),
        Ok(_) => panic!("expected error but got Ok"),
    }
}

#[tokio::test]
async fn test_file_writer_batch_merge_strategy_overwrites() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&output_dir).await.unwrap();

    let existing = output_dir.join("data.txt");
    tokio::fs::write(&existing, "old").await.unwrap();

    let files = vec![(PathBuf::from("data.txt"), "new".to_string())];
    let result = FileWriter::write_batch(files, &output_dir, OverrideStrategy::Merge).await;
    assert!(result.is_ok());
    let wr = result.unwrap();
    assert_eq!(wr.success.len(), 1);
    assert!(wr.success[0].is_overwritten);
    let content = tokio::fs::read_to_string(&existing).await.unwrap();
    assert_eq!(content, "new");
}

#[tokio::test]
async fn test_file_writer_atomic_no_parent_dir() {
    // 写入无父目录的文件（当前目录）
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("flat.txt");
    FileWriter::write_atomic(&path, "flat content")
        .await
        .unwrap();
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(content, "flat content");
}

// ── service.rs: parse_models / validate_templates / CodegenService::generate 成功路径 ──

#[tokio::test]
async fn test_parse_models_success() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::write(
        temp.path().join("user.rs"),
        r#"
#[derive(Model)]
pub struct User { pub id: i32, pub name: String }
"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        temp.path().join("order.rs"),
        r#"
#[derive(Model)]
pub struct Order { pub id: i32, pub total: f64 }
"#,
    )
    .await
    .unwrap();

    let result = parse_models(temp.path(), &["User".to_string()]).await;
    assert!(result.is_ok());
    let models = result.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].name, "User");
}

#[tokio::test]
async fn test_parse_models_not_found() {
    let result = parse_models(Path::new("/nonexistent/path"), &["User".to_string()]).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_DIR_NOT_FOUND");
}

#[tokio::test]
async fn test_parse_models_no_match() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::write(
        temp.path().join("user.rs"),
        r#"
#[derive(Model)]
pub struct User { pub id: i32 }
"#,
    )
    .await
    .unwrap();

    let result = parse_models(temp.path(), &["NonExistent".to_string()]).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[tokio::test]
async fn test_validate_templates_valid() {
    let temp = tempfile::tempdir().unwrap();
    let tmpl_dir = temp.path().join("templates");
    tokio::fs::create_dir_all(&tmpl_dir).await.unwrap();
    // 使用不引用变量的模板（validate_templates 用空上下文渲染）
    tokio::fs::write(tmpl_dir.join("valid.tera"), "Hello world")
        .await
        .unwrap();

    let result = validate_templates(&tmpl_dir).await;
    assert!(result.is_ok());
    let defs = result.unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "valid.tera");
    assert!(defs[0].is_custom);
}

#[tokio::test]
async fn test_validate_templates_with_non_tera_files() {
    let temp = tempfile::tempdir().unwrap();
    let tmpl_dir = temp.path().join("templates");
    tokio::fs::create_dir_all(&tmpl_dir).await.unwrap();
    tokio::fs::write(tmpl_dir.join("valid.tera"), "Hello world")
        .await
        .unwrap();
    tokio::fs::write(tmpl_dir.join("readme.md"), "# README")
        .await
        .unwrap();

    let result = validate_templates(&tmpl_dir).await;
    assert!(result.is_ok());
    let defs = result.unwrap();
    assert_eq!(defs.len(), 1);
}

#[tokio::test]
async fn test_validate_templates_dir_not_found() {
    let result = validate_templates(Path::new("/nonexistent/templates")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_codegen_service_generate_success() {
    let temp = tempfile::tempdir().unwrap();
    let model_dir = temp.path().join("models");
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&model_dir).await.unwrap();

    tokio::fs::write(
        model_dir.join("user.rs"),
        r#"
#[derive(Model)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: String,
    pub created_at: DateTime,
}
"#,
    )
    .await
    .unwrap();

    let service = CodegenService::new();
    let config = GenerationConfig {
        models: vec!["User".to_string()],
        model_dir,
        output_dir,
        framework: Framework::Vue,
        ..Default::default()
    };
    let result = service.generate(config).await;
    assert!(result.is_ok(), "generate failed: {:?}", result.err());
    let report = result.unwrap();
    assert!(!report.generated_files.is_empty());
}

#[tokio::test]
async fn test_codegen_service_generate_react_framework() {
    let temp = tempfile::tempdir().unwrap();
    let model_dir = temp.path().join("models");
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&model_dir).await.unwrap();

    tokio::fs::write(
        model_dir.join("product.rs"),
        r#"
#[derive(Model)]
pub struct Product {
    pub id: i32,
    pub title: String,
    pub price: f64,
}
"#,
    )
    .await
    .unwrap();

    let service = CodegenService::new();
    let config = GenerationConfig {
        models: vec!["Product".to_string()],
        model_dir,
        output_dir,
        framework: Framework::React,
        ..Default::default()
    };
    let result = service.generate(config).await;
    assert!(result.is_ok());
    let report = result.unwrap();
    assert!(!report.generated_files.is_empty());
}

#[tokio::test]
async fn test_codegen_service_generate_model_not_found_in_dir() {
    let temp = tempfile::tempdir().unwrap();
    let model_dir = temp.path().join("models");
    let output_dir = temp.path().join("output");
    tokio::fs::create_dir_all(&model_dir).await.unwrap();

    tokio::fs::write(
        model_dir.join("user.rs"),
        r#"
#[derive(Model)]
pub struct User { pub id: i32, pub name: String }
"#,
    )
    .await
    .unwrap();

    let service = CodegenService::new();
    let config = GenerationConfig {
        models: vec!["NonExistentModel".to_string()],
        model_dir,
        output_dir,
        ..Default::default()
    };
    let result = service.generate(config).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_MISSING");
}

#[tokio::test]
async fn test_codegen_service_default() {
    let service = CodegenService;
    // 仅验证 default() 能正常构造
    let config = GenerationConfig::default();
    let result = service.generate(config).await;
    assert!(result.is_err());
}

// ── 辅助函数 ──

fn create_test_model() -> ModelMetadata {
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
