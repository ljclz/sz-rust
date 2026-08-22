//! 单元测试：类型映射、敏感字段、UI 适配器、路由映射、过滤器、路径守卫、配置、错误、报告

use sz_rust_frontend_codegen::generators::route::{PageType, RoutePageMapper};
use sz_rust_frontend_codegen::metadata::{
    FieldMetadata, ModelMetadata, RelationKind, RelationMetadata, ValidationRule,
    ValidationRuleType,
};
use sz_rust_frontend_codegen::model_parser::{
    is_auto_timestamp, is_sensitive_field, rust_to_ts_type, ModelParser,
};
use sz_rust_frontend_codegen::ui_adapter::UiAdapter;
use sz_rust_frontend_codegen::{
    Framework, FrontendCodegenError, GenerationConfig, GenerationReport, OverrideStrategy,
    UiLibrary,
};

// ── 类型映射测试 ──

#[test]
fn test_rust_to_ts_type_string() {
    assert_eq!(rust_to_ts_type("String"), "string");
}

#[test]
fn test_rust_to_ts_type_integers() {
    assert_eq!(rust_to_ts_type("i32"), "number");
    assert_eq!(rust_to_ts_type("i64"), "number");
    assert_eq!(rust_to_ts_type("u32"), "number");
    assert_eq!(rust_to_ts_type("u64"), "number");
}

#[test]
fn test_rust_to_ts_type_float() {
    assert_eq!(rust_to_ts_type("f64"), "number");
}

#[test]
fn test_rust_to_ts_type_bool() {
    assert_eq!(rust_to_ts_type("bool"), "boolean");
}

#[test]
fn test_rust_to_ts_type_option() {
    assert_eq!(rust_to_ts_type("Option < String >"), "string | null");
}

#[test]
fn test_rust_to_ts_type_vec() {
    assert_eq!(rust_to_ts_type("Vec < Order >"), "Order[]");
}

#[test]
fn test_rust_to_ts_type_unknown() {
    assert_eq!(rust_to_ts_type("SomeUnknownType"), "any");
}

#[test]
fn test_rust_to_ts_type_datetime() {
    assert_eq!(rust_to_ts_type("DateTime"), "string");
}

// ── 敏感字段测试 ──

#[test]
fn test_is_sensitive_field_password() {
    assert!(is_sensitive_field("password"));
}

#[test]
fn test_is_sensitive_field_secret() {
    assert!(is_sensitive_field("secret"));
}

#[test]
fn test_is_sensitive_field_token() {
    assert!(is_sensitive_field("token"));
}

#[test]
fn test_is_sensitive_field_api_key() {
    assert!(is_sensitive_field("api_key"));
}

#[test]
fn test_is_sensitive_field_private_key() {
    assert!(is_sensitive_field("private_key"));
}

#[test]
fn test_is_sensitive_field_normal() {
    assert!(!is_sensitive_field("name"));
    assert!(!is_sensitive_field("email"));
}

// ── 自动时间戳测试 ──

#[test]
fn test_is_auto_timestamp_created() {
    assert!(is_auto_timestamp("created_at"));
}

#[test]
fn test_is_auto_timestamp_updated() {
    assert!(is_auto_timestamp("updated_at"));
}

#[test]
fn test_is_auto_timestamp_deleted() {
    assert!(is_auto_timestamp("deleted_at"));
}

#[test]
fn test_is_auto_timestamp_normal() {
    assert!(!is_auto_timestamp("name"));
}

// ── UI 适配器测试 ──

#[test]
fn test_ui_adapter_element_plus_table() {
    let model = create_test_model();
    let adapted = UiAdapter::adapt(&model, UiLibrary::ElementPlus);
    assert_eq!(adapted.tags.table, "el-table");
}

#[test]
fn test_ui_adapter_element_plus_form() {
    let model = create_test_model();
    let adapted = UiAdapter::adapt(&model, UiLibrary::ElementPlus);
    assert_eq!(adapted.tags.form, "el-form");
}

#[test]
fn test_ui_adapter_ant_design_table() {
    let model = create_test_model();
    let adapted = UiAdapter::adapt(&model, UiLibrary::AntDesignVue);
    assert_eq!(adapted.tags.table, "a-table");
}

#[test]
fn test_ui_adapter_ant_design_form() {
    let model = create_test_model();
    let adapted = UiAdapter::adapt(&model, UiLibrary::AntDesignVue);
    assert_eq!(adapted.tags.form, "a-form");
}

// ── 路由页面映射测试 ──

#[test]
fn test_route_mapper_get_list() {
    assert_eq!(RoutePageMapper::map("GET", false), Some(PageType::List));
}

#[test]
fn test_route_mapper_get_show() {
    assert_eq!(RoutePageMapper::map("GET", true), Some(PageType::Show));
}

#[test]
fn test_route_mapper_post_create() {
    assert_eq!(RoutePageMapper::map("POST", false), Some(PageType::Create));
}

#[test]
fn test_route_mapper_put_edit() {
    assert_eq!(RoutePageMapper::map("PUT", true), Some(PageType::Edit));
}

#[test]
fn test_route_mapper_patch_edit() {
    assert_eq!(RoutePageMapper::map("PATCH", true), Some(PageType::Edit));
}

#[test]
fn test_route_mapper_delete_none() {
    assert_eq!(RoutePageMapper::map("DELETE", true), None);
}

#[test]
fn test_route_mapper_options_none() {
    assert_eq!(RoutePageMapper::map("OPTIONS", false), None);
}

// ── 配置测试 ──

#[test]
fn test_config_default_framework() {
    let config = GenerationConfig::default();
    assert_eq!(config.framework, Framework::Vue);
}

#[test]
fn test_config_default_ui_library() {
    let config = GenerationConfig::default();
    assert_eq!(config.ui_library, UiLibrary::ElementPlus);
}

#[test]
fn test_config_default_override_strategy() {
    let config = GenerationConfig::default();
    assert_eq!(config.override_strategy, OverrideStrategy::Skip);
}

#[test]
fn test_config_default_lazy_load() {
    let config = GenerationConfig::default();
    assert!(config.lazy_load);
}

// ── 错误码测试 ──

#[test]
fn test_error_code_model_dir_not_found() {
    let err = FrontendCodegenError::ModelDirNotFound("/test".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_DIR_NOT_FOUND");
}

#[test]
fn test_error_code_missing_model() {
    let err = FrontendCodegenError::MissingModel;
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_MISSING");
}

#[test]
fn test_error_code_template_path_traversal() {
    let err = FrontendCodegenError::TemplatePathTraversal("../etc".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL");
}

#[test]
fn test_error_code_unsupported_ui_library() {
    let err = FrontendCodegenError::UnsupportedUiLibrary("unknown".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_UI_LIBRARY_UNSUPPORTED");
}

#[test]
fn test_error_code_framework_conflict() {
    let err = FrontendCodegenError::FrameworkConflict("vue+react".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_FRAMEWORK_CONFLICT");
}

#[test]
fn test_error_code_config_parse_error() {
    let err = FrontendCodegenError::ConfigParseError("bad".to_string());
    assert_eq!(err.error_code(), "FE_CODEGEN_CONFIG_PARSE_ERROR");
}

#[test]
fn test_error_from_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
    let err: FrontendCodegenError = io_err.into();
    assert_eq!(err.error_code(), "FE_CODEGEN_IO_ERROR");
}

// ── 报告测试 ──

#[test]
fn test_report_new_has_trace_id() {
    let report = GenerationReport::new();
    assert!(!report.trace_id.is_empty());
    assert!(report.generated_files.is_empty());
    assert!(report.skipped_files.is_empty());
}

#[test]
fn test_report_format_cli_contains_trace_id() {
    let report = GenerationReport::new();
    let output = report.format_cli();
    assert!(output.contains(&report.trace_id));
    assert!(output.contains("前端代码生成报告"));
}

#[test]
fn test_report_format_cli_contains_summary() {
    let report = GenerationReport::new();
    let output = report.format_cli();
    assert!(output.contains("总计"));
}

// ── ModelMetadata 测试 ──

#[test]
fn test_model_metadata_primary_key() {
    let model = create_test_model();
    let pk = model.primary_key();
    assert!(pk.is_some());
    assert_eq!(pk.unwrap().name, "id");
}

#[test]
fn test_model_metadata_writable_fields() {
    let model = create_test_model();
    let writable = model.writable_fields();
    // 排除主键 id 和自动时间戳 created_at
    assert_eq!(writable.len(), 2);
    let names: Vec<&str> = writable.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"name"));
    assert!(names.contains(&"email"));
}

// ── FieldMetadata Default 测试 ──

#[test]
fn test_field_metadata_default() {
    let field = FieldMetadata::default();
    assert!(!field.is_nullable);
    assert!(!field.is_primary_key);
    assert!(!field.is_indexed);
    assert!(!field.is_sensitive);
    assert!(!field.is_auto_timestamp);
    assert!(field.validation_rules.is_empty());
    assert!(field.relation.is_none());
}

// ── RelationMetadata 序列化测试 ──

#[test]
fn test_relation_kind_serialize_snake_case() {
    let kind = RelationKind::HasMany;
    let json = serde_json::to_string(&kind).unwrap();
    assert_eq!(json, "\"has_many\"");
}

#[test]
fn test_relation_metadata_serialize() {
    let rel = RelationMetadata {
        kind: RelationKind::BelongsTo,
        target_model: "User".to_string(),
        foreign_key: Some("user_id".to_string()),
        through_table: None,
    };
    let json = serde_json::to_string(&rel).unwrap();
    assert!(json.contains("belongs_to"));
    assert!(json.contains("User"));
}

// ── ValidationRule 序列化测试 ──

#[test]
fn test_validation_rule_type_serialize_snake_case() {
    let rule = ValidationRuleType::MaxLength;
    let json = serde_json::to_string(&rule).unwrap();
    assert_eq!(json, "\"max_length\"");
}

#[test]
fn test_validation_rule_serialize() {
    let rule = ValidationRule {
        rule_type: ValidationRuleType::Email,
        param: None,
        message: Some("邮箱格式错误".to_string()),
    };
    let json = serde_json::to_string(&rule).unwrap();
    assert!(json.contains("email"));
}

// ── ModelParser 目录不存在测试 ──

#[tokio::test]
async fn test_model_parser_dir_not_found() {
    let result = ModelParser::parse_dir(std::path::Path::new(
        "/nonexistent/path/that/does/not/exist",
    ))
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "FE_CODEGEN_MODEL_DIR_NOT_FOUND");
}

#[tokio::test]
async fn test_model_parser_empty_dir() {
    let temp = tempfile::tempdir().unwrap();
    let result = ModelParser::parse_dir(temp.path()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
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
            FieldMetadata {
                name: "created_at".to_string(),
                rust_type: "DateTime".to_string(),
                ts_type: "string".to_string(),
                sql_type: "TIMESTAMP".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_indexed: false,
                is_sensitive: false,
                is_auto_timestamp: true,
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
