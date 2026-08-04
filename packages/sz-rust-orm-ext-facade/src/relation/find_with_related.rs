//! find_with_related API 接入 — JOIN / 子查询 / eager_sql 三种模式
//!
//! 本模块 re-export sz-orm-core `find_with_related` 模块的
//! SQL 生成 API，并提供 PHP 命名约定辅助函数对齐 PHP `withJoin()` / `has()` 行为。
//!
//! ## PHP 端关联预加载的三种模式
//!
//! PHP think-orm 2.0.x 提供三种关联预加载模式：
//!
//! | 模式 | PHP 入口 | 执行方式 | 支持的关联类型 |
//! |------|---------|---------|---------------|
//! | **IN 查询模式** | `with(['relation'])` | `eagerlyResultSet($join=false)` 收集主键 → `WHERE fk IN (range)` | 全部 6 种 |
//! | **JOIN 模式** | `withJoin(['relation'])` | `eagerly()` 方法 + `eagerlyResultSet($join=true)` 通过 JOIN 一次性查询 | **仅 OneToOne**（HasOne / BelongsTo） |
//! | **has 过滤模式** | `has('relation', '>=', 1)` | JOIN + GROUP BY + HAVING COUNT 过滤主模型 | 全部 6 种（不同关联类型 JOIN 方向不同） |
//!
//! ### PHP `withJoin()` 源码（ModelRelationQuery.php 第 234-271 行）
//!
//! ```php
//! public function withJoin($with, string $joinType = '')
//! {
//!     $with  = (array) $with;
//!     $first = true;
//!     foreach ($with as $key => $relation) {
//!         // ... 解析闭包/字段/嵌套关系
//!         $result = $this->model->eagerly($this, $relation, $field, $joinType, $closure, $first);
//!         if (!$result) {
//!             unset($with[$key]);  // 不支持 JOIN 的关联类型被剔除
//!         } else {
//!             $first = false;
//!         }
//!     }
//!     // ...
//! }
//! ```
//!
//! **关键**：`eagerly()` 方法仅在 `OneToOne` trait 中实现（HasOne / BelongsTo），
//! `HasMany` / `BelongsToMany` / `MorphMany` / `MorphTo` 无 `eagerly()` 方法，
//! `withJoin()` 调用时这些关联会被静默剔除。
//!
//! ### PHP `eagerly()` JOIN SQL 生成（OneToOne.php 第 68-125 行）
//!
//! ```php
//! public function eagerly(Query $query, string $relation, $field = true, string $joinType = '', ...): void
//! {
//!     $joinTable = $this->query->getTable();
//!     $joinAlias = $relation;
//!     $joinType  = $joinType ?: $this->joinType;  // 默认 LEFT JOIN
//!
//!     if ($this instanceof BelongsTo) {
//!         // BelongsTo: main.fk = related.pk
//!         $joinOn = $name . '.' . $this->foreignKey . '=' . $joinAlias . '.' . $this->localKey;
//!     } else {
//!         // HasOne: main.pk = related.fk
//!         $joinOn = $name . '.' . $this->localKey . '=' . $joinAlias . '.' . $this->foreignKey;
//!     }
//!
//!     $query->join([$joinTable => $joinAlias], $joinOn, $joinType);
//! }
//! ```
//!
//! ### PHP `has()` JOIN + GROUP BY + HAVING COUNT（HasMany.php 第 298-319 行）
//!
//! ```php
//! public function has(string $operator = '>=', int $count = 1, string $id = '*', string $joinType = 'INNER', ...): Query
//! {
//!     return $query->field($model . '.*')
//!         ->join([$table => $relation], $model . '.' . $this->localKey . '=' . $relation . '.' . $this->foreignKey, $joinType)
//!         ->group($relation . '.' . $this->foreignKey)
//!         ->having('count(' . $id . ')' . $operator . $count);
//! }
//! ```
//!
//! ## sz-orm-core find_with_related API
//!
//! sz-orm-core `find_with_related` 模块提供三种 SQL 生成模式：
//!
//! | 模式 | sz-orm-core API | 对应 PHP 模式 |
//! |------|----------------|--------------|
//! | **JOIN 模式** | [`FindWithRelated`] struct + `build()` 方法 / `find_with_related_join()` 便捷函数 | `withJoin()` |
//! | **eager_sql 模式** | `find_with_related_eager_sql()` 函数 / [`FindWithRelation`] struct + `load_eager()` + `main_sql()` + `related_sql_with_ids()` | `with()` IN 查询 |
//! | **子查询模式** | `find_with_related_subquery()` 函数 | sz-orm 扩展（PHP 无直接对应） |
//!
//! ## 本模块提供的函数
//!
//! ### 1. re-export sz-orm-core find_with_related 公开类型
//!
//! - [`FindWithRelated`]：JOIN 模式 SQL 构造器
//! - [`FindWithRelation`]：多关系 eager 加载器（sz-orm-core `find_with_related::WithRelation` 的别名，避免与 `model::WithRelation` 冲突）
//! - [`inspect_relation`]：从 `HashMap<&str, Relation>` 提取关联元数据
//! - [`find_with_related_join`] / [`find_with_related_eager_sql`] / [`find_with_related_subquery`]：便捷函数
//!
//! ### 2. PHP 命名约定辅助函数
//!
//! - [`JoinMode`] 枚举：`Left`（默认对齐 PHP `joinType=''`）/ `Inner`（对齐 PHP `joinType='INNER'`）
//! - [`join_mode_str`]：`JoinMode` 转 PHP `joinType` 字符串
//! - [`php_with_join_sql`]：生成 PHP `withJoin()` + `eagerly()` JOIN SQL 片段
//! - [`php_has_join_sql`]：生成 PHP `has()` JOIN + GROUP BY + HAVING COUNT SQL 片段
//! - [`is_one_to_one`]：判断关系是否为 OneToOne（仅 OneToOne 支持 JOIN 预加载）
//!
//! ## 架构说明
//!
//! 沿用既有的 sz-orm-core::model 模块私有约束统一处理模式：
//!
//! - **re-export sz-orm-core 关联类型**：`FindWithRelated` / `FindWithRelation` / `inspect_relation` / 便捷函数
//! - **PHP 命名约定辅助函数**：`JoinMode` / `join_mode_str` / `php_with_join_sql` / `php_has_join_sql` / `is_one_to_one`
//! - **SQL 片段构造器**：仅用于测试验证 SQL 生成模式对齐 PHP
//!
//! 端到端关联加载由 sz-orm-core 内部实现，sz-rust 端通过 SQL 片段构造器验证 SQL 生成对齐 PHP。

// re-export sz-orm-core find_with_related 公开类型
pub use sz_rust_orm_facade::find_with_related::{
    find_with_related_eager_sql, find_with_related_join, find_with_related_subquery,
    inspect_relation, FindWithRelated, WithRelation as FindWithRelation,
};

use super::Relation;

// ============================================================================
// JoinMode 枚举（PHP joinType 字符串映射）
// ============================================================================

/// PHP `withJoin()` 的 JOIN 类型
///
/// 对齐 PHP think-orm 2.0.x `OneToOne::eagerly()` 第 89 行
/// `$joinType = $joinType ?: $this->joinType`（默认 `LEFT`）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::find_with_related::{JoinMode, join_mode_str};
///
/// assert_eq!(join_mode_str(JoinMode::Left), "LEFT");
/// assert_eq!(join_mode_str(JoinMode::Inner), "INNER");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinMode {
    /// LEFT JOIN（PHP 默认 `$joinType = ''` → `$this->joinType = 'LEFT'`）
    Left,
    /// INNER JOIN（PHP `$joinType = 'INNER'`）
    Inner,
}

/// `JoinMode` 转 PHP `joinType` 字符串
///
/// 对齐 PHP `OneToOne::eagerly()` 第 89 行 `$joinType = $joinType ?: $this->joinType`，
/// PHP 端 `joinType` 为空字符串时使用 `$this->joinType`（默认 `LEFT`）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::find_with_related::{JoinMode, join_mode_str};
///
/// assert_eq!(join_mode_str(JoinMode::Left), "LEFT");
/// assert_eq!(join_mode_str(JoinMode::Inner), "INNER");
/// ```
pub fn join_mode_str(mode: JoinMode) -> &'static str {
    match mode {
        JoinMode::Left => "LEFT",
        JoinMode::Inner => "INNER",
    }
}

// ============================================================================
// OneToOne 判断（仅 OneToOne 支持 JOIN 预加载）
// ============================================================================

/// 判断 `Relation` 是否为 OneToOne（HasOne 或 BelongsTo）
///
/// 对齐 PHP `withJoin()` 行为：`eagerly()` 方法仅在 `OneToOne` trait 中实现，
/// `HasMany` / `BelongsToMany` / `MorphMany` / `MorphTo` 不支持 JOIN 预加载，
/// `withJoin()` 调用时这些关联会被静默剔除（`unset($with[$key])`）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::find_with_related::is_one_to_one;
/// use sz_rust_core::relation::{Relation, HasOne, HasMany, BelongsTo};
///
/// assert!(is_one_to_one(&Relation::HasOne(HasOne { ... })));
/// assert!(is_one_to_one(&Relation::BelongsTo(BelongsTo { ... })));
/// assert!(!is_one_to_one(&Relation::HasMany(HasMany { ... })));
/// ```
pub fn is_one_to_one(relation: &Relation) -> bool {
    matches!(relation, Relation::HasOne(_) | Relation::BelongsTo(_))
}

// ============================================================================
// PHP withJoin() + eagerly() JOIN SQL 片段构造器
// ============================================================================

/// 生成 PHP `withJoin()` + `eagerly()` JOIN SQL 片段
///
/// 对齐 PHP `OneToOne::eagerly()` 第 123 行生成的 JOIN SQL：
///
/// ```sql
/// -- HasOne（main.pk = related.fk）
/// SELECT main.* FROM {main_table} main
/// {JOIN_TYPE} JOIN {related_table} related ON main.{main_key} = related.{related_key}
///
/// -- BelongsTo（main.fk = related.pk）
/// SELECT main.* FROM {main_table} main
/// {JOIN_TYPE} JOIN {related_table} related ON main.{main_key} = related.{related_key}
/// ```
///
/// ## 参数
///
/// - `main_table`：主表名（如 `"users"`）
/// - `related_table`：关联表名（如 `"profiles"`）
/// - `main_key`：主表参与 JOIN 的字段（HasOne 为 `localKey` 即主表 pk；BelongsTo 为 `foreignKey` 即主表外键）
/// - `related_key`：关联表参与 JOIN 的字段（HasOne 为 `foreignKey` 即关联表外键；BelongsTo 为 `localKey` 即关联表 pk）
/// - `join_mode`：JOIN 类型（`Left` 默认 / `Inner`）
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// 端到端 SQL 执行由 sz-orm-core 参数化查询 API 处理。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::find_with_related::{php_with_join_sql, JoinMode};
///
/// // HasOne: main.pk = related.fk
/// let sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
/// assert_eq!(sql, "SELECT main.* FROM users main LEFT JOIN profiles related ON main.id = related.user_id");
///
/// // BelongsTo: main.fk = related.pk
/// let sql = php_with_join_sql("orders", "users", "user_id", "id", JoinMode::Inner);
/// assert_eq!(sql, "SELECT main.* FROM orders main INNER JOIN users related ON main.user_id = related.id");
/// ```
pub fn php_with_join_sql(
    main_table: &str,
    related_table: &str,
    main_key: &str,
    related_key: &str,
    join_mode: JoinMode,
) -> String {
    format!(
        "SELECT main.* FROM {} main {} JOIN {} related ON main.{} = related.{}",
        main_table,
        join_mode_str(join_mode),
        related_table,
        main_key,
        related_key
    )
}

// ============================================================================
// PHP has() JOIN + GROUP BY + HAVING COUNT SQL 片段构造器
// ============================================================================

/// 生成 PHP `has()` JOIN + GROUP BY + HAVING COUNT SQL 片段
///
/// 对齐 PHP `HasMany::has()` 第 312-318 行生成的 SQL：
///
/// ```sql
/// SELECT main.* FROM {main_table} main
/// INNER JOIN {related_table} related ON main.{main_key} = related.{related_key}
/// GROUP BY related.{related_key}
/// HAVING count({count_field}) {operator} {count}
/// ```
///
/// ## 参数
///
/// - `main_table`：主表名（如 `"users"`）
/// - `related_table`：关联表名（如 `"orders"`）
/// - `main_key`：主表参与 JOIN 的字段（HasMany 为 `localKey` 即主表 pk）
/// - `related_key`：关联表参与 JOIN 的字段（HasMany 为 `foreignKey` 即关联表外键，也是 GROUP BY 字段）
/// - `count_field`：COUNT 字段（PHP 默认 `"*"`，对齐 `$id = '*'`）
/// - `operator`：比较运算符（如 `">="`、`">"`、`"="`、`"<"`、`"<="`）
/// - `count`：阈值数量
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// `operator` 参数应由调用方限制为 `>=`/`>`/`=`/`<`/`<=` 白名单。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::find_with_related::php_has_join_sql;
///
/// // 查询有至少 3 个订单的用户
/// let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 3);
/// assert_eq!(sql, "SELECT main.* FROM users main INNER JOIN orders related ON main.id = related.user_id GROUP BY related.user_id HAVING count(*) >= 3");
/// ```
pub fn php_has_join_sql(
    main_table: &str,
    related_table: &str,
    main_key: &str,
    related_key: &str,
    count_field: &str,
    operator: &str,
    count: i64,
) -> String {
    format!(
        "SELECT main.* FROM {} main INNER JOIN {} related ON main.{} = related.{} GROUP BY related.{} HAVING count({}) {} {}",
        main_table,
        related_table,
        main_key,
        related_key,
        related_key,
        count_field,
        operator,
        count
    )
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::{BelongsTo, HasMany, HasOne};

    // ====================================================================
    // 组 1：JoinMode 枚举 + join_mode_str
    // ====================================================================

    #[test]
    fn test_join_mode_str_left() {
        // PHP 默认 $joinType='' → $this->joinType='LEFT'
        assert_eq!(join_mode_str(JoinMode::Left), "LEFT");
    }

    #[test]
    fn test_join_mode_str_inner() {
        // PHP $joinType='INNER'
        assert_eq!(join_mode_str(JoinMode::Inner), "INNER");
    }

    #[test]
    fn test_join_mode_eq_and_copy() {
        // 验证 #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        let mode1 = JoinMode::Left;
        let mode2 = mode1; // Copy
        assert_eq!(mode1, mode2); // PartialEq
        assert_eq!(format!("{:?}", mode1), "Left"); // Debug
    }

    #[test]
    fn test_join_mode_default_is_left() {
        // PHP 默认行为：$joinType='' 使用 $this->joinType 默认 'LEFT'
        // sz-rust 端 JoinMode 没有 Default，但通过显式传 JoinMode::Left 对齐
        let default_mode = JoinMode::Left;
        assert_eq!(join_mode_str(default_mode), "LEFT");
    }

    // ====================================================================
    // 组 2：is_one_to_one 判断
    // ====================================================================

    #[test]
    fn test_is_one_to_one_has_one() {
        // PHP OneToOne trait 实现 eagerly()，支持 JOIN
        let rel = Relation::HasOne(HasOne {
            foreign_key: "user_id".to_string(),
            child_model: "profiles".to_string(),
            child_pk: "id".to_string(),
        });
        assert!(is_one_to_one(&rel));
    }

    #[test]
    fn test_is_one_to_one_belongs_to() {
        // PHP OneToOne trait 实现 eagerly()，支持 JOIN
        let rel = Relation::BelongsTo(BelongsTo {
            foreign_key: "user_id".to_string(),
            parent_model: "users".to_string(),
            parent_pk: "id".to_string(),
        });
        assert!(is_one_to_one(&rel));
    }

    #[test]
    fn test_is_one_to_one_has_many_returns_false() {
        // HasMany 无 eagerly() 方法，withJoin() 静默剔除
        let rel = Relation::HasMany(HasMany {
            foreign_key: "user_id".to_string(),
            child_model: "orders".to_string(),
            child_pk: "id".to_string(),
        });
        assert!(!is_one_to_one(&rel));
    }

    #[test]
    fn test_is_one_to_one_belongs_to_many_returns_false() {
        // BelongsToMany 无 eagerly() 方法
        let rel = Relation::BelongsToMany(crate::relation::BelongsToMany {
            junction_table: "user_role".to_string(),
            foreign_key: "user_id".to_string(),
            other_key: "role_id".to_string(),
            target_model: "roles".to_string(),
            target_pk: "id".to_string(),
        });
        assert!(!is_one_to_one(&rel));
    }

    // ====================================================================
    // 组 3：php_with_join_sql（PHP withJoin + eagerly）
    // ====================================================================

    #[test]
    fn test_php_with_join_sql_has_one_left_join() {
        // HasOne: main.pk = related.fk, LEFT JOIN（PHP 默认）
        // 对齐 OneToOne.php 第 110 行：$joinOn = $name . '.' . $this->localKey . '=' . $foreignKeyExp
        let sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert_eq!(
            sql,
            "SELECT main.* FROM users main LEFT JOIN profiles related ON main.id = related.user_id"
        );
    }

    #[test]
    fn test_php_with_join_sql_belongs_to_inner_join() {
        // BelongsTo: main.fk = related.pk, INNER JOIN
        // 对齐 OneToOne.php 第 101 行：$joinOn = $foreignKeyExp . '=' . $joinAlias . '.' . $this->localKey
        let sql = php_with_join_sql("orders", "users", "user_id", "id", JoinMode::Inner);
        assert_eq!(
            sql,
            "SELECT main.* FROM orders main INNER JOIN users related ON main.user_id = related.id"
        );
    }

    #[test]
    fn test_php_with_join_sql_has_one_default_left() {
        // PHP withJoin(['relation']) 默认 $joinType='' → LEFT JOIN
        let sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert!(sql.contains("LEFT JOIN"));
    }

    #[test]
    fn test_php_with_join_sql_multi_word_tables() {
        // 多单词表名
        let sql = php_with_join_sql(
            "order_items",
            "product_details",
            "id",
            "order_item_id",
            JoinMode::Left,
        );
        assert_eq!(
            sql,
            "SELECT main.* FROM order_items main LEFT JOIN product_details related ON main.id = related.order_item_id"
        );
    }

    #[test]
    fn test_php_with_join_sql_aligns_one_to_one_php_eagerly() {
        // 验证 SQL 模式与 PHP OneToOne::eagerly() 第 123 行一致
        // $query->join([$joinTable => $joinAlias], $joinOn, $joinType)
        let sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        // 验证包含 SELECT/FROM/JOIN/ON 关键字
        assert!(sql.starts_with("SELECT main.* FROM"));
        assert!(sql.contains(" LEFT JOIN "));
        assert!(sql.contains(" ON main."));
        assert!(sql.contains(" = related."));
    }

    // ====================================================================
    // 组 4：php_has_join_sql（PHP has JOIN + GROUP BY + HAVING COUNT）
    // ====================================================================

    #[test]
    fn test_php_has_join_sql_default_operator_ge() {
        // PHP has() 默认 $operator='>=', $count=1
        // 对齐 HasMany.php 第 312-318 行
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 3);
        assert_eq!(
            sql,
            "SELECT main.* FROM users main INNER JOIN orders related ON main.id = related.user_id GROUP BY related.user_id HAVING count(*) >= 3"
        );
    }

    #[test]
    fn test_php_has_join_sql_operator_gt() {
        // PHP has('>', 5)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">", 5);
        assert!(sql.contains("HAVING count(*) > 5"));
    }

    #[test]
    fn test_php_has_join_sql_operator_eq() {
        // PHP has('=', 1)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", "=", 1);
        assert!(sql.contains("HAVING count(*) = 1"));
    }

    #[test]
    fn test_php_has_join_sql_count_field_specific() {
        // PHP has($operator, $count, $id='id')，$id 非 '*' 时使用具体字段
        // 对齐 HasMany.php 第 305-307 行：if ('*' != $id) { $id = $relation . '.' . (new $this->model)->getPk(); }
        // sz-rust 端简化为 count_field 参数，调用方负责拼接表名前缀
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "orders.id", ">=", 2);
        assert!(sql.contains("HAVING count(orders.id) >= 2"));
    }

    #[test]
    fn test_php_has_join_sql_multi_word_tables() {
        // 多单词表名
        let sql = php_has_join_sql(
            "order_items",
            "product_details",
            "id",
            "order_item_id",
            "*",
            ">=",
            1,
        );
        assert!(sql.contains("FROM order_items main"));
        assert!(sql.contains("INNER JOIN product_details related"));
        assert!(sql.contains("ON main.id = related.order_item_id"));
        assert!(sql.contains("GROUP BY related.order_item_id"));
    }

    #[test]
    fn test_php_has_join_sql_aligns_php_has_many_has_method() {
        // 验证 SQL 模式与 PHP HasMany::has() 第 312-318 行一致
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        // 验证包含 SELECT/FROM/INNER JOIN/ON/GROUP BY/HAVING count 关键字
        assert!(sql.starts_with("SELECT main.* FROM"));
        assert!(sql.contains(" INNER JOIN "));
        assert!(sql.contains(" ON main."));
        assert!(sql.contains(" GROUP BY related."));
        assert!(sql.contains(" HAVING count("));
    }

    #[test]
    fn test_php_has_join_sql_default_inner_join() {
        // PHP has() 默认 $joinType='INNER'（对齐 HasMany.php 第 298 行签名）
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        assert!(sql.contains("INNER JOIN"));
        assert!(!sql.contains("LEFT JOIN"));
    }

    // ====================================================================
    // 组 5：re-export sz-orm-core 类型可访问性
    // ====================================================================

    #[test]
    fn test_re_export_find_with_related_accessible() {
        // 验证 FindWithRelated 类型可访问（编译时检查）
        // sz-orm-core FindWithRelated 需要 &'a dyn Dialect，无法在单元测试中无数据库构造
        // pub use 语句已在编译时验证符号可访问
        // 通过 type alias 守卫验证类型别名可解析（带生命周期）
        const _: () = {
            type _AssertFindWithRelatedResolves<'a> = FindWithRelated<'a>;
        };
    }

    #[test]
    fn test_re_export_find_with_relation_accessible() {
        // 验证 FindWithRelation 类型可访问（编译时检查）
        // FindWithRelation 是 sz-orm-core find_with_related::WithRelation 的别名
        // pub use 语句已在编译时验证符号可访问
        // 通过 type alias 守卫验证类型别名可解析（带生命周期）
        const _: () = {
            type _AssertFindWithRelationResolves<'a> = FindWithRelation<'a>;
        };
    }

    #[test]
    fn test_re_export_inspect_relation_callable() {
        // 验证 inspect_relation 函数可调用
        // inspect_relation 需要 HashMap<&str, Relation> 参数
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "orders",
            Relation::HasMany(HasMany {
                foreign_key: "user_id".to_string(),
                child_model: "orders".to_string(),
                child_pk: "id".to_string(),
            }),
        );
        let result = inspect_relation(&relations, "orders");
        assert!(result.is_some());
        let (related_table, foreign_key, primary_key, is_many) = result.unwrap();
        assert_eq!(related_table, "orders");
        assert_eq!(foreign_key, "user_id");
        assert_eq!(primary_key, "id");
        assert!(is_many);
    }

    #[test]
    fn test_re_export_inspect_relation_not_found() {
        // 验证 inspect_relation 对不存在的关系返回 None
        use std::collections::HashMap;
        let relations: HashMap<&str, Relation> = HashMap::new();
        let result = inspect_relation(&relations, "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_re_export_inspect_relation_has_one() {
        // 验证 inspect_relation 对 HasOne 返回 is_many=false
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "profile",
            Relation::HasOne(HasOne {
                foreign_key: "user_id".to_string(),
                child_model: "profiles".to_string(),
                child_pk: "id".to_string(),
            }),
        );
        let result = inspect_relation(&relations, "profile");
        let (_, _, _, is_many) = result.unwrap();
        assert!(!is_many);
    }

    #[test]
    fn test_re_export_inspect_relation_belongs_to() {
        // 验证 inspect_relation 对 BelongsTo 返回 is_many=false
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "dept",
            Relation::BelongsTo(BelongsTo {
                foreign_key: "dept_id".to_string(),
                parent_model: "depts".to_string(),
                parent_pk: "id".to_string(),
            }),
        );
        let result = inspect_relation(&relations, "dept");
        let (related_table, foreign_key, primary_key, is_many) = result.unwrap();
        assert_eq!(related_table, "depts");
        assert_eq!(foreign_key, "dept_id");
        assert_eq!(primary_key, "id");
        assert!(!is_many);
    }

    #[test]
    fn test_re_export_inspect_relation_belongs_to_many() {
        // 验证 inspect_relation 对 BelongsToMany 返回 is_many=true
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "roles",
            Relation::BelongsToMany(crate::relation::BelongsToMany {
                junction_table: "user_role".to_string(),
                foreign_key: "user_id".to_string(),
                other_key: "role_id".to_string(),
                target_model: "roles".to_string(),
                target_pk: "id".to_string(),
            }),
        );
        let result = inspect_relation(&relations, "roles");
        let (related_table, _, _, is_many) = result.unwrap();
        assert_eq!(related_table, "roles");
        assert!(is_many);
    }

    #[test]
    fn test_re_export_convenience_functions_callable() {
        // 验证 find_with_related_join / find_with_related_eager_sql / find_with_related_subquery 函数符号可访问
        // 这些函数需要 &dyn Dialect 参数，无法在无数据库场景下完整调用
        // pub use 语句已在编译时验证符号可访问，此测试通过 type alias 守卫进一步确认
        type _AssertJoinFn = for<'a> fn(
            &'a dyn sz_orm_core::Dialect,
            &'a str,
            &'a str,
            &'a str,
            &'a str,
            bool,
        )
            -> Result<FindWithRelated<'a>, sz_orm_core::DbError>;
        type _AssertEagerSqlFn = fn(
            &dyn sz_orm_core::Dialect,
            &str,
            &str,
            &str,
            Option<&str>,
        ) -> Result<(String, String), sz_orm_core::DbError>;
        type _AssertSubqueryFn = fn(
            &dyn sz_orm_core::Dialect,
            &str,
            &str,
            &str,
            &str,
            Option<&str>,
        ) -> Result<String, sz_orm_core::DbError>;

        const _: () = {
            let _join: _AssertJoinFn = find_with_related_join;
            let _eager_sql: _AssertEagerSqlFn = find_with_related_eager_sql;
            let _subquery: _AssertSubqueryFn = find_with_related_subquery;
        };
    }

    // ====================================================================
    // 组 6：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_with_join_only_for_one_to_one() {
        // R5-1：PHP withJoin() 仅 OneToOne 支持，其他关联类型静默剔除
        // 对齐 ModelRelationQuery.php 第 258-264 行：if (!$result) { unset($with[$key]); }
        let has_many = Relation::HasMany(HasMany {
            foreign_key: "user_id".to_string(),
            child_model: "orders".to_string(),
            child_pk: "id".to_string(),
        });
        assert!(!is_one_to_one(&has_many)); // HasMany 不支持 JOIN

        let has_one = Relation::HasOne(HasOne {
            foreign_key: "user_id".to_string(),
            child_model: "profiles".to_string(),
            child_pk: "id".to_string(),
        });
        assert!(is_one_to_one(&has_one)); // HasOne 支持 JOIN

        let belongs_to = Relation::BelongsTo(BelongsTo {
            foreign_key: "user_id".to_string(),
            parent_model: "users".to_string(),
            parent_pk: "id".to_string(),
        });
        assert!(is_one_to_one(&belongs_to)); // BelongsTo 支持 JOIN
    }

    #[test]
    fn test_r5_php_eagerly_join_on_has_one() {
        // R5-2：PHP OneToOne::eagerly() HasOne 分支 JOIN ON 条件
        // 对齐 OneToOne.php 第 110 行：$joinOn = $name . '.' . $this->localKey . '=' . $foreignKeyExp
        // HasOne: main.localKey = related.foreignKey（main.pk = related.fk）
        let sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert_eq!(
            sql,
            "SELECT main.* FROM users main LEFT JOIN profiles related ON main.id = related.user_id"
        );
    }

    #[test]
    fn test_r5_php_eagerly_join_on_belongs_to() {
        // R5-3：PHP OneToOne::eagerly() BelongsTo 分支 JOIN ON 条件
        // 对齐 OneToOne.php 第 101 行：$joinOn = $foreignKeyExp . '=' . $joinAlias . '.' . $this->localKey
        // BelongsTo: main.foreignKey = related.localKey（main.fk = related.pk）
        let sql = php_with_join_sql("orders", "users", "user_id", "id", JoinMode::Inner);
        assert_eq!(
            sql,
            "SELECT main.* FROM orders main INNER JOIN users related ON main.user_id = related.id"
        );
    }

    #[test]
    fn test_r5_php_with_join_default_left() {
        // R5-4：PHP withJoin() 默认 $joinType='' → $this->joinType='LEFT'
        // 对齐 OneToOne.php 第 89 行：$joinType = $joinType ?: $this->joinType
        let sql_left = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert!(sql_left.contains("LEFT JOIN"));
    }

    #[test]
    fn test_r5_php_has_default_inner_join() {
        // R5-5：PHP has() 默认 $joinType='INNER'
        // 对齐 HasMany.php 第 298 行：public function has(... $joinType = 'INNER' ...)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        assert!(sql.contains("INNER JOIN"));
    }

    #[test]
    fn test_r5_php_has_group_by_foreign_key() {
        // R5-6：PHP has() GROUP BY related.foreignKey
        // 对齐 HasMany.php 第 317 行：->group($relation . '.' . $this->foreignKey)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        assert!(sql.contains("GROUP BY related.user_id"));
    }

    #[test]
    fn test_r5_php_has_having_count() {
        // R5-7：PHP has() HAVING count($id) $operator $count
        // 对齐 HasMany.php 第 318 行：->having('count(' . $id . ')' . $operator . $count)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 3);
        assert!(sql.contains("HAVING count(*) >= 3"));
    }

    #[test]
    fn test_r5_php_has_count_field_star_default() {
        // R5-8：PHP has() 默认 $id='*'，使用 count(*)
        // 对齐 HasMany.php 第 298 行：public function has(... $id = '*' ...)
        let sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        assert!(sql.contains("count(*)"));
    }

    #[test]
    fn test_r5_php_with_join_returns_no_result_for_non_one_to_one() {
        // R5-9：PHP withJoin() 对非 OneToOne 关联返回 false → 静默剔除
        // sz-rust 端通过 is_one_to_one() 函数让调用方判断是否调用 php_with_join_sql
        let has_many_rel = Relation::HasMany(HasMany {
            foreign_key: "user_id".to_string(),
            child_model: "orders".to_string(),
            child_pk: "id".to_string(),
        });
        // 调用方应先检查 is_one_to_one，false 时不调用 php_with_join_sql
        if is_one_to_one(&has_many_rel) {
            panic!("HasMany should not be OneToOne");
        }
        // 不调用 php_with_join_sql，对齐 PHP unset($with[$key]) 行为
    }

    // ====================================================================
    // 组 7：集成测试
    // ====================================================================

    #[test]
    fn test_integration_with_join_then_has_combined() {
        // 集成测试：withJoin + has 组合场景
        // 场景：查询有至少 1 个 profile（HasOne，LEFT JOIN）且至少 3 个订单（HasMany，INNER JOIN + GROUP BY + HAVING）的用户

        // withJoin profile（HasOne）
        let with_join_sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert!(with_join_sql.contains("LEFT JOIN profiles"));

        // has orders（HasMany）
        let has_sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 3);
        assert!(has_sql.contains("INNER JOIN orders"));
        assert!(has_sql.contains("HAVING count(*) >= 3"));
    }

    #[test]
    fn test_integration_join_mode_round_trip() {
        // 集成测试：JoinMode 转字符串再判断
        for mode in [JoinMode::Left, JoinMode::Inner] {
            let mode_str = join_mode_str(mode);
            let sql = php_with_join_sql("users", "profiles", "id", "user_id", mode);
            assert!(sql.contains(&format!(" {} JOIN ", mode_str)));
        }
    }

    #[test]
    fn test_integration_inspect_relation_then_php_with_join_sql() {
        // 集成测试：inspect_relation 提取元数据 → 判断 is_one_to_one → 调用 php_with_join_sql
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "profile",
            Relation::HasOne(HasOne {
                foreign_key: "user_id".to_string(),
                child_model: "profiles".to_string(),
                child_pk: "id".to_string(),
            }),
        );

        let inspect_result = inspect_relation(&relations, "profile").unwrap();
        let (related_table, foreign_key, primary_key, is_many) = inspect_result;
        assert!(!is_many); // HasOne 不是 many

        let relation_ref = relations.get("profile").unwrap();
        assert!(is_one_to_one(relation_ref)); // OneToOne 支持 JOIN

        // 调用 php_with_join_sql（HasOne: main.pk = related.fk）
        let sql = php_with_join_sql(
            "users",
            related_table,
            primary_key,
            foreign_key,
            JoinMode::Left,
        );
        assert_eq!(
            sql,
            "SELECT main.* FROM users main LEFT JOIN profiles related ON main.id = related.user_id"
        );
    }

    #[test]
    fn test_integration_inspect_relation_rejects_has_many_for_join() {
        // 集成测试：inspect_relation 返回 is_many=true 时，调用方应跳过 php_with_join_sql
        use std::collections::HashMap;
        let mut relations: HashMap<&str, Relation> = HashMap::new();
        relations.insert(
            "orders",
            Relation::HasMany(HasMany {
                foreign_key: "user_id".to_string(),
                child_model: "orders".to_string(),
                child_pk: "id".to_string(),
            }),
        );

        let inspect_result = inspect_relation(&relations, "orders").unwrap();
        let (_, _, _, is_many) = inspect_result;
        assert!(is_many); // HasMany 是 many

        let relation_ref = relations.get("orders").unwrap();
        assert!(!is_one_to_one(relation_ref)); // HasMany 不支持 JOIN

        // 调用方应跳过 php_with_join_sql，改用 has 或 with IN 模式
    }

    #[test]
    fn test_integration_three_modes_comparison() {
        // 集成测试：三种模式 SQL 对比（同一对表 users + orders）
        // 模式 1：with IN（已实现，这里调用 with::has_many_in_sql）
        use crate::relation::with::has_many_in_sql;
        let in_sql = has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
        assert!(in_sql.contains("WHERE user_id IN (1, 2, 3)"));

        // 模式 2：withJoin（仅 OneToOne，HasMany 不支持，这里用 has_one 演示）
        let join_sql = php_with_join_sql("users", "profiles", "id", "user_id", JoinMode::Left);
        assert!(join_sql.contains("LEFT JOIN"));

        // 模式 3：has JOIN + GROUP BY + HAVING COUNT
        let has_sql = php_has_join_sql("users", "orders", "id", "user_id", "*", ">=", 1);
        assert!(has_sql.contains("INNER JOIN"));
        assert!(has_sql.contains("GROUP BY"));
        assert!(has_sql.contains("HAVING count"));
    }
}
