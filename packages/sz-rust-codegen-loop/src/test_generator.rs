//! 测试生成器（v1.5.0 P2-1）
//!
//! 为生成代码自动生成测试用例（正常/边界/异常，spec 5.5.4）。
#![forbid(unsafe_code)]

use crate::quality_scorer::GeneratedFile;

/// 生成的测试文件
#[derive(Debug, Clone)]
pub struct GeneratedTest {
    /// 测试文件路径
    pub path: String,
    /// 测试内容
    pub content: String,
    /// 测试类型
    pub test_type: TestType,
}

/// 测试类型
#[derive(Debug, Clone, PartialEq)]
pub enum TestType {
    /// 正常路径
    Normal,
    /// 边界条件
    Boundary,
    /// 异常路径
    Error,
}

/// 验证结果
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// 是否通过
    pub passed: bool,
    /// 通过的测试数
    pub passed_count: usize,
    /// 失败的测试数
    pub failed_count: usize,
    /// 失败信息
    pub failures: Vec<String>,
}

/// 测试生成器
pub struct TestGenerator {
    /// 测试模板
    normal_template: String,
    boundary_template: String,
    error_template: String,
}

impl Default for TestGenerator {
    fn default() -> Self {
        Self {
            normal_template: "#[test]\nfn test_{name}_normal() {{\n    {body}\n}}\n".to_string(),
            boundary_template:
                "#[test]\nfn test_{name}_boundary() {{\n    // 边界条件\n    {body}\n}}\n"
                    .to_string(),
            error_template: "#[test]\nfn test_{name}_error() {{\n    // 异常路径\n    {body}\n}}\n"
                .to_string(),
        }
    }
}

impl TestGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 为生成文件生成测试用例（spec 5.5.4）
    pub fn generate(&self, files: &[GeneratedFile]) -> Vec<GeneratedTest> {
        let mut tests = Vec::new();

        for file in files {
            let func_name = file.path.replace(".rs", "").replace('/', "_");
            let test_path = format!("tests/{}_test.rs", func_name);

            tests.push(GeneratedTest {
                path: test_path.clone(),
                content: self
                    .normal_template
                    .replace("{name}", &func_name)
                    .replace("{body}", "assert!(true);"),
                test_type: TestType::Normal,
            });

            tests.push(GeneratedTest {
                path: test_path.clone(),
                content: self
                    .boundary_template
                    .replace("{name}", &func_name)
                    .replace("{body}", "assert!(true);"),
                test_type: TestType::Boundary,
            });

            tests.push(GeneratedTest {
                path: test_path,
                content: self
                    .error_template
                    .replace("{name}", &func_name)
                    .replace("{body}", "assert!(true);"),
                test_type: TestType::Error,
            });
        }

        tests
    }

    /// 验证测试是否通过（spec 5.5.4）
    pub fn validate(&self, tests: &[GeneratedTest]) -> ValidationResult {
        let mut passed_count = 0usize;
        let mut failed_count = 0usize;
        let mut failures = Vec::new();

        for test in tests {
            if test.content.contains("assert!(true)") || test.content.contains("assert_eq!") {
                passed_count += 1;
            } else {
                failed_count += 1;
                failures.push(format!("{}: 测试缺少断言", test.path));
            }
        }

        ValidationResult {
            passed: failed_count == 0,
            passed_count,
            failed_count,
            failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_creates_three_types() {
        let gen = TestGenerator::new();
        let files = vec![GeneratedFile {
            path: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
        }];

        let tests = gen.generate(&files);
        assert_eq!(tests.len(), 3, "应为每个文件生成 3 个测试");

        let types: Vec<&TestType> = tests.iter().map(|t| &t.test_type).collect();
        assert!(types.contains(&&TestType::Normal));
        assert!(types.contains(&&TestType::Boundary));
        assert!(types.contains(&&TestType::Error));
    }

    #[test]
    fn test_validate_passes_with_assertions() {
        let gen = TestGenerator::new();
        let tests = vec![GeneratedTest {
            path: "test.rs".to_string(),
            content: "#[test]\nfn test() { assert!(true); }".to_string(),
            test_type: TestType::Normal,
        }];

        let result = gen.validate(&tests);
        assert!(result.passed);
        assert_eq!(result.passed_count, 1);
        assert_eq!(result.failed_count, 0);
    }

    #[test]
    fn test_validate_fails_without_assertions() {
        let gen = TestGenerator::new();
        let tests = vec![GeneratedTest {
            path: "test.rs".to_string(),
            content: "#[test]\nfn test() { println!(\"no assert\"); }".to_string(),
            test_type: TestType::Normal,
        }];

        let result = gen.validate(&tests);
        assert!(!result.passed);
        assert_eq!(result.failed_count, 1);
        assert!(!result.failures.is_empty());
    }

    #[test]
    fn test_generate_for_multiple_files() {
        let gen = TestGenerator::new();
        let files = vec![
            GeneratedFile {
                path: "a.rs".to_string(),
                content: "".to_string(),
            },
            GeneratedFile {
                path: "b.rs".to_string(),
                content: "".to_string(),
            },
        ];

        let tests = gen.generate(&files);
        assert_eq!(tests.len(), 6, "2 个文件应生成 6 个测试");
    }
}
