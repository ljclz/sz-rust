//! Phase 3.17 SQL 正确性 Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 3.17。
//!
//! # 验收标准（SzRSQL实施进度.md Phase 3.17）
//!
//! - **Fuzz**：随机生成 SQL（合法 + 非法混合 1000000 条），合法 SQL 必须执行正确，
//!   非法 SQL 必须报合理错误不 panic
//! - **判定**：0 panic, 0 误执行非法 SQL
//!
//! # 设计要点
//!
//! 1. **XorShift64 PRNG**：固定种子，测试可重现（与 `auth_fuzz` / `mvcc_fuzz` 同风格）
//! 2. **SQL 模板生成器**：10 个合法模板 + 12 个非法模板，随机填充参数
//! 3. **合法 SQL**：parse + plan + execute 全部成功，结果与参考模型一致
//! 4. **非法 SQL**：parse 或 plan 返回 Err，不 panic，不误执行
//! 5. **参考模型**：DML 序列一致性测试，维护独立状态投影（HashMap<i64, String>）
//! 6. **100 万条 SQL**：1M 混合（500K 合法 + 500K 非法），仅 parse + plan，验证 0 panic
//! 7. **10 万条 SQL**：100K 合法/非法分类验证，parse + plan 全部成功/失败
//! 8. **执行正确性**：10K 合法 SQL 全链路执行，验证结果与预期一致

use szrsql_sql::executor::{Executor, InMemoryTable, TableStorage};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, Planner};
use szrsql_types::value::{ColumnType, Value};

use std::collections::{HashMap, HashSet};

// =====================================================================
//  XorShift64 — 固定种子 PRNG（与 auth_fuzz.rs / mvcc_fuzz.rs 同风格）
// =====================================================================

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// [0, n) 范围
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// 50% 概率返回 true
    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// [-500, 500) 范围的 i64
    fn next_i64_small(&mut self) -> i64 {
        (self.next_range(1000) as i64) - 500
    }
}

// =====================================================================
//  SQL 模板生成器
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqlKind {
    Valid,
    Invalid,
}

const NAMES: &[&str] = &[
    "alice", "bob", "carol", "dave", "eve", "frank", "grace", "henry",
];

fn random_name(rng: &mut XorShift64) -> &'static str {
    NAMES[rng.next_range(NAMES.len() as u32) as usize]
}

/// 生成合法 SQL（应 parse + plan + execute 成功）
///
/// 模板覆盖：
/// - SELECT（全表扫描、列投影、WHERE 等值/范围、AND 组合、LIMIT、COUNT 聚合）
/// - INSERT（VALUES 单行）
/// - UPDATE（SET + WHERE）
/// - DELETE（WHERE）
fn gen_valid_sql(rng: &mut XorShift64) -> String {
    let n = rng.next_i64_small();
    let m = rng.next_i64_small();
    let name = random_name(rng);
    let limit = (rng.next_range(100) + 1) as i64; // [1, 100]
    match rng.next_range(10) {
        0 => "SELECT * FROM t".to_string(),
        1 => "SELECT id, name FROM t".to_string(),
        2 => format!("SELECT id FROM t WHERE id = {n}"),
        3 => format!("SELECT id FROM t WHERE id > {n}"),
        4 => format!("SELECT id FROM t LIMIT {limit}"),
        5 => "SELECT COUNT(*) FROM t".to_string(),
        6 => format!("INSERT INTO t (id, name) VALUES ({n}, '{name}')"),
        7 => format!("UPDATE t SET name = '{name}' WHERE id = {n}"),
        8 => format!("DELETE FROM t WHERE id = {n}"),
        9 => format!("SELECT id FROM t WHERE id > {n} AND id < {m}"),
        _ => unreachable!(),
    }
}

/// 生成非法 SQL（应 parse 或 plan 返回 Err）
///
/// 模板覆盖：
/// - 语法错误（SELECT FROM、SELECT * FROM、INSERT INTO t VALUES 等）
/// - 解析错误（INVALID SQL STATEMENT、SELECT *, FROM t 等）
/// - 计划错误（表不存在）
///
/// 注意：`SELECT bad_col_xyz FROM t` 不在此列 — 该 SQL 语法合法、plan 阶段不校验列名，
/// 仅在 execute 阶段报 ColumnNotFound。该用例由 `test_fuzz_execute_error_column_not_found` 覆盖。
fn gen_invalid_sql(rng: &mut XorShift64) -> String {
    match rng.next_range(12) {
        0 => "SELECT FROM".to_string(),
        1 => "SELECT * FROM".to_string(),
        2 => "INSERT INTO t VALUES".to_string(),
        3 => "SELECT * FROM nonexistent_table_xyz".to_string(),
        4 => "SELECT * FROM t WHERE id = 1 AND".to_string(),
        5 => "INVALID SQL STATEMENT".to_string(),
        6 => "SELECT * FROM t WHERE".to_string(),
        7 => "UPDATE t SET".to_string(),
        8 => "DELETE FROM".to_string(),
        9 => "SELECT *, FROM t".to_string(),
        10 => "SELECT id, FROM t".to_string(),
        11 => "CREATE TABLE".to_string(),
        _ => unreachable!(),
    }
}

fn generate_sql(rng: &mut XorShift64) -> (SqlKind, String) {
    if rng.next_bool() {
        (SqlKind::Valid, gen_valid_sql(rng))
    } else {
        (SqlKind::Invalid, gen_invalid_sql(rng))
    }
}

// =====================================================================
//  测试辅助函数
// =====================================================================

/// 构造测试用 catalog：单表 t(id BIGINT, name TEXT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog
}

/// 构造填充数据的测试表：n 行，id = 0..n，name 循环取自 NAMES
fn make_table_with_rows(n: usize) -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    for i in 0..n {
        let name = NAMES[i % NAMES.len()];
        table.insert(vec![Value::Int64(i as i64), Value::Text(name.into())]);
    }
    table
}

/// parse + plan，返回 Ok(()) 或 Err(reason)
fn parse_plan(sql: &str, catalog: &dyn szrsql_sql::plan::Catalog) -> Result<(), String> {
    let stmts = parse_sql(sql).map_err(|e| format!("parse: {e}"))?;
    let planner = Planner::new(catalog);
    for stmt in stmts {
        planner
            .plan_statement(stmt)
            .map_err(|e| format!("plan: {e}"))?;
    }
    Ok(())
}

/// parse + plan，返回 LogicalPlan（用于 execute）
fn parse_plan_execute(
    sql: &str,
    catalog: &dyn szrsql_sql::plan::Catalog,
) -> Result<szrsql_sql::plan::LogicalPlan, String> {
    let stmts = parse_sql(sql).map_err(|e| format!("parse: {e}"))?;
    if stmts.len() != 1 {
        return Err(format!("expected 1 statement, got {}", stmts.len()));
    }
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .map_err(|e| format!("plan: {e}"))
}

// =====================================================================
//  测试 1：1M 混合 SQL，0 panic（核心验收测试）
// =====================================================================

#[test]
fn test_fuzz_million_mixed_sql_no_panic() {
    let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);
    let catalog = make_catalog();
    let total: u64 = 1_000_000;
    let mut valid_count: u64 = 0;
    let mut invalid_count: u64 = 0;
    let mut valid_ok: u64 = 0;
    let mut invalid_fail: u64 = 0;

    for i in 0..total {
        let (kind, sql) = generate_sql(&mut rng);
        match kind {
            SqlKind::Valid => {
                valid_count += 1;
                if parse_plan(&sql, &catalog).is_ok() {
                    valid_ok += 1;
                }
            }
            SqlKind::Invalid => {
                invalid_count += 1;
                if parse_plan(&sql, &catalog).is_err() {
                    invalid_fail += 1;
                }
            }
        }
        // 进度输出（eprintln 不影响测试结果，仅用于长测试监控）
        if (i + 1) % 200_000 == 0 {
            eprintln!(
                "  [progress] {}/{} (valid: {}, invalid: {})",
                i + 1,
                total,
                valid_count,
                invalid_count
            );
        }
    }

    // 验收标准 1：0 panic（测试未 panic 即通过）
    // 验收标准 2：合法 SQL 全部 parse + plan 成功
    assert_eq!(
        valid_count, valid_ok,
        "所有合法 SQL 应 parse + plan 成功: {valid_count} 总计, {valid_ok} 成功"
    );
    // 验收标准 3：非法 SQL 全部 parse 或 plan 失败（0 误执行非法 SQL）
    assert_eq!(
        invalid_count, invalid_fail,
        "所有非法 SQL 应 parse 或 plan 失败: {invalid_count} 总计, {invalid_fail} 失败"
    );
    // 验收标准 4：总计 1M
    assert_eq!(valid_count + invalid_count, total);
    eprintln!("✅ 1M Fuzz 完成: valid={valid_count}, invalid={invalid_count}, 0 panic, 0 误执行");
}

// =====================================================================
//  测试 2：100K 合法 SQL 全部 parse + plan 成功
// =====================================================================

#[test]
fn test_fuzz_valid_sql_parse_plan_ok() {
    let mut rng = XorShift64::new(0xAABB_CCDD_EEFF_0011);
    let catalog = make_catalog();
    for _ in 0..100_000 {
        let sql = gen_valid_sql(&mut rng);
        assert!(
            parse_plan(&sql, &catalog).is_ok(),
            "合法 SQL 应 parse + plan 成功: {sql}"
        );
    }
}

// =====================================================================
//  测试 3：100K 非法 SQL 全部 parse 或 plan 失败
// =====================================================================

#[test]
fn test_fuzz_invalid_sql_fails_at_some_stage() {
    let mut rng = XorShift64::new(0xCCDD_EEFF_0011_2233);
    let catalog = make_catalog();
    for _ in 0..100_000 {
        let sql = gen_invalid_sql(&mut rng);
        assert!(
            parse_plan(&sql, &catalog).is_err(),
            "非法 SQL 应 parse 或 plan 失败: {sql}"
        );
    }
}

// =====================================================================
//  测试 4：10K 合法 SELECT 执行正确（WHERE id = n → 0 或 1 行）
// =====================================================================

#[test]
fn test_fuzz_valid_select_executes_correctly() {
    let mut rng = XorShift64::new(0x1111_2222_3333_4444);
    let catalog = make_catalog();
    let table = make_table_with_rows(100); // id = 0..99
    for _ in 0..10_000 {
        let n = rng.next_i64_small();
        let sql = format!("SELECT id FROM t WHERE id = {n}");
        let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
        let mut exec = Executor::new();
        exec.register_table(&table);
        let result = exec.execute(&plan).expect("execute failed");
        // 验证：id = n 的行数（0 或 1，因 id 范围 0..99）
        let expected_count = if (0..100).contains(&n) {
            1
        } else {
            0
        };
        assert_eq!(result.len(), expected_count, "SQL: {sql}");
    }
}

// =====================================================================
//  测试 5：1K 合法 INSERT 执行正确（每行插入 1 条）
// =====================================================================

#[test]
fn test_fuzz_valid_insert_executes_correctly() {
    let mut rng = XorShift64::new(0x2222_3333_4444_5555);
    let catalog = make_catalog();
    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let mut inserted = 0usize;
    for _ in 0..1_000 {
        let id = rng.next_i64_small();
        let name = random_name(&mut rng);
        let sql = format!("INSERT INTO t (id, name) VALUES ({id}, '{name}')");
        let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
        let exec = Executor::new();
        let result = exec
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
        assert_eq!(result.affected_rows, 1, "INSERT 应插入 1 行: {sql}");
        inserted += 1;
    }
    // 验证：表行数 == 插入次数（允许重复 id，无主键约束）
    assert_eq!(table.row_count(), inserted, "表行数应等于 INSERT 次数");
}

// =====================================================================
//  测试 6：10K 合法 UPDATE 执行正确（affected rows = 0 或 1）
// =====================================================================

#[test]
fn test_fuzz_valid_update_executes_correctly() {
    let mut rng = XorShift64::new(0x3333_4444_5555_6666);
    let catalog = make_catalog();
    let mut table = make_table_with_rows(100); // id = 0..99
    for _ in 0..10_000 {
        let id = rng.next_i64_small();
        let name = random_name(&mut rng);
        let sql = format!("UPDATE t SET name = '{name}' WHERE id = {id}");
        let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
        let exec = Executor::new();
        let result = exec
            .execute_update(&plan, &mut table)
            .expect("update failed");
        // 验证：affected rows = 1 if id in 0..99, else 0
        let expected = if (0..100).contains(&id) {
            1
        } else {
            0
        };
        assert_eq!(
            result.affected_rows, expected,
            "UPDATE affected rows: {sql}"
        );
    }
}

// =====================================================================
//  测试 7：10K 合法 DELETE 执行正确（affected rows = 0 或 1）
// =====================================================================

#[test]
fn test_fuzz_valid_delete_executes_correctly() {
    let mut rng = XorShift64::new(0x4444_5555_6666_7777);
    let catalog = make_catalog();
    let mut table = make_table_with_rows(100); // id = 0..99
    let mut remaining: HashSet<i64> = (0..100).collect();
    for _ in 0..10_000 {
        let id = rng.next_i64_small();
        let sql = format!("DELETE FROM t WHERE id = {id}");
        let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
        let exec = Executor::new();
        let result = exec
            .execute_delete(&plan, &mut table)
            .expect("delete failed");
        let expected = if remaining.contains(&id) {
            1
        } else {
            0
        };
        assert_eq!(
            result.affected_rows, expected,
            "DELETE affected rows: {sql}"
        );
        remaining.remove(&id);
    }
    // 验证：表行数 == remaining.len()
    assert_eq!(table.row_count(), remaining.len(), "剩余行数应匹配");
}

// =====================================================================
//  测试 8：10K 非法 SQL 0 误执行（不到达 execute 阶段）
// =====================================================================

#[test]
fn test_fuzz_invalid_sql_no_successful_execute() {
    let mut rng = XorShift64::new(0x5555_6666_7777_8888);
    let catalog = make_catalog();
    let mut parse_fail: u64 = 0;
    let mut plan_fail: u64 = 0;
    let mut reached_execute: u64 = 0;
    for _ in 0..10_000 {
        let sql = gen_invalid_sql(&mut rng);
        let parse_result = parse_sql(&sql);
        if parse_result.is_err() {
            parse_fail += 1;
            continue;
        }
        let stmts = parse_result.unwrap();
        let planner = Planner::new(&catalog);
        let mut plan_failed = false;
        for stmt in stmts {
            if planner.plan_statement(stmt).is_err() {
                plan_failed = true;
            }
        }
        if plan_failed {
            plan_fail += 1;
            continue;
        }
        // 到达 execute 阶段 — 不应发生（所有非法模板应在 parse/plan 阶段失败）
        reached_execute += 1;
    }
    // 验收标准：0 误执行非法 SQL（reached_execute 必须为 0）
    assert_eq!(
        reached_execute, 0,
        "非法 SQL 不应到达 execute 阶段: {reached_execute} 条到达"
    );
    eprintln!("✅ 10K 非法 SQL: parse_fail={parse_fail}, plan_fail={plan_fail}, reached_execute={reached_execute}");
}

// =====================================================================
//  测试 9：特定 parse 错误（14 种语法错误变体）
// =====================================================================

#[test]
fn test_fuzz_parse_error_variety() {
    let invalid_sqls = [
        "SELECT FROM",
        "SELECT * FROM",
        "INSERT INTO t VALUES",
        "INVALID SQL STATEMENT",
        "SELECT * FROM t WHERE",
        "UPDATE t SET",
        "DELETE FROM",
        "SELECT *, FROM t",
        "SELECT id, FROM t",
        "CREATE TABLE",
        "SELECT * FROM t WHERE id =",
        "INSERT INTO t (id, name) VALUES",
        "UPDATE t SET name = WHERE id = 1",
        "DELETE FROM t WHERE",
    ];
    for sql in invalid_sqls {
        assert!(parse_sql(sql).is_err(), "应 parse 失败: {sql}");
    }
}

// =====================================================================
//  测试 10：特定 plan 错误（表不存在）
// =====================================================================

#[test]
fn test_fuzz_plan_error_variety() {
    let catalog = make_catalog();
    let planner = Planner::new(&catalog);
    // 注意：`SELECT bad_col_xyz FROM t` 不在此列 — plan 阶段不校验 SELECT 投影列名，
    // 该用例由 `test_fuzz_execute_error_column_not_found` 覆盖。
    let plan_error_sqls = [
        "SELECT * FROM nonexistent_table_xyz",
        "INSERT INTO nonexistent_table (id) VALUES (1)",
        "UPDATE nonexistent_table SET id = 1",
        "DELETE FROM nonexistent_table",
    ];
    for sql in plan_error_sqls {
        let stmts = parse_sql(sql).unwrap();
        for stmt in stmts {
            assert!(planner.plan_statement(stmt).is_err(), "应 plan 失败: {sql}");
        }
    }
}

// =====================================================================
//  测试 15：execute 阶段列不存在错误（SELECT bad_col FROM t）
// =====================================================================

#[test]
fn test_fuzz_execute_error_column_not_found() {
    let catalog = make_catalog();
    let table = make_table_with_rows(10);
    let execute_error_sqls = [
        "SELECT bad_col_xyz FROM t",
        "SELECT id, bad_col_xyz FROM t",
        "SELECT * FROM t WHERE bad_col_xyz = 1",
    ];
    for sql in execute_error_sqls {
        let plan =
            parse_plan_execute(sql, &catalog).expect("plan 应成功（列名校验在 execute 阶段）");
        let mut exec = Executor::new();
        exec.register_table(&table);
        let result = exec.execute(&plan);
        assert!(result.is_err(), "execute 应报错（列不存在）: {sql}");
        // 验证错误信息包含 "column not found"
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("column not found") || msg.contains("ColumnNotFound"),
            "错误信息应包含 'column not found': {sql}, 实际: {msg}"
        );
    }
}

// =====================================================================
//  测试 11：100K 随机 token 流，0 panic
// =====================================================================

#[test]
fn test_fuzz_random_tokens_no_panic() {
    let mut rng = XorShift64::new(0x6666_7777_8888_9999);
    let tokens = [
        "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "*", ",",
        "(", ")", "1", "2", "abc", ";", "t", "id", "name", "=", ">", "<", "AND", "OR", "NULL",
        "LIMIT", "OFFSET", "CREATE", "TABLE", "DROP",
    ];
    for _ in 0..100_000 {
        let n_tokens = (rng.next_range(10) + 1) as usize;
        let sql: String = (0..n_tokens)
            .map(|_| tokens[rng.next_range(tokens.len() as u32) as usize])
            .collect::<Vec<_>>()
            .join(" ");
        // 仅验证不 panic（parse 返回 Ok 或 Err 均可）
        let _ = parse_sql(&sql);
    }
}

// =====================================================================
//  测试 12：Unicode + 特殊字符，0 panic
// =====================================================================

#[test]
fn test_fuzz_unicode_special_chars_no_panic() {
    let catalog = make_catalog();
    let unicode_sqls = [
        "INSERT INTO t (id, name) VALUES (1, '测试')",
        "INSERT INTO t (id, name) VALUES (2, '日本語')",
        "INSERT INTO t (id, name) VALUES (3, '🎉')",
        "INSERT INTO t (id, name) VALUES (4, 'αβγδ')",
        "INSERT INTO t (id, name) VALUES (5, 'مرحبا')",
        "SELECT * FROM t WHERE name = '测试'",
        "SELECT * FROM t WHERE name = '🎉'",
    ];
    for sql in unicode_sqls {
        // 仅验证不 panic，parse_plan 可能成功或失败
        let _ = parse_plan(sql, &catalog);
    }

    // 随机 CJK 字符测试
    let mut rng = XorShift64::new(0x7777_8888_9999_AAAA);
    for _ in 0..10_000 {
        let n = rng.next_i64_small();
        let code = 0x4E00 + rng.next_range(0x9FFF - 0x4E00); // CJK 统一汉字范围
        let ch = char::from_u32(code).unwrap_or('中');
        let sql = format!("INSERT INTO t (id, name) VALUES ({n}, '{ch}')");
        let _ = parse_plan(&sql, &catalog);
    }
}

// =====================================================================
//  测试 13：极端数值，0 panic
// =====================================================================

#[test]
fn test_fuzz_extreme_numeric_values_no_panic() {
    let catalog = make_catalog();
    let extreme_sqls = [
        "SELECT id FROM t WHERE id = 9223372036854775807", // i64::MAX
        "SELECT id FROM t WHERE id = -9223372036854775807", // -i64::MAX
        "SELECT id FROM t WHERE id = 0",
        "SELECT id FROM t WHERE id = 2147483647", // i32::MAX
        "SELECT id FROM t WHERE id = -2147483648", // i32::MIN
        "SELECT id FROM t LIMIT 9223372036854775807",
        "SELECT id FROM t LIMIT 0",
        "SELECT id FROM t LIMIT 1",
        "SELECT id FROM t WHERE id > -9223372036854775807",
        "SELECT id FROM t WHERE id < 9223372036854775807",
    ];
    for sql in extreme_sqls {
        // 仅验证不 panic
        let _ = parse_plan(sql, &catalog);
    }
}

// =====================================================================
//  测试 14：DML 序列一致性（参考模型 — 核心状态污染检测）
// =====================================================================

#[test]
fn test_fuzz_dml_sequence_consistency() {
    let mut rng = XorShift64::new(0x8888_9999_AAAA_BBBB);
    let catalog = make_catalog();
    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // 参考模型：id → name（唯一 id，避免重复 INSERT 导致参考模型复杂化）
    let mut reference: HashMap<i64, String> = HashMap::new();
    let mut next_unique_id: i64 = 0;

    for _ in 0..5_000 {
        let op = rng.next_range(3);
        match op {
            0 => {
                // INSERT — 使用唯一 id，保证参考模型与表状态一致
                let id = next_unique_id;
                next_unique_id += 1;
                let name = random_name(&mut rng);
                let sql = format!("INSERT INTO t (id, name) VALUES ({id}, '{name}')");
                let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
                let exec = Executor::new();
                let result = exec
                    .execute_insert(&plan, &mut table)
                    .expect("insert failed");
                assert_eq!(result.affected_rows, 1, "INSERT 应插入 1 行: {sql}");
                reference.insert(id, name.to_string());
            }
            1 => {
                // UPDATE — 随机 id（可能不存在），affected rows = 0 或 1
                let id = rng.next_range(next_unique_id as u32) as i64;
                let name = random_name(&mut rng);
                let sql = format!("UPDATE t SET name = '{name}' WHERE id = {id}");
                let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
                let exec = Executor::new();
                let result = exec
                    .execute_update(&plan, &mut table)
                    .expect("update failed");
                let expected = if reference.contains_key(&id) {
                    1
                } else {
                    0
                };
                assert_eq!(
                    result.affected_rows, expected,
                    "UPDATE affected rows: {sql}"
                );
                if expected == 1 {
                    reference.insert(id, name.to_string());
                }
            }
            2 => {
                // DELETE — 随机 id（可能不存在），affected rows = 0 或 1
                let id = rng.next_range(next_unique_id as u32) as i64;
                let sql = format!("DELETE FROM t WHERE id = {id}");
                let plan = parse_plan_execute(&sql, &catalog).expect("plan failed");
                let exec = Executor::new();
                let result = exec
                    .execute_delete(&plan, &mut table)
                    .expect("delete failed");
                let expected = if reference.contains_key(&id) {
                    1
                } else {
                    0
                };
                assert_eq!(
                    result.affected_rows, expected,
                    "DELETE affected rows: {sql}"
                );
                if expected == 1 {
                    reference.remove(&id);
                }
            }
            _ => unreachable!(),
        }
    }

    // 最终验证 1：表行数 == reference.len()
    assert_eq!(
        table.row_count(),
        reference.len(),
        "最终表行数应匹配参考模型: table={}, reference={}",
        table.row_count(),
        reference.len()
    );

    // 最终验证 2：SELECT id, name FROM t 结果与 reference 完全一致
    let select_plan = parse_plan_execute("SELECT id, name FROM t", &catalog).expect("plan failed");
    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&select_plan).expect("execute failed");

    let mut actual: HashMap<i64, String> = HashMap::new();
    for row in result {
        if let (Value::Int64(id), Value::Text(name)) = (&row[0], &row[1]) {
            actual.insert(*id, name.clone());
        }
    }
    assert_eq!(actual, reference, "最终表内容应与参考模型完全一致");
    eprintln!("✅ 5K DML 序列一致性验证通过: 最终 {} 行", reference.len());
}
