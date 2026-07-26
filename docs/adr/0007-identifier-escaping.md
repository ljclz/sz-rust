# ADR-0007: Identifier Escaping

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: SQL注入防护
- **相关代码**: `crates/szrsql-sql/src/ast.rs (quote_ident function)`, `crates/szrsql-sql/src/online_ddl.rs (to_sql method)`
- **修复编号**: 无

## 背景

SzRSQL 的 DDL（Data Definition Language）需要在 SQL 执行时动态拼接标识符（表名、列名、索引名等），例如：
- `CREATE TABLE {name} (...)`
- `CREATE INDEX {index_name} ON {table} ({columns})`
- `ALTER TABLE {name} RENAME TO {new_name}`

这些标识符来源多样：
1. 用户输入（DDL 语句中的标识符）
2. 内部生成（如 online DDL 临时表名 `_szrsql_ongoing_{original}`）
3. 元数据查询（从 `information_schema` 读取后拼接）

**风险**：若直接拼接而不转义，存在二阶 SQL 注入风险。例如：
- 用户创建表名为 `t; DROP TABLE users; --` 的表
- 内部 DDL 拼接生成 `CREATE TABLE t; DROP TABLE users; -- (...)` → 注入
- 元数据中存储的标识符若被恶意构造，再次拼接时触发注入

不解决的后果：
- 二阶 SQL 注入可绕过应用层参数化查询（DDL 不支持预编译）
- 用户可执行任意 DDL，破坏数据库完整性
- 在线 DDL 工具（如 gh-ost 风格的影子表）可能被注入劫持

## 决策

实现 `quote_ident` 与 `is_valid_ident` 两个函数，所有 DDL 标识符拼接必须经过转义。

### 标识符验证（`is_valid_ident`）

```rust
// crates/szrsql-sql/src/ast.rs
pub fn is_valid_ident(name: &str) -> bool {
    if name.is_empty() || name.len() > 63 {
        return false;  // PostgreSQL 风格 63 字符限制
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;  // 首字符必须字母或下划线
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return false;  // 后续字符必须字母数字下划线
        }
    }
    true
}
```

### 标识符转义（`quote_ident`）

```rust
// crates/szrsql-sql/src/ast.rs
pub fn quote_ident(name: &str) -> String {
    // PostgreSQL 风格：双引号包裹，内部双引号双写
    let escaped = name.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}
```

### 调用约束（`to_sql` 方法）

所有 DDL 节点的 `to_sql()` 必须使用 `quote_ident`：

```rust
// crates/szrsql-sql/src/online_ddl.rs
impl ToSql for OnlineDdlStmt {
    fn to_sql(&self) -> String {
        match self {
            OnlineDdlStmt::CreateShadowTable { original, shadow } => {
                // 必须转义，避免注入
                format!(
                    "CREATE TABLE {} (LIKE {})",
                    quote_ident(shadow),
                    quote_ident(original)
                )
            }
            OnlineDdlStmt::SwapTable { a, b } => {
                format!("RENAME TABLE {} TO {}", quote_ident(a), quote_ident(b))
            }
        }
    }
}
```

### 防御层次

1. **白名单验证**：`is_valid_ident` 拒绝非字母数字下划线
2. **强制转义**：`quote_ident` 双引号包裹 + 内部双引号双写
3. **长度限制**：63 字符上限（对齐 PostgreSQL）
4. **代码审查**：grep 所有 `format!` 涉及 DDL 标识符的代码，确认调用 `quote_ident`

## 后果

**正面**：
- 彻底阻断 DDL 路径的二阶 SQL 注入
- 标识符规则明确，符合 PostgreSQL 习惯
- 在线 DDL 工具可安全使用动态拼接
- 代码审查可机械化检查（grep `format!` + 标识符变量）

**负面**：
- 标识符仅支持 ASCII 字母数字下划线，不支持 Unicode（未来可扩展）
- 63 字符限制可能与现有数据冲突（需迁移）
- 增加开发约束：每个 DDL 拼接必须经过 `quote_ident`

## 注意事项

### 调用方约束
- 所有 DDL 语句拼接标识符必须调用 `quote_ident`，禁止直接 `format!("{}", name)`
- 用户提供的标识符必须先 `is_valid_ident` 校验，拒绝非法字符
- 内部生成的标识符（如影子表名）也必须 `quote_ident`，避免与关键字冲突
- 标识符长度必须 ≤ 63 字符，超长直接报错

### 迁移路径
- 现有代码审查：grep `format!` + DDL 关键字（CREATE/ALTER/RENAME/DROP）
- 单元测试：每个 `to_sql()` 实现必须有 SQL 注入测试用例
- 未来如支持 Unicode 标识符：扩展 `is_valid_ident` 并保留 `quote_ident` 转义

### Bug 定位提示

**如果出现 SQL 注入（用户输入被执行为 SQL 语句）**：
1. **查 `to_sql()` 实现**：grep `to_sql` 在 `online_ddl.rs` / `ast.rs`，确认是否调用 `quote_ident`
2. **查 `format!` 调用**：`grep -r "format!" crates/szrsql-sql/src/` 找到所有 DDL 拼接，逐一审查
3. **查用户输入路径**：DDL 解析器是否调用 `is_valid_ident` 校验标识符

**如果出现标识符异常（表名/列名被截断或报错）**：
1. **查 `is_valid_ident` 校验**：长度 > 63 字符是否被拒绝，首字符是否合规
2. **查 `quote_ident` 转义**：内部双引号是否被双写，特殊字符是否被正确包裹
3. **查关键字冲突**：标识符是否与 SQL 关键字冲突（如 `order`、`user`），`quote_ident` 应能解决

**如果 DDL 执行报语法错误**：
1. **查 `to_sql()` 输出**：打印生成的 SQL，确认标识符是否被双引号包裹
2. **查转义规则**：双引号包裹的标识符内部双引号必须双写（`"` → `""`）
3. **可排除**：业务逻辑（DDL 语法错误通常是转义问题）

**如果在线 DDL 失败（影子表创建/交换失败）**：
1. **查影子表名生成**：`_szrsql_ongoing_{original}` 是否经过 `quote_ident`
2. **查原表名是否含特殊字符**：若原表名含双引号，影子表名拼接必须先 `quote_ident`
3. **查 RENAME 语句**：交换表的 RENAME 必须转义两侧表名

**如果元数据查询触发的 DDL 失败**：
1. **查元数据来源**：从 `information_schema` 读取的表名是否经过 `is_valid_ident`
2. **查拼接逻辑**：元数据表名拼接必须 `quote_ident`，避免历史脏数据触发注入
3. **可排除**：MVCC / Raft 层（DDL 注入是 SQL 层问题）
