//! 只打印各方言兼容性（不含 adversarial），快速查看兼容度

fn main() {
    println!("=== MySQL 兼容性 ===");
    let mysql = szrsql_dialect_compat::MysqlCompat::run_all();
    print_dialect("MySQL", &mysql);

    println!("\n=== Oracle 兼容性 ===");
    let oracle = szrsql_dialect_compat::OracleCompat::run_all();
    print_dialect("Oracle", &oracle);

    println!("\n=== SQL Server 兼容性 ===");
    let sqlserver = szrsql_dialect_compat::SqlserverCompat::run_all();
    print_dialect("SQL Server", &sqlserver);

    println!("\n=== SQLite 兼容性 ===");
    let sqlite = szrsql_dialect_compat::SqliteCompat::run_all();
    print_dialect("SQLite", &sqlite);
}

fn print_dialect<T: DialectRow>(name: &str, items: &[T]) {
    let total = items.len();
    let pass = items.iter().filter(|r| r.status().is_passed()).count();
    let full = items
        .iter()
        .filter(|r| r.status() == szrsql_dialect_compat::CompatStatus::Pass)
        .count();
    let rate = if total == 0 {
        0.0
    } else {
        (pass as f64 / total as f64) * 100.0
    };
    let full_rate = if total == 0 {
        0.0
    } else {
        (full as f64 / total as f64) * 100.0
    };
    println!("{name}: {pass}/{total} 通过 ({rate:.1}%), {full}/{total} 完全通过 ({full_rate:.1}%)");

    // 只打印失败项
    let failures: Vec<&T> = items.iter().filter(|r| !r.status().is_passed()).collect();
    if !failures.is_empty() {
        println!("失败项 ({}):", failures.len());
        for f in failures {
            println!("  [{}] {} | {}", f.status().as_str(), f.name(), f.detail());
        }
    }
}

trait DialectRow {
    fn status(&self) -> szrsql_dialect_compat::CompatStatus;
    fn name(&self) -> &str;
    fn detail(&self) -> &str;
}

impl DialectRow for szrsql_dialect_compat::MysqlCompatResult {
    fn status(&self) -> szrsql_dialect_compat::CompatStatus {
        self.status
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn detail(&self) -> &str {
        &self.detail
    }
}
impl DialectRow for szrsql_dialect_compat::OracleCompatResult {
    fn status(&self) -> szrsql_dialect_compat::CompatStatus {
        self.status
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn detail(&self) -> &str {
        &self.detail
    }
}
impl DialectRow for szrsql_dialect_compat::SqlserverCompatResult {
    fn status(&self) -> szrsql_dialect_compat::CompatStatus {
        self.status
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn detail(&self) -> &str {
        &self.detail
    }
}
impl DialectRow for szrsql_dialect_compat::SqliteCompatResult {
    fn status(&self) -> szrsql_dialect_compat::CompatStatus {
        self.status
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn detail(&self) -> &str {
        &self.detail
    }
}
