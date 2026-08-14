//! 生成报告

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 生成报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationReport {
    /// 追踪 ID（UUID v4）
    pub trace_id: String,
    /// 已生成文件
    pub generated_files: Vec<GeneratedFile>,
    /// 跳过文件
    pub skipped_files: Vec<SkippedFile>,
    /// 警告
    pub warnings: Vec<Warning>,
    /// 失败
    pub failures: Vec<Failure>,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 开始时间
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// 完成时间
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

/// 已生成文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// 文件路径
    pub path: PathBuf,
    /// 文件大小（字节）
    pub size_bytes: u64,
    /// 来源模型
    pub source_model: String,
    /// 来源模板
    pub source_template: String,
    /// 是否覆盖了已存在文件
    pub is_overwritten: bool,
}

/// 跳过文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedFile {
    /// 文件路径
    pub path: PathBuf,
    /// 跳过原因
    pub reason: String,
}

/// 警告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    /// 警告码
    pub code: String,
    /// 警告消息
    pub message: String,
    /// 相关文件
    pub related_file: Option<PathBuf>,
}

/// 失败
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    /// 失败码
    pub code: String,
    /// 失败消息
    pub message: String,
    /// 来源模型
    pub source_model: Option<String>,
    /// 来源模板
    pub source_template: Option<String>,
}

impl GenerationReport {
    /// 创建新报告（自动生成 trace_id）
    pub fn new() -> Self {
        let now = chrono::Utc::now();
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            generated_files: Vec::new(),
            skipped_files: Vec::new(),
            warnings: Vec::new(),
            failures: Vec::new(),
            duration_ms: 0,
            started_at: now,
            finished_at: now,
        }
    }

    /// CLI 表格格式化输出
    pub fn format_cli(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("[trace_id: {}] 前端代码生成报告\n", self.trace_id));
        out.push_str(&format!(
            "开始: {}  完成: {}  耗时: {}ms\n\n",
            self.started_at, self.finished_at, self.duration_ms
        ));

        if !self.generated_files.is_empty() {
            out.push_str("✓ 生成文件:\n");
            for f in &self.generated_files {
                let flag = if f.is_overwritten { " (覆盖)" } else { "" };
                out.push_str(&format!(
                    "   ✓ {} ({}B){} ← {} / {}\n",
                    f.path.display(),
                    f.size_bytes,
                    flag,
                    f.source_model,
                    f.source_template
                ));
            }
            out.push('\n');
        }

        if !self.skipped_files.is_empty() {
            out.push_str("⊘ 跳过文件:\n");
            for f in &self.skipped_files {
                out.push_str(&format!("   ⊘ {} — {}\n", f.path.display(), f.reason));
            }
            out.push('\n');
        }

        if !self.warnings.is_empty() {
            out.push_str("⚠ 警告:\n");
            for w in &self.warnings {
                out.push_str(&format!("   ⚠ [{}] {}\n", w.code, w.message));
            }
            out.push('\n');
        }

        if !self.failures.is_empty() {
            out.push_str("✗ 失败:\n");
            for f in &self.failures {
                out.push_str(&format!("   ✗ [{}] {}\n", f.code, f.message));
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "总计: 生成 {}，跳过 {}，警告 {}，失败 {}，耗时 {}s",
            self.generated_files.len(),
            self.skipped_files.len(),
            self.warnings.len(),
            self.failures.len(),
            self.duration_ms / 1000
        ));
        out
    }
}

impl Default for GenerationReport {
    fn default() -> Self {
        Self::new()
    }
}
