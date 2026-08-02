//! MySQL 元数据查询处理器
//!
//! 拦截 Navicat 等客户端发送的 SHOW 和 information_schema 查询，
//! 从 shared_tables 读取真实元数据，构建 MySQL 兼容的结果集。

use std::collections::HashMap;
use std::sync::Arc;
use szrsql_protocol::pgwire::session::ResultColumn;
use szrsql_protocol::pgwire::InMemoryTable;
use szrsql_sql::ast::ColumnDefinition as SqlColumnDefinition;
use szrsql_sql::executor::TableStorage;
use szrsql_types::value::{ColumnType, Value};
use tokio::sync::{Mutex, RwLock};

/// 已知的 MySQL 数据库名列表（用于解析表键中的 schema 前缀）
const KNOWN_DATABASES: &[&str] = &[
    "information_schema",
    "njszjt",
    "sz_orm_test",
    "sz300",
    "shop",
    "mpay",
    "public",
];

/// 尝试处理 MySQL 元数据查询。
pub async fn try_handle_metadata_query(
    sql: &str,
    current_db: &Option<String>,
    shared_tables: &Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();
    let shared = shared_tables.as_ref()?;

    if upper == "SHOW TABLE STATUS" || upper.starts_with("SHOW TABLE STATUS ") {
        return handle_show_table_status(trimmed, current_db, shared).await;
    }
    if upper.starts_with("SHOW CREATE TABLE ") {
        return handle_show_create_table(trimmed, current_db, shared).await;
    }
    if upper.starts_with("SHOW COLUMNS FROM ")
        || upper.starts_with("SHOW FULL COLUMNS FROM ")
        || upper.starts_with("SHOW FIELDS FROM ")
    {
        return handle_show_columns(trimmed, current_db, shared).await;
    }
    if upper.starts_with("SHOW INDEX FROM ")
        || upper.starts_with("SHOW INDEXES FROM ")
        || upper.starts_with("SHOW KEYS FROM ")
    {
        return handle_show_index(trimmed, current_db, shared).await;
    }
    if upper == "SHOW FULL TABLES"
        || upper.starts_with("SHOW FULL TABLES ")
        || upper == "SHOW TABLES"
        || upper.starts_with("SHOW TABLES ")
    {
        return handle_show_tables(trimmed, current_db, shared).await;
    }
    if upper.contains("INFORMATION_SCHEMA.") {
        return handle_information_schema(trimmed, shared).await;
    }
    None
}

fn parse_table_key(key: &str) -> (String, String) {
    let key_lower = key.to_lowercase();
    for db in KNOWN_DATABASES {
        let prefix = format!("{}_", db);
        if key_lower.starts_with(&prefix) {
            return (db.to_string(), key[prefix.len()..].to_string());
        }
    }
    if let Some(pos) = key.find('_') {
        (key[..pos].to_string(), key[pos + 1..].to_string())
    } else {
        ("public".to_string(), key.to_string())
    }
}

/// 返回所有表信息：(db, table_name, original_key, columns)
///
/// `table_name` 是从 `original_key` 拆分出的纯表名（如 `soci_article`），
/// `original_key` 是 shared_tables 中的完整键名（如 `njszjt_soci_article`）。
async fn get_all_tables(
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Vec<(String, String, String, Vec<SqlColumnDefinition>)> {
    let guard = shared.read().await;
    let mut result = Vec::new();
    for (key, table_arc) in guard.iter() {
        let table_guard = table_arc.lock().await;
        let schema = table_guard.schema();
        let (db, table_name) = parse_table_key(key);
        let columns = schema.columns.clone();
        result.push((db, table_name, key.clone(), columns));
    }
    result
}

async fn get_tables_for_db(
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    db_name: &str,
) -> Vec<(String, Vec<SqlColumnDefinition>)> {
    let all = get_all_tables(shared).await;
    tracing::debug!(
        target: "mysql_metadata",
        db_name = %db_name,
        total_tables_in_shared = all.len(),
        all_dbs = ?all.iter().map(|(db, _, _, _)| db.clone()).collect::<Vec<_>>(),
        "get_tables_for_db: filtering"
    );
    all.into_iter()
        .filter(|(db, _, _, _)| db.eq_ignore_ascii_case(db_name))
        .map(|(_, name, _, cols)| (name, cols))
        .collect()
}

/// 查找表列信息，匹配优先级：
/// 1. 完整键名精确匹配（如 `njszjt_soci_article`）
/// 2. db + 拆分后的表名匹配（如 db=`njszjt`, table=`soci_article`）
/// 3. 仅拆分后的表名匹配
async fn find_table_columns(
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    db: Option<&str>,
    table_name: &str,
) -> Option<(String, String, Vec<SqlColumnDefinition>)> {
    let all = get_all_tables(shared).await;
    // 优先级 1：完整键名精确匹配（Navicat 可能发送带 schema 前缀的完整表名）
    for (t_db, t_name, orig_key, cols) in &all {
        if orig_key.eq_ignore_ascii_case(table_name) {
            return Some((t_db.clone(), t_name.clone(), cols.clone()));
        }
    }
    // 优先级 2：db + 拆分后的表名匹配
    if let Some(db) = db {
        for (t_db, t_name, _orig_key, cols) in &all {
            if t_db.eq_ignore_ascii_case(db) && t_name.eq_ignore_ascii_case(table_name) {
                return Some((t_db.clone(), t_name.clone(), cols.clone()));
            }
        }
    }
    // 优先级 3：仅拆分后的表名匹配
    for (t_db, t_name, _orig_key, cols) in &all {
        if t_name.eq_ignore_ascii_case(table_name) {
            return Some((t_db.clone(), t_name.clone(), cols.clone()));
        }
    }
    None
}

fn parse_qualified_table_name(s: &str) -> (Option<String>, String) {
    let s = s.trim().trim_end_matches(';').trim();
    let s = s.replace('`', "\"");
    if s.contains('.') {
        let parts: Vec<&str> = s.splitn(2, '.').collect();
        let db = parts[0].trim_matches('"').trim_matches('\'').to_string();
        let table = parts[1].trim_matches('"').trim_matches('\'').to_string();
        (Some(db), table)
    } else {
        (None, s.trim_matches('"').trim_matches('\'').to_string())
    }
}

/// 从 Value 提取可排序的字符串引用（避免 Value 未实现 Display 的问题）
fn value_as_str(v: &Value) -> &str {
    match v {
        Value::Text(s) => s.as_str(),
        Value::Null => "",
        _ => "",
    }
}

fn column_type_to_mysql(ct: &ColumnType) -> String {
    match ct {
        ColumnType::Int64 => "bigint".to_string(),
        ColumnType::Float64 => "double".to_string(),
        ColumnType::Text => "text".to_string(),
        ColumnType::Bool => "tinyint(1)".to_string(),
        ColumnType::Blob => "blob".to_string(),
        ColumnType::Date => "date".to_string(),
        ColumnType::Timestamp => "datetime".to_string(),
        ColumnType::Decimal { precision, scale } => {
            format!("decimal({},{})", precision, scale)
        }
        ColumnType::Json => "json".to_string(),
        _ => "text".to_string(),
    }
}

fn extract_schema_filter(sql_upper: &str, sql: &str) -> Option<String> {
    if let Some(pos) = sql_upper.find("TABLE_SCHEMA") {
        let rest = &sql[pos..];
        if let Some(sq) = rest.find('\'') {
            if rest[..sq].contains('=') {
                let after_quote = &rest[sq + 1..];
                if let Some(end) = after_quote.find('\'') {
                    return Some(after_quote[..end].to_string());
                }
            }
        }
    }
    None
}

fn extract_all_schema_filters(sql_upper: &str, sql: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = sql_upper[search_from..].find("TABLE_SCHEMA") {
        let abs_pos = search_from + pos;
        let rest = &sql[abs_pos..];
        if let Some(sq) = rest.find('\'') {
            if rest[..sq].contains('=') {
                let after_quote = &rest[sq + 1..];
                if let Some(end) = after_quote.find('\'') {
                    result.push(after_quote[..end].to_string());
                }
            }
        }
        search_from = abs_pos + 12;
    }
    result
}

fn rct(name: &str) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        column_type: ColumnType::Text,
    }
}
fn rci(name: &str) -> ResultColumn {
    ResultColumn {
        name: name.to_string(),
        column_type: ColumnType::Int64,
    }
}

async fn handle_show_table_status(
    sql: &str,
    current_db: &Option<String>,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let like_pattern: Option<String> = {
        if let Some(pos) = upper.find(" LIKE ") {
            let rest = sql[pos + 6..].trim();
            if rest.starts_with('\'') {
                let end = rest[1..].find('\'')?;
                Some(rest[1..1 + end].to_string())
            } else {
                None
            }
        } else {
            None
        }
    };

    let db = current_db.clone().unwrap_or_else(|| "njszjt".to_string());
    let tables = get_tables_for_db(shared, &db).await;

    let columns = vec![
        rct("Name"),
        rct("Engine"),
        rci("Version"),
        rct("Row_format"),
        rci("Rows"),
        rci("Avg_row_length"),
        rci("Data_length"),
        rci("Max_data_length"),
        rci("Index_length"),
        rci("Data_free"),
        rct("Auto_increment"),
        rct("Create_time"),
        rct("Update_time"),
        rct("Check_time"),
        rct("Collation"),
        rct("Checksum"),
        rct("Create_options"),
        rct("Comment"),
    ];

    let mut rows = Vec::new();
    for (table_name, _cols) in &tables {
        if let Some(ref pattern) = like_pattern {
            if !mysql_like_match(table_name, pattern) {
                continue;
            }
        }
        let table_key = format!("{}_{}", db, table_name);
        let row_count = {
            let guard = shared.read().await;
            if let Some(t) = guard.get(&table_key) {
                let tg = t.lock().await;
                tg.rows().len() as i64
            } else {
                0
            }
        };
        rows.push(vec![
            Value::Text(table_name.clone()),
            Value::Text("InnoDB".to_string()),
            Value::Int64(10),
            Value::Text("Dynamic".to_string()),
            Value::Int64(row_count),
            Value::Int64(0),
            Value::Int64(16384),
            Value::Int64(0),
            Value::Int64(0),
            Value::Int64(0),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Text("utf8mb4_general_ci".to_string()),
            Value::Null,
            Value::Text("".to_string()),
            Value::Text("".to_string()),
        ]);
    }
    Some((columns, rows))
}

async fn handle_show_create_table(
    sql: &str,
    _current_db: &Option<String>,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let after = sql[upper.find("SHOW CREATE TABLE ")? + 18..]
        .trim()
        .trim_end_matches(';')
        .trim();
    let (db_opt, table_name) = parse_qualified_table_name(after);
    let (_actual_db, actual_table, columns) =
        find_table_columns(shared, db_opt.as_deref(), &table_name).await?;

    let mut col_defs = Vec::new();
    let mut primary_keys = Vec::new();
    for col in &columns {
        let mysql_type = column_type_to_mysql(&col.data_type);
        let mut def = format!("  `{}` {}", col.name, mysql_type);
        if col.not_null {
            def.push_str(" NOT NULL");
        }
        if col.primary_key {
            primary_keys.push(col.name.clone());
        }
        col_defs.push(def);
    }
    if !primary_keys.is_empty() {
        let pk_list: Vec<String> = primary_keys.iter().map(|k| format!("`{}`", k)).collect();
        col_defs.push(format!("  PRIMARY KEY ({})", pk_list.join(", ")));
    }
    let ddl = format!(
        "CREATE TABLE `{}` (\n{}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        actual_table,
        col_defs.join(",\n")
    );

    Some((
        vec![rct("Table"), rct("Create Table")],
        vec![vec![Value::Text(actual_table), Value::Text(ddl)]],
    ))
}

async fn handle_show_columns(
    sql: &str,
    current_db: &Option<String>,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let is_full = upper.starts_with("SHOW FULL COLUMNS FROM");
    let after_from = if is_full {
        sql[23..].trim()
    } else if upper.starts_with("SHOW COLUMNS FROM ") {
        sql[18..].trim()
    } else {
        sql[17..].trim()
    };
    let table_part = after_from
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(';');
    let (db_opt, table_name) = parse_qualified_table_name(table_part);
    let db = db_opt.or_else(|| current_db.clone());
    let (_, _actual_table, columns) =
        find_table_columns(shared, db.as_deref(), &table_name).await?;

    let columns_result = if is_full {
        vec![
            rct("Field"),
            rct("Type"),
            rct("Collation"),
            rct("Null"),
            rct("Key"),
            rct("Default"),
            rct("Extra"),
            rct("Privileges"),
            rct("Comment"),
        ]
    } else {
        vec![
            rct("Field"),
            rct("Type"),
            rct("Null"),
            rct("Key"),
            rct("Default"),
            rct("Extra"),
        ]
    };

    let mut rows = Vec::new();
    for col in &columns {
        let mysql_type = column_type_to_mysql(&col.data_type);
        let null_str = if col.not_null {
            "NO"
        } else {
            "YES"
        };
        let key_str = if col.primary_key {
            "PRI"
        } else {
            ""
        };
        let default_val = col
            .default
            .as_ref()
            .map(|_| Value::Text("NULL".to_string()))
            .unwrap_or(Value::Null);
        let extra = if col.primary_key {
            "auto_increment".to_string()
        } else {
            String::new()
        };
        if is_full {
            rows.push(vec![
                Value::Text(col.name.clone()),
                Value::Text(mysql_type),
                Value::Text("utf8mb4_general_ci".to_string()),
                Value::Text(null_str.to_string()),
                Value::Text(key_str.to_string()),
                default_val,
                Value::Text(extra),
                Value::Text("select,insert,update,references".to_string()),
                Value::Text("".to_string()),
            ]);
        } else {
            rows.push(vec![
                Value::Text(col.name.clone()),
                Value::Text(mysql_type),
                Value::Text(null_str.to_string()),
                Value::Text(key_str.to_string()),
                default_val,
                Value::Text(extra),
            ]);
        }
    }
    Some((columns_result, rows))
}

async fn handle_show_index(
    sql: &str,
    current_db: &Option<String>,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let after_from = if upper.starts_with("SHOW INDEX FROM ") {
        sql[16..].trim()
    } else if upper.starts_with("SHOW INDEXES FROM ") {
        sql[18..].trim()
    } else {
        sql[15..].trim() // SHOW KEYS FROM
    };
    let table_part = after_from
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(';');
    let (db_opt, table_name) = parse_qualified_table_name(table_part);
    let db = db_opt.or_else(|| current_db.clone());
    let (actual_db, actual_table, columns) =
        find_table_columns(shared, db.as_deref(), &table_name).await?;

    let result_columns = vec![
        rct("Table"),
        rci("Non_unique"),
        rct("Key_name"),
        rci("Seq_in_index"),
        rct("Column_name"),
        rct("Collation"),
        rci("Cardinality"),
        rct("Sub_part"),
        rct("Packed"),
        rct("Null"),
        rct("Index_type"),
        rct("Comment"),
        rct("Index_comment"),
        rct("Visible"),
        rct("Expression"),
    ];

    let mut rows = Vec::new();
    let table_display = format!("{}.{}", actual_db, actual_table);

    // PRIMARY KEY 索引：合并所有 primary_key 列为单条索引
    let pk_cols: Vec<&SqlColumnDefinition> = columns.iter().filter(|c| c.primary_key).collect();
    if !pk_cols.is_empty() {
        for (seq, col) in pk_cols.iter().enumerate() {
            let null_str = if col.not_null {
                ""
            } else {
                "YES"
            };
            rows.push(vec![
                Value::Text(table_display.clone()),
                Value::Int64(0), // Non_unique=0 for PRIMARY
                Value::Text("PRIMARY".to_string()),
                Value::Int64((seq + 1) as i64),
                Value::Text(col.name.clone()),
                Value::Text("A".to_string()), // Collation: A=ascending
                Value::Int64(0),              // Cardinality
                Value::Null,                  // Sub_part
                Value::Null,                  // Packed
                Value::Text(null_str.to_string()),
                Value::Text("BTREE".to_string()),
                Value::Text(String::new()),
                Value::Text(String::new()),
                Value::Text("YES".to_string()),
                Value::Null,
            ]);
        }
    }

    // UNIQUE 索引：每个 unique 列单独一条
    for col in &columns {
        if col.unique && !col.primary_key {
            let null_str = if col.not_null {
                ""
            } else {
                "YES"
            };
            let key_name = format!("{}_key", col.name);
            rows.push(vec![
                Value::Text(table_display.clone()),
                Value::Int64(0), // Non_unique=0 for UNIQUE
                Value::Text(key_name),
                Value::Int64(1),
                Value::Text(col.name.clone()),
                Value::Text("A".to_string()),
                Value::Int64(0),
                Value::Null,
                Value::Null,
                Value::Text(null_str.to_string()),
                Value::Text("BTREE".to_string()),
                Value::Text(String::new()),
                Value::Text(String::new()),
                Value::Text("YES".to_string()),
                Value::Null,
            ]);
        }
    }

    Some((result_columns, rows))
}

async fn handle_show_tables(
    sql: &str,
    current_db: &Option<String>,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let is_full = upper.starts_with("SHOW FULL TABLES");
    let filter_views =
        if upper.contains("TABLE_TYPE != 'VIEW'") || upper.contains("TABLE_TYPE!='VIEW'") {
            Some(false)
        } else if upper.contains("TABLE_TYPE = 'VIEW'") || upper.contains("TABLE_TYPE='VIEW'") {
            Some(true)
        } else {
            None
        };

    let db = current_db.clone().unwrap_or_else(|| "njszjt".to_string());
    let tables = get_tables_for_db(shared, &db).await;
    let col_name = format!("Tables_in_{}", db);
    let columns = if is_full {
        vec![rct(&col_name), rct("Table_type")]
    } else {
        vec![rct(&col_name)]
    };

    let mut rows = Vec::new();
    for (table_name, _) in &tables {
        match filter_views {
            Some(true) => continue,
            Some(false) => {
                if is_full {
                    rows.push(vec![
                        Value::Text(table_name.clone()),
                        Value::Text("BASE TABLE".to_string()),
                    ]);
                } else {
                    rows.push(vec![Value::Text(table_name.clone())]);
                }
            }
            None => {
                if is_full {
                    rows.push(vec![
                        Value::Text(table_name.clone()),
                        Value::Text("BASE TABLE".to_string()),
                    ]);
                } else {
                    rows.push(vec![Value::Text(table_name.clone())]);
                }
            }
        }
    }
    Some((columns, rows))
}

async fn handle_information_schema(
    sql: &str,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();

    if upper.contains("INFORMATION_SCHEMA.SCHEMATA") {
        return handle_info_schema_schemata(shared).await;
    }
    if upper.contains("INFORMATION_SCHEMA.TABLES") {
        if upper.contains("COUNT(*)") {
            return handle_info_schema_tables_count(sql, shared).await;
        }
        return handle_info_schema_tables(sql, shared).await;
    }
    if upper.contains("INFORMATION_SCHEMA.COLUMNS") {
        return handle_info_schema_columns(sql, shared).await;
    }
    if upper.contains("INFORMATION_SCHEMA.ROUTINES") {
        return Some((
            vec![rct("ROUTINE_SCHEMA"), rct("ROUTINE_NAME"), rct("PARAMETER")],
            Vec::new(),
        ));
    }
    if upper.contains("INFORMATION_SCHEMA.VIEWS") {
        return Some((
            vec![
                rct("TABLE_NAME"),
                rct("CHECK_OPTION"),
                rct("IS_UPDATABLE"),
                rct("SECURITY_TYPE"),
                rct("DEFINER"),
            ],
            Vec::new(),
        ));
    }
    if upper.contains("INFORMATION_SCHEMA.PARAMETERS") {
        return Some((vec![rct("PARAMETER_NAME")], Vec::new()));
    }
    if upper.contains("INFORMATION_SCHEMA.KEY_COLUMN_USAGE")
        || upper.contains("INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS")
        || upper.contains("INFORMATION_SCHEMA.TABLE_CONSTRAINTS")
    {
        return Some((
            vec![
                rct("CONSTRAINT_NAME"),
                rct("TABLE_SCHEMA"),
                rct("TABLE_NAME"),
                rct("COLUMN_NAME"),
            ],
            Vec::new(),
        ));
    }
    if upper.contains("INFORMATION_SCHEMA.STATISTICS") {
        return Some((
            vec![
                rct("TABLE_SCHEMA"),
                rct("TABLE_NAME"),
                rci("NON_UNIQUE"),
                rct("INDEX_SCHEMA"),
                rct("INDEX_NAME"),
                rci("SEQ_IN_INDEX"),
                rct("COLUMN_NAME"),
                rct("COLLATION"),
                rci("CARDINALITY"),
                rct("SUB_PART"),
                rct("PACKED"),
                rct("NULLABLE"),
                rct("INDEX_TYPE"),
                rct("COMMENT"),
                rct("INDEX_COMMENT"),
            ],
            Vec::new(),
        ));
    }
    if upper.contains("INFORMATION_SCHEMA.ENGINES") {
        if upper.contains("COUNT") {
            return Some((vec![rct("support_ndb")], vec![vec![Value::Int64(0)]]));
        }
        return Some((
            vec![
                rct("ENGINE"),
                rct("SUPPORT"),
                rct("COMMENT"),
                rct("TRANSACTIONS"),
                rct("XA"),
                rct("SAVEPOINTS"),
            ],
            vec![vec![
                Value::Text("InnoDB".to_string()),
                Value::Text("YES".to_string()),
                Value::Text(
                    "Supports transactions, row-level locking, and foreign keys".to_string(),
                ),
                Value::Text("YES".to_string()),
                Value::Text("YES".to_string()),
                Value::Text("YES".to_string()),
            ]],
        ));
    }
    if upper.contains("INFORMATION_SCHEMA.EVENTS") {
        return Some((
            vec![
                rct("EVENT_CATALOG"),
                rct("EVENT_SCHEMA"),
                rct("EVENT_NAME"),
                rct("DEFINER"),
                rct("TIME_ZONE"),
                rct("EVENT_DEFINITION"),
            ],
            Vec::new(),
        ));
    }
    Some((vec![rct("dummy")], Vec::new()))
}

async fn handle_info_schema_schemata(
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let all_tables = get_all_tables(shared).await;
    let mut dbs: Vec<String> = all_tables
        .iter()
        .map(|(db, _, _, _)| db.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    dbs.sort();
    let columns = vec![
        rct("SCHEMA_NAME"),
        rct("DEFAULT_CHARACTER_SET_NAME"),
        rct("DEFAULT_COLLATION_NAME"),
    ];
    let mut rows = Vec::new();
    for db in &dbs {
        rows.push(vec![
            Value::Text(db.clone()),
            Value::Text("utf8mb4".to_string()),
            Value::Text("utf8mb4_general_ci".to_string()),
        ]);
    }
    if !dbs
        .iter()
        .any(|d| d.eq_ignore_ascii_case("information_schema"))
    {
        rows.push(vec![
            Value::Text("information_schema".to_string()),
            Value::Text("utf8".to_string()),
            Value::Text("utf8_general_ci".to_string()),
        ]);
    }
    Some((columns, rows))
}

async fn handle_info_schema_tables(
    sql: &str,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let schema_filter = extract_schema_filter(&upper, sql);
    let columns = vec![rct("TABLE_SCHEMA"), rct("TABLE_NAME"), rct("TABLE_TYPE")];
    let all_tables = get_all_tables(shared).await;
    let mut rows = Vec::new();
    for (db, table_name, _, _) in &all_tables {
        if let Some(ref filter) = schema_filter {
            if !db.eq_ignore_ascii_case(filter) {
                continue;
            }
        }
        rows.push(vec![
            Value::Text(db.clone()),
            Value::Text(table_name.clone()),
            Value::Text("BASE TABLE".to_string()),
        ]);
    }
    rows.sort_by(|a, b| {
        (value_as_str(&a[0]), value_as_str(&a[1])).cmp(&(value_as_str(&b[0]), value_as_str(&b[1])))
    });
    Some((columns, rows))
}

async fn handle_info_schema_tables_count(
    sql: &str,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let schema_filter = extract_schema_filter(&upper, sql);
    let all_tables = get_all_tables(shared).await;
    let table_count = if let Some(ref filter) = schema_filter {
        all_tables
            .iter()
            .filter(|(db, _, _, _)| db.eq_ignore_ascii_case(filter))
            .count()
    } else {
        all_tables.len()
    };
    let has_routines_union = upper.contains("ROUTINES");
    let columns = vec![rct("COUNT(*)")];
    let mut rows = Vec::new();
    rows.push(vec![Value::Int64(table_count as i64)]);
    if has_routines_union {
        rows.push(vec![Value::Int64(0)]);
    }
    Some((columns, rows))
}

async fn handle_info_schema_columns(
    sql: &str,
    shared: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
) -> Option<(Vec<ResultColumn>, Vec<Vec<Value>>)> {
    let upper = sql.to_uppercase();
    let schemas = extract_all_schema_filters(&upper, sql);
    let columns = vec![
        rct("TABLE_SCHEMA"),
        rct("TABLE_NAME"),
        rct("COLUMN_NAME"),
        rct("COLUMN_TYPE"),
    ];
    let all_tables = get_all_tables(shared).await;
    let mut rows = Vec::new();
    for (db, table_name, _, cols) in &all_tables {
        if !schemas.is_empty() {
            if !schemas.iter().any(|s| db.eq_ignore_ascii_case(s)) {
                continue;
            }
        }
        for col in cols {
            let mysql_type = column_type_to_mysql(&col.data_type);
            rows.push(vec![
                Value::Text(db.clone()),
                Value::Text(table_name.clone()),
                Value::Text(col.name.clone()),
                Value::Text(mysql_type),
            ]);
        }
    }
    rows.sort_by(|a, b| {
        (
            value_as_str(&a[0]),
            value_as_str(&a[1]),
            value_as_str(&a[2]),
        )
            .cmp(&(
                value_as_str(&b[0]),
                value_as_str(&b[1]),
                value_as_str(&b[2]),
            ))
    });
    Some((columns, rows))
}

fn mysql_like_match(text: &str, pattern: &str) -> bool {
    let text_chars: Vec<char> = text.chars().collect();
    let pattern_chars: Vec<char> = pattern.chars().collect();
    like_match_impl(&text_chars, &pattern_chars, 0, 0)
}

fn like_match_impl(text: &[char], pattern: &[char], ti: usize, pi: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }
    match pattern[pi] {
        '%' => {
            for skip in 0..=(text.len() - ti) {
                if like_match_impl(text, pattern, ti + skip, pi + 1) {
                    return true;
                }
            }
            false
        }
        '_' => {
            if ti < text.len() {
                like_match_impl(text, pattern, ti + 1, pi + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && text[ti].to_ascii_lowercase() == c.to_ascii_lowercase() {
                like_match_impl(text, pattern, ti + 1, pi + 1)
            } else {
                false
            }
        }
    }
}
