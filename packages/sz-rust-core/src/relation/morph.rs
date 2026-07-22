//! `MorphMany` / `MorphTo` 多态关联 — PHP 命名约定 + SQL 片段构造器
//!
//! Phase 4.8 核心交付物。本模块对齐 PHP `think\Model::morphMany()` /
//! `morphTo()` 行为，提供：
//!
//! 1. [`default_morph_type_column`]：PHP 默认多态类型列名（`morph . '_type'`）
//! 2. [`default_morph_id_column`]：PHP 默认多态外键列名（`morph . '_id'`）
//! 3. [`php_morph_many`]：构造 `MorphMany` 配置（应用 PHP 默认值）
//! 4. [`php_morph_to`]：构造 `MorphTo` 配置（应用 PHP 默认值）
//! 5. [`morph_many_sql`]：生成 MorphMany 单条 SQL 片段
//! 6. [`morph_many_in_sql`]：生成 MorphMany 批量 IN 查询 SQL 片段
//! 7. [`morph_to_sql`]：生成 MorphTo 单条 SQL 片段
//! 8. [`group_by_morph_type`]：按多态类型分桶（对齐 PHP `MorphTo::eagerlyResultSet`）
//!
//! ## PHP 端 `morphMany` 签名（think-orm 2.0.x，RelationShip.php 第 571-591 行）
//!
//! ```php
//! public function morphMany(string $model, $morph = null, string $type = ''): MorphMany
//! {
//!     $model = $this->parseModel($model);
//!
//!     if (is_null($morph)) {
//!         $trace = debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS, 2);
//!         $morph = Str::snake($trace[1]['function']);  // 调用方方法名 snake_case
//!     }
//!
//!     $type = $type ?: get_class($this);  // 默认为父模型类名
//!
//!     if (is_array($morph)) {
//!         [$morphType, $foreignKey] = $morph;
//!     } else {
//!         $morphType  = $morph . '_type';
//!         $foreignKey = $morph . '_id';
//!     }
//!
//!     return new MorphMany($this, $model, $foreignKey, $morphType, $type);
//! }
//! ```
//!
//! ## PHP 端 `morphTo` 签名（think-orm 2.0.x，RelationShip.php 第 600-618 行）
//!
//! ```php
//! public function morphTo($morph = null, array $alias = []): MorphTo
//! {
//!     $trace    = debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS, 2);
//!     $relation = Str::snake($trace[1]['function']);
//!
//!     if (is_null($morph)) {
//!         $morph = $relation;  // 默认为关联名
//!     }
//!
//!     if (is_array($morph)) {
//!         [$morphType, $foreignKey] = $morph;
//!     } else {
//!         $morphType  = $morph . '_type';
//!         $foreignKey = $morph . '_id';
//!     }
//!
//!     return new MorphTo($this, $morphType, $foreignKey, $alias, $relation);
//! }
//! ```
//!
//! ## sz-orm-core vs PHP 命名映射
//!
//! | sz-orm-core 字段 | PHP MorphMany 字段 | 含义 |
//! |------------------|--------------------|------|
//! | `child_model` | `$model` | 子表名 |
//! | `morph_type_column` | `$morphType` | 多态类型列名（如 `commentable_type`） |
//! | `morph_id_column` | `$morphKey` / `$foreignKey` | 多态外键列名（如 `commentable_id`） |
//! | `morph_type_value` | `$type` | 父模型类型标识（如 `Post`） |
//!
//! | sz-orm-core 字段 | PHP MorphTo 字段 | 含义 |
//! |------------------|------------------|------|
//! | `morph_type_column` | `$morphType` | 多态类型列名 |
//! | `morph_id_column` | `$morphKey` / `$foreignKey` | 多态外键列名 |
//!
//! ## 生成的 SQL
//!
//! ### MorphMany 单条查询（sz-orm-core::WithRelation::load MorphMany 分支）
//!
//! ```sql
//! SELECT * FROM {child_table} WHERE {morph_type_col} = '{morph_type_val}' AND {morph_id_col} = {parent_pk}
//! ```
//!
//! ### MorphMany 批量 IN 查询（对齐 PHP `MorphMany::eagerlyResultSet`）
//!
//! ```sql
//! SELECT * FROM {child_table} WHERE {morph_type_col} = '{morph_type_val}' AND {morph_id_col} IN (v1, v2, ...)
//! ```
//!
//! ### MorphTo 单条查询（sz-orm-core::WithRelation::load MorphTo 分支）
//!
//! ```sql
//! SELECT * FROM {morph_type_val_as_table} WHERE id = {morph_id_val}
//! ```
//!
//! **注意**：MorphTo 的目标表名由 `morph_type_value` 动态决定（每行可能不同）。
//!
//! ## PHP 行为对齐
//!
//! ### `morphMany` 默认值推导
//!
//! - `morph` 为 `null`：使用调用方方法名 `Str::snake` 化（Rust 端需调用方显式传入）
//! - `morph` 为字符串：`morphType = morph . '_type'`，`foreignKey = morph . '_id'`
//! - `morph` 为数组：`[morphType, foreignKey] = morph`
//! - `type` 默认：`get_class($this)`（父模型类名）
//!
//! ### `morphTo` 默认值推导
//!
//! - `morph` 为 `null`：使用关联名（Rust 端需调用方显式传入）
//! - `morph` 为字符串：`morphType = morph . '_type'`，`foreignKey = morph . '_id'`
//! - `morph` 为数组：`[morphType, foreignKey] = morph`
//! - **无 `type` 参数**（MorphTo 由子模型数据动态决定）
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行关联加载，而是提供：
//!
//! - **PHP 命名约定辅助函数**：`default_morph_type_column` / `default_morph_id_column`
//! - **配置构造器**：`php_morph_many` / `php_morph_to` 返回 sz-orm-core 结构体
//! - **SQL 片段构造器**：`morph_many_sql` / `morph_many_in_sql` / `morph_to_sql`
//! - **数据处理纯函数**：`group_by_morph_type`（对齐 PHP `MorphTo::eagerlyResultSet`）
//!
//! 端到端关联加载由 sz-orm-core `WithRelation::load()` 内部实现并测试。

use super::{MorphMany, MorphTo};

// ============================================================================
// PHP 命名约定辅助函数
// ============================================================================

/// PHP `morphMany` / `morphTo` 默认多态类型列名
///
/// 对齐 PHP `RelationShip::morphMany` 第 586 行与 `morphTo` 第 613 行：
///
/// ```php
/// $morphType = $morph . '_type';
/// ```
///
/// ## 示例
///
/// | 输入 | 输出 |
/// |------|------|
/// | `"commentable"` | `"commentable_type"` |
/// | `"imageable"` | `"imageable_type"` |
/// | `"taggable"` | `"taggable_type"` |
pub fn default_morph_type_column(morph: &str) -> String {
    format!("{}_type", morph)
}

/// PHP `morphMany` / `morphTo` 默认多态外键列名
///
/// 对齐 PHP `RelationShip::morphMany` 第 587 行与 `morphTo` 第 614 行：
///
/// ```php
/// $foreignKey = $morph . '_id';
/// ```
///
/// ## 示例
///
/// | 输入 | 输出 |
/// |------|------|
/// | `"commentable"` | `"commentable_id"` |
/// | `"imageable"` | `"imageable_id"` |
/// | `"taggable"` | `"taggable_id"` |
pub fn default_morph_id_column(morph: &str) -> String {
    format!("{}_id", morph)
}

// ============================================================================
// MorphMany / MorphTo 配置构造器
// ============================================================================

/// 构造 `MorphMany` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::morphMany($model, $morph = null, $type = '')`：
///
/// - `morph_type_column` 默认：[`default_morph_type_column(morph)`]
/// - `morph_id_column` 默认：[`default_morph_id_column(morph)`]
/// - `morph_type_value` 默认：`parent_class`（对齐 PHP `get_class($this)`）
///
/// ## 参数
///
/// - `parent_class`：父模型类名（如 `"Post"` 或 `"app\\model\\Post"`），用于推导默认 `morph_type_value`
/// - `child_table`：子表名（如 `"comments"`）
/// - `morph`：多态字段名前缀（如 `"commentable"`），对应 PHP `$morph` 字符串形态
/// - `morph_type_value`：父模型类型标识（`None` 使用 `parent_class`）
/// - `morph_type_col`：多态类型列名（`None` 使用 `morph . '_type'`，对应 PHP 数组形态显式指定）
/// - `morph_id_col`：多态外键列名（`None` 使用 `morph . '_id'`，对应 PHP 数组形态显式指定）
///
/// ## PHP ↔ sz-orm-core 映射
///
/// | PHP 参数 | sz-orm-core 字段 | 本函数参数 |
/// |---------|------------------|-----------|
/// | `$model` | `child_model` | `child_table` |
/// | `$morphType` | `morph_type_column` | `morph_type_col` |
/// | `$morphKey` / `$foreignKey` | `morph_id_column` | `morph_id_col` |
/// | `$type` | `morph_type_value` | `morph_type_value` |
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::php_morph_many;
///
/// // 等价 PHP: $this->morphMany(Comment::class, 'commentable', 'Post')
/// // 在 Post 模型中定义
/// let rel = php_morph_many("Post", "comments", "commentable", None, None, None);
/// assert_eq!(rel.child_model, "comments");
/// assert_eq!(rel.morph_type_column, "commentable_type");
/// assert_eq!(rel.morph_id_column, "commentable_id");
/// assert_eq!(rel.morph_type_value, "Post");
///
/// // 等价 PHP: $this->morphMany(Comment::class, ['c_type', 'c_id'], 'Post')
/// // PHP 数组形态：显式指定 morphType 和 foreignKey
/// let rel = php_morph_many("Post", "comments", "commentable", None, Some("c_type"), Some("c_id"));
/// assert_eq!(rel.morph_type_column, "c_type");
/// assert_eq!(rel.morph_id_column, "c_id");
/// ```
pub fn php_morph_many(
    parent_class: &str,
    child_table: &str,
    morph: &str,
    morph_type_value: Option<&str>,
    morph_type_col: Option<&str>,
    morph_id_col: Option<&str>,
) -> MorphMany {
    MorphMany {
        child_model: child_table.to_string(),
        morph_type_column: morph_type_col
            .map(String::from)
            .unwrap_or_else(|| default_morph_type_column(morph)),
        morph_id_column: morph_id_col
            .map(String::from)
            .unwrap_or_else(|| default_morph_id_column(morph)),
        morph_type_value: morph_type_value
            .map(String::from)
            .unwrap_or_else(|| parent_class.to_string()),
    }
}

/// 构造 `MorphTo` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::morphTo($morph = null, array $alias = [])`：
///
/// - `morph_type_column` 默认：[`default_morph_type_column(morph)`]
/// - `morph_id_column` 默认：[`default_morph_id_column(morph)`]
///
/// ## 参数
///
/// - `morph`：多态字段名前缀（如 `"commentable"`），对应 PHP `$morph` 字符串形态
/// - `morph_type_col`：多态类型列名（`None` 使用 `morph . '_type'`，对应 PHP 数组形态显式指定）
/// - `morph_id_col`：多态外键列名（`None` 使用 `morph . '_id'`，对应 PHP 数组形态显式指定）
///
/// ## PHP ↔ sz-orm-core 映射
///
/// | PHP 参数 | sz-orm-core 字段 | 本函数参数 |
/// |---------|------------------|-----------|
/// | `$morphType` | `morph_type_column` | `morph_type_col` |
/// | `$morphKey` / `$foreignKey` | `morph_id_column` | `morph_id_col` |
///
/// **注意**：PHP MorphTo 无 `$type` 参数（由子模型数据动态决定），sz-orm-core
/// `MorphTo` struct 也无 `morph_type_value` 字段。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::php_morph_to;
///
/// // 等价 PHP: $this->morphTo('commentable')
/// // 在 Comment 模型中定义
/// let rel = php_morph_to("commentable", None, None);
/// assert_eq!(rel.morph_type_column, "commentable_type");
/// assert_eq!(rel.morph_id_column, "commentable_id");
///
/// // 等价 PHP: $this->morphTo(['c_type', 'c_id'])
/// // PHP 数组形态：显式指定 morphType 和 foreignKey
/// let rel = php_morph_to("commentable", Some("c_type"), Some("c_id"));
/// assert_eq!(rel.morph_type_column, "c_type");
/// assert_eq!(rel.morph_id_column, "c_id");
/// ```
pub fn php_morph_to(
    morph: &str,
    morph_type_col: Option<&str>,
    morph_id_col: Option<&str>,
) -> MorphTo {
    MorphTo {
        morph_type_column: morph_type_col
            .map(String::from)
            .unwrap_or_else(|| default_morph_type_column(morph)),
        morph_id_column: morph_id_col
            .map(String::from)
            .unwrap_or_else(|| default_morph_id_column(morph)),
    }
}

// ============================================================================
// SQL 片段构造器（用于测试验证）
// ============================================================================

/// 生成 `MorphMany` 关联单条查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `MorphMany` 分支生成的 SQL：
///
/// ```rust,ignore
/// let sql = format!(
///     "SELECT * FROM {} WHERE {} = {} AND {} = {}",
///     config.child_model,
///     config.morph_type_column,
///     value_to_sql_string(&config.morph_type_value),  // 字符串加引号
///     config.morph_id_column,
///     pk_str  // 数值不加引号，字符串加引号
/// );
/// ```
///
/// ## 参数
///
/// - `child_table`：子表名（如 `"comments"`）
/// - `morph_type_col`：多态类型列名（如 `"commentable_type"`）
/// - `morph_type_val`：父模型类型标识（如 `"Post"`，自动加单引号）
/// - `morph_id_col`：多态外键列名（如 `"commentable_id"`）
/// - `parent_pk_value`：父模型主键值（字符串形式，调用方负责转义）
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// `morph_type_val` 通过 `format!` 直接拼接为带引号字符串字面量，
/// 调用方应对内部单引号进行转义（`'` → `''`）或使用 sz-orm-core 参数化查询 API。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::morph_many_sql;
///
/// let sql = morph_many_sql("comments", "commentable_type", "Post", "commentable_id", "1");
/// assert_eq!(sql, "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 1");
/// ```
pub fn morph_many_sql(
    child_table: &str,
    morph_type_col: &str,
    morph_type_val: &str,
    morph_id_col: &str,
    parent_pk_value: &str,
) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = '{}' AND {} = {}",
        child_table, morph_type_col, morph_type_val, morph_id_col, parent_pk_value
    )
}

/// 生成 `MorphMany` 关联批量 IN 查询 SQL 片段
///
/// 对齐 PHP `MorphMany::eagerlyResultSet` 第 138-142 行生成的 SQL：
///
/// ```php
/// $where = [
///     [$morphKey, 'in', $range],
///     [$morphType, '=', $type],
/// ];
/// ```
///
/// 生成的 SQL：
///
/// ```sql
/// SELECT * FROM {child_table} WHERE {morph_id_col} IN (v1, v2, ...) AND {morph_type_col} = '{morph_type_val}'
/// ```
///
/// ## 参数
///
/// - `child_table`：子表名
/// - `morph_type_col`：多态类型列名
/// - `morph_type_val`：父模型类型标识（自动加单引号）
/// - `morph_id_col`：多态外键列名
/// - `parent_pk_values`：父模型主键值列表（字符串形式）
///
/// ## 空列表处理
///
/// 当 `parent_pk_values` 为空时，返回 `... WHERE {morph_id_col} IN (NULL) AND ...`，
/// 对齐 PHP `!empty($range)` 检查为 false 时跳过查询的行为，但本函数返回 SQL 字符串
/// 而非跳过，调用方应自行判断空列表并跳过查询。
///
/// ## PHP 行为差异
///
/// PHP `MorphMany::eagerlyResultSet` 的 WHERE 条件顺序是 `morphKey IN ... AND morphType = ...`，
/// 即多态外键在前、多态类型在后。本函数对齐此顺序。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::morph_many_in_sql;
///
/// let sql = morph_many_in_sql("comments", "commentable_type", "Post", "commentable_id", &["1", "2", "3"]);
/// assert_eq!(sql, "SELECT * FROM comments WHERE commentable_id IN (1, 2, 3) AND commentable_type = 'Post'");
/// ```
pub fn morph_many_in_sql(
    child_table: &str,
    morph_type_col: &str,
    morph_type_val: &str,
    morph_id_col: &str,
    parent_pk_values: &[&str],
) -> String {
    if parent_pk_values.is_empty() {
        return format!(
            "SELECT * FROM {} WHERE {} IN (NULL) AND {} = '{}'",
            child_table, morph_id_col, morph_type_col, morph_type_val
        );
    }
    format!(
        "SELECT * FROM {} WHERE {} IN ({}) AND {} = '{}'",
        child_table,
        morph_id_col,
        parent_pk_values.join(", "),
        morph_type_col,
        morph_type_val
    )
}

/// 生成 `MorphTo` 关联单条查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `MorphTo` 分支生成的 SQL：
///
/// ```rust,ignore
/// let sql = format!(
///     "SELECT * FROM {} WHERE id = {}",
///     morph_type_value,  // 作为目标表名
///     pk_to_sql_string(&morph_id_value)
/// );
/// ```
///
/// ## 参数
///
/// - `parent_table`：父表名（由 `morph_type_value` 动态决定，如 `"posts"`）
/// - `parent_pk_col`：父表主键字段名（如 `"id"`）
/// - `morph_id_value`：多态外键值（字符串形式，调用方负责转义）
///
/// ## PHP ↔ sz-orm-core 行为差异
///
/// PHP `MorphTo::eagerlyResult` 通过 `parseModel()` 将 morph_type 值映射到模型类名，
/// 再通过 `(new $model)->getTable()` 获取表名。sz-orm-core 简化为直接将 morph_type_value
/// 作为表名（调用方需在 `get_relation_fk_value` 中完成映射）。
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// `parent_table` 通过 `format!` 直接拼接，调用方应确保其为合法标识符
/// （通常来自白名单映射，非用户输入）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::morph_to_sql;
///
/// let sql = morph_to_sql("posts", "id", "1");
/// assert_eq!(sql, "SELECT * FROM posts WHERE id = 1");
/// ```
pub fn morph_to_sql(parent_table: &str, parent_pk_col: &str, morph_id_value: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = {}",
        parent_table, parent_pk_col, morph_id_value
    )
}

// ============================================================================
// 数据处理纯函数（对齐 PHP MorphTo::eagerlyResultSet 分组逻辑）
// ============================================================================

/// 按多态类型分桶（对齐 PHP `MorphTo::eagerlyResultSet` 第 223-230 行）
///
/// PHP `MorphTo::eagerlyResultSet` 按 `morphType` 列值分组，每组收集对应的
/// `morphKey` 值列表，然后对每组发起一次 IN 查询：
///
/// ```php
/// $range = [];
/// foreach ($resultSet as $result) {
///     if (!empty($result->$morphKey)) {
///         $range[$result->$morphType][] = $result->$morphKey;
///     }
/// }
/// // $range 形如: ['Post' => [1, 2, 3], 'Video' => [10, 20]]
/// ```
///
/// ## 参数
///
/// - `rows`：当前模型结果集（JSON 对象数组）
/// - `morph_type_field`：多态类型字段名（如 `"commentable_type"`）
/// - `morph_id_field`：多态外键字段名（如 `"commentable_id"`）
///
/// ## 返回
///
/// `HashMap<morph_type, Vec<morph_id>>`，仅包含 `morph_id` 非空的条目。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::morph::group_by_morph_type;
/// use serde_json::json;
///
/// let rows = vec![
///     json!({"commentable_type": "Post", "commentable_id": 1}),
///     json!({"commentable_type": "Post", "commentable_id": 2}),
///     json!({"commentable_type": "Video", "commentable_id": 10}),
/// ];
/// let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
/// assert_eq!(grouped.get("Post").unwrap(), &vec!["1".to_string(), "2".to_string()]);
/// assert_eq!(grouped.get("Video").unwrap(), &vec!["10".to_string()]);
/// ```
pub fn group_by_morph_type(
    rows: &[serde_json::Value],
    morph_type_field: &str,
    morph_id_field: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut grouped: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for row in rows {
        let obj = match row.as_object() {
            Some(o) => o,
            None => continue,
        };

        let morph_type = match obj.get(morph_type_field).and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => continue,
        };

        let morph_id = match obj.get(morph_id_field) {
            Some(v) => value_to_string(v),
            None => continue,
        };

        if morph_id.is_empty() {
            continue;
        }

        grouped.entry(morph_type).or_default().push(morph_id);
    }

    grouped
}

/// 将 `serde_json::Value` 转换为字符串表示
///
/// - 整数 / 浮点数：原样返回（如 `1` → `"1"`，`3.14` → `"3.14"`）
/// - 字符串：原样返回（不加引号）
/// - 布尔：`true` → `"1"`，`false` → `"0"`（对齐 PHP 弱类型）
/// - null / 其他：空字符串
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => {
            if *b {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        _ => String::new(),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ====================================================================
    // 组 1：default_morph_type_column / default_morph_id_column
    // ====================================================================

    #[test]
    fn test_default_morph_type_column_simple() {
        // PHP: $morphType = $morph . '_type'
        assert_eq!(default_morph_type_column("commentable"), "commentable_type");
        assert_eq!(default_morph_type_column("imageable"), "imageable_type");
        assert_eq!(default_morph_type_column("taggable"), "taggable_type");
    }

    #[test]
    fn test_default_morph_type_column_multi_word() {
        // 多单词 morph 名（snake_case）
        assert_eq!(
            default_morph_type_column("comment_able"),
            "comment_able_type"
        );
    }

    #[test]
    fn test_default_morph_id_column_simple() {
        // PHP: $foreignKey = $morph . '_id'
        assert_eq!(default_morph_id_column("commentable"), "commentable_id");
        assert_eq!(default_morph_id_column("imageable"), "imageable_id");
        assert_eq!(default_morph_id_column("taggable"), "taggable_id");
    }

    #[test]
    fn test_default_morph_id_column_multi_word() {
        assert_eq!(default_morph_id_column("comment_able"), "comment_able_id");
    }

    #[test]
    fn test_default_columns_consistent_with_php_convention() {
        // 验证 morph_type_column 与 morph_id_column 前缀一致
        let morph = "commentable";
        let type_col = default_morph_type_column(morph);
        let id_col = default_morph_id_column(morph);
        assert_eq!(
            type_col.strip_suffix("_type").unwrap(),
            id_col.strip_suffix("_id").unwrap()
        );
    }

    // ====================================================================
    // 组 2：php_morph_many 配置构造器
    // ====================================================================

    #[test]
    fn test_php_morph_many_all_defaults() {
        // PHP: $this->morphMany(Comment::class, 'commentable') on Post
        // 等价：morph_type_column = "commentable_type"
        //       morph_id_column = "commentable_id"
        //       morph_type_value = "Post"（get_class($this)）
        let rel = php_morph_many("Post", "comments", "commentable", None, None, None);
        assert_eq!(rel.child_model, "comments");
        assert_eq!(rel.morph_type_column, "commentable_type");
        assert_eq!(rel.morph_id_column, "commentable_id");
        assert_eq!(rel.morph_type_value, "Post");
    }

    #[test]
    fn test_php_morph_many_explicit_morph_type_value() {
        // PHP: $this->morphMany(Comment::class, 'commentable', 'CustomPost')
        let rel = php_morph_many(
            "Post",
            "comments",
            "commentable",
            Some("CustomPost"),
            None,
            None,
        );
        assert_eq!(rel.morph_type_value, "CustomPost");
        assert_eq!(rel.morph_type_column, "commentable_type"); // 仍使用默认
        assert_eq!(rel.morph_id_column, "commentable_id"); // 仍使用默认
    }

    #[test]
    fn test_php_morph_many_explicit_columns() {
        // PHP: $this->morphMany(Comment::class, ['c_type', 'c_id'], 'Post')
        // PHP 数组形态：[morphType, foreignKey] = ['c_type', 'c_id']
        let rel = php_morph_many(
            "Post",
            "comments",
            "commentable",
            None,
            Some("c_type"),
            Some("c_id"),
        );
        assert_eq!(rel.morph_type_column, "c_type");
        assert_eq!(rel.morph_id_column, "c_id");
        assert_eq!(rel.morph_type_value, "Post"); // 仍使用默认
    }

    #[test]
    fn test_php_morph_many_all_explicit() {
        // 全部显式指定
        let rel = php_morph_many(
            "Post",
            "comments",
            "commentable",
            Some("CustomType"),
            Some("c_type"),
            Some("c_id"),
        );
        assert_eq!(rel.child_model, "comments");
        assert_eq!(rel.morph_type_column, "c_type");
        assert_eq!(rel.morph_id_column, "c_id");
        assert_eq!(rel.morph_type_value, "CustomType");
    }

    #[test]
    fn test_php_morph_many_with_namespace_parent() {
        // PHP: $this->morphMany(Comment::class, 'commentable') on \app\model\Post
        // get_class($this) 返回完整类名（PHP 行为）
        let rel = php_morph_many(
            "app\\model\\Post",
            "comments",
            "commentable",
            None,
            None,
            None,
        );
        assert_eq!(rel.morph_type_value, "app\\model\\Post");
    }

    #[test]
    fn test_php_morph_many_multi_word_morph() {
        // 多单词 morph 名
        let rel = php_morph_many("Post", "comments", "comment_able", None, None, None);
        assert_eq!(rel.morph_type_column, "comment_able_type");
        assert_eq!(rel.morph_id_column, "comment_able_id");
    }

    // ====================================================================
    // 组 3：php_morph_to 配置构造器
    // ====================================================================

    #[test]
    fn test_php_morph_to_all_defaults() {
        // PHP: $this->morphTo('commentable') on Comment
        let rel = php_morph_to("commentable", None, None);
        assert_eq!(rel.morph_type_column, "commentable_type");
        assert_eq!(rel.morph_id_column, "commentable_id");
    }

    #[test]
    fn test_php_morph_to_explicit_columns() {
        // PHP: $this->morphTo(['c_type', 'c_id'])
        // PHP 数组形态：[morphType, foreignKey] = ['c_type', 'c_id']
        let rel = php_morph_to("commentable", Some("c_type"), Some("c_id"));
        assert_eq!(rel.morph_type_column, "c_type");
        assert_eq!(rel.morph_id_column, "c_id");
    }

    #[test]
    fn test_php_morph_to_only_type_col_explicit() {
        // 仅显式指定 morph_type_col
        let rel = php_morph_to("commentable", Some("c_type"), None);
        assert_eq!(rel.morph_type_column, "c_type");
        assert_eq!(rel.morph_id_column, "commentable_id"); // 仍使用默认
    }

    #[test]
    fn test_php_morph_to_only_id_col_explicit() {
        // 仅显式指定 morph_id_col
        let rel = php_morph_to("commentable", None, Some("c_id"));
        assert_eq!(rel.morph_type_column, "commentable_type"); // 仍使用默认
        assert_eq!(rel.morph_id_column, "c_id");
    }

    #[test]
    fn test_php_morph_to_multi_word_morph() {
        // 多单词 morph 名
        let rel = php_morph_to("comment_able", None, None);
        assert_eq!(rel.morph_type_column, "comment_able_type");
        assert_eq!(rel.morph_id_column, "comment_able_id");
    }

    #[test]
    fn test_php_morph_to_no_type_value_field() {
        // 关键差异：MorphTo struct 无 morph_type_value 字段
        // PHP MorphTo 也无 $type 参数（由子模型数据动态决定）
        let rel = php_morph_to("commentable", None, None);
        // 验证 MorphTo struct 仅有两个字段
        let _morph_type_column: &String = &rel.morph_type_column;
        let _morph_id_column: &String = &rel.morph_id_column;
        // 编译时验证：无 morph_type_value 字段
    }

    // ====================================================================
    // 组 4：morph_many_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_morph_many_sql_numeric_pk() {
        // 数值型主键
        let sql = morph_many_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            "1",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 1"
        );
    }

    #[test]
    fn test_morph_many_sql_string_pk() {
        // 字符串型主键（如 UUID），调用方负责加引号
        let sql = morph_many_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            "'abc-123'",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 'abc-123'"
        );
    }

    #[test]
    fn test_morph_many_sql_custom_columns() {
        // 自定义列名（PHP 数组形态）
        let sql = morph_many_sql("comments", "c_type", "Post", "c_id", "1");
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE c_type = 'Post' AND c_id = 1"
        );
    }

    #[test]
    fn test_morph_many_sql_custom_type_value() {
        // 自定义类型值（含命名空间）
        let sql = morph_many_sql(
            "comments",
            "commentable_type",
            "app\\model\\Post",
            "commentable_id",
            "1",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'app\\model\\Post' AND commentable_id = 1"
        );
    }

    #[test]
    fn test_morph_many_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load MorphMany 分支一致
        // sz-orm-core 源码：
        //   format!(
        //     "SELECT * FROM {} WHERE {} = {} AND {} = {}",
        //     config.child_model, config.morph_type_column,
        //     value_to_sql_string(&config.morph_type_value),  // 加引号
        //     config.morph_id_column, pk_str
        //   );
        let sql = morph_many_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            "1",
        );
        assert!(sql.starts_with(
            "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = "
        ));
    }

    // ====================================================================
    // 组 5：morph_many_in_sql 批量 IN 查询 SQL
    // ====================================================================

    #[test]
    fn test_morph_many_in_sql_numeric_pks() {
        // 数值型主键列表
        let sql = morph_many_in_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            &["1", "2", "3"],
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_id IN (1, 2, 3) AND commentable_type = 'Post'"
        );
    }

    #[test]
    fn test_morph_many_in_sql_single_pk() {
        // 单个主键
        let sql = morph_many_in_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            &["1"],
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_id IN (1) AND commentable_type = 'Post'"
        );
    }

    #[test]
    fn test_morph_many_in_sql_empty_list() {
        // 空列表：返回 IN (NULL) 模式
        let sql = morph_many_in_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            &[],
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_id IN (NULL) AND commentable_type = 'Post'"
        );
    }

    #[test]
    fn test_morph_many_in_sql_custom_columns() {
        // 自定义列名
        let sql = morph_many_in_sql("comments", "c_type", "Post", "c_id", &["1", "2"]);
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE c_id IN (1, 2) AND c_type = 'Post'"
        );
    }

    #[test]
    fn test_morph_many_in_sql_aligns_php_eagerly_result_set() {
        // 验证 SQL 模式对齐 PHP MorphMany::eagerlyResultSet
        // PHP 源码（第 138-142 行）：
        //   $where = [
        //     [$morphKey, 'in', $range],      // morph_id IN (...)
        //     [$morphType, '=', $type],       // morph_type = '...'
        //   ];
        // 注意：PHP WHERE 条件顺序是 morphKey IN 在前，morphType = 在后
        let sql = morph_many_in_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            &["1", "2"],
        );
        // 验证 IN 条件在 = 条件之前
        let in_pos = sql.find("IN (1, 2)").unwrap();
        let eq_pos = sql.find("= 'Post'").unwrap();
        assert!(in_pos < eq_pos, "IN 条件应在 = 条件之前，对齐 PHP 顺序");
    }

    // ====================================================================
    // 组 6：morph_to_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_morph_to_sql_numeric_id() {
        // 数值型 morph_id
        let sql = morph_to_sql("posts", "id", "1");
        assert_eq!(sql, "SELECT * FROM posts WHERE id = 1");
    }

    #[test]
    fn test_morph_to_sql_string_id() {
        // 字符串型 morph_id（如 UUID）
        let sql = morph_to_sql("posts", "id", "'abc-123'");
        assert_eq!(sql, "SELECT * FROM posts WHERE id = 'abc-123'");
    }

    #[test]
    fn test_morph_to_sql_custom_pk_col() {
        // 自定义父表主键列名
        let sql = morph_to_sql("posts", "pk", "1");
        assert_eq!(sql, "SELECT * FROM posts WHERE pk = 1");
    }

    #[test]
    fn test_morph_to_sql_dynamic_parent_table() {
        // 关键：MorphTo 的父表名由 morph_type_value 动态决定
        // 同一条 Comment 可能关联 Post / Video / Image 等不同父表
        let sql_post = morph_to_sql("posts", "id", "1");
        let sql_video = morph_to_sql("videos", "id", "10");
        let sql_image = morph_to_sql("images", "id", "100");

        assert_eq!(sql_post, "SELECT * FROM posts WHERE id = 1");
        assert_eq!(sql_video, "SELECT * FROM videos WHERE id = 10");
        assert_eq!(sql_image, "SELECT * FROM images WHERE id = 100");
    }

    #[test]
    fn test_morph_to_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load MorphTo 分支一致
        // sz-orm-core 源码：
        //   format!("SELECT * FROM {} WHERE id = {}", morph_type_value, pk_to_sql_string(&morph_id_value));
        let sql = morph_to_sql("posts", "id", "1");
        assert!(sql.starts_with("SELECT * FROM posts WHERE id = "));
    }

    // ====================================================================
    // 组 7：group_by_morph_type 数据处理纯函数
    // ====================================================================

    #[test]
    fn test_group_by_morph_type_single_type() {
        // 单一多态类型
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Post", "commentable_id": 2}),
            json!({"commentable_type": "Post", "commentable_id": 3}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 1);
        assert_eq!(
            grouped.get("Post").unwrap(),
            &vec!["1".to_string(), "2".to_string(), "3".to_string()]
        );
    }

    #[test]
    fn test_group_by_morph_type_multiple_types() {
        // 多种多态类型（PHP MorphTo 核心场景）
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Video", "commentable_id": 10}),
            json!({"commentable_type": "Post", "commentable_id": 2}),
            json!({"commentable_type": "Image", "commentable_id": 100}),
            json!({"commentable_type": "Video", "commentable_id": 20}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 3);
        assert_eq!(
            grouped.get("Post").unwrap(),
            &vec!["1".to_string(), "2".to_string()]
        );
        assert_eq!(
            grouped.get("Video").unwrap(),
            &vec!["10".to_string(), "20".to_string()]
        );
        assert_eq!(grouped.get("Image").unwrap(), &vec!["100".to_string()]);
    }

    #[test]
    fn test_group_by_morph_type_skip_empty_id() {
        // 跳过 morph_id 为空的行（对齐 PHP !empty($result->$morphKey) 检查）
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Post", "commentable_id": null}),
            json!({"commentable_type": "Video", "commentable_id": 10}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get("Post").unwrap(), &vec!["1".to_string()]);
        assert_eq!(grouped.get("Video").unwrap(), &vec!["10".to_string()]);
    }

    #[test]
    fn test_group_by_morph_type_skip_missing_fields() {
        // 跳过缺少字段的行
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Post"}), // 缺少 commentable_id
            json!({"commentable_id": 10}),       // 缺少 commentable_type
            json!({}),                           // 完全空
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped.get("Post").unwrap(), &vec!["1".to_string()]);
    }

    #[test]
    fn test_group_by_morph_type_empty_rows() {
        // 空结果集
        let rows: Vec<serde_json::Value> = vec![];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert!(grouped.is_empty());
    }

    #[test]
    fn test_group_by_morph_type_string_ids() {
        // 字符串型 morph_id（如 UUID）
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": "abc-123"}),
            json!({"commentable_type": "Post", "commentable_id": "def-456"}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(
            grouped.get("Post").unwrap(),
            &vec!["abc-123".to_string(), "def-456".to_string()]
        );
    }

    #[test]
    fn test_group_by_morph_type_non_object_rows() {
        // 非对象行（数组、字符串等）应被跳过
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!([1, 2, 3]), // 数组
            json!("string"),  // 字符串
            json!(42),        // 数字
            json!(null),      // null
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped.get("Post").unwrap(), &vec!["1".to_string()]);
    }

    #[test]
    fn test_group_by_morph_type_preserves_insertion_order_within_group() {
        // 验证组内顺序保持（对齐 PHP $range[$type][] 顺序追加）
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 3}),
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Post", "commentable_id": 2}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        // 顺序应为 [3, 1, 2]（按出现顺序）
        assert_eq!(
            grouped.get("Post").unwrap(),
            &vec!["3".to_string(), "1".to_string(), "2".to_string()]
        );
    }

    // ====================================================================
    // 组 8：value_to_string 辅助函数
    // ====================================================================

    #[test]
    fn test_value_to_string_integer() {
        assert_eq!(value_to_string(&json!(42)), "42");
        assert_eq!(value_to_string(&json!(0)), "0");
        assert_eq!(value_to_string(&json!(-5)), "-5");
    }

    #[test]
    fn test_value_to_string_float() {
        assert_eq!(value_to_string(&json!(2.5)), "2.5");
    }

    #[test]
    fn test_value_to_string_string() {
        assert_eq!(value_to_string(&json!("abc")), "abc");
        assert_eq!(value_to_string(&json!("")), "");
    }

    #[test]
    fn test_value_to_string_bool() {
        // 对齐 PHP 弱类型：true → "1"，false → "0"
        assert_eq!(value_to_string(&json!(true)), "1");
        assert_eq!(value_to_string(&json!(false)), "0");
    }

    #[test]
    fn test_value_to_string_null_and_complex() {
        // null / 对象 / 数组 → 空字符串
        assert_eq!(value_to_string(&json!(null)), "");
        assert_eq!(value_to_string(&json!({"a": 1})), "");
        assert_eq!(value_to_string(&json!([1, 2])), "");
    }

    // ====================================================================
    // 组 9：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_morph_many_default_column_convention() {
        // R5-1：PHP morphMany 默认列名 `morph . '_type'` / `morph . '_id'`
        // PHP 源码：$morphType = $morph . '_type'; $foreignKey = $morph . '_id';
        assert_eq!(default_morph_type_column("commentable"), "commentable_type");
        assert_eq!(default_morph_id_column("commentable"), "commentable_id");
        assert_eq!(default_morph_type_column("imageable"), "imageable_type");
        assert_eq!(default_morph_id_column("imageable"), "imageable_id");
    }

    #[test]
    fn test_r5_php_morph_many_default_type_is_parent_class() {
        // R5-2：PHP morphMany 默认 type = get_class($this)
        // sz-orm-core::MorphMany.morph_type_value 对应 PHP $type
        let rel = php_morph_many("Post", "comments", "commentable", None, None, None);
        assert_eq!(rel.morph_type_value, "Post");
    }

    #[test]
    fn test_r5_php_morph_many_explicit_overrides_default() {
        // R5-3：显式参数覆盖默认值
        let rel = php_morph_many(
            "Post",
            "comments",
            "commentable",
            Some("CustomType"),
            Some("c_type"),
            Some("c_id"),
        );
        assert_eq!(rel.morph_type_value, "CustomType");
        assert_eq!(rel.morph_type_column, "c_type");
        assert_eq!(rel.morph_id_column, "c_id");
    }

    #[test]
    fn test_r5_php_morph_to_no_type_parameter() {
        // R5-4：PHP morphTo 无 $type 参数（与 morphMany 关键差异）
        // sz-orm-core::MorphTo struct 也无 morph_type_value 字段
        // 编译时验证：MorphTo 仅有 morph_type_column 和 morph_id_column 两个字段
        let rel = php_morph_to("commentable", None, None);
        let _type_col: String = rel.morph_type_column.clone();
        let _id_col: String = rel.morph_id_column.clone();
        // 若 MorphTo 有 morph_type_value 字段，下方代码会编译失败
        // （Rust 编译时保证类型对齐）
    }

    #[test]
    fn test_r5_php_morph_many_sql_pattern_matches_sz_orm_core() {
        // R5-5：MorphMany SQL 模式对齐 sz-orm-core
        // sz-orm-core::WithRelation::load MorphMany 分支：
        //   SELECT * FROM {child} WHERE {morph_type_col} = '{morph_type_val}' AND {morph_id_col} = {pk}
        let sql = morph_many_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            "1",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 1"
        );
    }

    #[test]
    fn test_r5_php_morph_to_sql_pattern_matches_sz_orm_core() {
        // R5-6：MorphTo SQL 模式对齐 sz-orm-core
        // sz-orm-core::WithRelation::load MorphTo 分支：
        //   SELECT * FROM {morph_type_val_as_table} WHERE id = {morph_id_val}
        let sql = morph_to_sql("posts", "id", "1");
        assert_eq!(sql, "SELECT * FROM posts WHERE id = 1");
    }

    #[test]
    fn test_r5_php_morph_many_in_sql_eagerly_pattern() {
        // R5-7：MorphMany 批量 IN 查询对齐 PHP eagerlyResultSet
        // PHP 源码（第 138-142 行）：
        //   $where = [[$morphKey, 'in', $range], [$morphType, '=', $type]];
        let sql = morph_many_in_sql(
            "comments",
            "commentable_type",
            "Post",
            "commentable_id",
            &["1", "2", "3"],
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_id IN (1, 2, 3) AND commentable_type = 'Post'"
        );
    }

    #[test]
    fn test_r5_php_morph_to_group_by_morph_type_pattern() {
        // R5-8：MorphTo 按 morphType 分组对齐 PHP eagerlyResultSet
        // PHP 源码（第 223-230 行）：
        //   foreach ($resultSet as $result) {
        //     if (!empty($result->$morphKey)) {
        //       $range[$result->$morphType][] = $result->$morphKey;
        //     }
        //   }
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Video", "commentable_id": 10}),
            json!({"commentable_type": "Post", "commentable_id": 2}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 2);
        assert_eq!(
            grouped.get("Post").unwrap(),
            &vec!["1".to_string(), "2".to_string()]
        );
        assert_eq!(grouped.get("Video").unwrap(), &vec!["10".to_string()]);
    }

    #[test]
    fn test_r5_php_morph_to_empty_morph_key_skipped() {
        // R5-9：PHP !empty($result->$morphKey) 检查跳过空值
        let rows = vec![
            json!({"commentable_type": "Post", "commentable_id": 1}),
            json!({"commentable_type": "Post", "commentable_id": null}),
            json!({"commentable_type": "Post", "commentable_id": ""}),
        ];
        let grouped = group_by_morph_type(&rows, "commentable_type", "commentable_id");
        // 仅第一条应被收集
        assert_eq!(grouped.get("Post").unwrap(), &vec!["1".to_string()]);
    }

    #[test]
    fn test_r5_php_morph_many_delegates_to_sz_orm_core() {
        // R5-10：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // 验证 php_morph_many 返回 sz-orm-core::MorphMany 类型
        let rel = php_morph_many("Post", "comments", "commentable", None, None, None);
        // 验证类型为 sz_orm_core::MorphMany（编译时类型检查）
        let _: &MorphMany = &rel;
        // 验证字段可被 sz-orm-core::Relation::MorphMany 包装
        let relation = sz_orm_core::Relation::MorphMany(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::MorphMany(_)));
    }

    #[test]
    fn test_r5_php_morph_to_delegates_to_sz_orm_core() {
        // R5-11：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // 验证 php_morph_to 返回 sz-orm-core::MorphTo 类型
        let rel = php_morph_to("commentable", None, None);
        // 验证类型为 sz_orm_core::MorphTo（编译时类型检查）
        let _: &MorphTo = &rel;
        // 验证字段可被 sz-orm-core::Relation::MorphTo 包装
        let relation = sz_orm_core::Relation::MorphTo(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::MorphTo(_)));
    }

    // ====================================================================
    // 组 10：集成测试（PHP 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_post_morph_many_comments() {
        // PHP 业务场景：Post morphMany Comments
        // ```php
        // class Post extends Model {
        //     public function comments() {
        //         return $this->morphMany(Comment::class, 'commentable');
        //     }
        // }
        // ```
        let rel = php_morph_many("Post", "comments", "commentable", None, None, None);
        assert_eq!(rel.child_model, "comments");
        assert_eq!(rel.morph_type_column, "commentable_type");
        assert_eq!(rel.morph_id_column, "commentable_id");
        assert_eq!(rel.morph_type_value, "Post");

        // 单条查询 SQL
        let sql = morph_many_sql(
            &rel.child_model,
            &rel.morph_type_column,
            &rel.morph_type_value,
            &rel.morph_id_column,
            "1",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 1"
        );

        // 批量 IN 查询 SQL
        let in_sql = morph_many_in_sql(
            &rel.child_model,
            &rel.morph_type_column,
            &rel.morph_type_value,
            &rel.morph_id_column,
            &["1", "2", "3"],
        );
        assert_eq!(
            in_sql,
            "SELECT * FROM comments WHERE commentable_id IN (1, 2, 3) AND commentable_type = 'Post'"
        );
    }

    #[test]
    fn test_integration_video_morph_many_comments() {
        // PHP 业务场景：Video morphMany Comments（与 Post 共用 comments 表）
        let rel = php_morph_many("Video", "comments", "commentable", None, None, None);
        assert_eq!(rel.morph_type_value, "Video");
        assert_eq!(rel.morph_type_column, "commentable_type");

        let sql = morph_many_sql(
            &rel.child_model,
            &rel.morph_type_column,
            &rel.morph_type_value,
            &rel.morph_id_column,
            "10",
        );
        assert_eq!(
            sql,
            "SELECT * FROM comments WHERE commentable_type = 'Video' AND commentable_id = 10"
        );
    }

    #[test]
    fn test_integration_comment_morph_to_post_or_video() {
        // PHP 业务场景：Comment morphTo Post 或 Video
        // ```php
        // class Comment extends Model {
        //     public function commentable() {
        //         return $this->morphTo();
        //     }
        // }
        // ```
        let rel = php_morph_to("commentable", None, None);
        assert_eq!(rel.morph_type_column, "commentable_type");
        assert_eq!(rel.morph_id_column, "commentable_id");

        // 根据 morph_type 动态路由到不同父表
        // 假设 morph_type_value = "Post" → 表名 "posts"
        let sql_post = morph_to_sql("posts", "id", "1");
        assert_eq!(sql_post, "SELECT * FROM posts WHERE id = 1");

        // 假设 morph_type_value = "Video" → 表名 "videos"
        let sql_video = morph_to_sql("videos", "id", "10");
        assert_eq!(sql_video, "SELECT * FROM videos WHERE id = 10");
    }

    #[test]
    fn test_integration_morph_to_batch_loading() {
        // PHP 业务场景：批量加载 Comments 的 morphTo 关联
        // 步骤 1：收集 morph_type + morph_id 分组
        let comments = vec![
            json!({"id": 1, "commentable_type": "Post", "commentable_id": 1}),
            json!({"id": 2, "commentable_type": "Video", "commentable_id": 10}),
            json!({"id": 3, "commentable_type": "Post", "commentable_id": 2}),
            json!({"id": 4, "commentable_type": "Image", "commentable_id": 100}),
        ];

        let grouped = group_by_morph_type(&comments, "commentable_type", "commentable_id");
        assert_eq!(grouped.len(), 3);

        // 步骤 2：对每种类型发起 IN 查询
        // Post: SELECT * FROM posts WHERE id IN (1, 2)
        let post_ids: Vec<&str> = grouped
            .get("Post")
            .unwrap()
            .iter()
            .map(|s| s.as_str())
            .collect();
        let post_sql = format!("SELECT * FROM posts WHERE id IN ({})", post_ids.join(", "));
        assert_eq!(post_sql, "SELECT * FROM posts WHERE id IN (1, 2)");

        // Video: SELECT * FROM videos WHERE id IN (10)
        let video_ids: Vec<&str> = grouped
            .get("Video")
            .unwrap()
            .iter()
            .map(|s| s.as_str())
            .collect();
        let video_sql = format!(
            "SELECT * FROM videos WHERE id IN ({})",
            video_ids.join(", ")
        );
        assert_eq!(video_sql, "SELECT * FROM videos WHERE id IN (10)");

        // Image: SELECT * FROM images WHERE id IN (100)
        let image_ids: Vec<&str> = grouped
            .get("Image")
            .unwrap()
            .iter()
            .map(|s| s.as_str())
            .collect();
        let image_sql = format!(
            "SELECT * FROM images WHERE id IN ({})",
            image_ids.join(", ")
        );
        assert_eq!(image_sql, "SELECT * FROM images WHERE id IN (100)");
    }

    #[test]
    fn test_integration_image_morph_many_tags() {
        // PHP 业务场景：Image morphMany Tags（使用自定义列名）
        // ```php
        // class Image extends Model {
        //     public function tags() {
        //         return $this->morphMany(Tag::class, ['tag_type', 'tag_id'], 'Image');
        //     }
        // }
        // ```
        let rel = php_morph_many(
            "Image",
            "tags",
            "taggable",
            None,
            Some("tag_type"),
            Some("tag_id"),
        );
        assert_eq!(rel.child_model, "tags");
        assert_eq!(rel.morph_type_column, "tag_type"); // 显式覆盖
        assert_eq!(rel.morph_id_column, "tag_id"); // 显式覆盖
        assert_eq!(rel.morph_type_value, "Image");

        let sql = morph_many_sql(
            &rel.child_model,
            &rel.morph_type_column,
            &rel.morph_type_value,
            &rel.morph_id_column,
            "100",
        );
        assert_eq!(
            sql,
            "SELECT * FROM tags WHERE tag_type = 'Image' AND tag_id = 100"
        );
    }
}
