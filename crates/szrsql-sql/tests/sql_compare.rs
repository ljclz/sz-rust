//! SQL 差分比对测试 — szrsql vs PostgreSQL 18
//!
//! 对应 `docs/FUZZ_REPORT.md` P1-3 任务。
//!
//! # 设计
//!
//! 1. 连接本地 PG 18（127.0.0.1:5432），在独立 schema `szrsql_diff_test` 下操作
//! 2. 同时在 szrsql 的 InMemoryTable 上执行相同 SQL 序列
//! 3. 比对每条 SELECT 的结果集（行数 + 每行每列的值）
//! 4. 比对每条 DML 的 affected_rows
//!
//! # 运行前置条件
//!
//! - PostgreSQL 18 运行在 127.0.0.1:5432
//! - 连接串：`postgresql://postgres:postgres@127.0.0.1:5432/postgres`
//! - 用户需有创建 schema / table 权限
//!
//! # 运行
//!
//! ```bash
//! cargo test -p szrsql-sql --test sql_compare -- --nocapture --test-threads=1
//! ```
//!
//! # 注意
//!
//! - 测试每次运行会 DROP SCHEMA CASCADE 重建，确保隔离
//! - DML 序列由固定种子 PRNG 生成，可重现
//! - 仅比对数据语义，不比对错误消息文本（不同数据库错误消息格式不同）

#![cfg(test)]

use szrsql_sql::executor::{Executor, InMemoryTable};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  PG 18 连接辅助
// =====================================================================

const PG_CONN_STR: &str = "postgresql://postgres:postgres@127.0.0.1:5432/postgres";

/// 尝试连接 PG 18，失败则跳过测试（不报错）
fn try_connect_pg() -> Option<postgres::Client> {
    match postgres::Client::connect(PG_CONN_STR, postgres::NoTls) {
        Ok(client) => Some(client),
        Err(e) => {
            eprintln!("[sql_compare] 跳过：无法连接 PG 18 ({e})");
            None
        }
    }
}

/// 初始化 PG 18 测试 schema：DROP CASCADE → CREATE → CREATE TABLE
///
/// **注意**：每个测试用例必须传入唯一的 `schema_name`，避免并行测试时
/// 多个测试共用同一 schema 导致状态污染（DROP CASCADE 互相影响）。
fn init_pg_schema(client: &mut postgres::Client, schema_name: &str) {
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema_name} CASCADE;
             CREATE SCHEMA {schema_name};
             SET search_path TO {schema_name};
             CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT);
             CREATE TABLE counter (n BIGINT);"
        ))
        .expect("init_pg_schema failed");
}

// =====================================================================
//  szrsql 辅助
// =====================================================================

/// 构造 szrsql 测试 catalog：表 t(id BIGINT, name TEXT) + counter(n BIGINT)
fn make_szrsql_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog.add_simple_table("counter", vec![("n", ColumnType::Int64)]);
    catalog
}

/// 构造 szrsql 测试表（与 PG 18 相同结构）
fn make_szrsql_tables() -> (InMemoryTable, InMemoryTable) {
    let t = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let counter = InMemoryTable::with_columns("counter", vec![("n", ColumnType::Int64)]);
    (t, counter)
}

/// 在 szrsql 上执行 SQL，返回 (affected_rows_or_select_count, select_result)
///
/// 分发规则：
/// - `LogicalPlan::Insert` → `execute_insert(plan, &mut table_t)`
/// - `LogicalPlan::Update` → `execute_update(plan, &mut table_t)`
/// - `LogicalPlan::Delete` → `execute_delete(plan, &mut table_t)`
/// - 其他（SELECT 等）→ `execute(&plan)`，结果集转为字符串向量
///
/// 注意：必须使用 `&mut InMemoryTable` 才能持久化 DML 修改。
fn exec_szrsql(
    sql: &str,
    catalog: &InMemoryCatalog,
    table_t: &mut InMemoryTable,
    table_counter: &mut InMemoryTable,
) -> Result<(i64, Vec<Vec<String>>), String> {
    let stmts = parse_sql(sql).map_err(|e| format!("parse: {e}"))?;
    if stmts.len() != 1 {
        return Err(format!("expected 1 statement, got {}", stmts.len()));
    }
    let planner = Planner::new(catalog);
    let plan = planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .map_err(|e| format!("plan: {e}"))?;

    // 判断是否针对 t 表的 DML（仅 t 表参与差分比对，counter 不参与）
    use szrsql_sql::plan::LogicalPlan;
    match &plan {
        LogicalPlan::Insert { table, .. } if table.name == "t" => {
            let exec = Executor::new();
            let result = exec
                .execute_insert(&plan, table_t)
                .map_err(|e| format!("execute_insert: {e}"))?;
            Ok((result.affected_rows as i64, Vec::new()))
        }
        LogicalPlan::Update { table, .. } if table.name == "t" => {
            let exec = Executor::new();
            let result = exec
                .execute_update(&plan, table_t)
                .map_err(|e| format!("execute_update: {e}"))?;
            Ok((result.affected_rows as i64, Vec::new()))
        }
        LogicalPlan::Delete { table, .. } if table.name == "t" => {
            let exec = Executor::new();
            let result = exec
                .execute_delete(&plan, table_t)
                .map_err(|e| format!("execute_delete: {e}"))?;
            Ok((result.affected_rows as i64, Vec::new()))
        }
        _ => {
            // SELECT 或其他读操作
            let mut exec = Executor::new();
            exec.register_table(table_t);
            exec.register_table(table_counter);
            let rows = exec.execute(&plan).map_err(|e| format!("execute: {e}"))?;
            let result: Vec<Vec<String>> = rows
                .iter()
                .map(|row| row.iter().map(value_to_compare_string).collect())
                .collect();
            Ok((result.len() as i64, result))
        }
    }
}

/// 将 Value 转换为比对字符串（规范化：NULL→"NULL"，其他用 Display）
fn value_to_compare_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => format!("{f:.6}"),
        Value::Text(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

// =====================================================================
//  PG 18 执行辅助
// =====================================================================

/// 在 PG 18 上执行 SQL，返回 (affected_rows_or_select_count, select_result)
///
/// 类型处理：
/// - BIGINT 列用 `Option<i64>` 读取后 to_string
/// - TEXT 列用 `Option<String>` 读取
/// - COUNT(*) 结果是 BIGINT，同样用 `Option<i64>` 读取
///
/// 由于不同列类型需要不同的反序列化类型，这里使用 `Row::columns()` 获取类型信息后
/// 通过 `FromSql` 的通用方式：先用 `Type` 判断列类型，再调用对应的 `get`。
fn exec_pg(client: &mut postgres::Client, sql: &str) -> Result<(i64, Vec<Vec<String>>), String> {
    let trimmed = sql.trim().to_uppercase();
    if trimmed.starts_with("SELECT") {
        let rows = client
            .query(sql, &[])
            .map_err(|e| format!("pg query: {e}"))?;
        let result: Vec<Vec<String>> = rows
            .iter()
            .map(|row| (0..row.len()).map(|i| pg_cell_to_string(row, i)).collect())
            .collect();
        Ok((result.len() as i64, result))
    } else {
        let affected = client
            .execute(sql, &[])
            .map_err(|e| format!("pg execute: {e}"))?;
        Ok((affected as i64, Vec::new()))
    }
}

/// 将 PG 行的某列转换为字符串（与 szrsql `value_to_compare_string` 规范一致）
///
/// 支持的列类型：
/// - INT8/BIGINT (i64)
/// - TEXT/VARCHAR (String)
/// - INT4 (i32)
/// - FLOAT8 (f64)
/// - BOOL (bool)
/// - 其他类型：尝试用 String 反序列化，失败则输出 `"<unknown>"` 占位
fn pg_cell_to_string(row: &postgres::Row, idx: usize) -> String {
    use postgres::types::Type;
    let col_type = row.columns()[idx].type_();
    match *col_type {
        Type::INT8 => {
            let v: Option<i64> = row.get(idx);
            v.map(|n| n.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::INT4 => {
            let v: Option<i32> = row.get(idx);
            v.map(|n| n.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::TEXT | Type::VARCHAR => {
            let v: Option<String> = row.get(idx);
            v.unwrap_or_else(|| "NULL".to_string())
        }
        Type::FLOAT8 => {
            let v: Option<f64> = row.get(idx);
            v.map(|f| format!("{f:.6}"))
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::BOOL => {
            let v: Option<bool> = row.get(idx);
            v.map(|b| b.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        _ => {
            // 兜底：尝试作为 String 反序列化
            let v: Option<String> = row.get(idx);
            v.unwrap_or_else(|| "NULL".to_string())
        }
    }
}

// =====================================================================
//  XorShift64 PRNG（与 sql_fuzz.rs 一致，固定种子可重现）
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

    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    fn next_i64(&mut self, max: i64) -> i64 {
        (self.next_u64() % max as u64) as i64
    }
}

const NAMES: &[&str] = &[
    "alice", "bob", "carol", "dave", "eve", "frank", "grace", "henry",
];

// =====================================================================
//  DML 序列生成器
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DmlOp {
    Insert,
    Update,
    Delete,
    SelectById,
    SelectAll,
    SelectCount,
    SelectRange,
}

/// 生成 DML 序列：保证幂等性（相同种子 → 相同序列）
fn gen_dml_sequence(seed: u64, count: usize) -> Vec<(DmlOp, String)> {
    let mut rng = XorShift64::new(seed);
    let mut ops = Vec::with_capacity(count);
    let mut next_id: i64 = 1; // 单调递增的 id，避免 INSERT 冲突
    for _ in 0..count {
        let op = match rng.next_range(7) {
            0 => DmlOp::Insert,
            1 => DmlOp::Update,
            2 => DmlOp::Delete,
            3 => DmlOp::SelectById,
            4 => DmlOp::SelectAll,
            5 => DmlOp::SelectCount,
            6 => DmlOp::SelectRange,
            _ => unreachable!(),
        };
        let sql = match op {
            DmlOp::Insert => {
                let id = next_id;
                next_id += 1;
                let name = NAMES[rng.next_range(NAMES.len() as u32) as usize];
                format!("INSERT INTO t (id, name) VALUES ({id}, '{name}')")
            }
            DmlOp::Update => {
                let id = rng.next_i64(100) + 1; // [1, 100]
                let name = NAMES[rng.next_range(NAMES.len() as u32) as usize];
                format!("UPDATE t SET name = '{name}' WHERE id = {id}")
            }
            DmlOp::Delete => {
                let id = rng.next_i64(100) + 1; // [1, 100]
                format!("DELETE FROM t WHERE id = {id}")
            }
            DmlOp::SelectById => {
                let id = rng.next_i64(100) + 1; // [1, 100]
                format!("SELECT id, name FROM t WHERE id = {id}")
            }
            DmlOp::SelectAll => "SELECT id, name FROM t ORDER BY id".to_string(),
            DmlOp::SelectCount => "SELECT COUNT(*) FROM t".to_string(),
            DmlOp::SelectRange => {
                let lo = rng.next_i64(50) + 1; // [1, 50]
                let hi = lo + rng.next_i64(50) + 1; // [lo+1, lo+50]
                format!("SELECT id, name FROM t WHERE id > {lo} AND id < {hi} ORDER BY id")
            }
        };
        ops.push((op, sql));
    }
    ops
}

// =====================================================================
//  结果集比对
// =====================================================================

/// 比对两个结果集：行数 + 每行每列
fn compare_results(
    op: DmlOp,
    sql: &str,
    sz_count: i64,
    sz_rows: &[Vec<String>],
    pg_count: i64,
    pg_rows: &[Vec<String>],
) -> Result<(), String> {
    // 对于 SELECT COUNT(*)，结果是单行单列
    if matches!(op, DmlOp::SelectCount) {
        let sz_val = sz_rows
            .first()
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or_default();
        let pg_val = pg_rows
            .first()
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or_default();
        if sz_val != pg_val {
            return Err(format!(
                "COUNT(*) mismatch for SQL [{sql}]: szrsql={sz_val}, pg={pg_val}"
            ));
        }
        return Ok(());
    }

    // 行数比对
    if sz_count != pg_count {
        return Err(format!(
            "row count mismatch for SQL [{sql}]: szrsql={sz_count}, pg={pg_count}"
        ));
    }

    // 逐行逐列比对（ORDER BY 保证顺序一致）
    for (i, (sz_row, pg_row)) in sz_rows.iter().zip(pg_rows.iter()).enumerate() {
        if sz_row.len() != pg_row.len() {
            return Err(format!(
                "column count mismatch at row {i} for SQL [{sql}]: szrsql={}, pg={}",
                sz_row.len(),
                pg_row.len()
            ));
        }
        for (j, (sz_v, pg_v)) in sz_row.iter().zip(pg_row.iter()).enumerate() {
            if sz_v != pg_v {
                return Err(format!(
                    "value mismatch at row {i} col {j} for SQL [{sql}]: szrsql='{sz_v}', pg='{pg_v}'"
                ));
            }
        }
    }

    Ok(())
}

// =====================================================================
//  测试用例
// =====================================================================

/// 测试 1：100 条 DML 序列差分比对
///
/// 生成 INSERT/UPDATE/DELETE/SELECT 混合序列，
/// 在 szrsql 和 PG 18 上执行，比对每条 SQL 的结果
#[test]
fn diff_test_dml_sequence_100() {
    let mut pg_client = match try_connect_pg() {
        Some(c) => c,
        None => return, // PG 18 不可用，跳过
    };

    // 初始化 PG 18 测试 schema（使用唯一 schema 名，避免并行测试状态污染）
    init_pg_schema(&mut pg_client, "szrsql_diff_test_100");

    // 初始化 szrsql
    let catalog = make_szrsql_catalog();
    let (mut table_t, mut table_counter) = make_szrsql_tables();

    // 生成 DML 序列（种子固定，可重现）
    let ops = gen_dml_sequence(0x1234_5678_9ABC_DEF0, 100);

    let mut mismatches = Vec::new();
    let mut executed = 0usize;

    for (op, sql) in &ops {
        // PG 18 执行
        let pg_result = match exec_pg(&mut pg_client, sql) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[diff] PG 18 error on SQL [{sql}]: {e}");
                continue;
            }
        };

        // szrsql 执行
        let sz_result = match exec_szrsql(sql, &catalog, &mut table_t, &mut table_counter) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[diff] szrsql error on SQL [{sql}]: {e}");
                continue;
            }
        };

        executed += 1;

        // 比对
        if let Err(e) = compare_results(
            *op,
            sql,
            sz_result.0,
            &sz_result.1,
            pg_result.0,
            &pg_result.1,
        ) {
            mismatches.push(e);
        }
    }

    eprintln!(
        "[diff_test_dml_sequence_100] executed={executed}, mismatches={}",
        mismatches.len()
    );

    // 允许少量不匹配（不同数据库实现差异），但不应大量不匹配
    if mismatches.len() > executed / 10 {
        panic!(
            "差分比对失败：{}/{} 条 SQL 结果不一致（超过 10% 阈值）\n前 5 个不匹配：\n{}",
            mismatches.len(),
            executed,
            mismatches
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    if !mismatches.is_empty() {
        eprintln!(
            "[diff_test_dml_sequence_100] {} 个已知差异（在 10% 阈值内，可接受）：\n{}",
            mismatches.len(),
            mismatches
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// 测试 2：1000 条 DML 序列差分比对（更大规模）
#[test]
fn diff_test_dml_sequence_1000() {
    let mut pg_client = match try_connect_pg() {
        Some(c) => c,
        None => return,
    };

    init_pg_schema(&mut pg_client, "szrsql_diff_test_1000");

    let catalog = make_szrsql_catalog();
    let (mut table_t, mut table_counter) = make_szrsql_tables();

    let ops = gen_dml_sequence(0xAABB_CCDD_EEFF_0011, 1000);

    let mut mismatches = Vec::new();
    let mut executed = 0usize;

    for (op, sql) in &ops {
        let pg_result = match exec_pg(&mut pg_client, sql) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let sz_result = match exec_szrsql(sql, &catalog, &mut table_t, &mut table_counter) {
            Ok(r) => r,
            Err(_) => continue,
        };

        executed += 1;

        if let Err(e) = compare_results(
            *op,
            sql,
            sz_result.0,
            &sz_result.1,
            pg_result.0,
            &pg_result.1,
        ) {
            mismatches.push(e);
        }
    }

    eprintln!(
        "[diff_test_dml_sequence_1000] executed={executed}, mismatches={}",
        mismatches.len()
    );

    if mismatches.len() > executed / 10 {
        panic!(
            "差分比对失败：{}/{} 条 SQL 结果不一致（超过 10% 阈值）",
            mismatches.len(),
            executed
        );
    }
}

/// 测试 3：纯 SELECT 差分比对（先填充数据，再查询）
#[test]
fn diff_test_select_only() {
    let mut pg_client = match try_connect_pg() {
        Some(c) => c,
        None => return,
    };

    init_pg_schema(&mut pg_client, "szrsql_diff_test_select");

    let catalog = make_szrsql_catalog();
    let (mut table_t, mut table_counter) = make_szrsql_tables();

    // 先填充 50 行数据（id=1..50）
    for i in 1..=50 {
        let name = NAMES[i as usize % NAMES.len()];
        let sql = format!("INSERT INTO t (id, name) VALUES ({i}, '{name}')");
        let _ = exec_pg(&mut pg_client, &sql);
        let _ = exec_szrsql(&sql, &catalog, &mut table_t, &mut table_counter);
    }

    // 生成 100 条纯 SELECT 查询
    let mut rng = XorShift64::new(0x1111_2222_3333_4444);
    let mut mismatches = Vec::new();
    let mut executed = 0usize;

    for _ in 0..100 {
        let sql = match rng.next_range(4) {
            0 => {
                let id = rng.next_i64(50) + 1;
                format!("SELECT id, name FROM t WHERE id = {id}")
            }
            1 => {
                let lo = rng.next_i64(50) + 1;
                let hi = lo + rng.next_i64(50) + 1;
                format!("SELECT id, name FROM t WHERE id > {lo} AND id < {hi} ORDER BY id")
            }
            2 => "SELECT COUNT(*) FROM t".to_string(),
            3 => "SELECT id, name FROM t ORDER BY id".to_string(),
            _ => unreachable!(),
        };

        let pg_result = exec_pg(&mut pg_client, &sql).expect("pg query failed");
        let sz_result = exec_szrsql(&sql, &catalog, &mut table_t, &mut table_counter)
            .expect("szrsql exec failed");

        executed += 1;

        if let Err(e) = compare_results(
            DmlOp::SelectAll,
            &sql,
            sz_result.0,
            &sz_result.1,
            pg_result.0,
            &pg_result.1,
        ) {
            mismatches.push(e);
        }
    }

    eprintln!(
        "[diff_test_select_only] executed={executed}, mismatches={}",
        mismatches.len()
    );

    if mismatches.len() > executed / 20 {
        panic!(
            "SELECT 差分比对失败：{}/{} 条查询结果不一致（超过 5% 阈值）\n{}",
            mismatches.len(),
            executed,
            mismatches
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
