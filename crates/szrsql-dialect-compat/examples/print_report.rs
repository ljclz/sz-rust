//! 打印 SzRSQL 多方言兼容性报告
//!
//! 运行：`cargo run -p szrsql-dialect-compat --example print_report`

use std::fs::File;
use std::io::{BufWriter, Write};

fn main() {
    println!("正在运行 SzRSQL 多方言兼容性 + 对抗性边界测试...\n");

    let report = szrsql_dialect_compat::DialectCompatReport::run_all();

    // 构建完整报告文本
    let mut output = String::new();
    output.push_str("正在运行 SzRSQL 多方言兼容性 + 对抗性边界测试...\n\n");

    // 1. 摘要
    output.push_str(&format!("{}\n", report.summary()));
    output.push_str("---\n");

    // 2. 各方言详细结果（按方言分组）
    append_section(
        &mut output,
        "MySQL 兼容性",
        &format_dialect_results(&report.mysql, |r| {
            (
                r.name.clone(),
                r.category_label(),
                r.sql.clone(),
                r.status,
                r.detail.clone(),
            )
        }),
    );
    append_section(
        &mut output,
        "Oracle 兼容性",
        &format_dialect_results(&report.oracle, |r| {
            (
                r.name.clone(),
                r.category_label(),
                r.sql.clone(),
                r.status,
                r.detail.clone(),
            )
        }),
    );
    append_section(
        &mut output,
        "SQL Server 兼容性",
        &format_dialect_results(&report.sqlserver, |r| {
            (
                r.name.clone(),
                r.category_label(),
                r.sql.clone(),
                r.status,
                r.detail.clone(),
            )
        }),
    );
    append_section(
        &mut output,
        "SQLite 兼容性",
        &format_dialect_results(&report.sqlite, |r| {
            (
                r.name.clone(),
                r.category_label(),
                r.sql.clone(),
                r.status,
                r.detail.clone(),
            )
        }),
    );
    append_section(
        &mut output,
        "对抗性边界测试",
        &format_dialect_results(&report.adversarial, |r| {
            (
                r.name.clone(),
                r.category_label(),
                r.sql.clone(),
                r.status,
                r.detail.clone(),
            )
        }),
    );

    // 3. JSON 报告
    output.push_str("\n========== JSON 报告 ==========\n");
    match report.to_json() {
        Ok(json) => output.push_str(&json),
        Err(e) => output.push_str(&format!("JSON 序列化失败: {e}")),
    }

    // 打印到 stdout
    print!("{output}");

    // 保存到文件
    let report_path = "多方言兼容性测试报告.txt";
    match File::create(report_path) {
        Ok(file) => {
            let mut writer = BufWriter::new(file);
            if let Err(e) = writer.write_all(output.as_bytes()) {
                eprintln!("写入报告文件失败: {e}");
            } else {
                println!("\n报告已保存到: {report_path}");
            }
        }
        Err(e) => eprintln!("创建报告文件失败: {e}"),
    }
}

trait CategoryLabel {
    fn category_label(&self) -> String;
}

impl CategoryLabel for szrsql_dialect_compat::MysqlCompatResult {
    fn category_label(&self) -> String {
        format!("{:?}", self.category)
    }
}
impl CategoryLabel for szrsql_dialect_compat::OracleCompatResult {
    fn category_label(&self) -> String {
        format!("{:?}", self.category)
    }
}
impl CategoryLabel for szrsql_dialect_compat::SqlserverCompatResult {
    fn category_label(&self) -> String {
        format!("{:?}", self.category)
    }
}
impl CategoryLabel for szrsql_dialect_compat::SqliteCompatResult {
    fn category_label(&self) -> String {
        format!("{:?}", self.category)
    }
}
impl CategoryLabel for szrsql_dialect_compat::AdversarialTestResult {
    fn category_label(&self) -> String {
        format!("{:?}", self.category)
    }
}

fn format_dialect_results<T, F>(
    results: &[T],
    mapper: F,
) -> Vec<(
    String,
    String,
    String,
    szrsql_dialect_compat::CompatStatus,
    String,
)>
where
    F: Fn(
        &T,
    ) -> (
        String,
        String,
        String,
        szrsql_dialect_compat::CompatStatus,
        String,
    ),
{
    results.iter().map(mapper).collect()
}

fn print_section(
    title: &str,
    items: &[(
        String,
        String,
        String,
        szrsql_dialect_compat::CompatStatus,
        String,
    )],
) {
    println!("\n========== {title} ==========");
    let total = items.len();
    let pass = items.iter().filter(|i| i.3.is_passed()).count();
    let full = items
        .iter()
        .filter(|i| i.3 == szrsql_dialect_compat::CompatStatus::Pass)
        .count();
    println!("小计: {pass}/{total} 通过 (含部分), {full}/{total} 完全通过\n");
    println!("{:<5} {:<40} {:<15} {}", "状态", "名称", "分类", "说明");
    for (name, category, _sql, status, detail) in items {
        println!(
            "[{:<4}] {:<40} {:<15} {}",
            status.as_str(),
            name,
            category,
            detail
        );
    }
}

/// 将一个方言小节追加到输出缓冲区
fn append_section(
    output: &mut String,
    title: &str,
    items: &[(
        String,
        String,
        String,
        szrsql_dialect_compat::CompatStatus,
        String,
    )],
) {
    output.push_str(&format!("\n========== {title} ==========\n"));
    let total = items.len();
    let pass = items.iter().filter(|i| i.3.is_passed()).count();
    let full = items
        .iter()
        .filter(|i| i.3 == szrsql_dialect_compat::CompatStatus::Pass)
        .count();
    output.push_str(&format!(
        "小计: {pass}/{total} 通过 (含部分), {full}/{total} 完全通过\n\n"
    ));
    output.push_str(&format!(
        "{:<5} {:<40} {:<15} {}\n",
        "状态", "名称", "分类", "说明"
    ));
    for (name, category, _sql, status, detail) in items {
        output.push_str(&format!(
            "[{:<4}] {:<40} {:<15} {}\n",
            status.as_str(),
            name,
            category,
            detail
        ));
    }
}
