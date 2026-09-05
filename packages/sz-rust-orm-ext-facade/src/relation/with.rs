// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 关联预加载（with）— 批量 IN 查询 SQL 片段构造器 + 数据处理纯函数
//!
//! 本模块对齐 PHP `think\Model::with()` + `eagerlyResultSet()`
//! 的批量预加载机制，**强制使用 IN 查询避免 N+1 问题**。
//!
//! ## PHP 端 with() 机制（think-orm 2.0.x）
//!
//! PHP `with()` 仅存储关系名数组到 `options['with']`，真正的预加载在结果集转模型时触发：
//!
//! ```php
//! // ModelRelationQuery.php 第 217-225 行
//! public function with($with)
//! {
//!     if (empty($this->model) || empty($with)) {
//!         return $this;
//!     }
//!     $this->options['with'] = (array) $with;
//!     return $this;
//! }
//! ```
//!
//! ### 批量预加载核心（`eagerlyResultSet`）
//!
//! PHP `RelationShip::eagerlyResultSet` 第 252 行遍历关系名，调用各关系对象的
//! `eagerlyResultSet()` 方法。以 `HasMany` 为例（`HasMany.php` 第 77-102 行）：
//!
//! ```php
//! public function eagerlyResultSet(array &$resultSet, string $relation, ...): void
//! {
//!     $localKey = $this->localKey;
//!     $range    = [];
//!     foreach ($resultSet as $result) {
//!         if (isset($result->$localKey)) {
//!             $range[] = $result->$localKey;            // 收集所有主键
//!         }
//!     }
//!     if (!empty($range)) {
//!         $data = $this->eagerlyOneToMany([
//!             [$this->foreignKey, 'in', $range],        // 真正的 IN 查询
//!         ], $subRelation, $closure, $cache);
//!         foreach ($resultSet as $result) {
//!             $pk = $result->$localKey;
//!             if (!isset($data[$pk])) {
//!                 $data[$pk] = [];
//!             }
//!             $result->setRelation($relation, $this->resultSetBuild($data[$pk], clone $this->parent));
//!         }
//!     }
//! }
//! ```
//!
//! ### 嵌套关系语法解析
//!
//! PHP `RelationShip::eagerlyResultSet` 第 266-270 行解析 `"relation.sub"` 语法：
//!
//! ```php
//! if (strpos($relation, '.')) {
//!     [$relation, $subRelation] = explode('.', $relation, 2);
//!     $subRelation = [$subRelation];
//! }
//! ```
//!
//! ## 本模块提供的函数
//!
//! ### 1. IN 查询 SQL 片段构造器（4 种关联类型）
//!
//! | 关联类型 | 函数 | SQL 模式 |
//! |---------|------|---------|
//! | HasMany | [`has_many_in_sql`] | `SELECT * FROM {child} WHERE {fk} IN (v1, v2, ...)` |
//! | HasOne | [`has_one_in_sql`] | 同 HasMany |
//! | BelongsTo | [`belongs_to_in_sql`] | `SELECT * FROM {parent} WHERE {parent_pk} IN (v1, v2, ...)` |
//! | BelongsToMany | [`belongs_to_many_in_sql`] | `SELECT t.* FROM {target} t INNER JOIN {junction} j ON t.{target_pk} = j.{other_key} WHERE j.{foreign_key} IN (v1, v2, ...)` |
//!
//! ### 2. 数据处理纯函数（对齐 PHP `$range` 收集与 `$data[pk]` 分桶）
//!
//! - [`collect_pk_values`]：从结果集收集主键值（对齐 PHP `$range`）
//! - [`group_by_fk`]：按外键分桶（对齐 PHP `$data[$pk]`）
//!
//! ### 3. PHP `with()` 语法解析辅助
//!
//! - [`parse_with_notation`]：解析 `"relation.sub"` 嵌套关系语法
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行关联加载，而是提供：
//!
//! - **IN 查询 SQL 片段构造器**：生成批量 IN 查询 SQL，对齐 PHP `eagerlyOneToMany`
//! - **数据处理纯函数**：主键收集 + 外键分桶，对齐 PHP `$range` / `$data[$pk]`
//! - **语法解析辅助**：解析 PHP `with(['relation.sub'])` 嵌套语法
//!
//! 端到端批量预加载由调用方协调：收集主键 → 生成 IN SQL → 执行查询 → 分桶回填。
//! 单条模型加载由 sz-orm-core `WithRelation::load()` 内部覆盖（N+1 模式，本模块不重复）。
//!
//! ## SQL 注入防护
//!
//! 本模块的 SQL 片段构造器通过 `format!` 拼接参数到 SQL，**仅用于测试验证 SQL 生成模式**，
//! **不应直接用于业务代码**。业务代码应通过 sz-orm-core 的参数化查询 API 执行 SQL，
//! 或使用 [`sanitize_pk_value`] 对主键值进行转义后再拼接。

use serde_json::Value;
use std::collections::HashMap;

// ============================================================================
// IN 查询 SQL 片段构造器（4 种关联类型）
// ============================================================================

/// 生成 `HasMany` 关联批量预加载 IN 查询 SQL 片段
///
/// 对齐 PHP `HasMany::eagerlyResultSet` 第 87 行 `[$this->foreignKey, 'in', $range]`
/// 生成的 SQL：`SELECT * FROM {child_table} WHERE {foreign_key} IN (v1, v2, ...)`
///
/// ## 参数
///
/// - `child_table`：子表名（如 `"orders"`）
/// - `foreign_key`：外键字段名（如 `"user_id"`）
/// - `parent_pk_values`：父模型主键值列表（字符串形式）
///
/// ## 空列表处理
///
/// 当 `parent_pk_values` 为空时，返回 `SELECT * FROM {child} WHERE {fk} IN (NULL)`，
/// 对齐 PHP `!empty($range)` 检查为 false 时跳过查询的行为，但本函数返回 SQL 字符串
/// 而非跳过，调用方应自行判断空列表并跳过查询。
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// 主键值通过 `format!` 直接拼接，存在 SQL 注入风险，调用方应通过
/// [`sanitize_pk_value`] 转义或使用 sz-orm-core 参数化查询 API。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::has_many_in_sql;
///
/// let sql = has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
/// assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");
/// ```
pub fn has_many_in_sql(child_table: &str, foreign_key: &str, parent_pk_values: &[&str]) -> String {
    if parent_pk_values.is_empty() {
        return format!(
            "SELECT * FROM {} WHERE {} IN (NULL)",
            child_table, foreign_key
        );
    }
    format!(
        "SELECT * FROM {} WHERE {} IN ({})",
        child_table,
        foreign_key,
        parent_pk_values.join(", ")
    )
}

/// 生成 `HasOne` 关联批量预加载 IN 查询 SQL 片段
///
/// 与 [`has_many_in_sql`] SQL 模式**完全相同**（对齐 PHP `HasOne::eagerlyResultSet`
/// 与 `HasMany::eagerlyResultSet` 使用相同的 `eagerlyOneToMany` 方法）。
/// 区别仅在返回语义：HasOne 每个主键取第一行，HasMany 取所有行。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::has_one_in_sql;
///
/// let sql = has_one_in_sql("profiles", "user_id", &["1", "2"]);
/// assert_eq!(sql, "SELECT * FROM profiles WHERE user_id IN (1, 2)");
/// ```
pub fn has_one_in_sql(child_table: &str, foreign_key: &str, parent_pk_values: &[&str]) -> String {
    has_many_in_sql(child_table, foreign_key, parent_pk_values)
}

/// 生成 `BelongsTo` 关联批量预加载 IN 查询 SQL 片段
///
/// 对齐 PHP `BelongsTo::eagerlyResultSet` 使用 `[$this->localKey, 'in', $range]`
/// 生成的 SQL：`SELECT * FROM {parent_table} WHERE {parent_pk} IN (v1, v2, ...)`
///
/// ## 参数
///
/// - `parent_table`：父表名（如 `"depts"`）
/// - `parent_pk`：父表主键字段名（如 `"id"`）
/// - `fk_values`：当前模型外键值列表（字符串形式）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::belongs_to_in_sql;
///
/// let sql = belongs_to_in_sql("depts", "id", &["1", "2", "3"]);
/// assert_eq!(sql, "SELECT * FROM depts WHERE id IN (1, 2, 3)");
/// ```
pub fn belongs_to_in_sql(parent_table: &str, parent_pk: &str, fk_values: &[&str]) -> String {
    if fk_values.is_empty() {
        return format!(
            "SELECT * FROM {} WHERE {} IN (NULL)",
            parent_table, parent_pk
        );
    }
    format!(
        "SELECT * FROM {} WHERE {} IN ({})",
        parent_table,
        parent_pk,
        fk_values.join(", ")
    )
}

/// 生成 `BelongsToMany` 关联批量预加载 IN 查询 SQL 片段
///
/// 对齐 PHP `BelongsToMany::eagerlyResultSet` 使用
/// `[$this->localKey, 'in', $range]`（注意：PHP localKey 对应 sz-orm-core foreign_key）
/// 生成的 SQL：
///
/// ```sql
/// SELECT t.* FROM {target} t
/// INNER JOIN {junction} j ON t.{target_pk} = j.{other_key}
/// WHERE j.{foreign_key} IN (v1, v2, ...)
/// ```
///
/// ## 参数
///
/// - `target_table`：目标表名（如 `"roles"`）
/// - `junction_table`：中间表名（如 `"user_role"`）
/// - `target_pk`：目标表主键字段名（如 `"id"`）
/// - `other_key`：中间表中指向目标模型的 FK（如 `"role_id"`）
/// - `foreign_key`：中间表中指向当前模型的 FK（如 `"user_id"`）
/// - `current_pk_values`：当前模型主键值列表（字符串形式）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::belongs_to_many_in_sql;
///
/// let sql = belongs_to_many_in_sql("roles", "user_role", "id", "role_id", "user_id", &["1", "2"]);
/// assert_eq!(sql, "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2)");
/// ```
pub fn belongs_to_many_in_sql(
    target_table: &str,
    junction_table: &str,
    target_pk: &str,
    other_key: &str,
    foreign_key: &str,
    current_pk_values: &[&str],
) -> String {
    if current_pk_values.is_empty() {
        return format!(
            "SELECT t.* FROM {} t INNER JOIN {} j ON t.{} = j.{} WHERE j.{} IN (NULL)",
            target_table, junction_table, target_pk, other_key, foreign_key
        );
    }
    format!(
        "SELECT t.* FROM {} t INNER JOIN {} j ON t.{} = j.{} WHERE j.{} IN ({})",
        target_table,
        junction_table,
        target_pk,
        other_key,
        foreign_key,
        current_pk_values.join(", ")
    )
}

// ============================================================================
// 主键值转义辅助（SQL 注入防护）
// ============================================================================

/// 转义主键值用于 IN 查询拼接
///
/// 数值型主键原样返回，字符串型主键包裹单引号并转义内部单引号。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::sanitize_pk_value;
///
/// assert_eq!(sanitize_pk_value("1"), "1");
/// assert_eq!(sanitize_pk_value("abc"), "'abc'");
/// assert_eq!(sanitize_pk_value("a'b"), "'a''b'");
/// ```
pub fn sanitize_pk_value(value: &str) -> String {
    // 数值型主键（整数或浮点数）原样返回
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        value.to_string()
    } else {
        // 字符串型主键：包裹单引号并转义内部单引号（对齐 SQL 标准 '' 转义）
        format!("'{}'", value.replace('\'', "''"))
    }
}

// ============================================================================
// 数据处理纯函数（对齐 PHP $range 收集与 $data[pk] 分桶）
// ============================================================================

/// 从结果集收集主键值（对齐 PHP `$range` 收集逻辑）
///
/// 对齐 PHP `HasMany::eagerlyResultSet` 第 81-86 行：
///
/// ```php
/// $range = [];
/// foreach ($resultSet as $result) {
///     if (isset($result->$localKey)) {
///         $range[] = $result->$localKey;
///     }
/// }
/// ```
///
/// ## 参数
///
/// - `rows`：主表结果集（JSON 对象数组）
/// - `pk_field`：主键字段名（如 `"id"`）
///
/// ## 返回
///
/// 主键值列表（字符串形式），跳过 `null` 或不存在的字段。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::collect_pk_values;
/// use serde_json::json;
///
/// let rows = vec![
///     json!({"id": 1, "name": "Alice"}),
///     json!({"id": 2, "name": "Bob"}),
///     json!({"id": 3, "name": "Charlie"}),
/// ];
/// let pks = collect_pk_values(&rows, "id");
/// assert_eq!(pks, vec!["1".to_string(), "2".to_string(), "3".to_string()]);
/// ```
pub fn collect_pk_values(rows: &[Value], pk_field: &str) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            row.get(pk_field)
                .filter(|v| !v.is_null())
                .map(value_to_pk_string)
        })
        .collect()
}

/// 按外键分桶（对齐 PHP `$data[$pk]` 分桶逻辑）
///
/// 对齐 PHP `HasMany::eagerlyResultSet` 第 93-100 行：
///
/// ```php
/// foreach ($resultSet as $result) {
///     $pk = $result->$localKey;
///     if (!isset($data[$pk])) {
///         $data[$pk] = [];
///     }
///     $result->setRelation($relation, $this->resultSetBuild($data[$pk], clone $this->parent));
/// }
/// ```
///
/// 本函数对关联表结果集按外键字段分组，返回 `HashMap<外键值, Vec<行>>`。
///
/// ## 参数
///
/// - `rows`：关联表结果集（JSON 对象数组）
/// - `fk_field`：外键字段名（如 `"user_id"`）
///
/// ## 返回
///
/// `HashMap<String, Vec<Value>>`，键为外键值字符串，值为该外键对应的所有行。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::group_by_fk;
/// use serde_json::json;
///
/// let rows = vec![
///     json!({"id": 101, "user_id": 1, "name": "Order A"}),
///     json!({"id": 102, "user_id": 2, "name": "Order B"}),
///     json!({"id": 103, "user_id": 1, "name": "Order C"}),
/// ];
/// let grouped = group_by_fk(rows, "user_id");
/// assert_eq!(grouped.get("1").unwrap().len(), 2);
/// assert_eq!(grouped.get("2").unwrap().len(), 1);
/// ```
pub fn group_by_fk(rows: Vec<Value>, fk_field: &str) -> HashMap<String, Vec<Value>> {
    let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows {
        let fk_value = row
            .get(fk_field)
            .filter(|v| !v.is_null())
            .map(value_to_pk_string)
            .unwrap_or_default();
        grouped.entry(fk_value).or_default().push(row);
    }
    grouped
}

// ============================================================================
// PHP with() 语法解析辅助
// ============================================================================

/// 解析 PHP `with()` 嵌套关系语法
///
/// 对齐 PHP `RelationShip::eagerlyResultSet` 第 266-270 行：
///
/// ```php
/// if (strpos($relation, '.')) {
///     [$relation, $subRelation] = explode('.', $relation, 2);
///     $subRelation = [$subRelation];
/// }
/// ```
///
/// ## 参数
///
/// - `with`：关系名字符串（如 `"category"` 或 `"items.product"`）
///
/// ## 返回
///
/// `(主关系名, Option<子关系名>)`，子关系名为 `Some` 时表示存在嵌套。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::with::parse_with_notation;
///
/// assert_eq!(parse_with_notation("category"), ("category", None));
/// assert_eq!(parse_with_notation("items.product"), ("items", Some("product")));
/// assert_eq!(parse_with_notation("a.b.c"), ("a", Some("b.c"))); // 仅按第一个 . 分割
/// ```
pub fn parse_with_notation(with: &str) -> (&str, Option<&str>) {
    match with.split_once('.') {
        Some((relation, sub)) => (relation, Some(sub)),
        None => (with, None),
    }
}

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 将 `serde_json::Value` 转换为主键字符串
///
/// - 数值型：直接转字符串（如 `1` → `"1"`）
/// - 字符串型：原样返回（如 `"abc"` → `"abc"`）
/// - 布尔型：转 `"true"` / `"false"`
/// - null/对象/数组：返回空字符串（不应出现在主键字段）
fn value_to_pk_string(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
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
    // 组 1：has_many_in_sql IN 查询 SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_has_many_in_sql_multiple_pks() {
        // PHP: WHERE user_id IN (1, 2, 3)
        let sql = has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");
    }

    #[test]
    fn test_has_many_in_sql_single_pk() {
        let sql = has_many_in_sql("orders", "user_id", &["1"]);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1)");
    }

    #[test]
    fn test_has_many_in_sql_empty_pks() {
        // 空列表返回 IN (NULL)，调用方应自行判断并跳过查询
        let sql = has_many_in_sql("orders", "user_id", &[]);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (NULL)");
    }

    #[test]
    fn test_has_many_in_sql_string_pks() {
        // 字符串主键（如 UUID）— 调用方负责加引号
        let sql = has_many_in_sql("orders", "user_id", &["'abc-1'", "'abc-2'"]);
        assert_eq!(
            sql,
            "SELECT * FROM orders WHERE user_id IN ('abc-1', 'abc-2')"
        );
    }

    #[test]
    fn test_has_many_in_sql_custom_foreign_key() {
        let sql = has_many_in_sql("orders", "uid", &["1", "2"]);
        assert_eq!(sql, "SELECT * FROM orders WHERE uid IN (1, 2)");
    }

    #[test]
    fn test_has_many_in_sql_multi_word_table() {
        let sql = has_many_in_sql("order_items", "order_id", &["1", "2"]);
        assert_eq!(sql, "SELECT * FROM order_items WHERE order_id IN (1, 2)");
    }

    // ====================================================================
    // 组 2：has_one_in_sql（与 has_many_in_sql 算法相同）
    // ====================================================================

    #[test]
    fn test_has_one_in_sql_multiple_pks() {
        // HasOne 与 HasMany SQL 模式完全相同
        let sql = has_one_in_sql("profiles", "user_id", &["1", "2", "3"]);
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id IN (1, 2, 3)");
    }

    #[test]
    fn test_has_one_in_sql_empty_pks() {
        let sql = has_one_in_sql("profiles", "user_id", &[]);
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id IN (NULL)");
    }

    #[test]
    fn test_has_one_in_sql_equals_has_many_in_sql() {
        // 验证 HasOne 与 HasMany IN 查询 SQL 完全相同
        let has_one = has_one_in_sql("profiles", "user_id", &["1", "2"]);
        let has_many = has_many_in_sql("profiles", "user_id", &["1", "2"]);
        assert_eq!(has_one, has_many);
    }

    // ====================================================================
    // 组 3：belongs_to_in_sql IN 查询 SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_belongs_to_in_sql_multiple_fks() {
        // PHP: WHERE id IN (1, 2, 3)（父表主键）
        let sql = belongs_to_in_sql("depts", "id", &["1", "2", "3"]);
        assert_eq!(sql, "SELECT * FROM depts WHERE id IN (1, 2, 3)");
    }

    #[test]
    fn test_belongs_to_in_sql_single_fk() {
        let sql = belongs_to_in_sql("depts", "id", &["1"]);
        assert_eq!(sql, "SELECT * FROM depts WHERE id IN (1)");
    }

    #[test]
    fn test_belongs_to_in_sql_empty_fks() {
        let sql = belongs_to_in_sql("depts", "id", &[]);
        assert_eq!(sql, "SELECT * FROM depts WHERE id IN (NULL)");
    }

    #[test]
    fn test_belongs_to_in_sql_custom_parent_pk() {
        // 自定义父表主键字段名
        let sql = belongs_to_in_sql("categories", "cid", &["1", "2"]);
        assert_eq!(sql, "SELECT * FROM categories WHERE cid IN (1, 2)");
    }

    #[test]
    fn test_belongs_to_in_sql_multi_word_table() {
        let sql = belongs_to_in_sql("user_profiles", "id", &["1", "2"]);
        assert_eq!(sql, "SELECT * FROM user_profiles WHERE id IN (1, 2)");
    }

    // ====================================================================
    // 组 4：belongs_to_many_in_sql IN 查询 SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_belongs_to_many_in_sql_multiple_pks() {
        let sql = belongs_to_many_in_sql(
            "roles",
            "user_role",
            "id",
            "role_id",
            "user_id",
            &["1", "2", "3"],
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2, 3)"
        );
    }

    #[test]
    fn test_belongs_to_many_in_sql_single_pk() {
        let sql = belongs_to_many_in_sql("roles", "user_role", "id", "role_id", "user_id", &["1"]);
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1)"
        );
    }

    #[test]
    fn test_belongs_to_many_in_sql_empty_pks() {
        let sql = belongs_to_many_in_sql("roles", "user_role", "id", "role_id", "user_id", &[]);
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (NULL)"
        );
    }

    #[test]
    fn test_belongs_to_many_in_sql_custom_keys() {
        // 自定义外键（foreign_key=uid, other_key=rid, target_pk=pk）
        let sql = belongs_to_many_in_sql("roles", "user_role", "pk", "rid", "uid", &["1", "2"]);
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.pk = j.rid WHERE j.uid IN (1, 2)"
        );
    }

    #[test]
    fn test_belongs_to_many_in_sql_multi_word_tables() {
        let sql = belongs_to_many_in_sql(
            "order_items",
            "order_item_tag",
            "id",
            "tag_id",
            "order_item_id",
            &["1", "2"],
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM order_items t INNER JOIN order_item_tag j ON t.id = j.tag_id WHERE j.order_item_id IN (1, 2)"
        );
    }

    #[test]
    fn test_belongs_to_many_in_sql_aligns_php_pattern() {
        // 验证 SQL 模式对齐 PHP BelongsToMany::eagerlyResultSet
        // PHP 使用 [$this->localKey, 'in', $range]，对应 sz-orm-core foreign_key
        let sql = belongs_to_many_in_sql(
            "roles",
            "user_role",
            "id",
            "role_id",
            "user_id",
            &["1", "2"],
        );
        assert!(sql.starts_with("SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN ("));
    }

    // ====================================================================
    // 组 5：sanitize_pk_value 主键值转义
    // ====================================================================

    #[test]
    fn test_sanitize_pk_value_numeric_integer() {
        assert_eq!(sanitize_pk_value("1"), "1");
        assert_eq!(sanitize_pk_value("12345"), "12345");
        assert_eq!(sanitize_pk_value("-100"), "-100");
    }

    #[test]
    fn test_sanitize_pk_value_numeric_float() {
        assert_eq!(sanitize_pk_value("1.5"), "1.5");
        assert_eq!(sanitize_pk_value("-0.5"), "-0.5");
    }

    #[test]
    fn test_sanitize_pk_value_string() {
        assert_eq!(sanitize_pk_value("abc"), "'abc'");
        assert_eq!(sanitize_pk_value("uuid-123"), "'uuid-123'");
    }

    #[test]
    fn test_sanitize_pk_value_string_with_quote() {
        // SQL 标准转义：' → ''
        assert_eq!(sanitize_pk_value("a'b"), "'a''b'");
        assert_eq!(sanitize_pk_value("'"), "''''");
    }

    #[test]
    fn test_sanitize_pk_value_empty_string() {
        // 空字符串非数值型，包裹单引号
        assert_eq!(sanitize_pk_value(""), "''");
    }

    // ====================================================================
    // 组 6：collect_pk_values 主键收集
    // ====================================================================

    #[test]
    fn test_collect_pk_values_integer_pks() {
        let rows = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": 2, "name": "Bob"}),
            json!({"id": 3, "name": "Charlie"}),
        ];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["1", "2", "3"]);
    }

    #[test]
    fn test_collect_pk_values_string_pks() {
        let rows = vec![
            json!({"id": "uuid-1", "name": "Alice"}),
            json!({"id": "uuid-2", "name": "Bob"}),
        ];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["uuid-1", "uuid-2"]);
    }

    #[test]
    fn test_collect_pk_values_skip_null() {
        // 对齐 PHP isset($result->$localKey) 检查
        let rows = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": null, "name": "Bob"}), // 跳过 null
            json!({"id": 3, "name": "Charlie"}),
        ];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["1", "3"]);
    }

    #[test]
    fn test_collect_pk_values_skip_missing_field() {
        // 对齐 PHP isset($result->$localKey) 检查缺失字段
        let rows = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"name": "Bob"}), // 缺失 id 字段
            json!({"id": 3, "name": "Charlie"}),
        ];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["1", "3"]);
    }

    #[test]
    fn test_collect_pk_values_empty_rows() {
        let rows: Vec<Value> = vec![];
        let pks = collect_pk_values(&rows, "id");
        assert!(pks.is_empty());
    }

    #[test]
    fn test_collect_pk_values_custom_pk_field() {
        let rows = vec![
            json!({"uid": 1, "name": "Alice"}),
            json!({"uid": 2, "name": "Bob"}),
        ];
        let pks = collect_pk_values(&rows, "uid");
        assert_eq!(pks, vec!["1", "2"]);
    }

    #[test]
    fn test_collect_pk_values_dedup_not_applied() {
        // collect_pk_values 不去重，调用方按需处理
        let rows = vec![json!({"id": 1}), json!({"id": 1}), json!({"id": 2})];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["1", "1", "2"]);
    }

    // ====================================================================
    // 组 7：group_by_fk 外键分桶
    // ====================================================================

    #[test]
    fn test_group_by_fk_basic_grouping() {
        let rows = vec![
            json!({"id": 101, "user_id": 1, "name": "Order A"}),
            json!({"id": 102, "user_id": 2, "name": "Order B"}),
            json!({"id": 103, "user_id": 1, "name": "Order C"}),
        ];
        let grouped = group_by_fk(rows, "user_id");
        assert_eq!(grouped.get("1").unwrap().len(), 2);
        assert_eq!(grouped.get("2").unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_fk_string_fk() {
        let rows = vec![
            json!({"id": 101, "user_id": "uuid-1"}),
            json!({"id": 102, "user_id": "uuid-2"}),
            json!({"id": 103, "user_id": "uuid-1"}),
        ];
        let grouped = group_by_fk(rows, "user_id");
        assert_eq!(grouped.get("uuid-1").unwrap().len(), 2);
        assert_eq!(grouped.get("uuid-2").unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_fk_skip_null() {
        // 对齐 PHP isset 检查：null 外键归入空字符串桶
        let rows = vec![
            json!({"id": 101, "user_id": 1}),
            json!({"id": 102, "user_id": null}),
            json!({"id": 103, "user_id": 1}),
        ];
        let grouped = group_by_fk(rows, "user_id");
        assert_eq!(grouped.get("1").unwrap().len(), 2);
        // null 外键归入空字符串桶（不应匹配任何主键）
        assert_eq!(grouped.get("").unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_fk_empty_rows() {
        let rows: Vec<Value> = vec![];
        let grouped = group_by_fk(rows, "user_id");
        assert!(grouped.is_empty());
    }

    #[test]
    fn test_group_by_fk_custom_fk_field() {
        let rows = vec![json!({"id": 101, "uid": 1}), json!({"id": 102, "uid": 2})];
        let grouped = group_by_fk(rows, "uid");
        assert_eq!(grouped.get("1").unwrap().len(), 1);
        assert_eq!(grouped.get("2").unwrap().len(), 1);
    }

    #[test]
    fn test_group_by_fk_preserves_row_data() {
        // 验证分桶后行数据完整保留
        let rows = vec![json!({"id": 101, "user_id": 1, "name": "Order A", "amount": 100.5})];
        let grouped = group_by_fk(rows, "user_id");
        let bucket = grouped.get("1").unwrap();
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0]["id"], json!(101));
        assert_eq!(bucket[0]["name"], json!("Order A"));
        assert_eq!(bucket[0]["amount"], json!(100.5));
    }

    // ====================================================================
    // 组 8：parse_with_notation PHP with() 语法解析
    // ====================================================================

    #[test]
    fn test_parse_with_notation_simple_relation() {
        assert_eq!(parse_with_notation("category"), ("category", None));
        assert_eq!(parse_with_notation("dept"), ("dept", None));
    }

    #[test]
    fn test_parse_with_notation_nested_relation() {
        // 对齐 PHP explode('.', $relation, 2)：仅按第一个 . 分割
        assert_eq!(
            parse_with_notation("items.product"),
            ("items", Some("product"))
        );
        assert_eq!(
            parse_with_notation("user.profile"),
            ("user", Some("profile"))
        );
    }

    #[test]
    fn test_parse_with_notation_deep_nested() {
        // 深层嵌套：仅按第一个 . 分割，剩余部分作为 sub 字符串
        assert_eq!(parse_with_notation("a.b.c"), ("a", Some("b.c")));
        assert_eq!(
            parse_with_notation("user.orders.items"),
            ("user", Some("orders.items"))
        );
    }

    #[test]
    fn test_parse_with_notation_empty_string() {
        assert_eq!(parse_with_notation(""), ("", None));
    }

    #[test]
    fn test_parse_with_notation_trailing_dot() {
        // 末尾点号：sub 为空字符串
        assert_eq!(parse_with_notation("relation."), ("relation", Some("")));
    }

    #[test]
    fn test_parse_with_notation_leading_dot() {
        // 前导点号：relation 为空字符串
        assert_eq!(parse_with_notation(".sub"), ("", Some("sub")));
    }

    // ====================================================================
    // 组 9：value_to_pk_string 内部辅助函数
    // ====================================================================

    #[test]
    fn test_value_to_pk_string_integer() {
        assert_eq!(value_to_pk_string(&json!(1)), "1");
        assert_eq!(value_to_pk_string(&json!(-100)), "-100");
    }

    #[test]
    fn test_value_to_pk_string_float() {
        assert_eq!(value_to_pk_string(&json!(1.5)), "1.5");
    }

    #[test]
    fn test_value_to_pk_string_string() {
        assert_eq!(value_to_pk_string(&json!("uuid-123")), "uuid-123");
    }

    #[test]
    fn test_value_to_pk_string_bool() {
        assert_eq!(value_to_pk_string(&json!(true)), "true");
        assert_eq!(value_to_pk_string(&json!(false)), "false");
    }

    #[test]
    fn test_value_to_pk_string_null_returns_empty() {
        // null 返回空字符串（不应出现在主键字段）
        assert_eq!(value_to_pk_string(&Value::Null), "");
    }

    #[test]
    fn test_value_to_pk_string_object_returns_empty() {
        // 对象返回空字符串（不应出现在主键字段）
        assert_eq!(value_to_pk_string(&json!({"a": 1})), "");
    }

    #[test]
    fn test_value_to_pk_string_array_returns_empty() {
        // 数组返回空字符串（不应出现在主键字段）
        assert_eq!(value_to_pk_string(&json!([1, 2, 3])), "");
    }

    // ====================================================================
    // 组 10：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_with_in_query_pattern_has_many() {
        // R5-1：PHP HasMany::eagerlyResultSet 使用 IN 查询
        // PHP 源码：[$this->foreignKey, 'in', $range]
        let sql = has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");
    }

    #[test]
    fn test_r5_php_with_in_query_pattern_belongs_to() {
        // R5-2：PHP BelongsTo::eagerlyResultSet 使用 IN 查询
        // PHP 源码：[$this->localKey, 'in', $range]
        let sql = belongs_to_in_sql("depts", "id", &["1", "2", "3"]);
        assert_eq!(sql, "SELECT * FROM depts WHERE id IN (1, 2, 3)");
    }

    #[test]
    fn test_r5_php_with_in_query_pattern_belongs_to_many() {
        // R5-3：PHP BelongsToMany::eagerlyResultSet 使用 IN 查询
        // PHP localKey → sz-orm-core foreign_key（命名反转）
        let sql = belongs_to_many_in_sql(
            "roles",
            "user_role",
            "id",
            "role_id",
            "user_id",
            &["1", "2"],
        );
        assert!(sql.contains("INNER JOIN user_role j"));
        assert!(sql.contains("WHERE j.user_id IN (1, 2)"));
    }

    #[test]
    fn test_r5_php_collect_range_skips_null() {
        // R5-4：PHP $range 收集跳过 isset 检查失败的行
        let rows = vec![
            json!({"id": 1}),
            json!({"id": null}), // isset 返回 false
            json!({"id": 3}),
        ];
        let pks = collect_pk_values(&rows, "id");
        assert_eq!(pks, vec!["1", "3"]);
    }

    #[test]
    fn test_r5_php_group_by_fk_matches_php_data_bucket() {
        // R5-5：PHP $data[$pk] 分桶逻辑
        let rows = vec![
            json!({"id": 101, "user_id": 1, "name": "A"}),
            json!({"id": 102, "user_id": 2, "name": "B"}),
            json!({"id": 103, "user_id": 1, "name": "C"}),
        ];
        let grouped = group_by_fk(rows, "user_id");
        // user_id=1 桶包含 2 行（A 和 C）
        assert_eq!(grouped.get("1").unwrap().len(), 2);
        // user_id=2 桶包含 1 行（B）
        assert_eq!(grouped.get("2").unwrap().len(), 1);
    }

    #[test]
    fn test_r5_php_parse_with_notation_explode_dot() {
        // R5-6：PHP explode('.', $relation, 2) 语法解析
        assert_eq!(parse_with_notation("category"), ("category", None));
        assert_eq!(
            parse_with_notation("items.product"),
            ("items", Some("product"))
        );
        // PHP explode limit=2：仅按第一个 . 分割
        assert_eq!(parse_with_notation("a.b.c"), ("a", Some("b.c")));
    }

    #[test]
    fn test_r5_php_has_one_in_sql_same_as_has_many() {
        // R5-7：PHP HasOne 与 HasMany IN 查询 SQL 模式相同
        let has_one = has_one_in_sql("profiles", "user_id", &["1", "2"]);
        let has_many = has_many_in_sql("profiles", "user_id", &["1", "2"]);
        assert_eq!(has_one, has_many);
    }

    #[test]
    fn test_r5_php_empty_range_returns_in_null() {
        // R5-8：PHP !empty($range) 为 false 时跳过查询
        // 本函数返回 IN (NULL) 而非跳过，调用方应自行判断空列表
        let sql_has_many = has_many_in_sql("orders", "user_id", &[]);
        let sql_belongs_to = belongs_to_in_sql("depts", "id", &[]);
        let sql_belongs_to_many =
            belongs_to_many_in_sql("roles", "user_role", "id", "role_id", "user_id", &[]);
        assert!(sql_has_many.contains("IN (NULL)"));
        assert!(sql_belongs_to.contains("IN (NULL)"));
        assert!(sql_belongs_to_many.contains("IN (NULL)"));
    }

    #[test]
    fn test_r5_php_sanitize_pk_value_sql_escaping() {
        // R5-9：SQL 标准单引号转义（' → ''）
        assert_eq!(sanitize_pk_value("1"), "1");
        assert_eq!(sanitize_pk_value("abc"), "'abc'");
        assert_eq!(sanitize_pk_value("a'b"), "'a''b'");
    }

    // ====================================================================
    // 组 11：集成测试（PHP with() 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_with_has_many_user_orders() {
        // PHP 业务场景：User::with('orders')->select([1, 2, 3])
        // 1. 收集主键
        let users = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": 2, "name": "Bob"}),
            json!({"id": 3, "name": "Charlie"}),
        ];
        let user_pks = collect_pk_values(&users, "id");
        assert_eq!(user_pks, vec!["1", "2", "3"]);

        // 2. 生成 IN 查询 SQL
        let pk_refs: Vec<&str> = user_pks.iter().map(|s| s.as_str()).collect();
        let sql = has_many_in_sql("orders", "user_id", &pk_refs);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");

        // 3. 模拟关联表结果集分桶
        let orders = vec![
            json!({"id": 101, "user_id": 1, "amount": 100}),
            json!({"id": 102, "user_id": 2, "amount": 200}),
            json!({"id": 103, "user_id": 1, "amount": 150}),
        ];
        let grouped = group_by_fk(orders, "user_id");
        assert_eq!(grouped.get("1").unwrap().len(), 2);
        assert_eq!(grouped.get("2").unwrap().len(), 1);
        assert!(!grouped.contains_key("3")); // user_id=3 无订单
    }

    #[test]
    fn test_integration_with_belongs_to_order_user() {
        // PHP 业务场景：Order::with('user')->select()
        // 1. 收集外键值
        let orders = vec![
            json!({"id": 101, "user_id": 1, "amount": 100}),
            json!({"id": 102, "user_id": 2, "amount": 200}),
            json!({"id": 103, "user_id": 1, "amount": 150}),
        ];
        let user_fks = collect_pk_values(&orders, "user_id");
        assert_eq!(user_fks, vec!["1", "2", "1"]);

        // 2. 去重后生成 IN 查询 SQL（调用方负责去重）
        let unique_fks: Vec<&str> = vec!["1", "2"];
        let sql = belongs_to_in_sql("users", "id", &unique_fks);
        assert_eq!(sql, "SELECT * FROM users WHERE id IN (1, 2)");

        // 3. 模拟父表结果集分桶（BelongsTo 按父表主键分桶）
        let users = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": 2, "name": "Bob"}),
        ];
        let grouped = group_by_fk(users, "id");
        assert_eq!(grouped.get("1").unwrap().len(), 1);
        assert_eq!(grouped.get("2").unwrap().len(), 1);
    }

    #[test]
    fn test_integration_with_belongs_to_many_user_roles() {
        // PHP 业务场景：User::with('roles')->select([1, 2])
        let users = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": 2, "name": "Bob"}),
        ];
        let user_pks = collect_pk_values(&users, "id");
        let pk_refs: Vec<&str> = user_pks.iter().map(|s| s.as_str()).collect();

        // 生成 BelongsToMany IN 查询
        let sql =
            belongs_to_many_in_sql("roles", "user_role", "id", "role_id", "user_id", &pk_refs);
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2)"
        );
    }

    #[test]
    fn test_integration_with_nested_notation_user_orders_items() {
        // PHP 业务场景：User::with(['orders.items'])->select()
        // 解析嵌套语法
        let (relation, sub) = parse_with_notation("orders.items");
        assert_eq!(relation, "orders");
        assert_eq!(sub, Some("items"));

        // 第一层：User hasMany Orders
        let users = vec![json!({"id": 1, "name": "Alice"})];
        let user_pks = collect_pk_values(&users, "id");
        let pk_refs: Vec<&str> = user_pks.iter().map(|s| s.as_str()).collect();
        let sql = has_many_in_sql("orders", "user_id", &pk_refs);
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1)");

        // 第二层：Order hasMany Items（递归处理 sub）
        let orders = vec![
            json!({"id": 101, "user_id": 1}),
            json!({"id": 102, "user_id": 1}),
        ];
        let order_pks = collect_pk_values(&orders, "id");
        let pk_refs2: Vec<&str> = order_pks.iter().map(|s| s.as_str()).collect();
        let sql2 = has_many_in_sql("order_items", "order_id", &pk_refs2);
        assert_eq!(
            sql2,
            "SELECT * FROM order_items WHERE order_id IN (101, 102)"
        );
    }

    #[test]
    fn test_integration_with_has_one_user_profile() {
        // PHP 业务场景：User::with('profile')->select([1, 2])
        let users = vec![
            json!({"id": 1, "name": "Alice"}),
            json!({"id": 2, "name": "Bob"}),
        ];
        let user_pks = collect_pk_values(&users, "id");
        let pk_refs: Vec<&str> = user_pks.iter().map(|s| s.as_str()).collect();

        // HasOne IN 查询与 HasMany 相同，区别在调用方取第一行
        let sql = has_one_in_sql("profiles", "user_id", &pk_refs);
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id IN (1, 2)");

        // 模拟 profile 结果集
        let profiles = vec![
            json!({"id": 11, "user_id": 1, "bio": "Alice's bio"}),
            json!({"id": 22, "user_id": 2, "bio": "Bob's bio"}),
        ];
        let grouped = group_by_fk(profiles, "user_id");
        // HasOne 场景：每个 user_id 桶取第一行
        let alice_profile = &grouped.get("1").unwrap()[0];
        assert_eq!(alice_profile["bio"], json!("Alice's bio"));
    }

    #[test]
    fn test_integration_sanitize_pk_values_for_in_query() {
        // 混合主键类型：数值 + 字符串
        let raw_pks = ["1", "abc", "2", "x'y"];
        let sanitized: Vec<String> = raw_pks.iter().map(|s| sanitize_pk_value(s)).collect();
        let joined = sanitized.join(", ");
        assert_eq!(joined, "1, 'abc', 2, 'x''y'");

        // 生成安全的 IN 查询 SQL
        let sanitized_refs: Vec<&str> = sanitized.iter().map(|s| s.as_str()).collect();
        let sql = has_many_in_sql("orders", "user_id", &sanitized_refs);
        assert_eq!(
            sql,
            "SELECT * FROM orders WHERE user_id IN (1, 'abc', 2, 'x''y')"
        );
    }
}
