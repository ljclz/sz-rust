// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! `HasMany` 关联 — PHP 命名约定 + SQL 片段构造器
//!
//! 本模块对齐 PHP `think\Model::hasMany()` 行为，提供：
//!
//! 1. [`class_to_snake_case`]：PHP `Str::snake` 命名转换（如 `User` → `user`，`OrderItem` → `order_item`）
//! 2. [`default_foreign_key`]：PHP `getForeignKey` 默认外键（`snake_case(class) . '_id'`）
//! 3. [`php_has_many`]：构造 `HasMany` 配置（应用 PHP 默认值）
//! 4. [`has_many_sql`]：生成 SQL 片段（用于测试验证）
//!
//! ## PHP 端 `hasMany` 签名（think-orm 2.0.x）
//!
//! ```php
//! public function hasMany(
//!     string $model,
//!     string $foreignKey = '',
//!     string $localKey = ''
//! ): HasMany
//! ```
//!
//! ## PHP 默认值（`RelationShip` trait）
//!
//! - `foreignKey` 默认：`Str::snake($this->name) . '_id'`
//!   - `$this->name` 为 model 类名（不含命名空间）
//!   - 例如 `User` → `user_id`，`OrderItem` → `order_item_id`
//! - `localKey` 默认：`$this->pk`（model 主键字段名，通常为 `id`）
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
//! - **PHP 命名约定辅助函数**：`class_to_snake_case` / `default_foreign_key`
//! - **配置构造器**：`php_has_many` 返回 sz-orm-core `HasMany` 结构体
//! - **SQL 片段构造器**：`has_many_sql` 返回 SQL 字符串，用于测试验证
//!
//! 端到端关联加载由 sz-orm-core `WithRelation::load()` 内部实现并测试。

use super::HasMany;

// ============================================================================
// PHP 命名约定辅助函数
// ============================================================================

/// PHP `Str::snake` 命名转换（think-orm `RelationShip::getForeignKey` 使用）
///
/// 算法对齐 PHP `think\helper\Str::snake`：
///
/// 1. 全小写字符串原样返回
/// 2. 否则：每个大写字母前插入下划线（首字符除外），全部转小写
///
/// ## 示例
///
/// | 输入 | 输出 |
/// |------|------|
/// | `"User"` | `"user"` |
/// | `"OrderItem"` | `"order_item"` |
/// | `"Customer"` | `"customer"` |
/// | `"URL"` | `"u_r_l"` |
/// | `"user_id"` | `"user_id"` |
///
/// ## PHP 对齐
///
/// ```php
/// // think-orm 2.0.x
/// Str::snake("User") // "user"
/// Str::snake("OrderItem") // "order_item"
/// Str::snake("URL") // "u_r_l"
/// ```
pub fn class_to_snake_case(name: &str) -> String {
    // 全小写原样返回（对齐 PHP `ctype_lower` 检查）
    if name.chars().all(|c| !c.is_uppercase()) {
        return name.to_string();
    }

    let mut result = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

/// PHP `getForeignKey` 默认外键名
///
/// 对齐 PHP `think-orm RelationShip::getForeignKey`：
///
/// ```php
/// protected function getForeignKey(string $name): string
/// {
///     if (strpos($name, '\\')) {
///         $name = basename(str_replace('\\', '/', $name));
///     }
///     return Str::snake($name) . '_id';
/// }
/// ```
///
/// 若输入含命名空间（含 `\`），先提取最后一段作为类名。
///
/// ## 示例
///
/// | 输入 | 输出 |
/// |------|------|
/// | `"User"` | `"user_id"` |
/// | `"OrderItem"` | `"order_item_id"` |
/// | `"app\\model\\User"` | `"user_id"` |
/// | `"app\\model\\OrderItem"` | `"order_item_id"` |
pub fn default_foreign_key(parent_class: &str) -> String {
    // 对齐 PHP `strpos($name, '\\')` + `basename(str_replace('\\', '/', $name))`
    let class_name = parent_class
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(parent_class);
    format!("{}_id", class_to_snake_case(class_name))
}

// ============================================================================
// HasMany 配置构造器
// ============================================================================

/// 构造 `HasMany` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::hasMany($model, $foreignKey = '', $localKey = '')`：
///
/// - `foreignKey` 默认：`default_foreign_key(parent_class)`
/// - `child_pk` 默认：`"id"`（对齐 PHP `$this->pk` 通常为 `id`）
///
/// ## 参数
///
/// - `parent_class`：父模型类名（如 `"User"` 或 `"app\\model\\User"`），用于推导默认外键
/// - `child_table`：子表名（如 `"orders"`）
/// - `foreign_key`：外键字段名（`None` 使用默认值）
/// - `child_pk`：子表主键字段名（`None` 使用 `"id"`）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::has_many::php_has_many;
///
/// // 等价 PHP: $this->hasMany(Order::class)
/// let rel = php_has_many("User", "orders", None, None);
/// assert_eq!(rel.foreign_key, "user_id");
/// assert_eq!(rel.child_model, "orders");
/// assert_eq!(rel.child_pk, "id");
///
/// // 等价 PHP: $this->hasMany(Order::class, 'uid')
/// let rel = php_has_many("User", "orders", Some("uid"), None);
/// assert_eq!(rel.foreign_key, "uid");
///
/// // 等价 PHP: $this->hasMany(Order::class, 'uid', 'pk')
/// // 注意：PHP localKey 对应 sz-orm-core::HasMany 不直接存储（由 model.pk() 获取）
/// // sz-rust 端 child_pk 对应子表主键，与 PHP localKey 不同
/// let rel = php_has_many("User", "orders", Some("uid"), Some("oid"));
/// assert_eq!(rel.foreign_key, "uid");
/// assert_eq!(rel.child_pk, "oid");
/// ```
pub fn php_has_many(
    parent_class: &str,
    child_table: &str,
    foreign_key: Option<&str>,
    child_pk: Option<&str>,
) -> HasMany {
    HasMany {
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

/// 生成 `HasMany` 关联查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `HasMany` 分支生成的 SQL：
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
/// use sz_rust_core::relation::has_many::has_many_sql;
///
/// let sql = has_many_sql("orders", "user_id", "1");
/// assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 1");
/// ```
pub fn has_many_sql(child_table: &str, foreign_key: &str, parent_pk_value: &str) -> String {
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
    // 组 1：class_to_snake_case（PHP Str::snake 对齐）
    // ====================================================================

    #[test]
    fn test_snake_single_word() {
        // PHP: Str::snake("User") = "user"
        assert_eq!(class_to_snake_case("User"), "user");
        assert_eq!(class_to_snake_case("Order"), "order");
        assert_eq!(class_to_snake_case("Customer"), "customer");
    }

    #[test]
    fn test_snake_multi_word_camel_case() {
        // PHP: Str::snake("OrderItem") = "order_item"
        assert_eq!(class_to_snake_case("OrderItem"), "order_item");
        assert_eq!(class_to_snake_case("UserRole"), "user_role");
        assert_eq!(class_to_snake_case("UploadFile"), "upload_file");
    }

    #[test]
    fn test_snake_all_lowercase_unchanged() {
        // PHP: ctype_lower 检查，全小写原样返回
        assert_eq!(class_to_snake_case("user"), "user");
        assert_eq!(class_to_snake_case("user_id"), "user_id");
        assert_eq!(class_to_snake_case(""), "");
    }

    #[test]
    fn test_snake_all_uppercase() {
        // PHP: Str::snake("URL") = "u_r_l"
        // 每个大写字母前插入下划线（首字符除外）
        assert_eq!(class_to_snake_case("URL"), "u_r_l");
        assert_eq!(class_to_snake_case("ABC"), "a_b_c");
    }

    #[test]
    fn test_snake_mixed_case() {
        // 混合大小写
        assert_eq!(class_to_snake_case("userID"), "user_i_d");
        assert_eq!(class_to_snake_case("OrderID"), "order_i_d");
    }

    // ====================================================================
    // 组 2：default_foreign_key（PHP getForeignKey 对齐）
    // ====================================================================

    #[test]
    fn test_default_foreign_key_simple_class() {
        // PHP: $this->hasMany(Order::class) → foreign_key = "user_id"
        assert_eq!(default_foreign_key("User"), "user_id");
        assert_eq!(default_foreign_key("Order"), "order_id");
        assert_eq!(default_foreign_key("Customer"), "customer_id");
    }

    #[test]
    fn test_default_foreign_key_multi_word_class() {
        // PHP: $this->hasMany(OrderItem::class) → foreign_key = "order_item_id"
        assert_eq!(default_foreign_key("OrderItem"), "order_item_id");
        assert_eq!(default_foreign_key("UserRole"), "user_role_id");
    }

    #[test]
    fn test_default_foreign_key_with_namespace() {
        // PHP: $this->hasMany(\app\model\Order::class)
        // getForeignKey 先 basename(str_replace('\\', '/', $name)) 提取类名
        assert_eq!(default_foreign_key("app\\model\\User"), "user_id");
        assert_eq!(
            default_foreign_key("app\\model\\OrderItem"),
            "order_item_id"
        );
        // 支持正斜杠路径
        assert_eq!(default_foreign_key("app/model/User"), "user_id");
    }

    #[test]
    fn test_default_foreign_key_all_uppercase_class() {
        // 极端情况：全大写类名（不常见，但需对齐 PHP Str::snake 行为）
        assert_eq!(default_foreign_key("URL"), "u_r_l_id");
    }

    // ====================================================================
    // 组 3：php_has_many 配置构造器
    // ====================================================================

    #[test]
    fn test_php_has_many_default_foreign_key() {
        // PHP: $this->hasMany(Order::class)
        // 等价：foreign_key = "user_id", child_pk = "id"
        let rel = php_has_many("User", "orders", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.child_model, "orders");
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_php_has_many_explicit_foreign_key() {
        // PHP: $this->hasMany(Order::class, 'uid')
        let rel = php_has_many("User", "orders", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.child_model, "orders");
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_php_has_many_explicit_child_pk() {
        // PHP: $this->hasMany(Order::class, 'uid', 'pk')
        // 注意：sz-orm-core::HasMany 的 child_pk 是子表主键字段名
        let rel = php_has_many("User", "orders", Some("uid"), Some("oid"));
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.child_model, "orders");
        assert_eq!(rel.child_pk, "oid");
    }

    #[test]
    fn test_php_has_many_multi_word_parent() {
        // PHP: $this->hasMany(OrderItem::class) on OrderItemController
        let rel = php_has_many("OrderItem", "order_items", None, None);
        assert_eq!(rel.foreign_key, "order_item_id");
        assert_eq!(rel.child_model, "order_items");
    }

    #[test]
    fn test_php_has_many_with_namespace_parent() {
        // PHP: $this->hasMany(\app\model\Order::class)
        let rel = php_has_many("app\\model\\User", "orders", None, None);
        assert_eq!(rel.foreign_key, "user_id");
    }

    // ====================================================================
    // 组 4：has_many_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_has_many_sql_numeric_pk() {
        // 数值型主键
        let sql = has_many_sql("orders", "user_id", "1");
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 1");
    }

    #[test]
    fn test_has_many_sql_string_pk() {
        // 字符串型主键（如 UUID）
        // 注意：sz-orm-core WithRelation::load 内部通过 pk_to_sql_string 自动加引号
        // 本函数仅用于测试 SQL 模式，调用方负责转义
        let sql = has_many_sql("orders", "user_id", "'abc-123'");
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 'abc-123'");
    }

    #[test]
    fn test_has_many_sql_custom_foreign_key() {
        // 自定义外键
        let sql = has_many_sql("orders", "uid", "1");
        assert_eq!(sql, "SELECT * FROM orders WHERE uid = 1");
    }

    #[test]
    fn test_has_many_sql_multi_word_table() {
        // 多单词表名
        let sql = has_many_sql("order_items", "order_id", "1");
        assert_eq!(sql, "SELECT * FROM order_items WHERE order_id = 1");
    }

    #[test]
    fn test_has_many_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load HasMany 分支一致
        // sz-orm-core 源码：
        //   format!("SELECT * FROM {} WHERE {} = {}", config.child_model, config.foreign_key, pk_str)
        let sql = has_many_sql("orders", "user_id", "1");
        assert!(sql.starts_with("SELECT * FROM orders WHERE user_id = "));
    }

    // ====================================================================
    // 组 5：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_has_many_default_foreign_key_convention() {
        // R5-1：PHP `hasMany(Order::class)` 默认外键 `user_id`
        // PHP 源码：$foreignKey = $foreignKey ?: $this->getForeignKey($this->name);
        //          getForeignKey($name) = Str::snake($name) . '_id'
        assert_eq!(default_foreign_key("User"), "user_id");
        assert_eq!(default_foreign_key("Order"), "order_id");
        assert_eq!(default_foreign_key("Customer"), "customer_id");
        assert_eq!(default_foreign_key("OrderItem"), "order_item_id");
    }

    #[test]
    fn test_r5_php_has_many_explicit_foreign_key_overrides_default() {
        // R5-2：PHP `hasMany(Order::class, 'uid')` 显式外键覆盖默认值
        let rel = php_has_many("User", "orders", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_ne!(rel.foreign_key, default_foreign_key("User"));
    }

    #[test]
    fn test_r5_php_has_many_default_local_key_is_id() {
        // R5-3：PHP `hasMany` 默认 localKey = $this->pk（通常为 `id`）
        // sz-orm-core::HasMany.child_pk 对应子表主键字段名，默认 "id"
        let rel = php_has_many("User", "orders", None, None);
        assert_eq!(rel.child_pk, "id");
    }

    #[test]
    fn test_r5_php_has_many_sql_pattern_matches_think_orm() {
        // R5-4：PHP hasMany SQL 模式 `SELECT * FROM child WHERE fk = pk_value`
        // sz-orm-core::WithRelation::load HasMany 分支生成相同模式
        let sql = has_many_sql("orders", "user_id", "1");
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 1");
    }

    #[test]
    fn test_r5_php_has_many_returns_collection() {
        // R5-5：PHP hasMany 返回集合（多行），sz-orm-core::Relation::HasMany 同样返回 Vec<Value>
        // sz-orm-core::RelationAccess::get_has_many 返回 Vec<HashMap<String, Value>>
        // 本测试验证 HasMany struct 字段完整性（端到端加载由 sz-orm-core 覆盖）
        let rel = php_has_many("User", "orders", None, None);
        assert!(!rel.child_model.is_empty());
        assert!(!rel.foreign_key.is_empty());
        assert!(!rel.child_pk.is_empty());
    }

    #[test]
    fn test_r5_php_str_snake_naming_convention() {
        // R5-6：PHP Str::snake 命名转换对齐
        // think-orm 2.0.x 使用 think\helper\Str::snake
        assert_eq!(class_to_snake_case("User"), "user");
        assert_eq!(class_to_snake_case("OrderItem"), "order_item");
        assert_eq!(class_to_snake_case("UploadFile"), "upload_file");
        // 全大写（极端情况）
        assert_eq!(class_to_snake_case("URL"), "u_r_l");
    }

    #[test]
    fn test_r5_php_has_many_namespace_handling() {
        // R5-7：PHP getForeignKey 处理命名空间
        // PHP 源码：if (strpos($name, '\\')) { $name = basename(str_replace('\\', '/', $name)); }
        assert_eq!(default_foreign_key("app\\model\\User"), "user_id");
        assert_eq!(
            default_foreign_key("app\\model\\OrderItem"),
            "order_item_id"
        );
        assert_eq!(default_foreign_key("App\\Model\\User"), "user_id");
    }

    #[test]
    fn test_r5_php_has_many_delegates_to_sz_orm_core() {
        // R5-8：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // sz-rust 端不重新实现关联加载机制，仅提供 PHP 命名约定辅助函数
        // 验证 php_has_many 返回 sz-orm-core::HasMany 类型
        let rel = php_has_many("User", "orders", None, None);
        // 验证类型为 sz_orm_core::HasMany（编译时类型检查）
        let _: &HasMany = &rel;
        // 验证字段可被 sz-orm-core::Relation::HasMany 包装
        let relation = sz_orm_core::Relation::HasMany(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::HasMany(_)));
    }

    // ====================================================================
    // 组 6：集成测试（PHP 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_user_has_many_orders() {
        // PHP 业务场景：User hasMany Orders
        // ```php
        // public function orders()
        // {
        //     return $this->hasMany(Order::class);
        // }
        // ```
        let rel = php_has_many("User", "orders", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.child_model, "orders");
        assert_eq!(rel.child_pk, "id");

        // 生成 SQL
        let sql = has_many_sql(&rel.child_model, &rel.foreign_key, "1");
        assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 1");
    }

    #[test]
    fn test_integration_customer_has_many_contracts() {
        // PHP 业务场景：Customer hasMany Contracts
        // ```php
        // public function contracts()
        // {
        //     return $this->hasMany(Contract::class, 'customer_id');
        // }
        // ```
        let rel = php_has_many("Customer", "contracts", Some("customer_id"), None);
        assert_eq!(rel.foreign_key, "customer_id");
        assert_eq!(rel.child_model, "contracts");

        let sql = has_many_sql(&rel.child_model, &rel.foreign_key, "100");
        assert_eq!(sql, "SELECT * FROM contracts WHERE customer_id = 100");
    }

    #[test]
    fn test_integration_dept_has_many_users() {
        // PHP 业务场景：Dept hasMany Users（部门有多个用户）
        let rel = php_has_many("Dept", "users", None, None);
        assert_eq!(rel.foreign_key, "dept_id");
        assert_eq!(rel.child_model, "users");

        let sql = has_many_sql(&rel.child_model, &rel.foreign_key, "5");
        assert_eq!(sql, "SELECT * FROM users WHERE dept_id = 5");
    }
}
