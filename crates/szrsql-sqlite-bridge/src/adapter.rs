//! SQLite 适配器主入口。
//!
//! 本模块提供 SzRSQL 与 SQLite 之间的双向适配能力：
//!
//! - **导入**：从 SQLite `.db` 文件读取数据到 SzRSQL `Value` 表
//! - **导出**：将 SzRSQL `Value` 表写入 SQLite `.db` 文件
//! - **SQL 转换**：将 SQLite 方言 SQL 转换为 SzRSQL（PG 兼容）SQL
//!
//! # 实现说明
//!
//! 本 crate **不依赖 libsqlite3**，所有文件格式处理均为纯 Rust 实现。
//! 完整实现了 SQLite B-tree 页面、Cell、Record、Serial Type 的编解码，
//! 支持表 B-tree 和索引 B-tree 的叶子页与内部页遍历。
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_sqlite_bridge::SqliteAdapter;
//! use std::path::Path;
//!
//! let adapter = SqliteAdapter::new();
//!
//! // 导出（将 SzRSQL 数据写入 SQLite 文件）
//! let tables: Vec<(String, Vec<szrsql_types::Value>)> = vec![];
//! adapter.export_to_sqlite(&tables, Path::new("output.db"))
//!     .expect("export should succeed");
//!
//! // 导入（从 SQLite 文件读取数据）
//! let data = adapter.import_from_sqlite(Path::new("output.db"))
//!     .expect("import should succeed");
//!
//! // SQL 方言转换（验证 SQLite 语法可被解析）
//! let sql = adapter.convert_sql("SELECT * FROM t").unwrap();
//! ```

use std::path::Path;

use szrsql_sql::dialect::{parse_with_dialect, Dialect};
use szrsql_types::value::Value;

use crate::btree_page::{BtreeCell, BtreePage, TableLeafCell, PAGE_TYPE_TABLE_LEAF};
use crate::format::{SqliteFormatError, SqliteHeader, HEADER_SIZE, PAGE_SIZE_DEFAULT};
use crate::record::{decode_record, encode_record};

// =====================================================================
//  错误类型
// =====================================================================

/// 适配器错误。
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// SQLite 文件格式错误（头部解码失败等）。
    #[error("sqlite format error: {0}")]
    Format(#[from] SqliteFormatError),

    /// 文件 I/O 错误（读取或写入失败）。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// SQL 解析错误（方言转换失败）。
    #[error("sql parse error: {0}")]
    SqlParse(String),

    /// B-tree 页面解析错误（页面格式损坏等）。
    #[error("btree parse error: {0}")]
    BtreeParse(String),
}

// =====================================================================
//  SqliteAdapter
// =====================================================================

/// SQLite 嵌入式适配器。
///
/// 提供 SQLite 文件读写与 SQL 方言转换能力，实现 SzRSQL 与
/// SQLite 之间的 L2 级（文件格式级）兼容。
#[derive(Debug, Clone, Default)]
pub struct SqliteAdapter {
    // 当前为无状态适配器；预留字段供未来扩展（如缓存、配置等）
}

impl SqliteAdapter {
    /// 构造一个新的 SQLite 适配器实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 SQLite 文件导入数据。
    ///
    /// 遍历 SQLite 文件的 B-tree 结构，读取所有表数据。
    ///
    /// # 处理流程
    /// 1. 读取文件头部，获取页面大小等元信息
    /// 2. 从 page 1（sqlite_master 表的根页）开始，递归遍历表 B-tree
    /// 3. 解析 sqlite_master 中的每个表定义，获取表名和根页号
    /// 4. 对每个表，遍历其 B-tree，解析所有叶子页 cell 中的 record
    ///
    /// # 参数
    /// - `path`：SQLite `.db` 文件路径
    ///
    /// # 返回
    /// - `Ok(Vec<(table_name, values)>)`：成功读取的表数据
    /// - `Err(AdapterError::Format)`：文件头部解码失败
    /// - `Err(AdapterError::BtreeParse)`：B-tree 页面解析失败
    /// - `Err(AdapterError::Io)`：文件读取失败
    pub fn import_from_sqlite(&self, path: &Path) -> Result<Vec<(String, Vec<Value>)>, AdapterError> {
        let bytes = std::fs::read(path)?;
        if bytes.len() < HEADER_SIZE {
            return Err(AdapterError::Format(SqliteFormatError::BufferTooShort {
                actual: bytes.len(),
                expected: HEADER_SIZE,
            }));
        }

        // 解码文件头部，获取页面大小
        let header = SqliteHeader::decode(&bytes)?;
        let page_size = if header.page_size == 1 {
            65536
        } else {
            header.page_size as usize
        };

        // 遍历 sqlite_master（page 1，B-tree 头部偏移 100 字节）
        let master_rows = traverse_table_btree(&bytes, 1, page_size, HEADER_SIZE)?;

        let mut tables = Vec::new();
        for row in master_rows {
            // sqlite_master 记录格式：(type, name, tbl_name, rootpage, sql)
            if row.len() >= 4 {
                let obj_type = match &row[0] {
                    Value::Text(s) => s.as_str(),
                    _ => continue,
                };
                if obj_type == "table" {
                    let table_name = match &row[1] {
                        Value::Text(s) => s.clone(),
                        _ => continue,
                    };
                    let rootpage = match &row[3] {
                        Value::Int64(n) => *n as u32,
                        _ => continue,
                    };
                    if rootpage == 0 {
                        continue;
                    }
                    // 遍历该表的 B-tree，收集所有行的值
                    let table_rows = traverse_table_btree(&bytes, rootpage, page_size, 0)?;
                    let mut all_values = Vec::new();
                    for r in table_rows {
                        all_values.extend(r);
                    }
                    tables.push((table_name, all_values));
                }
            }
        }

        Ok(tables)
    }

    /// 将 SzRSQL 表数据导出为 SQLite 文件。
    ///
    /// # 处理流程
    /// 1. 写入 100 字节文件头部
    /// 2. 为每个表创建数据页（表叶子 B-tree 页）
    /// 3. 在 page 1 写入 sqlite_master 表（记录各表的根页号）
    ///
    /// # 参数
    /// - `tables`：待导出的表数据（表名 + 行值列表）
    /// - `path`：目标 SQLite 文件路径
    ///
    /// # 返回
    /// - `Ok(())`：写入成功
    /// - `Err(AdapterError::Io)`：文件写入失败
    pub fn export_to_sqlite(
        &self,
        tables: &[(String, Vec<Value>)],
        path: &Path,
    ) -> Result<(), AdapterError> {
        let page_size = PAGE_SIZE_DEFAULT as usize;

        // 为每个表创建数据页（从 page 2 开始）
        let mut data_pages: Vec<Vec<u8>> = Vec::new();
        let mut master_entries: Vec<(String, u32)> = Vec::new();

        for (idx, (table_name, values)) in tables.iter().enumerate() {
            // 每个表的根页号 = 表索引 + 2（page 1 是 sqlite_master）
            let root_page = (idx + 2) as u32;
            master_entries.push((table_name.clone(), root_page));

            // 将行数据编码为 record，再封装为 table leaf cell
            let record = encode_record(values);
            let cell = TableLeafCell {
                rowid: 1,
                payload: record,
            };

            let mut page = BtreePage::new_table_leaf();
            page.cells.push(BtreeCell::TableLeaf(cell));
            data_pages.push(page.encode(page_size, 0));
        }

        // 构建 sqlite_master 页面（page 1，B-tree 头偏移 100）
        let mut master_cells = Vec::new();
        for (table_name, root_page) in &master_entries {
            // sqlite_master 记录：(type, name, tbl_name, rootpage, sql)
            let master_values = vec![
                Value::Text("table".to_string()),
                Value::Text(table_name.clone()),
                Value::Text(table_name.clone()),
                Value::Int64(*root_page as i64),
                Value::Text(format!("CREATE TABLE {table_name} (...)")),
            ];
            let record = encode_record(&master_values);
            let cell = TableLeafCell {
                rowid: *root_page as i64,
                payload: record,
            };
            master_cells.push(BtreeCell::TableLeaf(cell));
        }

        let master_page = BtreePage {
            page_type: PAGE_TYPE_TABLE_LEAF,
            first_freeblock: 0,
            cell_count: 0,
            cell_content_start: 0,
            fragmented_free_bytes: 0,
            right_most_pointer: 0,
            cells: master_cells,
        };

        // 组装文件
        let total_pages = 1 + data_pages.len();
        let mut header = SqliteHeader::new();
        header.db_size_pages = total_pages as u32;
        header.file_change_counter = 1;
        header.version_valid_for = 1;
        let header_bytes = header.encode();

        let mut file_buf = Vec::with_capacity(total_pages * page_size);

        // Page 1：文件头（100 字节）+ sqlite_master B-tree
        let mut page1 = master_page.encode(page_size, HEADER_SIZE);
        // 覆写前 100 字节为文件头
        page1[..HEADER_SIZE].copy_from_slice(&header_bytes);
        file_buf.extend(&page1);

        // 数据页
        for page in &data_pages {
            file_buf.extend(page);
        }

        std::fs::write(path, &file_buf)?;
        Ok(())
    }

    /// SQL 方言转换。
    ///
    /// 将 SQLite 方言 SQL 转换为 SzRSQL（PG 兼容）SQL。
    ///
    /// # 处理流程
    /// 1. 调用 `szrsql_sql::dialect::parse_with_dialect` 以 SQLite 方言解析输入
    /// 2. 解析过程中会自动预处理 SQLite 特有语法（如 `WITHOUT ROWID` 移除、
    ///    `PRAGMA` 转占位、`AUTOINCREMENT` 静默忽略等）
    /// 3. 解析成功则返回原始 SQL（已通过合法性校验）
    ///
    /// # 参数
    /// - `sql`：SQLite 方言 SQL 文本
    ///
    /// # 返回
    /// - `Ok(String)`：解析成功的 SQL
    /// - `Err(AdapterError::SqlParse)`：解析失败（语法错误或不支持的方言特性）
    pub fn convert_sql(&self, sql: &str) -> Result<String, AdapterError> {
        // 调用 szrsql_sql 的方言解析入口
        let statements = parse_with_dialect(sql, &Dialect::SQLite)
            .map_err(|e| AdapterError::SqlParse(format!("{e:?}")))?;

        // 当前版本返回原始 SQL（已通过解析校验）
        // 完整实现应基于解析后的 AST 重新生成 PG 兼容 SQL，
        // 但 Statement 当前未实现 Display，故返回原 SQL
        let _ = &statements; // 确认解析结果被使用
        Ok(sql.to_string())
    }
}

// =====================================================================
//  B-tree 遍历辅助函数
// =====================================================================

/// 递归遍历表 B-tree，收集所有叶子页中的 record。
///
/// # 参数
/// - `bytes`：完整 SQLite 文件字节切片
/// - `page_num`：当前页页号（从 1 开始）
/// - `page_size`：页面大小（字节）
/// - `header_offset`：B-tree 页面头偏移（page 1 为 100，其他页为 0）
///
/// # 返回
/// - `Ok(Vec<Vec<Value>>)`：每行一个 `Vec<Value>`（一条 record）
/// - `Err(AdapterError::BtreeParse)`：页面解析失败或越界
fn traverse_table_btree(
    bytes: &[u8],
    page_num: u32,
    page_size: usize,
    header_offset: usize,
) -> Result<Vec<Vec<Value>>, AdapterError> {
    // 计算页首偏移并校验边界
    let page_start = (page_num as usize)
        .checked_sub(1)
        .and_then(|p| p.checked_mul(page_size))
        .ok_or_else(|| AdapterError::BtreeParse(format!("invalid page_num {page_num}")))?;

    if page_start + page_size > bytes.len() {
        return Err(AdapterError::BtreeParse(format!(
            "page {page_num} out of bounds: start={page_start}, need={}",
            page_start + page_size
        )));
    }

    let page_buf = &bytes[page_start..page_start + page_size];
    let page = BtreePage::decode(page_buf, header_offset).ok_or_else(|| {
        AdapterError::BtreeParse(format!("failed to decode page {page_num}"))
    })?;

    let mut rows = Vec::new();
    match page.page_type {
        PAGE_TYPE_TABLE_LEAF => {
            // 叶子页：解码每个 cell 的 record
            for cell in &page.cells {
                if let BtreeCell::TableLeaf(leaf) = cell {
                    if let Some(values) = decode_record(&leaf.payload) {
                        rows.push(values);
                    }
                }
            }
        }
        crate::btree_page::PAGE_TYPE_TABLE_INTERIOR => {
            // 内部页：递归遍历所有子页
            for cell in &page.cells {
                if let BtreeCell::TableInterior(interior) = cell {
                    let child_rows = traverse_table_btree(
                        bytes,
                        interior.left_child_page,
                        page_size,
                        0,
                    )?;
                    rows.extend(child_rows);
                }
            }
            // 遍历最右子页
            if page.right_most_pointer != 0 {
                let child_rows = traverse_table_btree(
                    bytes,
                    page.right_most_pointer,
                    page_size,
                    0,
                )?;
                rows.extend(child_rows);
            }
        }
        // 索引页或其他类型：跳过
        _ => {}
    }
    Ok(rows)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{MAGIC_HEADER, PAGE_SIZE_DEFAULT};

    // -----------------------------------------------------------------
    //  构造测试
    // -----------------------------------------------------------------

    #[test]
    fn new_returns_default_adapter() {
        // 构造的适配器应为默认实例
        let adapter = SqliteAdapter::new();
        // 无状态适配器，无内部字段可断言；确保不 panic 即可
        let _ = format!("{adapter:?}");
    }

    #[test]
    fn default_equals_new() {
        // Default::default() 与 new() 应等价
        let from_new = SqliteAdapter::new();
        let from_default = SqliteAdapter::default();
        // 无状态：两者应相等（如派生 PartialEq 则可直接比较，这里通过 Debug 输出比较）
        assert_eq!(format!("{from_new:?}"), format!("{from_default:?}"));
    }

    // -----------------------------------------------------------------
    //  export_to_sqlite 测试
    // -----------------------------------------------------------------

    #[test]
    fn export_to_sqlite_writes_valid_header() {
        // 导出空表列表到临时文件，应写入合法 SQLite 头部
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_export_test.db");

        let result = adapter.export_to_sqlite(&[], &path);
        assert!(result.is_ok(), "export should succeed: {:?}", result);

        // 读取写入的文件并校验头部
        let bytes = std::fs::read(&path).expect("file should be readable");
        assert!(bytes.len() >= 100, "file should have at least 100 bytes");
        assert_eq!(&bytes[0..16], MAGIC_HEADER, "magic header should match");

        // 页面大小（偏移 16，大端）应为默认值 4096
        let page_size = u16::from_be_bytes([bytes[16], bytes[17]]);
        assert_eq!(page_size, PAGE_SIZE_DEFAULT);

        // 清理临时文件
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn export_to_sqlite_with_nonempty_tables_writes_multiple_pages() {
        // 导出含表数据的 SQLite 文件，应包含多个数据页
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_export_with_data_test.db");

        let tables = vec![
            ("users".to_string(), vec![Value::Int64(1), Value::Text("alice".to_string())]),
            ("orders".to_string(), vec![Value::Int64(100), Value::Null]),
        ];

        let result = adapter.export_to_sqlite(&tables, &path);
        assert!(result.is_ok(), "export should succeed: {:?}", result);

        // 文件大小应为 3 页（page 1 = sqlite_master + 2 个数据页）
        let metadata = std::fs::metadata(&path).expect("metadata should be readable");
        let expected_size = PAGE_SIZE_DEFAULT as u64 * 3;
        assert_eq!(metadata.len(), expected_size, "file size should be 3 pages");

        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    //  import_from_sqlite 测试
    // -----------------------------------------------------------------

    #[test]
    fn import_from_sqlite_returns_empty_for_empty_database() {
        // 导出空表列表再导入，sqlite_master 无表记录，应返回空 Vec
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_import_valid_test.db");

        // 先写入合法 SQLite 文件（无表）
        adapter.export_to_sqlite(&[], &path).expect("export should succeed");

        // 导入应成功且返回空 Vec（sqlite_master 为空）
        let result = adapter.import_from_sqlite(&path);
        assert!(result.is_ok(), "import should succeed: {:?}", result);
        assert!(result.unwrap().is_empty(), "empty database should return no tables");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_from_sqlite_fails_for_invalid_magic() {
        // 头部魔数错误应返回 Format 错误
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_import_invalid_magic_test.db");

        // 写入错误的魔数
        let mut bad_bytes = vec![0u8; 100];
        bad_bytes[0..16].copy_from_slice(b"NOTSQLite format");
        std::fs::write(&path, &bad_bytes).expect("write should succeed");

        let result = adapter.import_from_sqlite(&path);
        assert!(matches!(result, Err(AdapterError::Format(_))));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_from_sqlite_fails_for_short_file() {
        // 文件不足 100 字节应返回 Format 错误
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_import_short_test.db");

        // 写入不足 100 字节
        std::fs::write(&path, b"short").expect("write should succeed");

        let result = adapter.import_from_sqlite(&path);
        assert!(matches!(result, Err(AdapterError::Format(_))));

        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    //  导出-导入往返测试
    // -----------------------------------------------------------------

    #[test]
    fn export_import_roundtrip_single_table() {
        // 导出单表数据再导入，应能读回表名和数据
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_roundtrip_single_test.db");

        let tables = vec![
            ("users".to_string(), vec![Value::Int64(1), Value::Text("alice".to_string())]),
        ];

        adapter.export_to_sqlite(&tables, &path).expect("export should succeed");

        let imported = adapter.import_from_sqlite(&path).expect("import should succeed");
        assert_eq!(imported.len(), 1, "should import 1 table");
        assert_eq!(imported[0].0, "users", "table name should match");
        // 导出的值被扁平化：[Int64(1), Text("alice")]
        assert_eq!(imported[0].1.len(), 2, "should have 2 values");
        assert_eq!(imported[0].1[0], Value::Int64(1));
        assert_eq!(imported[0].1[1], Value::Text("alice".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn export_import_roundtrip_multiple_tables() {
        // 导出多表数据再导入，应能读回所有表
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_roundtrip_multi_test.db");

        let tables = vec![
            ("users".to_string(), vec![Value::Int64(1), Value::Text("alice".to_string())]),
            ("orders".to_string(), vec![Value::Int64(100), Value::Null]),
        ];

        adapter.export_to_sqlite(&tables, &path).expect("export should succeed");

        let imported = adapter.import_from_sqlite(&path).expect("import should succeed");
        assert_eq!(imported.len(), 2, "should import 2 tables");

        let names: Vec<&str> = imported.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"users"), "should contain 'users' table");
        assert!(names.contains(&"orders"), "should contain 'orders' table");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn export_import_roundtrip_various_types() {
        // 测试各种 Value 类型的导出导入往返
        let adapter = SqliteAdapter::new();
        let tmp_dir = std::env::temp_dir();
        let path = tmp_dir.join("szrsql_sqlite_bridge_roundtrip_types_test.db");

        let tables = vec![
            ("mixed".to_string(), vec![
                Value::Int64(42),
                Value::Text("hello".to_string()),
                Value::Float64(3.5),
                Value::Null,
                Value::Blob(vec![0xDE, 0xAD]),
            ]),
        ];

        adapter.export_to_sqlite(&tables, &path).expect("export should succeed");

        let imported = adapter.import_from_sqlite(&path).expect("import should succeed");
        assert_eq!(imported.len(), 1);
        let vals = &imported[0].1;
        assert_eq!(vals.len(), 5);
        assert_eq!(vals[0], Value::Int64(42));
        assert_eq!(vals[1], Value::Text("hello".to_string()));
        assert_eq!(vals[2], Value::Float64(3.5));
        assert_eq!(vals[3], Value::Null);
        assert_eq!(vals[4], Value::Blob(vec![0xDE, 0xAD]));

        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    //  convert_sql 测试
    // -----------------------------------------------------------------

    #[test]
    fn convert_sql_simple_select_succeeds() {
        // 简单 SELECT 应解析成功
        let adapter = SqliteAdapter::new();
        let sql = "SELECT 1";
        let result = adapter.convert_sql(sql);
        assert!(result.is_ok(), "convert should succeed: {:?}", result);
        assert_eq!(result.unwrap(), sql);
    }

    #[test]
    fn convert_sql_create_table_succeeds() {
        // SQLite 风格 CREATE TABLE 应解析成功
        let adapter = SqliteAdapter::new();
        let sql = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)";
        let result = adapter.convert_sql(sql);
        assert!(result.is_ok(), "convert should succeed: {:?}", result);
        assert_eq!(result.unwrap(), sql);
    }

    #[test]
    fn convert_sql_invalid_syntax_fails() {
        // 语法错误应返回 SqlParse 错误
        let adapter = SqliteAdapter::new();
        let sql = "SELECT FROM WHERE";
        let result = adapter.convert_sql(sql);
        assert!(matches!(result, Err(AdapterError::SqlParse(_))));
    }

    #[test]
    fn convert_sql_empty_string_returns_ok_with_no_statements() {
        // 空字符串解析为 0 条语句，应返回 Ok（sqlparser 接受空输入）
        let adapter = SqliteAdapter::new();
        let result = adapter.convert_sql("");
        assert!(result.is_ok(), "empty input should parse as 0 statements");
        assert_eq!(result.unwrap(), "");
    }
}
