//! 打印 SzRSQL 与 PostgreSQL 兼容性报告

fn main() {
    let report = szrsql_pgcompat::CompatReport::run_all();
    println!("{}", report.summary());
    println!("---");
    println!("详细结果:");
    for item in report.items() {
        println!(
            "[{}] {:<40} {:<15} {}",
            item.status.as_str(),
            item.name,
            item.category,
            item.detail
        );
    }
}
