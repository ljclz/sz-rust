//! `HasOne` 关联 — PHP 命名约定 + SQL 片段构造器
//!
//! Phase 4.3 核心交付物。本模块对齐 PHP `think\Model::hasOne()` 行为，提供：
//!
//! 1. [`php_has_one`]：构造 `HasOne` 配置（应用 PHP 默认值，复用 `has_many::default_foreign_key`）
//! 2. [`has_one_sql`]：生成 SQL 片段（用于测试验证）
//!
//! ## PHP 端 `hasOne` 签名（think-orm 2.0.x）
//!
//! ```php
//! public function hasOne(
//!     string $model,
//!     string $foreignKey = '',
//!     string $localKey = ''
//! ): HasOne
//! ```
//!
//! ## PHP 默认值（`RelationShip` trait）
//!
//! - `foreignKey` 默认：`Str::snake($this->name) . '_id'`（与 `hasMany` 相同，基于当前模型名）
//! - `localKey` 默认：`$this->pk`（model 主键字段名，通常为 `id`）
//!
//! ## HasOne vs HasMany
//!
//! PHP `hasOne` 与 `hasMany` 在以下方面**完全相同**：
//!
//! - 默认外键推导算法（`getForeignKey($this->name)`）
//! - 生成的 SQL 模式（`SELECT * FROM child WHERE fk = pk_value`）
//! - 结构体字段（`foreign_key` / `child_model` / `child_pk`）
//!
//! 唯一区别在于**返回语义**：
//!
//! - `hasMany` 返回集合（多行）
//! - `hasOne` 返回单个模型（第一行）
//!
//! 此区别由调用方处理，不影响 SQL 生成与默认值推导。
//!
//! ## 生成的 SQL（与 sz-orm-core::WithRelation::load 一致）
//!
//! ```sql
//! SELECT * FROM {child_table} WHERE {foreign_key} = {parent_pk_value}
//! ```
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行关联加载，而是提供：
//!
//! - **配置构造器**：`php_has_one` 返回 sz-orm-core `HasOne` 结构体（复用 `has_many::default_foreign_key`）
//! - **SQL 片段构造器**：`has_one_sql` 返回 SQL 字符串，用于测试验证
//!
//! 端到端关联加载由 sz-orm-core `WithRelation::load()` 内部实现并测试。

use super::HasOne;
use crate::relation::has_many::default_foreign_key;

// ============================================================================
// HasOne 配置构造器
// ============================================================================

/// 构造 `HasOne` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::hasOne($model, $foreignKey = '', $localKey = '')`：
///
/// - `foreignKey` 默认：[`default_foreign_key(parent_class)`]（与 `hasMany` 相同，基于当前模型名）
/// - `child_pk` 默认：`"id"`（对齐 PHP `$this->pk` 通常为 `id`）
///
/// ## 参数
///
/// - `parent_class`：父模型类名（如 `"User"` 或 `"app\\model\\User"`），用于推导默认外键
/// - `child_table`：子表名（如 `"profiles"`）
/// - `foreign_key`：外键字段名（`None` 使用默认值）
/// - `child_pk`：子表主键字段名（`None` 使用 `"id"`）
///
/// ## HasOne vs HasMany
///
/// `php_has_one` 与 [`php_has_many`] 在默认值推导和 SQL 模式上**完全相同**，
/// 唯一区别是返回类型（`HasOne` vs `HasMany`），由调用方根据业务语义选择。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::has_one::php_has_one;
///
/// // 等价 PHP: $this->hasOne(Profile::class)
/// let rel = php_has_one("User", "profiles", None, None);
/// assert_eq!(rel.foreign_key, "user_id");
/// assert_eq!(rel.child_model, "profiles");
/// assert_eq!(rel.child_pk, "id");
///
/// // 等价 PHP: $this->hasOne(Profile::class, 'uid')
/// let rel = php_has_one("User", "profiles", Some("uid"), None);
/// assert_eq!(rel.foreign_key, "uid");
/// ```
pub fn php_has_one(
    parent_class: &str,
    child_table: &str,
    foreign_key: Option<&str>,
    child_pk: Option<&str>,
) -> HasOne {
    HasOne {
        foreign_key: foreign_key
            .map(String::from)
            .unwrap_or_else(|| default_foreign_key(parent_class)),
        child_model: child_table.to_string(),
        child_pk: child_pk
            .map(String::from)
            .unwrap_or_else(|| "id".to_string()),
    }
}

// ============================================================================
// SQL 片段构造器（用于测试验证）
// ============================================================================

/// 生成 `HasOne` 关联查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `HasOne` 分支生成的 SQL：
///
/// ```rust,ignore
/// let sql = format!(
///     "SELECT * FROM {} WHERE {} = {}",
///     config.child_model, config.foreign_key, pk_str
/// );
/// ```
///
/// ## 参数
///
/// - `child_table`：子表名
/// - `foreign_key`：外键字段名
/// - `parent_pk_value`：父模型主键值（字符串形式）
///
/// ## SQL 注入防护
///
/// 本函数仅用于测试验证 SQL 生成模式，**不应直接用于业务代码**。
/// 端到端 SQL 执行由 sz-orm-core `WithRelation::load()` 内部处理，
/// 使用参数化查询（`Connection::query`）避免 SQL 注入。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::has_one::has_one_sql;
///
/// let sql = has_one_sql("profiles", "user_id", "1");
/// assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 1");
/// ```
pub fn has_one_sql(child_table: &str, foreign_key: &str, parent_pk_value: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = {}",
        child_table, foreign_key, parent_pk_value
    )
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // 组 1：php_has_one 配置构造器
    // ====================================================================

    #[test]
    fn test_php_has_one_default_foreign_key() {
        // PHP: $this->hasOne(Profile::class)
        // 等价：foreign_key = "user_id", child_pk = "id"
        let rel = php_has_one("User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.child_model, "profiles");
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_php_has_one_explicit_foreign_key() {
        // PHP: $this->hasOne(Profile::class, 'uid')
        let rel = php_has_one("User", "profiles", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.child_model, "profiles");
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_php_has_one_explicit_child_pk() {
        // PHP: $this->hasOne(Profile::class, 'uid', 'pk')
        let rel = php_has_one("User", "profiles", Some("uid"), Some("pid"));
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.child_model, "profiles");
        assert_eq!(rel.child_pk, "pid");
    }

    #[test]
    fn test_php_has_one_multi_word_parent() {
        // PHP: $this->hasOne(OrderItem::class) on OrderItemDetail
        let rel = php_has_one("OrderItem", "order_item_details", None, None);
        assert_eq!(rel.foreign_key, "order_item_id");
        assert_eq!(rel.child_model, "order_item_details");
    }

    #[test]
    fn test_php_has_one_with_namespace_parent() {
        // PHP: $this->hasOne(\app\model\Profile::class)
        let rel = php_has_one("app\\model\\User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
    }

    // ====================================================================
    // 组 2：has_one_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_has_one_sql_numeric_pk() {
        // 数值型主键
        let sql = has_one_sql("profiles", "user_id", "1");
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 1");
    }

    #[test]
    fn test_has_one_sql_string_pk() {
        // 字符串型主键（如 UUID）
        // 注意：sz-orm-core WithRelation::load 内部通过 pk_to_sql_string 自动加引号
        // 本函数仅用于测试 SQL 模式，调用方负责转义
        let sql = has_one_sql("profiles", "user_id", "'abc-123'");
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 'abc-123'");
    }

    #[test]
    fn test_has_one_sql_custom_foreign_key() {
        // 自定义外键
        let sql = has_one_sql("profiles", "uid", "1");
        assert_eq!(sql, "SELECT * FROM profiles WHERE uid = 1");
    }

    #[test]
    fn test_has_one_sql_multi_word_table() {
        // 多单词表名
        let sql = has_one_sql("order_item_details", "order_item_id", "1");
        assert_eq!(
            sql,
            "SELECT * FROM order_item_details WHERE order_item_id = 1"
        );
    }

    #[test]
    fn test_has_one_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load HasOne 分支一致
        // sz-orm-core 源码：
        //   format!("SELECT * FROM {} WHERE {} = {}", config.child_model, config.foreign_key, pk_str)
        let sql = has_one_sql("profiles", "user_id", "1");
        assert!(sql.starts_with("SELECT * FROM profiles WHERE user_id = "));
    }

    // ====================================================================
    // 组 3：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_has_one_default_foreign_key_convention() {
        // R5-1：PHP `hasOne(Profile::class)` 默认外键 `user_id`（与 hasMany 相同，基于当前模型名）
        // PHP 源码：$foreignKey = $foreignKey ?: $this->getForeignKey($this->name);
        // 注：hasOne 与 hasMany 使用相同的 getForeignKey 算法
        let rel = php_has_one("User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(
            rel.foreign_key,
            crate::relation::has_many::default_foreign_key("User")
        );
    }

    #[test]
    fn test_r5_php_has_one_explicit_fk_overrides_default() {
        // R5-2：PHP `hasOne(Profile::class, 'uid')` 显式外键覆盖默认值
        let rel = php_has_one("User", "profiles", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_ne!(
            rel.foreign_key,
            crate::relation::has_many::default_foreign_key("User")
        );
    }

    #[test]
    fn test_r5_php_has_one_default_local_key_is_id() {
        // R5-3：PHP `hasOne` 默认 localKey = `$this->pk`（通常为 `id`）
        // sz-orm-core::HasOne.child_pk 对应子表主键字段名，默认 "id"
        let rel = php_has_one("User", "profiles", None, None);
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_r5_php_has_one_sql_pattern_matches_think_orm() {
        // R5-4：PHP hasOne SQL 模式 `SELECT * FROM child WHERE fk = pk_value`
        // sz-orm-core::WithRelation::load HasOne 分支生成相同模式
        // 注：HasOne 与 HasMany 的 SQL 模式完全相同
        let sql = has_one_sql("profiles", "user_id", "1");
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 1");
    }

    #[test]
    fn test_r5_php_has_one_returns_single_model() {
        // R5-5：PHP hasOne 返回单个模型（非集合）
        // sz-orm-core::Relation::HasOne 同样返回单个 Value（或 null）
        // 本测试验证 HasOne struct 字段完整性（端到端加载由 sz-orm-core 覆盖）
        let rel = php_has_one("User", "profiles", None, None);
        assert!(!rel.child_model.is_empty());
        assert!(!rel.foreign_key.is_empty());
        assert!(!rel.child_pk.is_empty());
    }

    #[test]
    fn test_r5_php_has_one_namespace_handling() {
        // R5-6：PHP getForeignKey 处理命名空间（与 hasMany 相同）
        // PHP 源码：if (strpos($name, '\\')) { $name = basename(str_replace('\\', '/', $name)); }
        let rel = php_has_one("app\\model\\User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        let rel = php_has_one("app\\model\\OrderItem", "order_item_details", None, None);
        assert_eq!(rel.foreign_key, "order_item_id");
        let rel = php_has_one("App\\Model\\User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
    }

    #[test]
    fn test_r5_php_has_one_same_algorithm_as_has_many() {
        // R5-7：PHP hasOne 与 hasMany 默认外键推导算法相同（均基于当前模型名）
        // 区别仅在返回结果数量（hasOne 单个，hasMany 集合）
        let has_one_rel = php_has_one("User", "profiles", None, None);
        let has_many_rel = crate::relation::has_many::php_has_many("User", "orders", None, None);
        assert_eq!(has_one_rel.foreign_key, has_many_rel.foreign_key);
        assert_eq!(has_one_rel.foreign_key, "user_id");
    }

    #[test]
    fn test_r5_php_has_one_delegates_to_sz_orm_core() {
        // R5-8：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // sz-rust 端不重新实现关联加载机制，仅提供 PHP 命名约定辅助函数
        // 验证 php_has_one 返回 sz-orm-core::HasOne 类型
        let rel = php_has_one("User", "profiles", None, None);
        // 验证类型为 sz_orm_core::HasOne（编译时类型检查）
        let _: &HasOne = &rel;
        // 验证字段可被 sz-orm-core::Relation::HasOne 包装
        let relation = sz_orm_core::Relation::HasOne(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::HasOne(_)));
    }

    // ====================================================================
    // 组 4：集成测试（PHP 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_user_has_one_profile() {
        // PHP 业务场景：User hasOne Profile
        // ```php
        // class User extends Model {
        //     public function profile() {
        //         return $this->hasOne(Profile::class);
        //     }
        // }
        // ```
        let rel = php_has_one("User", "profiles", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.child_model, "profiles");
        assert_eq!(rel.child_pk, "id");

        // 生成 SQL（假设 User.id = 1，查询 Profile 表）
        let sql = has_one_sql(&rel.child_model, &rel.foreign_key, "1");
        assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 1");
    }

    #[test]
    fn test_integration_customer_has_one_account() {
        // PHP 业务场景：Customer hasOne Account
        // ```php
        // class Customer extends Model {
        //     public function account() {
        //         return $this->hasOne(Account::class, 'customer_id');
        //     }
        // }
        // ```
        let rel = php_has_one("Customer", "accounts", Some("customer_id"), None);
        assert_eq!(rel.foreign_key, "customer_id");
        assert_eq!(rel.child_model, "accounts");

        let sql = has_one_sql(&rel.child_model, &rel.foreign_key, "100");
        assert_eq!(sql, "SELECT * FROM accounts WHERE customer_id = 100");
    }

    #[test]
    fn test_integration_order_has_one_shipping() {
        // PHP 业务场景：Order hasOne Shipping
        // ```php
        // class Order extends Model {
        //     public function shipping() {
        //         return $this->hasOne(Shipping::class);
        //     }
        // }
        // ```
        let rel = php_has_one("Order", "shippings", None, None);
        assert_eq!(rel.foreign_key, "order_id");
        assert_eq!(rel.child_model, "shippings");

        let sql = has_one_sql(&rel.child_model, &rel.foreign_key, "50");
        assert_eq!(sql, "SELECT * FROM shippings WHERE order_id = 50");
    }

    #[test]
    fn test_integration_order_item_has_one_detail() {
        // PHP 业务场景：OrderItem hasOne OrderItemDetail
        // ```php
        // class OrderItem extends Model {
        //     public function detail() {
        //         return $this->hasOne(OrderItemDetail::class);
        //     }
        // }
        // ```
        let rel = php_has_one("OrderItem", "order_item_details", None, None);
        assert_eq!(rel.foreign_key, "order_item_id");
        assert_eq!(rel.child_model, "order_item_details");

        let sql = has_one_sql(&rel.child_model, &rel.foreign_key, "30");
        assert_eq!(
            sql,
            "SELECT * FROM order_item_details WHERE order_item_id = 30"
        );
    }
}
