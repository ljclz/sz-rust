//! SzRSQL 内部 AST — Phase 3.1 交付物。
//!
//! 设计原则：
//! - **比 sqlparser-rs AST 更精简**：只保留执行器需要的信息，去掉语法糖和方言细节
//! - **强类型**：使用 `szrsql_types::Value` 与 `ColumnType`，避免重复定义
//! - **不可变**：所有节点都是 `Clone + PartialEq + Debug`，方便测试断言
//! - **覆盖 PG 标准 SQL**：DDL（CREATE/DROP TABLE/INDEX）、DML（INSERT/UPDATE/DELETE/SELECT）、
//!   事务（BEGIN/COMMIT/ROLLBACK/SAVEPOINT）、表达式（字面量/列/二元/一元/函数/CASE/CAST/IN/BETWEEN/LIKE/IS NULL/EXISTS）
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.1。

use serde::{Deserialize, Serialize};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  顶层 Statement
// =====================================================================

/// SQL 语句 AST 节点
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Statement {
    /// CREATE TABLE / CREATE TEMPORARY TABLE
    CreateTable {
        /// 表名（含可选 schema 前缀）
        name: TableName,
        /// 列定义
        columns: Vec<ColumnDefinition>,
        /// 表级约束（PRIMARY KEY / UNIQUE / FOREIGN KEY / CHECK）
        constraints: Vec<TableConstraint>,
        /// IF NOT EXISTS
        if_not_exists: bool,
        /// 是否为临时表 — Phase 3.28
        ///
        /// true 表示 `CREATE TEMPORARY TABLE`，表会话级隔离，断开自动删除。
        /// false 表示普通 `CREATE TABLE`。
        temporary: bool,
        /// ON COMMIT 行为 — Phase 3.28
        ///
        /// 仅对 temporary=true 的表有效。
        /// - None：默认（PG 等价于 ON COMMIT PRESERVE ROWS）
        /// - Some(OnCommitAction::DeleteRows)：ON COMMIT DELETE ROWS
        /// - Some(OnCommitAction::PreserveRows)：ON COMMIT PRESERVE ROWS
        /// - Some(OnCommitAction::Drop)：ON COMMIT DROP
        on_commit: Option<OnCommitAction>,
    },
    /// DROP TABLE
    DropTable {
        /// 待删除的表名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT
        cascade: bool,
    },
    /// CREATE INDEX
    CreateIndex {
        /// 索引名（可选）
        name: Option<String>,
        /// 表名
        table: TableName,
        /// 索引列
        columns: Vec<IndexColumn>,
        /// UNIQUE 索引
        unique: bool,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// DROP INDEX
    DropIndex {
        /// 索引名列表
        names: Vec<String>,
        /// IF EXISTS
        if_exists: bool,
    },
    /// TRUNCATE TABLE — 清空表数据（保留表结构）
    ///
    /// 等价于 `DELETE FROM table` 但不触发触发器、不记录逐行 WAL，
    /// 通常通过重建表文件实现，性能远高于 DELETE。
    /// 各方言均支持：PG/MySQL/Oracle/SQL Server/SQLite。
    Truncate {
        /// 待清空的表名列表
        names: Vec<TableName>,
        /// IF EXISTS（PG/MySQL 支持）
        if_exists: bool,
        /// CASCADE / RESTRICT（PG/Oracle 支持，当前仅记录）
        cascade: bool,
    },
    /// CREATE SEQUENCE — Phase 3.22
    CreateSequence {
        /// 序列名
        name: TableName,
        /// IF NOT EXISTS
        if_not_exists: bool,
        /// 起始值（默认 1）
        start: i64,
        /// 步长（默认 1）
        increment: i64,
        /// 最小值（None 表示使用类型默认下界）
        min_value: Option<i64>,
        /// 最大值（None 表示使用类型默认上界）
        max_value: Option<i64>,
        /// 是否循环（达到 max 后回到 min）
        cycle: bool,
    },
    /// DROP SEQUENCE — Phase 3.22
    DropSequence {
        /// 序列名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录，不实际级联）
        cascade: bool,
    },
    /// INSERT INTO
    Insert {
        /// 目标表
        table: TableName,
        /// 显式列名（None 表示全部列）
        columns: Option<Vec<String>>,
        /// 数据源
        source: InsertSource,
        /// ON CONFLICT 处理
        on_conflict: Option<OnConflict>,
        /// RETURNING 子句
        returning: Option<Vec<SelectItem>>,
    },
    /// REPLACE INTO — MySQL 扩展（Phase 3.25）
    ///
    /// 行为与 MySQL 一致：
    /// - 主键/UNIQUE 冲突时 DELETE 旧行 + INSERT 新行（受影响行数 = 2）
    /// - 无冲突时直接 INSERT（受影响行数 = 1）
    /// - 不支持 RETURNING（MySQL 不支持）
    Replace {
        /// 目标表
        table: TableName,
        /// 显式列名（None 表示全部列）
        columns: Option<Vec<String>>,
        /// 数据源（与 INSERT 共用）
        source: InsertSource,
    },
    /// UPDATE
    Update {
        /// 目标表
        table: TableName,
        /// 别名
        alias: Option<String>,
        /// SET 赋值
        assignments: Vec<Assignment>,
        /// WHERE 条件
        where_clause: Option<Expr>,
        /// FROM 子句（PG 扩展）
        from: Vec<TableFactor>,
        /// RETURNING 子句
        returning: Option<Vec<SelectItem>>,
    },
    /// DELETE
    Delete {
        /// 目标表
        table: TableName,
        /// 别名
        alias: Option<String>,
        /// USING 子句（PG 扩展）
        using: Vec<TableFactor>,
        /// WHERE 条件
        where_clause: Option<Expr>,
        /// RETURNING 子句
        returning: Option<Vec<SelectItem>>,
    },
    /// SELECT
    Select(Box<Select>),
    /// CREATE VIEW / CREATE MATERIALIZED VIEW — Phase 6.10
    ///
    /// `materialized=true` 表示 `CREATE MATERIALIZED VIEW`，会在 catalog 中
    /// 同时注册为可扫描的表（与 PG 行为一致）；`materialized=false` 表示普通
    /// `CREATE VIEW`，仅存储查询定义，查询时展开为子查询。
    CreateView {
        /// 视图名（含可选 schema 前缀）
        name: TableName,
        /// 显式列别名（`CREATE VIEW v (a, b) AS ...`）；空 Vec 表示未指定
        columns: Vec<String>,
        /// 视图查询体
        query: Box<Select>,
        /// 是否为物化视图
        materialized: bool,
        /// IF NOT EXISTS
        if_not_exists: bool,
        /// OR REPLACE（CREATE OR REPLACE VIEW）
        or_replace: bool,
    },
    /// DROP VIEW / DROP MATERIALIZED VIEW — Phase 6.10
    DropView {
        /// 视图名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录，不实际级联）
        cascade: bool,
        /// 是否为物化视图
        materialized: bool,
    },
    /// REFRESH MATERIALIZED VIEW — Phase 6.10
    ///
    /// 重新执行物化视图的查询，将结果写入物化视图的存储表。
    RefreshMaterializedView {
        /// 物化视图名
        name: TableName,
        /// WITH DATA / WITH NO DATA（当前仅记录，统一按 WITH DATA 处理）
        with_data: bool,
    },
    /// BEGIN / START TRANSACTION
    Begin {
        /// 事务隔离级别
        isolation: Option<TransactionIsolation>,
        /// 事务访问模式
        access: Option<TransactionAccess>,
    },
    /// COMMIT
    Commit,
    /// ROLLBACK（可选回滚到 SAVEPOINT）
    Rollback {
        /// 回滚到指定 SAVEPOINT
        savepoint: Option<String>,
    },
    /// SAVEPOINT name
    Savepoint(String),
    /// RELEASE [SAVEPOINT] name
    ReleaseSavepoint(String),
    /// SET TRANSACTION ISOLATION LEVEL ...
    SetTransaction {
        /// 隔离级别
        isolation: Option<TransactionIsolation>,
        /// 访问模式
        access: Option<TransactionAccess>,
    },
    /// EXPLAIN / EXPLAIN ANALYZE
    Explain {
        /// 被分析的语句
        statement: Box<Statement>,
        /// ANALYZE 标志（实际执行）
        analyze: bool,
        /// VERBOSE 标志
        verbose: bool,
    },
    /// MERGE INTO target USING source ON condition WHEN ... THEN ...
    ///
    /// 行为与 SQL:2003 标准一致：
    /// - WHEN MATCHED THEN UPDATE SET ... — 匹配时更新
    /// - WHEN MATCHED THEN DELETE — 匹配时删除
    /// - WHEN NOT MATCHED THEN INSERT ... — 不匹配时插入
    /// - WHEN NOT MATCHED BY SOURCE THEN DELETE/UPDATE — 目标表存在但源表无匹配时操作
    Merge {
        /// 目标表（MERGE INTO t）
        target: TableName,
        /// 目标表别名
        target_alias: Option<String>,
        /// 源表（USING s）
        source: TableFactor,
        /// ON 条件
        on: Expr,
        /// WHEN 子句列表
        clauses: Vec<MergeClause>,
    },
    /// PREPARE name [ (data_types...) ] AS statement — Phase 3.26
    ///
    /// 行为与 PG 一致：
    /// - 创建命名预处理语句，存储 AST 供后续 EXECUTE 调用
    /// - 可选参数类型声明（用于类型校验，当前仅记录不强制）
    /// - 参数占位符 `$1`、`$2` ... 在 EXECUTE 时被实际值替换
    Prepare {
        /// 预处理语句名
        name: String,
        /// 参数类型声明（None 表示未声明）
        parameter_types: Vec<ColumnType>,
        /// 被预处理的 SQL 语句
        statement: Box<Statement>,
    },
    /// EXECUTE name [ (params...) ] — Phase 3.26
    ///
    /// 行为与 PG 一致：
    /// - 用实际参数值替换 `$1`、`$2` ... 后执行已预处理的语句
    /// - 参数数量必须与 PREPARE 中的占位符数量一致
    Execute {
        /// 预处理语句名
        name: String,
        /// 实际参数值列表（按 $1, $2, ... 顺序）
        parameters: Vec<Expr>,
    },
    /// DEALLOCATE [PREPARE] { name | ALL } — Phase 3.26
    ///
    /// 行为与 PG 一致：
    /// - 删除指定名称的预处理语句
    /// - DEALLOCATE ALL 删除所有预处理语句
    Deallocate {
        /// 待删除的预处理语句名（None 表示 DEALLOCATE ALL）
        name: Option<String>,
    },
    /// CREATE TYPE name AS ENUM (...) — Phase 3.31
    ///
    /// 行为与 PG 一致：
    /// - 创建命名枚举类型，存储到 Catalog 的 `enum_types` 表
    /// - 后续 `CREATE TABLE t (c name)` 可引用此类型作为列类型
    /// - `if_not_exists=true` 时若类型已存在则跳过（不报错）
    CreateType {
        /// 类型名（含可选 schema 前缀）
        name: TableName,
        /// ENUM 标签值列表（按声明顺序）
        as_enum: Vec<String>,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// DROP TYPE — Phase 3.31
    ///
    /// 行为与 PG 一致：
    /// - 从 Catalog 移除命名类型
    /// - `if_exists=true` 时若类型不存在则跳过（不报错）
    /// - `cascade=true` 时级联删除引用此类型的列（当前仅记录，不实际级联）
    DropType {
        /// 待删除的类型名列表
        names: Vec<TableName>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT
        cascade: bool,
    },
    /// ALTER TYPE name action — Phase 3.31
    ///
    /// 行为与 PG 一致：
    /// - `ADD VALUE 'val'` — 向枚举类型追加新值（不允许已存在）
    /// - `ADD VALUE IF NOT EXISTS 'val'` — 若已存在则跳过
    /// - `RENAME VALUE 'old' TO 'new'` — 重命名枚举值
    /// - `RENAME TO new_name` — 重命名类型
    AlterType {
        /// 类型名
        name: TableName,
        /// 操作
        action: AlterTypeAction,
    },
    /// ALTER TABLE — Phase F-10
    ///
    /// 支持 PG/MySQL/Oracle/SQL Server/SQLite 通用的 ALTER TABLE 操作：
    /// - `ADD COLUMN [IF NOT EXISTS] col TYPE [options]`
    /// - `DROP COLUMN [IF EXISTS] col [CASCADE]`
    /// - `RENAME COLUMN old TO new`
    /// - `RENAME TO new_table`
    /// - `ALTER COLUMN col TYPE new_type`
    /// - `ALTER COLUMN col SET DEFAULT expr`
    /// - `ALTER COLUMN col DROP DEFAULT`
    /// - `ALTER COLUMN col SET NOT NULL`
    /// - `ALTER COLUMN col DROP NOT NULL`
    AlterTable {
        /// 目标表名
        name: TableName,
        /// IF EXISTS（表不存在时是否跳过）
        if_exists: bool,
        /// 仅作用于表本身（PG `ALTER TABLE ONLY`，不影响子表）
        only: bool,
        /// 操作列表（按顺序执行）
        operations: Vec<AlterTableOperation>,
    },
    /// SHOW TABLES — Phase 3.34
    ///
    /// 行为与 MySQL 一致：列出当前 catalog 中所有表名。
    /// 返回单列 `Tables_in_<db>` 的结果集，每行一个表名。
    ShowTables,
    /// SHOW CREATE TABLE name — Phase 3.34
    ///
    /// 行为与 MySQL 一致：输出指定表的 DDL 重建语句。
    /// 返回两列：`Table`（表名）和 `Create Table`（DDL 文本）。
    ShowCreateTable {
        /// 目标表名
        name: TableName,
    },
    /// SET NAMES 'charset' [COLLATE 'collation'] — Phase 3.34
    ///
    /// 行为与 MySQL 一致：设置会话字符集和可选 collation。
    /// 当前实现记录到 `SessionState`，影响后续字符串处理（简化：仅存储不强制）。
    SetNames {
        /// 字符集名称（如 `utf8mb4`）
        charset: String,
        /// 可选 collation 名称
        collation: Option<String>,
    },
    /// SET variable = value — Phase 3.34
    ///
    /// 行为与 PG/MySQL 一致：设置会话参数（如 `statement_timeout`、`search_path`）。
    /// 当前实现记录到 `SessionState`，影响后续执行（简化：仅存储不强制）。
    SetVariable {
        /// 参数名（如 `statement_timeout`）
        variable: String,
        /// 参数值表达式（求值后存储到 SessionState）
        value: Expr,
    },
    /// SHOW variable — Phase 3.34
    ///
    /// 行为与 PG 一致：显示会话参数当前值。
    /// 返回单列 `setting` 的结果集，单行包含参数值文本。
    ShowVariable {
        /// 参数名
        variable: String,
    },
    /// FLASHBACK TRANSACTION <txn_id> — Phase 3.35
    ///
    /// 行为与 Oracle Flashback Transaction 类似：
    /// - 撤销指定已提交事务的所有修改，将相关表恢复到事务开始前的状态
    /// - 仅能闪回已提交事务；未提交事务或已闪回事务会报错
    /// - 闪回是物理恢复：直接用事务前快照替换当前表内容（非反向 DML）
    FlashbackTransaction {
        /// 事务 ID（由 TransactionHistory 在 COMMIT 时分配）
        txn_id: u64,
    },
    /// FLASHBACK TABLE <name> TO TIMESTAMP '<ts>' — Phase 3.35
    ///
    /// 行为与 Oracle Flashback Query 类似：
    /// - 查询指定表在某个历史时间点的状态
    /// - 找到 commit_ts <= timestamp 的最近一个事务，返回该事务"事务前"的表快照
    /// - 若无符合条件的事务，返回空结果集
    FlashbackTable {
        /// 目标表名
        table: TableName,
        /// 时间戳字符串（ISO 8601 或可解析格式）
        timestamp: String,
    },
    /// LISTEN <channel> — Phase 4.6
    ///
    /// 行为与 PG 一致：注册当前会话监听指定频道。
    /// 后续 NOTIFY <channel> 时，监听该频道的会话将收到 NotificationResponse。
    Listen {
        /// 频道名
        channel: String,
    },
    /// UNLISTEN <channel> / UNLISTEN * — Phase 4.6
    ///
    /// 行为与 PG 一致：
    /// - UNLISTEN <channel>：取消监听指定频道
    /// - UNLISTEN *：取消监听所有频道
    Unlisten {
        /// 频道名；`*` 表示取消所有
        channel: String,
    },
    /// NOTIFY <channel> [, <payload>] — Phase 4.6
    ///
    /// 行为与 PG 一致：向指定频道发送通知。
    /// 所有监听该频道的会话（含当前会话）将收到 NotificationResponse。
    Notify {
        /// 频道名
        channel: String,
        /// 负载字符串（可选，PG 中默认为空字符串）
        payload: String,
    },
    /// COPY FROM / COPY TO — Phase 4.8
    ///
    /// 支持 PostgreSQL COPY 语法子集：
    /// - `COPY table [(col1, col2, ...)] FROM '/path/to/file' [WITH (...)]`
    /// - `COPY table [(col1, col2, ...)] TO '/path/to/file' [WITH (...)]`
    /// - `COPY (SELECT ...) TO '/path/to/file' [WITH (...)]`
    ///
    /// 限制：
    /// - 不支持 COPY FROM STDIN / COPY TO STDOUT（pgwire 协议层未实现）
    /// - 不支持 PROGRAM
    /// - 不支持 BINARY 格式
    /// - 不支持 FORCE_QUOTE / FORCE_NOT_NULL / FORCE_NULL
    /// - 不支持 FREEZE / ENCODING
    Copy {
        /// 目标：表名或 SELECT 查询
        target: CopyTarget,
        /// 列列表（可选，仅 COPY table FROM/TO 时有效）
        columns: Option<Vec<String>>,
        /// 方向：FROM（导入）或 TO（导出）
        direction: CopyDirection,
        /// 文件路径
        file_path: String,
        /// 格式选项
        options: CopyOptions,
    },
    /// CREATE TRIGGER — Phase 6.4
    ///
    /// PG 兼容语法：
    /// ```sql
    /// CREATE [OR REPLACE] [CONSTRAINT] TRIGGER name
    ///   { BEFORE | AFTER | INSTEAD OF } { event [OR ...] }
    ///   ON table_name
    ///   [FROM referenced_table_name]
    ///   [ { NOT DEFERRABLE | [DEFERRABLE] [INITIALLY IMMEDIATE|DEFERRED] } ]
    ///   [ FOR [EACH] { ROW | STATEMENT } ]
    ///   [ WHEN (condition) ]
    ///   EXECUTE { FUNCTION | PROCEDURE } func_name ( [args] )
    /// ```
    ///
    /// 注：Phase 6.4 仅支持 Rust 内建触发器函数（通过 `TriggerRegistry` 注册）；
    /// PL/pgSQL 函数体将留待 Phase 6.5/6.6 实现。
    CreateTrigger {
        /// 触发器定义
        definition: TriggerDefinition,
        /// OR REPLACE
        or_replace: bool,
        /// IF NOT EXISTS（PG 不支持，但部分方言支持）
        if_not_exists: bool,
    },
    /// DROP TRIGGER — Phase 6.4
    DropTrigger {
        /// 触发器名
        name: String,
        /// 所属表名
        table: TableName,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// CREATE FUNCTION — Phase 6.5
    ///
    /// PG 兼容语法（简化）：
    /// ```sql
    /// CREATE [OR REPLACE] FUNCTION name ( [argmode] [argname] argtype [DEFAULT default] [, ...] )
    ///   RETURNS rettype
    ///   LANGUAGE lang_name
    ///   [ IMMUTABLE | STABLE | VOLATILE ]
    ///   [ STRICT ]
    ///   [ SECURITY DEFINER | SECURITY INVOKER ]
    ///   AS $$ body $$
    /// ```
    ///
    /// 注：Phase 6.5 仅解析与存储函数定义；PL/pgSQL 函数体执行留待 Phase 6.6 实现。
    /// `body` 为函数体原文（已剥离 `$$` / `'` 等定界符），执行时由
    /// `plpgsql::parse_function_body` 懒解析为 `PlPgSqlBlock`。
    CreateFunction {
        /// 函数名（含可选 schema 前缀，如 `public.my_func`）
        name: String,
        /// 参数列表（按声明顺序）
        parameters: Vec<FunctionParameter>,
        /// 返回类型原文（如 `integer`、`void`、`TABLE(...)`）
        return_type: String,
        /// 函数语言（如 `plpgsql`、`sql`；Phase 6.5 仅 `plpgsql` 由解释器执行）
        language: String,
        /// 函数体原文（`$$ ... $$` / `'...'` 内部内容）
        body: String,
        /// OR REPLACE
        or_replace: bool,
        /// 函数波动性（IMMUTABLE / STABLE / VOLATILE；None 表示默认 VOLATILE）
        volatility: Option<FunctionVolatility>,
        /// STRICT（又称 RETURNS NULL ON NULL INPUT）
        strict: bool,
        /// SECURITY DEFINER（true）或 SECURITY INVOKER（false，默认）
        security_definer: bool,
    },
    /// DROP FUNCTION — Phase 6.5
    DropFunction {
        /// 函数名（含可选 schema 前缀）
        name: String,
        /// 参数类型列表（用于重载解析；为空表示不限参数签名）
        parameter_types: Vec<String>,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// COMMENT ON TABLE/COLUMN — Phase TDengine-P2
    Comment {
        /// 对象类型（TABLE / COLUMN）
        object_type: CommentObjectType,
        /// 对象名（表名）
        object_name: TableName,
        /// 列名（仅 COLUMN 时有值）
        column_name: Option<String>,
        /// 注释内容（NULL 表示删除注释）
        comment: Option<String>,
    },
    /// ANALYZE [ table_name [, ...] ] — P2-1
    ///
    /// 行为与 PostgreSQL 一致：
    /// - 收集指定表（或所有表）的统计信息（行数、列基数、NULL 比例、直方图等）
    /// - 统计结果存入 StatisticsStore，供 CostModel 进行基于成本的优化
    /// - 无指定表时分析所有用户表
    Analyze {
        /// 目标表列表（空表示分析所有用户表）
        tables: Vec<TableName>,
        /// VERBOSE 标志（控制输出详细程度，当前仅记录日志）
        verbose: bool,
    },
}

/// 函数参数 — Phase 6.5
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionParameter {
    /// 参数模式（IN / OUT / INOUT / VARIADIC；None 表示默认 IN）
    pub mode: Option<FunctionArgMode>,
    /// 参数名（PG 允许匿名参数，故为 Option）
    pub name: Option<String>,
    /// 参数数据类型原文（如 `integer`、`text`、`my_type%TYPE`）
    pub data_type: String,
    /// 默认值表达式原文（`DEFAULT expr` 或 `= expr`）
    pub default_expr: Option<String>,
}

/// 函数参数模式 — Phase 6.5
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionArgMode {
    /// IN（默认）
    In,
    /// OUT
    Out,
    /// INOUT
    InOut,
    /// VARIADIC
    Variadic,
}

/// 函数波动性 — Phase 6.5
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionVolatility {
    /// IMMUTABLE：相同参数永远返回相同结果，可被预计算
    Immutable,
    /// STABLE：同一事务内相同参数返回相同结果
    Stable,
    /// VOLATILE（默认）：可能每次调用都不同
    Volatile,
}

/// 触发器时机 — Phase 6.4
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerTiming {
    /// BEFORE：DML 之前触发（可修改 NEW 行或阻止操作）
    Before,
    /// AFTER：DML 之后触发（用于审计/级联）
    After,
    /// INSTEAD OF：替代 DML 执行（主要用于视图）
    InsteadOf,
}

/// 触发器级别 — Phase 6.4
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerLevel {
    /// FOR EACH ROW — 每行触发一次
    Row,
    /// FOR EACH STATEMENT — 每语句触发一次（默认）
    Statement,
}

/// 触发器事件 — Phase 6.4
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerEvent {
    /// INSERT
    Insert,
    /// UPDATE [OF col1, col2, ...] — 列列表为空表示任意列更新
    Update(Vec<String>),
    /// DELETE
    Delete,
    /// TRUNCATE
    Truncate,
}

/// 触发器定义 — Phase 6.4
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerDefinition {
    /// 触发器名
    pub name: String,
    /// 所属表名
    pub table: TableName,
    /// 时机（BEFORE / AFTER / INSTEAD OF）
    pub timing: TriggerTiming,
    /// 级别（ROW / STATEMENT）
    pub level: TriggerLevel,
    /// 事件列表（支持 INSERT OR UPDATE OR DELETE）
    pub events: Vec<TriggerEvent>,
    /// WHEN 条件（仅 Row 级别有效；Statement 级别 PG 不支持）
    pub when_clause: Option<Expr>,
    /// 触发器函数名（EXECUTE FUNCTION func_name）
    pub func_name: String,
    /// 触发器函数参数（PG 触发器函数通常无参数，但语法允许）
    pub func_args: Vec<Expr>,
    /// 是否启用（CREATE TRIGGER 默认启用；ALTER TRIGGER ... DISABLE 可禁用）
    pub enabled: bool,
    /// 是否为 CONSTRAINT 触发器（DEFERRABLE）
    pub is_constraint: bool,
}

/// COPY 目标 — Phase 4.8
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CopyTarget {
    /// 表名
    Table(TableName),
    /// SELECT 查询（仅 COPY TO 支持）
    Query(Box<Select>),
}

/// COPY 方向 — Phase 4.8
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyDirection {
    /// 导入：文件 → 表
    From,
    /// 导出：表/查询 → 文件
    To,
}

/// COPY 格式 — Phase 4.8
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyFormat {
    /// CSV 格式（RFC 4180 子集）
    Csv,
    /// TEXT 格式（PG 默认，TAB 分隔）
    Text,
}

/// COPY 选项 — Phase 4.8
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyOptions {
    /// 格式（CSV 或 TEXT，默认 TEXT）
    pub format: CopyFormat,
    /// 是否包含表头（CSV 默认 false；TEXT 不适用，固定无表头）
    pub header: bool,
    /// 字段分隔符（CSV 默认 ','，TEXT 默认 '\t'）
    pub delimiter: char,
    /// 引用字符（仅 CSV，默认 '"'）
    pub quote: char,
    /// 转义字符（仅 CSV，默认与 quote 相同）
    pub escape: char,
    /// NULL 字符串（TEXT 默认 "\N"，CSV 默认空字符串 ""）
    pub null_string: String,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            format: CopyFormat::Text,
            header: false,
            delimiter: '\t',
            quote: '"',
            escape: '"',
            null_string: "\\N".to_string(),
        }
    }
}

impl CopyOptions {
    /// 应用 FORMAT csv 选项后的默认值（delimiter=',', header=false, null_string=""）
    pub fn csv_defaults() -> Self {
        Self {
            format: CopyFormat::Csv,
            header: false,
            delimiter: ',',
            quote: '"',
            escape: '"',
            null_string: String::new(),
        }
    }
}

/// ALTER TYPE 操作 — Phase 3.31
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AlterTypeAction {
    /// ADD VALUE [IF NOT EXISTS] value [BEFORE | AFTER existing]
    AddValue {
        /// 新值
        value: String,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// RENAME VALUE old TO new
    RenameValue {
        /// 旧值
        old: String,
        /// 新值
        new: String,
    },
    /// RENAME TO new_name
    Rename {
        /// 新类型名
        new_name: TableName,
    },
}

/// ALTER TABLE 操作 — Phase F-10
///
/// 覆盖 PG/MySQL/Oracle/SQL Server/SQLite 通用的 ALTER TABLE 操作。
/// 执行器按 operations 列表顺序依次执行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum AlterTableOperation {
    /// `ADD [COLUMN] [IF NOT EXISTS] <column_def>`
    AddColumn {
        /// 列定义
        column_def: ColumnDefinition,
        /// IF NOT EXISTS
        if_not_exists: bool,
    },
    /// `DROP [COLUMN] [IF EXISTS] <name> [CASCADE]`
    DropColumn {
        /// 列名
        name: String,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT（当前仅记录）
        cascade: bool,
    },
    /// `RENAME COLUMN old TO new`
    RenameColumn {
        /// 旧列名
        old_name: String,
        /// 新列名
        new_name: String,
    },
    /// `RENAME TO new_table`
    RenameTable {
        /// 新表名
        new_name: TableName,
    },
    /// `ALTER COLUMN name TYPE new_type [USING expr]`
    AlterColumnType {
        /// 列名
        name: String,
        /// 新类型
        data_type: ColumnType,
        /// USING expr（当前仅记录，不实际执行表达式转换）
        using: Option<Expr>,
    },
    /// `ALTER COLUMN name SET DEFAULT expr` / `DROP DEFAULT`
    AlterColumnDefault {
        /// 列名
        name: String,
        /// None 表示 DROP DEFAULT
        default: Option<Expr>,
    },
    /// `ALTER COLUMN name SET NOT NULL` / `DROP NOT NULL`
    AlterColumnNotNull {
        /// 列名
        name: String,
        /// true 表示 SET NOT NULL，false 表示 DROP NOT NULL
        not_null: bool,
    },
    /// `ADD <table_constraint>` — 主键/唯一/外键/CHECK
    AddConstraint {
        /// 表级约束
        constraint: TableConstraint,
    },
    /// `DROP CONSTRAINT [IF EXISTS] name [CASCADE]`
    DropConstraint {
        /// 约束名
        name: String,
        /// IF EXISTS
        if_exists: bool,
        /// CASCADE / RESTRICT
        cascade: bool,
    },
}

// =====================================================================
//  MERGE 辅助类型
// =====================================================================

/// MERGE 的 WHEN 子句类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MergeClauseKind {
    /// WHEN MATCHED — 目标行与源行匹配
    Matched,
    /// WHEN NOT MATCHED — 源行在目标中无匹配（标准 SQL:2003）
    NotMatched,
    /// WHEN NOT MATCHED BY SOURCE — 目标行在源中无匹配（BigQuery/PG 扩展）
    NotMatchedBySource,
}

/// MERGE 的 WHEN 子句动作
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MergeAction {
    /// INSERT (cols) VALUES (vals) 或 INSERT VALUES (vals)
    Insert {
        /// 列名列表（可为空，表示按表顺序）
        columns: Vec<String>,
        /// VALUES 表达式列表（单行）
        values: Vec<Expr>,
    },
    /// UPDATE SET col = expr, ...
    Update {
        /// SET 赋值
        assignments: Vec<Assignment>,
    },
    /// DELETE
    Delete,
}

/// MERGE 的 WHEN 子句
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeClause {
    /// 子句类型（MATCHED / NOT MATCHED / NOT MATCHED BY SOURCE）
    pub kind: MergeClauseKind,
    /// 可选的 AND 谓词
    pub predicate: Option<Expr>,
    /// 动作
    pub action: MergeAction,
}

// =====================================================================
//  DDL 辅助类型
// =====================================================================

/// 表名（含可选 schema 前缀）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableName {
    /// schema 名称（如 `public`、`my_schema`），None 时使用默认 schema
    pub schema: Option<String>,
    /// 表名
    pub name: String,
}

impl TableName {
    /// 创建无 schema 的表名
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    /// 创建带 schema 的表名
    pub fn with_schema(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// 全限定名（`schema.name` 或 `name`）
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", s, self.name),
            None => self.name.clone(),
        }
    }

    /// 返回经过 SQL 标识符转义后的全限定名（用于拼接 DDL/DML SQL 字符串）。
    ///
    /// 每个组成部分均通过 [`quote_ident`] 转义，避免二阶 SQL 注入。
    /// 例如 `schema="public", name="users"` → `"public"."users"`，
    /// `name="user name"` → `"user name"`，`name='a"b'` → `"a""b"`。
    pub fn quoted_qualified_name(&self) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", quote_ident(s), quote_ident(&self.name)),
            None => quote_ident(&self.name),
        }
    }
}

// =====================================================================
//  标识符转义工具（SQL 注入防护）
// =====================================================================

/// 对 SQL 标识符进行转义并使用双引号包裹，符合 PostgreSQL 标识符引用规则。
///
/// # 规则
///
/// 1. 外层使用双引号 `"` 包裹
/// 2. 内部双引号 `"` 转义为 `""`
/// 3. 空标识符返回 `""`（合法的 PostgreSQL 空标识符）
///
/// # 示例
///
/// ```
/// use szrsql_sql::ast::quote_ident;
/// assert_eq!(quote_ident("users"), "\"users\"");
/// assert_eq!(quote_ident("user name"), "\"user name\"");
/// assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
/// assert_eq!(quote_ident(""), "\"\"");
/// ```
///
/// # 安全说明
///
/// 所有来自用户输入或元数据的标识符（表名/列名/索引名/schema 名）在拼接
/// SQL 字符串前**必须**经过此函数转义，否则存在二阶 SQL 注入风险。
/// 推荐优先使用参数化查询；DDL 等无法参数化的场景必须使用本函数。
pub fn quote_ident(ident: &str) -> String {
    let escaped = ident.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// 智能引用标识符：仅在必要时（含特殊字符/空/超长）才加双引号。
///
/// 与 [`quote_ident`] 的区别：
/// - `quote_ident` 总是加双引号（保守安全策略，用于直接拼接 SQL）
/// - `quote_ident_smart` 仅在标识符不符合"裸写安全规则"时加双引号
///
/// # 裸写安全规则
///
/// 满足以下全部条件时，标识符可不加双引号直接拼接：
/// 1. 非空且长度 ≤ 63（PostgreSQL NAMEDATALEN-1）
/// 2. 首字符为字母或下划线
/// 3. 后续字符为字母/数字/下划线/`$`
/// 4. 不是 SQL 保留字（避免歧义）
///
/// # 用途
///
/// 用于 `SHOW CREATE TABLE` 等需要生成可读 DDL 的场景，
/// 输出格式更贴近 PostgreSQL 实际行为（`pg_get_constraintdef` 等也采用智能引用）。
///
/// # 安全说明
///
/// 此函数仍然安全：含特殊字符的标识符会被 `quote_ident` 处理。
/// 仅对"已通过白名单校验"的标识符省略双引号，不会引入二阶 SQL 注入。
///
/// # 示例
///
/// ```
/// use szrsql_sql::ast::quote_ident_smart;
/// assert_eq!(quote_ident_smart("users"), "users");
/// assert_eq!(quote_ident_smart("col_1"), "col_1");
/// assert_eq!(quote_ident_smart("user name"), "\"user name\"");
/// assert_eq!(quote_ident_smart("a\"b"), "\"a\"\"b\"");
/// assert_eq!(quote_ident_smart(""), "\"\"");
/// ```
pub fn quote_ident_smart(ident: &str) -> String {
    if is_valid_ident(ident) && !is_sql_reserved_keyword(ident) {
        ident.to_string()
    } else {
        quote_ident(ident)
    }
}

/// 判断标识符是否为 SQL 保留字（需加引号避免歧义）。
///
/// 仅列出最常用的保留字；非保留字允许裸写。
/// 完整列表参考 PostgreSQL 文档：https://www.postgresql.org/docs/current/sql-keywords-appendix.html
fn is_sql_reserved_keyword(ident: &str) -> bool {
    matches!(
        ident.to_uppercase().as_str(),
        "SELECT"
            | "FROM"
            | "WHERE"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
            | "CREATE"
            | "DROP"
            | "TABLE"
            | "INDEX"
            | "VIEW"
            | "SEQUENCE"
            | "ORDER"
            | "GROUP"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "JOIN"
            | "INNER"
            | "OUTER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "ON"
            | "AS"
            | "AND"
            | "OR"
            | "NOT"
            | "NULL"
            | "TRUE"
            | "FALSE"
            | "DEFAULT"
            | "PRIMARY"
            | "KEY"
            | "UNIQUE"
            | "FOREIGN"
            | "REFERENCES"
            | "CHECK"
            | "CONSTRAINT"
            | "BEGIN"
            | "COMMIT"
            | "ROLLBACK"
            | "SAVEPOINT"
            | "INTO"
            | "VALUES"
            | "SET"
            | "SHOW"
            | "EXPLAIN"
            | "WITH"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "DISTINCT"
            | "ALL"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
            | "IF"
            | "EXISTS"
            | "BETWEEN"
            | "LIKE"
            | "ILIKE"
            | "IN"
            | "IS"
            | "CAST"
            | "ALTER"
            | "ADD"
            | "COLUMN"
            | "RENAME"
            | "TO"
            | "TRUNCATE"
            | "GRANT"
            | "REVOKE"
            | "PRIVILEGES"
            | "PUBLIC"
            | "USER"
            | "ROLE"
            | "SCHEMA"
            | "DATABASE"
            | "TYPE"
            | "FUNCTION"
            | "PROCEDURE"
            | "TRIGGER"
            | "RETURNING"
            | "CONFLICT"
            | "DO"
            | "NOTHING"
            | "REPLACE"
            | "IGNORE"
            | "TEMPORARY"
            | "TEMP"
            | "CASCADE"
            | "RESTRICT"
            | "USING"
            | "FETCH"
            | "FOR"
            | "ONLY"
            | "NOWAIT"
            | "SKIP"
            | "LOCKED"
            | "SHARE"
            | "MODE"
            | "LOCK"
            | "UNLOCKED"
            | "ASC"
            | "DESC"
            | "NULLS"
            | "FIRST"
            | "LAST"
            | "WINDOW"
            | "OVER"
            | "PARTITION"
            | "RANGE"
            | "ROWS"
            | "GROUPS"
            | "PRECEDING"
            | "FOLLOWING"
            | "CURRENT"
            | "ROW"
            | "FILTER"
            | "EXCLUDE"
            | "TIES"
            | "OTHERS"
            | "EXTRACT"
            | "EPOCH"
            | "YEAR"
            | "MONTH"
            | "DAY"
            | "HOUR"
            | "MINUTE"
            | "SECOND"
            | "INTERVAL"
            | "TIMESTAMP"
            | "DATE"
            | "TIME"
            | "TEXT"
            | "INTEGER"
            | "INT"
            | "BIGINT"
            | "SMALLINT"
            | "DECIMAL"
            | "NUMERIC"
            | "REAL"
            | "FLOAT"
            | "DOUBLE"
            | "BOOLEAN"
            | "BOOL"
            | "CHAR"
            | "VARCHAR"
            | "BLOB"
            | "BYTEA"
            | "JSON"
            | "JSONB"
            | "UUID"
            | "ARRAY"
            | "ENUM"
    )
}

/// 仅对标识符内部的特殊字符进行转义，不添加外层双引号。
///
/// 用于需要手动控制引用方式的场景（如已经在外层添加了双引号）。
/// 普通场景请直接使用 [`quote_ident`]。
///
/// # 示例
///
/// ```
/// use szrsql_sql::ast::escape_ident;
/// assert_eq!(escape_ident("users"), "users");
/// assert_eq!(escape_ident("a\"b"), "a\"\"b");
/// ```
pub fn escape_ident(ident: &str) -> String {
    ident.replace('"', "\"\"")
}

/// 校验标识符是否仅包含安全字符（白名单校验）。
///
/// # 规则
///
/// - 首字符：字母（a-z, A-Z）、下划线 `_`、中文等 Unicode 字母
/// - 后续字符：字母、数字、下划线、`$`、Unicode 字母
/// - 长度：1..=63（PostgreSQL 默认 NAMEDATALEN-1 = 63）
/// - 不允许：空字符串、含双引号/分号/空格等特殊字符（这些必须用 quote_ident 引用）
///
/// # 返回
///
/// - `true`：标识符安全，可直接拼接（仍建议使用 quote_ident）
/// - `false`：标识符包含特殊字符，**必须**使用 quote_ident 引用
///
/// # 示例
///
/// ```
/// use szrsql_sql::ast::is_valid_ident;
/// assert!(is_valid_ident("users"));
/// assert!(is_valid_ident("_id"));
/// assert!(is_valid_ident("col_1"));
/// assert!(!is_valid_ident(""));
/// assert!(!is_valid_ident("user name"));
/// assert!(!is_valid_ident("a\"b"));
/// assert!(!is_valid_ident("a;b"));
/// ```
pub fn is_valid_ident(ident: &str) -> bool {
    if ident.is_empty() || ident.len() > 63 {
        return false;
    }
    let mut chars = ident.chars();
    let first = chars.next().unwrap();
    if !is_ident_start_char(first) {
        return false;
    }
    for c in chars {
        if !is_ident_continue_char(c) {
            return false;
        }
    }
    true
}

/// 判断字符是否可作为标识符首字符
fn is_ident_start_char(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

/// 判断字符是否可作为标识符后续字符
fn is_ident_continue_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

/// 列定义（CREATE TABLE 中的列声明）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnDefinition {
    /// 列名
    pub name: String,
    /// 列类型
    pub data_type: ColumnType,
    /// NOT NULL
    pub not_null: bool,
    /// PRIMARY KEY（列级）
    pub primary_key: bool,
    /// UNIQUE（列级）
    pub unique: bool,
    /// DEFAULT 表达式
    pub default: Option<Expr>,
    /// CHECK 表达式（列级）
    pub check: Option<Expr>,
    /// 引用（列级外键）
    pub references: Option<ForeignKeyReference>,
    /// ENUM 类型的可选值（仅 col_type=Enum 时有意义）
    pub enum_values: Option<Vec<String>>,
    /// 自定义类型名 — Phase 3.31
    ///
    /// 当列声明使用命名自定义类型（如 `CREATE TABLE t (c mood)` 中的 `mood`）时，
    /// parser 无法仅凭 SQL 解析出实际类型，故保留原始类型名。
    /// Planner 在 `plan_create_table` 时查询 Catalog 的 `enum_types`：
    /// - 若该名是已注册的 enum 类型 → 设置 `data_type = ColumnType::Enum(values)`
    /// - 若该名不是已知类型 → 保持 `data_type = ColumnType::Text`（向后兼容）
    pub custom_type_name: Option<String>,
    /// 生成列定义 — Phase 6.18
    ///
    /// `GENERATED ALWAYS AS (expr) STORED` 语法的生成列。
    /// 生成列的值由表达式自动计算，不能在 INSERT/UPDATE 中显式赋值。
    /// 表达式可引用同行的其他列，求值顺序按列声明顺序。
    pub generated: Option<GeneratedColumn>,
    /// 列注释 — Phase TDengine-P2
    ///
    /// 由 `COMMENT ON COLUMN <table>.<column> IS '...'` 设置。
    /// 仅用于元数据展示，不影响查询语义。
    pub comment: Option<String>,
    /// AUTO_INCREMENT 标记 — P2-16.3
    ///
    /// MySQL `AUTO_INCREMENT` 列选项。当前仅作解析记录，
    /// 真正的自增语义（INSERT 时自动填充序列值）留待后续阶段实现。
    pub auto_increment: bool,
}

/// 生成列定义 — Phase 6.18
///
/// 对应 PG 12+ 的 `GENERATED ALWAYS AS (expr) STORED` 语法。
/// `stored` 为 true 表示存储生成值（当前实现仅支持 STORED）；
/// `expr` 为生成表达式，在 INSERT/UPDATE 时自动求值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedColumn {
    /// 生成表达式
    pub expr: Expr,
    /// 是否为 STORED（当前实现仅支持 STORED = true）
    pub stored: bool,
}

impl ColumnDefinition {
    /// 创建新列定义
    pub fn new(name: impl Into<String>, data_type: ColumnType) -> Self {
        Self {
            name: name.into(),
            data_type,
            not_null: false,
            primary_key: false,
            unique: false,
            default: None,
            check: None,
            references: None,
            enum_values: None,
            custom_type_name: None,
            generated: None,
            comment: None,
            auto_increment: false,
        }
    }
}

/// DEFERRABLE 模式 — SQL:2016 F-9
///
/// 控制外键约束的检查时机：
/// - `Immediate`：每条 DML 语句执行时立即检查（默认行为）
/// - `Deferred`：推迟到事务 COMMIT 时统一检查
///
/// PG 语义：
/// - `DEFERRABLE` 单独出现 = `DEFERRABLE INITIALLY IMMEDIATE`（可延迟，但默认立即检查）
/// - `DEFERRABLE INITIALLY DEFERRED` = 默认延迟到 COMMIT 检查
/// - `NOT DEFERRABLE` = 不允许延迟，始终立即检查
/// - 非 DEFERRABLE 约束（未指定）= 始终立即检查（等价于 NOT DEFERRABLE）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DeferrableMode {
    /// 立即检查（默认）— 每条 DML 后立即校验
    #[default]
    Immediate,
    /// 延迟检查 — 入队，COMMIT 时统一校验
    Deferred,
}

/// 列级外键引用
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ForeignKeyReference {
    /// 引用表名
    pub table: TableName,
    /// 引用列名（None 表示引用对方主键）
    pub columns: Option<Vec<String>>,
    /// ON DELETE 动作
    pub on_delete: Option<ReferenceAction>,
    /// ON UPDATE 动作
    pub on_update: Option<ReferenceAction>,
    /// DEFERRABLE 模式 — P3-3 (SQL:2016 F-9)
    ///
    /// `None` 表示未声明 DEFERRABLE（等价于 NOT DEFERRABLE，始终立即检查）。
    /// `Some(Immediate)` = DEFERRABLE INITIALLY IMMEDIATE（可 SET CONSTRAINTS 切换）
    /// `Some(Deferred)` = DEFERRABLE INITIALLY DEFERRED（默认延迟到 COMMIT）
    pub deferrable_mode: Option<DeferrableMode>,
}

/// 参照动作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReferenceAction {
    /// NO ACTION（默认）
    NoAction,
    /// RESTRICT
    Restrict,
    /// CASCADE
    Cascade,
    /// SET NULL
    SetNull,
    /// SET DEFAULT
    SetDefault,
}

/// ON COMMIT 行为 — Phase 3.28
///
/// 仅对 `CREATE TEMPORARY TABLE` 有效，控制事务提交时临时表的行为：
/// - `DeleteRows`：ON COMMIT DELETE ROWS — 提交时清空数据但保留表结构（PG 默认）
/// - `PreserveRows`：ON COMMIT PRESERVE ROWS — 提交时保留数据（MySQL/SQL Server 默认）
/// - `Drop`：ON COMMIT DROP — 提交时直接删除临时表
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OnCommitAction {
    /// ON COMMIT DELETE ROWS — 提交时清空数据
    DeleteRows,
    /// ON COMMIT PRESERVE ROWS — 提交时保留数据
    PreserveRows,
    /// ON COMMIT DROP — 提交时删除临时表
    Drop,
}

/// 表级约束（CREATE TABLE 中的表级声明）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TableConstraint {
    /// PRIMARY KEY (cols...)
    PrimaryKey {
        /// 约束名（可选）
        name: Option<String>,
        /// 列名
        columns: Vec<String>,
    },
    /// UNIQUE (cols...)
    Unique {
        /// 约束名（可选）
        name: Option<String>,
        /// 列名
        columns: Vec<String>,
    },
    /// FOREIGN KEY (cols...) REFERENCES table(cols...)
    ForeignKey {
        /// 约束名（可选）
        name: Option<String>,
        /// 本表列名
        columns: Vec<String>,
        /// 引用表与列
        reference: ForeignKeyReference,
    },
    /// CHECK (expr)
    Check {
        /// 约束名（可选）
        name: Option<String>,
        /// CHECK 表达式
        expr: Expr,
    },
}

impl TableConstraint {
    /// 返回约束名（L8 新增：配合 catalog 持久化做重名检测）
    pub fn name(&self) -> Option<&str> {
        match self {
            TableConstraint::PrimaryKey { name, .. }
            | TableConstraint::Unique { name, .. }
            | TableConstraint::ForeignKey { name, .. }
            | TableConstraint::Check { name, .. } => name.as_deref(),
        }
    }
}

/// 索引列定义
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexColumn {
    /// 列名
    pub column: String,
    /// 排序方向
    pub asc: bool,
    /// NULLS 顺序（true = NULLS FIRST）
    pub nulls_first: bool,
    /// 表达式索引（None 表示普通列索引）
    pub expr: Option<Expr>,
}

impl IndexColumn {
    /// 创建普通列索引项（默认 ASC NULLS LAST）
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            asc: true,
            nulls_first: false,
            expr: None,
        }
    }
}

// =====================================================================
//  DML 辅助类型
// =====================================================================

/// INSERT 数据源
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InsertSource {
    /// VALUES (...) [, (...) ...]
    Values(Vec<Vec<Expr>>),
    /// INSERT INTO ... SELECT ...
    Select(Box<Select>),
    /// INSERT INTO ... DEFAULT VALUES
    DefaultValues,
}

/// ON CONFLICT 处理
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OnConflict {
    /// ON CONFLICT DO NOTHING
    DoNothing {
        /// 冲突判定的列（None 表示 ON CONFLICT 不带列，使用主键）
        conflict_columns: Option<Vec<String>>,
    },
    /// ON CONFLICT (cols...) DO UPDATE SET ...
    DoUpdate {
        /// 冲突判定的列（None 表示 ON CONFLICT 不带列，使用主键）
        conflict_columns: Option<Vec<String>>,
        /// DO UPDATE SET 赋值
        assignments: Vec<Assignment>,
        /// WHERE 条件
        where_clause: Option<Expr>,
    },
}

/// UPDATE 赋值（SET col = expr）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    /// 列名
    pub column: String,
    /// 赋值表达式
    pub value: Expr,
}

// =====================================================================
//  SELECT 语句
// =====================================================================

/// SELECT 语句
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Select {
    /// WITH 子句（CTE）— Phase 6.1
    ///
    /// None 表示无 WITH；Some 表示 WITH [RECURSIVE] cte_name AS (...)。
    /// CTE 在 FROM 中可被引用，等价于派生表。
    pub with: Option<WithClause>,
    /// DISTINCT 标志
    pub distinct: bool,
    /// 投影列
    pub projection: Vec<SelectItem>,
    /// FROM 子句（含 JOIN）
    pub from: Vec<TableWithJoins>,
    /// WHERE 条件
    pub where_clause: Option<Expr>,
    /// GROUP BY 列
    pub group_by: Vec<Expr>,
    /// GROUPING SETS / CUBE / ROLLUP 分组集 — P3-1
    ///
    /// None 表示普通 GROUP BY（使用 `group_by` 字段）。
    /// Some 表示多分组集聚合，此时 `group_by` 字段忽略，以本字段为准。
    /// 每个内层 Vec 是一个分组集（一组 GROUP BY 表达式）。
    pub grouping_sets: Option<Vec<Vec<Expr>>>,
    /// HAVING 条件
    pub having: Option<Expr>,
    /// ORDER BY 列
    pub order_by: Vec<OrderByExpr>,
    /// LIMIT
    pub limit: Option<Expr>,
    /// OFFSET
    pub offset: Option<Expr>,
    /// 集合操作（INTERSECT / EXCEPT / UNION）— Phase 3.27
    ///
    /// None 表示普通 SELECT；Some 表示与右侧 SELECT 进行集合运算。
    /// ORDER BY / LIMIT / OFFSET 作用于集合操作的整体结果。
    pub set_op: Option<SetOperation>,
}

/// WITH 子句（通用表表达式，CTE）— Phase 6.1
///
/// 对应 PG 语法：`WITH [RECURSIVE] cte_name [(col1, col2, ...)] AS (query) [, ...]`
///
/// # 语义（与 PG 一致）
/// - **非递归 CTE**：每个 CTE 的 query 不引用自身（也不引用后续 CTE — PG 行为）。
///   执行时一次性物化结果，FROM 中引用 CTE 名即读取物化结果。
/// - **递归 CTE**：`recursive=true` 时，CTE 的 query 必须为
///   `anchor UNION [ALL] recursive_part` 形式。anchor 不引用 CTE 自身，
///   recursive_part 引用 CTE 自身。执行时迭代：
///   1. 物化 anchor → 初始结果集 R₀
///   2. 用 R_i 执行 recursive_part → R_{i+1}（仅新增行）
///   3. R_{i+1} 为空则停止；否则 R_{i+1} 累加到 R₀（UNION ALL）或去重后累加（UNION）
/// - **列别名**：`cte_name (col1, col2, ...)` 重命名 CTE 输出列。若未指定则用 query 的输出列名。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WithClause {
    /// RECURSIVE 标志
    pub recursive: bool,
    /// CTE 列表（按声明顺序）
    pub ctes: Vec<CommonTableExpr>,
}

/// 通用表表达式（CTE）单个定义 — Phase 6.1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommonTableExpr {
    /// CTE 名称
    pub name: String,
    /// 显式列别名列表（`cte_name (col1, col2, ...)`）。空 Vec 表示未指定。
    pub columns: Vec<String>,
    /// CTE 查询体
    pub query: Box<Select>,
}

/// 集合操作类型 — Phase 3.27
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SetOperator {
    /// UNION — 并集
    Union,
    /// INTERSECT — 交集
    Intersect,
    /// EXCEPT — 差集
    Except,
}

/// 集合量词 — Phase 3.27
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SetQuantifier {
    /// ALL — 保留重复行
    All,
    /// DISTINCT（默认）— 去除重复行
    Distinct,
    /// 未指定（按 DISTINCT 处理）
    None,
}

/// 集合操作 — Phase 3.27
///
/// 表示 `left_select OP [ALL|DISTINCT] right_select`。
/// `left_select` 既保留在 `SetOperation.left` 中（用于递归嵌套集合操作），
/// 其最内层 SELECT 的字段也"展开"到包含此字段的 `Select` 中（用于外层 ORDER BY / LIMIT 等）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetOperation {
    /// 操作类型（UNION / INTERSECT / EXCEPT）
    pub op: SetOperator,
    /// 量词（ALL / DISTINCT / None）
    pub quantifier: SetQuantifier,
    /// 左侧 SELECT（可为嵌套集合操作）
    pub left: Box<Select>,
    /// 右侧 SELECT
    pub right: Box<Select>,
}

/// GROUPING SETS / CUBE / ROLLUP 分组集类型 — P3-1
///
/// 对应 SQL:2016 F-9 分析查询扩展。
/// - `Plain(exprs)`：普通 GROUP BY 列表，等价于单个分组集 `[exprs]`
/// - `GroupingSets(sets)`：`GROUP BY GROUPING SETS (set1, set2, ...)`
/// - `Cube(exprs)`：`GROUP BY CUBE(a, b, c)` → 所有子集组合（2^n 个分组集）
/// - `Rollup(exprs)`：`GROUP BY ROLLUP(a, b, c)` → 前缀链（n+1 个分组集）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GroupingSet {
    /// 普通 GROUP BY 列表示（回退路径）
    Plain(Vec<Expr>),
    /// GROUPING SETS：每个内层 Vec 是一个分组集
    GroupingSets(Vec<Vec<Expr>>),
    /// CUBE：对给定列求所有子集组合
    Cube(Vec<Expr>),
    /// ROLLUP：对给定列求前缀链
    Rollup(Vec<Expr>),
}

/// SELECT 投影项
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SelectItem {
    /// 表达式（无别名）
    UnnamedExpr(Expr),
    /// 表达式 AS alias
    ExprWithAlias {
        /// 表达式
        expr: Expr,
        /// 别名
        alias: String,
    },
    /// table.* 通配
    QualifiedWildcard(String),
    /// * 通配
    Wildcard,
}

/// FROM 子句中的表与 JOIN
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableWithJoins {
    /// 主表
    pub relation: TableFactor,
    /// JOIN 列表
    pub joins: Vec<Join>,
}

/// 表因子（FROM 的基本单元）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TableFactor {
    /// 物理表
    Table {
        /// 表名
        name: TableName,
        /// 别名
        alias: Option<TableAlias>,
        /// P3-6：SQL:2011 F-751 时间旅行查询 — `FOR SYSTEM_TIME AS OF <expr>`
        /// 表达式求值结果为时间戳，执行器据此过滤行版本
        system_time_as_of: Option<Box<Expr>>,
    },
    /// 子查询 `(SELECT ...) AS alias`
    Derived {
        /// 子查询
        subquery: Box<Select>,
        /// 别名（必填）
        alias: TableAlias,
        /// P3-2: LATERAL 标志 — 为 true 时右侧可引用左侧表的列
        lateral: bool,
    },
    /// 表函数 `func(args) AS alias`
    TableFunction {
        /// 函数名
        name: String,
        /// 参数列表
        args: Vec<Expr>,
        /// 别名
        alias: Option<TableAlias>,
    },
    /// P4-1: MATCH_RECOGNIZE 行模式匹配（SQL:2016 复杂事件处理）
    ///
    /// 语法：`table MATCH_RECOGNIZE ( PARTITION BY ... ORDER BY ... MEASURES ...
    ///         ONE ROW PER MATCH AFTER MATCH SKIP ... PATTERN (...) DEFINE ... )`
    MatchRecognize {
        /// 被匹配的表
        table: Box<TableFactor>,
        /// 行模式匹配子句
        clause: MatchRecognizeClause,
        /// 可选别名
        alias: Option<TableAlias>,
    },
}

// =====================================================================
//  MATCH_RECOGNIZE AST 类型（P4-1）
// =====================================================================

/// MATCH_RECOGNIZE 子句
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchRecognizeClause {
    /// PARTITION BY 表达式
    pub partition_by: Vec<Expr>,
    /// ORDER BY 表达式
    pub order_by: Vec<OrderByExpr>,
    /// MEASURES：(表达式, 别名)
    pub measures: Vec<(Expr, String)>,
    /// 每匹配输出模式
    pub rows_per_match: RowsPerMatch,
    /// AFTER MATCH SKIP 选项
    pub after_match_skip: Option<AfterMatchSkip>,
    /// 模式表达式
    pub pattern: PatternExpr,
    /// 符号定义：(符号名, 条件表达式)
    pub symbols: Vec<(String, Expr)>,
}

/// ROWS PER MATCH 选项
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowsPerMatch {
    /// ONE ROW PER MATCH（默认）
    OneRow,
    /// ALL ROWS PER MATCH
    AllRows,
}

/// AFTER MATCH SKIP 选项
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterMatchSkip {
    /// PAST LAST ROW
    PastLastRow,
    /// TO NEXT ROW
    ToNextRow,
    /// TO FIRST <symbol>
    ToFirst(String),
    /// TO LAST <symbol>
    ToLast(String),
}

/// 模式表达式（正则表达式变体）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatternExpr {
    /// 命名符号，如 `A`
    Symbol(String),
    /// 连接：`A B C`
    Concat(Vec<PatternExpr>),
    /// 选择：`A | B`
    Alternation(Vec<PatternExpr>),
    /// 分组：`( pattern )`
    Group(Box<PatternExpr>),
    /// 重复：`pattern*` 或 `pattern+`
    Repetition(Box<PatternExpr>, Quantifier),
}

/// 量词
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantifier {
    /// `*` 零次或多次
    ZeroOrMore,
    /// `+` 一次或多次
    OneOrMore,
}

/// 表别名（含可选列别名）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableAlias {
    /// 表别名
    pub name: String,
    /// 列别名（可选，PG: `FROM t AS a (col1, col2)`）
    pub column_aliases: Option<Vec<String>>,
}

impl TableAlias {
    /// 创建表别名（不带列别名）
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_aliases: None,
        }
    }
}

/// JOIN 子句
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Join {
    /// 右侧表
    pub relation: TableFactor,
    /// JOIN 类型
    pub join_type: JoinType,
    /// JOIN 条件
    pub condition: JoinCondition,
}

/// JOIN 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JoinType {
    /// INNER JOIN
    Inner,
    /// LEFT [OUTER] JOIN
    LeftOuter,
    /// RIGHT [OUTER] JOIN
    RightOuter,
    /// FULL [OUTER] JOIN
    FullOuter,
    /// CROSS JOIN
    Cross,
    /// SEMI JOIN — Phase 5.6 子查询展平
    ///
    /// 仅保留左表行（当右表至少存在一行匹配时），不输出右表列。
    /// 由 `IN (SELECT ...)` / `EXISTS (SELECT ...)` 展平而来，不由 parser 直接产生。
    Semi,
    /// ANTI JOIN — Phase 5.6 子查询展平
    ///
    /// 仅保留左表行（当右表无任何匹配时），不输出右表列。
    /// 由 `NOT IN (SELECT ...)` / `NOT EXISTS (SELECT ...)` 展平而来，不由 parser 直接产生。
    Anti,
}

/// JOIN 条件
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JoinCondition {
    /// ON expr
    On(Expr),
    /// USING (cols...)
    Using(Vec<String>),
    /// NATURAL JOIN（无显式条件）
    Natural,
    /// CROSS JOIN 无条件
    None,
}

/// ORDER BY 表达式
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderByExpr {
    /// 排序表达式
    pub expr: Expr,
    /// ASC / DESC
    pub asc: bool,
    /// NULLS FIRST / NULLS LAST
    pub nulls_first: bool,
}

// =====================================================================
//  窗口函数 — Phase 6.2
// =====================================================================

/// 窗口规格 `OVER (...)`
///
/// 对应 SQL 标准的 WindowSpec，描述分区、排序与帧定义。
/// 例：`SUM(x) OVER (PARTITION BY y ORDER BY z ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowSpec {
    /// `PARTITION BY` 表达式列表（空表示不分区）
    pub partition_by: Vec<Expr>,
    /// `ORDER BY` 表达式列表（空表示不排序）
    pub order_by: Vec<OrderByExpr>,
    /// 窗口帧定义（None 表示使用默认帧）
    pub window_frame: Option<WindowFrame>,
}

/// 窗口帧 `ROWS|RANGE|GROUPS BETWEEN ... AND ...`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    /// 帧单位
    pub units: WindowFrameUnits,
    /// 起始边界
    pub start_bound: WindowFrameBound,
    /// 结束边界（None 表示 CURRENT ROW）
    pub end_bound: Option<WindowFrameBound>,
}

/// 窗口帧单位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WindowFrameUnits {
    /// `ROWS` — 按物理行计数
    Rows,
    /// `RANGE` — 按值范围（默认）
    Range,
    /// `GROUPS` — 按对等组计数
    Groups,
}

/// 窗口帧边界
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WindowFrameBound {
    /// `CURRENT ROW`
    CurrentRow,
    /// `N PRECEDING`（None 表示 `UNBOUNDED PRECEDING`）
    Preceding(Option<Box<Expr>>),
    /// `N FOLLOWING`（None 表示 `UNBOUNDED FOLLOWING`）
    Following(Option<Box<Expr>>),
}

// =====================================================================
//  表达式
// =====================================================================

/// 表达式 AST 节点
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// 字面量
    Literal(Value),
    /// 列引用（按层级：col / table.col / schema.table.col）
    Identifier(Vec<String>),
    /// 二元运算 `left op right`
    BinaryOp {
        /// 左操作数
        left: Box<Expr>,
        /// 运算符
        op: BinaryOp,
        /// 右操作数
        right: Box<Expr>,
    },
    /// 一元运算 `op expr`
    UnaryOp {
        /// 运算符
        op: UnaryOp,
        /// 操作数
        expr: Box<Expr>,
    },
    /// 函数调用 `name([DISTINCT] args...)`
    Function {
        /// 函数名（小写）
        name: String,
        /// 参数列表
        args: Vec<Expr>,
        /// DISTINCT 标志
        distinct: bool,
    },
    /// CASE [operand] WHEN then THEN result ... [ELSE else] END
    Case {
        /// CASE 表达式操作数（None 表示 CASE WHEN expr 形式）
        operand: Option<Box<Expr>>,
        /// WHEN ... THEN ... 列表
        when_then: Vec<(Expr, Expr)>,
        /// ELSE 表达式
        else_expr: Option<Box<Expr>>,
    },
    /// CAST(expr AS type)
    Cast {
        /// 被转换的表达式
        expr: Box<Expr>,
        /// 目标类型
        data_type: ColumnType,
    },
    /// expr [NOT] IN (val1, val2, ...)
    InList {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// 列表
        list: Vec<Expr>,
        /// NOT IN
        negated: bool,
    },
    /// expr [NOT] IN (SELECT ...)
    InSubquery {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// 子查询
        subquery: Box<Select>,
        /// NOT IN
        negated: bool,
    },
    /// expr [NOT] BETWEEN low AND high
    Between {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// 下界
        low: Box<Expr>,
        /// 上界
        high: Box<Expr>,
        /// NOT BETWEEN
        negated: bool,
    },
    /// expr [NOT] LIKE pattern  /  expr [NOT] ILIKE pattern
    ///
    /// `case_insensitive=true` 表示 ILIKE（大小写不敏感），false 表示 LIKE（大小写敏感）。
    /// PG 语义：ILIKE 等价于对 expr 与 pattern 都调用 lower() 后再 LIKE。
    Like {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// 模式
        pattern: Box<Expr>,
        /// NOT LIKE / NOT ILIKE
        negated: bool,
        /// true 表示 ILIKE（大小写不敏感）
        case_insensitive: bool,
    },
    /// expr IS [NOT] NULL
    IsNull {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// IS NOT NULL
        negated: bool,
    },
    /// expr IS DISTINCT FROM other  /  expr IS NOT DISTINCT FROM other
    ///
    /// PG 语义：NULL 安全的不等比较。
    /// - `IS DISTINCT FROM`：两值不同（含 NULL 与非 NULL 视为不同）→ true
    /// - `IS NOT DISTINCT FROM`：两值相同（NULL = NULL 视为相同）→ true
    IsDistinctFrom {
        /// 左表达式
        left: Box<Expr>,
        /// 右表达式
        right: Box<Expr>,
        /// true 表示 `IS NOT DISTINCT FROM`（相等），false 表示 `IS DISTINCT FROM`（不等）
        not: bool,
    },
    /// expr [NOT] SIMILAR TO pattern
    ///
    /// PG 语义：SQL 标准正则匹配（介于 LIKE 与 POSIX ~ 之间）。
    /// SIMILAR TO 使用 SQL 正则语法（支持 `|`、`*`、`+`、`?`、`[...]`、`(...)`），
    /// 必须完全匹配整个字符串（不像 ~ 是部分匹配）。
    SimilarTo {
        /// 被测试的表达式
        expr: Box<Expr>,
        /// 模式
        pattern: Box<Expr>,
        /// NOT SIMILAR TO
        negated: bool,
    },
    /// 子查询 (SELECT ...)
    Subquery(Box<Select>),
    /// [NOT] EXISTS (SELECT ...)
    Exists {
        /// 子查询
        subquery: Box<Select>,
        /// NOT EXISTS
        negated: bool,
    },
    /// SUBSTRING(expr [FROM start] [FOR length])  /  SUBSTRING(expr, start [, length])
    ///
    /// PG 语义：1-based 索引截取子串。
    /// - `SUBSTRING('hello' FROM 2)` → `'ello'`
    /// - `SUBSTRING('hello' FROM 2 FOR 3)` → `'ell'`
    /// - `SUBSTRING('hello' FROM 0 FOR 3)` → `'he'`（PG 特殊语义：start=0 时实际从 1 开始，长度-1）
    /// - `SUBSTRING('hello', 2, 3)` → `'ell'`（MySQL/SQL Server 语法）
    /// - start/length 为 NULL 或负数时返回 NULL
    Substring {
        /// 源字符串
        expr: Box<Expr>,
        /// 起始位置（1-based）
        from: Option<Box<Expr>>,
        /// 长度
        for_len: Option<Box<Expr>>,
    },
    /// 元组 `(expr1, expr2, ...)`
    Tuple(Vec<Expr>),
    /// 通配符 `*`
    Wildcard,
    /// 参数占位符 `$1`、`$2` ...（1-based 索引）— Phase 3.26
    ///
    /// 用于 PREPARE/EXECUTE 语句的参数化查询。
    /// 执行 EXECUTE 时由执行器将 `Parameter(idx)` 替换为实际参数值 `Literal(value)`。
    Parameter(usize),
    /// 数组字面量 `ARRAY[expr1, expr2, ...]` 或 `[expr1, expr2, ...]` — Phase 3.32
    ///
    /// 注意：PG 中字符串形式的数组字面量 `'{1,2,3}'` 在解析阶段被降级为 `Value::Text`，
    /// 由执行器在 INSERT/UPDATE 时根据目标列类型 `ColumnType::Array(_)` 解析为
    /// `Value::Array(...)`。这与 PG 行为一致（PG 也允许文本到数组的隐式转换）。
    Array(Vec<Expr>),
    /// `left OP ANY(right)` / `left OP SOME(right)` — Phase 3.32
    ///
    /// PG 语义：`left OP ANY(arr)` 等价于 `exists(elem in arr, left OP elem)`。
    /// `ANY` 与 `SOME` 完全同义。
    /// `right` 通常是数组字面量、数组列或子查询（子查询返回单列）。
    AnyOp {
        /// 左操作数
        left: Box<Expr>,
        /// 比较运算符（= / <> / < / <= / > / >=）
        op: BinaryOp,
        /// 右操作数（数组表达式）
        right: Box<Expr>,
    },
    /// `left OP ALL(right)` — Phase 3.32
    ///
    /// PG 语义：`left OP ALL(arr)` 等价于 `forall(elem in arr, left OP elem)`。
    /// 空数组的 `ALL` 永远返回 true。
    AllOp {
        /// 左操作数
        left: Box<Expr>,
        /// 比较运算符
        op: BinaryOp,
        /// 右操作数（数组表达式）
        right: Box<Expr>,
    },
    /// P3-1: GROUPING SETS — `GROUP BY GROUPING SETS ((a,b), (c), ())`
    ///
    /// 内层 Vec 是一个分组集（一组 GROUP BY 表达式）。
    /// 由解析器从 sqlparser AST 转换而来，执行器展开为多个分组集迭代。
    GroupingSets(Vec<Vec<Expr>>),
    /// P3-1: CUBE — `GROUP BY CUBE(a, b, c)`
    ///
    /// 规划阶段已由 `expand_cube()` 展开为 `GroupingSets`，
    /// 保留此变体以支持 AST 层的直接表示（如子查询中的表达式）。
    Cube(Vec<Expr>),
    /// P3-1: ROLLUP — `GROUP BY ROLLUP(a, b, c)`
    ///
    /// 规划阶段已由 `expand_rollup()` 展开为 `GroupingSets`，
    /// 保留此变体以支持 AST 层的直接表示。
    Rollup(Vec<Expr>),
    /// 窗口函数 `func(args) OVER (window_spec)` — Phase 6.2
    ///
    /// 与 `Expr::Function` 分离，避免被聚合函数识别逻辑误判
    /// （`expr_contains_aggregate` / `extract_aggregates` / `substitute_aggregates`）。
    WindowFunction {
        /// 函数名（小写）
        name: String,
        /// 参数列表
        args: Vec<Expr>,
        /// DISTINCT 标志（部分窗口函数支持，如 COUNT(DISTINCT x) OVER (...)）
        distinct: bool,
        /// 窗口规格
        window: WindowSpec,
    },
}

/// 二元运算符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Modulo,
    /// `=`
    Eq,
    /// `<>` / `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `AND`
    And,
    /// `OR`
    Or,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `#`（PG 异或）/ `^`
    BitXor,
    /// `<<`
    ShiftLeft,
    /// `>>`
    ShiftRight,
    /// `||`（字符串拼接）
    StringConcat,
    /// `@@`（PG 全文检索匹配操作符，Phase 3.33）
    ///
    /// 语义：`tsvector @@ tsquery` → bool
    AtAt,
    /// `~`（PG 正则匹配，大小写敏感）
    ///
    /// 语义：`text ~ pattern` → bool，等价于 `text SIMILAR TO pattern` 但使用 POSIX 正则
    RegexMatch,
    /// `~*`（PG 正则匹配，大小写不敏感）
    RegexIMatch,
    /// `!~`（PG 正则不匹配，大小写敏感）
    RegexNotMatch,
    /// `!~*`（PG 正则不匹配，大小写不敏感）
    RegexNotIMatch,
    /// `->`（PG JSON/JSONB 路径访问：`json -> 'key'` 返回 json/jsonb，`json -> 1` 返回数组元素）
    ///
    /// 语义：左侧为 JSON/JSONB，右侧为 text（键名）或 int（数组索引）
    JsonArrow,
    /// `->>`PG JSON/JSONB 路径访问（文本结果）：`json ->> 'key'` 返回 text
    ///
    /// 语义：与 JsonArrow 一致，但返回 text 而非 json
    JsonLongArrow,
    /// `#>`（PG JSON/JSONB 路径数组访问：`json #> '{a,b}'` 返回 json）
    ///
    /// 语义：按路径数组访问嵌套 JSON
    JsonHashArrow,
    /// `#>>`（PG JSON/JSONB 路径数组访问（文本结果）：`json #>> '{a,b}'` 返回 text）
    JsonHashLongArrow,
    /// `@>`（PG JSON 包含：`json @> json` 返回 bool）
    ///
    /// 语义：左侧 JSON 是否包含右侧 JSON
    JsonAtArrow,
    /// `<@`（PG JSON 被包含：`json <@ json` 返回 bool）
    ///
    /// 语义：左侧 JSON 是否被右侧 JSON 包含
    JsonArrowAt,
}

/// 一元运算符
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnaryOp {
    /// `+expr`
    Plus,
    /// `-expr`
    Minus,
    /// `NOT expr`
    Not,
    /// `~expr`（按位取反）
    BitNot,
}

// =====================================================================
//  事务辅助类型
// =====================================================================

/// 事务隔离级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransactionIsolation {
    /// READ UNCOMMITTED
    ReadUncommitted,
    /// READ COMMITTED（PG 默认）
    ReadCommitted,
    /// REPEATABLE READ
    RepeatableRead,
    /// SERIALIZABLE
    Serializable,
}

/// 事务访问模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransactionAccess {
    /// READ ONLY
    ReadOnly,
    /// READ WRITE（默认）
    ReadWrite,
}

/// COMMENT ON 的对象类型 — Phase TDengine-P2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommentObjectType {
    /// TABLE
    Table,
    /// COLUMN
    Column,
}
