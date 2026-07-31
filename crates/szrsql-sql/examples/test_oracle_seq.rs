//! 临时调试：验证 Oracle ALTER TABLE MODIFY 预处理

use szrsql_sql::dialect::{parse_with_dialect, Dialect};

fn main() {
    let tests = [
        ("Oracle MODIFY 括号", "ALTER TABLE users MODIFY (name VARCHAR2(200) NOT NULL)"),
        ("Oracle MODIFY 无括号", "ALTER TABLE users MODIFY name VARCHAR2(200)"),
        ("PG ALTER TYPE", "ALTER TABLE users ALTER COLUMN name TYPE VARCHAR(200)"),
        ("PG ALTER TYPE + SET NOT NULL", "ALTER TABLE users ALTER COLUMN name TYPE VARCHAR(200); ALTER TABLE users ALTER COLUMN name SET NOT NULL"),
    ];
    for (label, sql) in tests {
        let r = parse_with_dialect(sql, &Dialect::Oracle);
        println!("{label}: {sql}");
        println!("  => {r:?}\n");
    }
}
