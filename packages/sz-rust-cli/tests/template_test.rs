use sz_rust_cli::safety_validator::SafetyValidator;
use sz_rust_cli::template_engine::TemplateEngine;

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates")
}

#[tokio::test]
async fn test_all_4_templates_discoverable() {
    let engine = TemplateEngine::init(&template_dir())
        .await
        .expect("模板引擎初始化失败");
    let templates = engine.list_templates();
    assert!(
        templates.contains(&"plugin-crud".to_string()),
        "缺少 plugin-crud"
    );
    assert!(
        templates.contains(&"plugin-master-slave".to_string()),
        "缺少 plugin-master-slave"
    );
    assert!(
        templates.contains(&"plugin-workflow".to_string()),
        "缺少 plugin-workflow"
    );
    assert!(
        templates.contains(&"plugin-report".to_string()),
        "缺少 plugin-report"
    );
}

#[tokio::test]
async fn test_crud_template_files_exist() {
    let dir = template_dir().join("plugin-crud");
    let expected = [
        "model.rs.tera",
        "controller.rs.tera",
        "service.rs.tera",
        "repository.rs.tera",
        "migration.sql.tera",
        "routes.rs.tera",
        "manifest.json.tera",
        "tests.rs.tera",
    ];
    for f in &expected {
        assert!(dir.join(f).exists(), "缺少文件: plugin-crud/{f}");
    }
}

#[tokio::test]
async fn test_workflow_template_files_exist() {
    let dir = template_dir().join("plugin-workflow");
    let expected = [
        "model.rs.tera",
        "controller.rs.tera",
        "routes.rs.tera",
        "migration.sql.tera",
        "manifest.json.tera",
        "tests.rs.tera",
    ];
    for f in &expected {
        assert!(dir.join(f).exists(), "缺少文件: plugin-workflow/{f}");
    }
}

#[tokio::test]
async fn test_report_template_files_exist() {
    let dir = template_dir().join("plugin-report");
    let expected = [
        "model.rs.tera",
        "controller.rs.tera",
        "routes.rs.tera",
        "migration.sql.tera",
        "manifest.json.tera",
        "tests.rs.tera",
    ];
    for f in &expected {
        assert!(dir.join(f).exists(), "缺少文件: plugin-report/{f}");
    }
}

#[test]
fn test_safety_validator_clean_code() {
    let files = vec![
        (
            "src/model.rs".to_string(),
            "pub struct Foo { pub x: i32 }\n".to_string(),
        ),
        (
            "src/controller.rs".to_string(),
            "impl Foo { pub fn bar(&self) -> i32 { self.x } }\n".to_string(),
        ),
    ];
    let violations = SafetyValidator::validate_files(&files);
    assert!(violations.is_empty(), "干净代码不应有违规");
}

#[test]
fn test_safety_validator_detects_unsafe() {
    let files = vec![(
        "src/foo.rs".to_string(),
        "fn bar() { unsafe { } }\n".to_string(),
    )];
    let violations = SafetyValidator::validate_files(&files);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].rule.contains("铁律3"));
}

#[test]
fn test_safety_validator_detects_unwrap() {
    let files = vec![(
        "src/foo.rs".to_string(),
        "let x = opt.unwrap();\n".to_string(),
    )];
    let violations = SafetyValidator::validate_files(&files);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].rule.contains("铁律2"));
}

#[test]
fn test_safety_validator_detects_std_fs() {
    let files = vec![(
        "src/foo.rs".to_string(),
        "let f = std::fs::read_to_string(\"x\")?;\n".to_string(),
    )];
    let violations = SafetyValidator::validate_files(&files);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].rule.contains("铁律4"));
}

#[test]
fn test_safety_validator_detects_select_star() {
    let files = vec![(
        "migrations/table.sql".to_string(),
        "SELECT * FROM users;\n".to_string(),
    )];
    let violations = SafetyValidator::validate_files(&files);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].rule.contains("铁律8"));
}

#[test]
fn test_safety_validator_report_format() {
    let violations = vec![];
    let report = SafetyValidator::format_report(&violations);
    assert!(report.contains("0 个违规项"));
}
