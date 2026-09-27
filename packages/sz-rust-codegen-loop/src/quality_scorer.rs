//! 质量评分器（v1.5.0 P2-1）
//!
//! 对生成代码进行多维度质量评分：可读性/复杂度/覆盖率/安全（spec 5.5.7）。
#![forbid(unsafe_code)]

use std::collections::HashSet;

/// 质量报告
#[derive(Debug, Clone, serde::Serialize)]
pub struct QualityReport {
    /// 可读性分数（0.0-1.0）
    pub readability: f32,
    /// 复杂度分数（0.0-1.0，越高越好）
    pub complexity: f32,
    /// 覆盖率分数（0.0-1.0）
    pub coverage: f32,
    /// 安全分数（0.0-1.0）
    pub security: f32,
    /// 综合分数（0.0-1.0）
    pub overall: f32,
}

impl QualityReport {
    /// 是否通过质量门禁（overall >= threshold）
    pub fn passes(&self, threshold: f32) -> bool {
        self.overall >= threshold
    }
}

/// 生成的文件
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// 文件路径
    pub path: String,
    /// 文件内容
    pub content: String,
}

/// 质量评分器
pub struct QualityScorer {
    /// 依赖白名单（安全评分用）
    dependency_whitelist: HashSet<String>,
}

impl QualityScorer {
    /// 创建评分器，指定依赖白名单
    pub fn new(whitelist: Vec<String>) -> Self {
        Self {
            dependency_whitelist: whitelist.into_iter().collect(),
        }
    }

    /// 对生成文件进行质量评分（spec 5.5.7）
    pub fn score(&self, files: &[GeneratedFile]) -> QualityReport {
        let mut total_lines = 0usize;
        let mut total_comments = 0usize;
        let mut total_complexity = 0usize;
        let mut security_issues = 0usize;
        let mut has_tests = false;

        for file in files {
            let lines: Vec<&str> = file.content.lines().collect();
            total_lines += lines.len();

            for line in &lines {
                let trimmed = line.trim();
                if trimmed.starts_with("//")
                    || trimmed.starts_with("/*")
                    || trimmed.starts_with('*')
                {
                    total_comments += 1;
                }
                if trimmed.contains("if ")
                    || trimmed.contains("match ")
                    || trimmed.contains("for ")
                    || trimmed.contains("while ")
                {
                    total_complexity += 1;
                }
                if trimmed.contains("unsafe") || trimmed.contains("unwrap()") {
                    security_issues += 1;
                }
                if trimmed.contains("#[test]") || trimmed.contains("#[tokio::test]") {
                    has_tests = true;
                }
            }

            for dep in self.find_dependencies(&file.content) {
                if !self.dependency_whitelist.contains(&dep) {
                    security_issues += 1;
                }
            }
        }

        let readability = if total_lines > 0 {
            (total_comments as f32 / total_lines as f32).min(1.0)
        } else {
            0.0
        };

        let complexity = if total_lines > 0 {
            1.0 - (total_complexity as f32 / total_lines as f32).min(1.0)
        } else {
            1.0
        };

        let coverage = if has_tests { 0.8 } else { 0.0 };

        let security = if files.is_empty() {
            1.0
        } else {
            (1.0 - (security_issues as f32 / files.len() as f32).min(1.0)).max(0.0)
        };

        let overall = (readability * 0.2 + complexity * 0.3 + coverage * 0.3 + security * 0.2)
            .clamp(0.0, 1.0);

        QualityReport {
            readability,
            complexity,
            coverage,
            security,
            overall,
        }
    }

    fn find_dependencies(&self, content: &str) -> Vec<String> {
        let mut deps = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("use ") {
                if let Some(crate_name) = rest.split("::").next() {
                    let cleaned = crate_name.trim().trim_end_matches(';');
                    if !cleaned.is_empty()
                        && !cleaned.starts_with("self")
                        && !cleaned.starts_with("super")
                        && !cleaned.starts_with("crate")
                    {
                        deps.push(cleaned.to_string());
                    }
                }
            }
        }
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_score_empty_files() {
        let scorer = QualityScorer::new(vec![]);
        let report = scorer.score(&[]);
        assert_eq!(report.readability, 0.0, "空文件可读性应为 0");
        assert_eq!(report.complexity, 1.0, "空文件复杂度分数应为 1.0");
        assert_eq!(report.security, 1.0, "空文件安全分数应为 1.0");
    }

    #[test]
    fn test_quality_score_good_code() {
        let scorer = QualityScorer::new(vec!["tokio".to_string(), "serde".to_string()]);
        let files = vec![GeneratedFile {
            path: "main.rs".to_string(),
            content: "// well documented\n// another comment\nfn main() {\n    println!(\"hello\");\n}\n#[test]\nfn test_main() {\n    assert!(true);\n}\n".to_string(),
        }];
        let report = scorer.score(&files);
        assert!(report.readability > 0.0, "应有可读性分数");
        assert!(report.coverage > 0.0, "有测试应有覆盖率分数");
    }

    #[test]
    fn test_quality_score_unsafe_detected() {
        let scorer = QualityScorer::new(vec![]);
        let files = vec![GeneratedFile {
            path: "bad.rs".to_string(),
            content: "unsafe fn bad() {}\n".to_string(),
        }];
        let report = scorer.score(&files);
        assert!(report.security < 1.0, "unsafe 应降低安全分数");
    }

    #[test]
    fn test_quality_score_non_whitelist_dep() {
        let scorer = QualityScorer::new(vec!["tokio".to_string()]);
        let files = vec![GeneratedFile {
            path: "main.rs".to_string(),
            content: "use evil_crate::something;\nfn main() {}\n".to_string(),
        }];
        let report = scorer.score(&files);
        assert!(report.security < 1.0, "非白名单依赖应降低安全分数");
    }

    #[test]
    fn test_quality_report_passes_threshold() {
        let report = QualityReport {
            readability: 0.8,
            complexity: 0.7,
            coverage: 0.6,
            security: 0.9,
            overall: 0.75,
        };
        assert!(report.passes(0.7));
        assert!(!report.passes(0.8));
    }
}
