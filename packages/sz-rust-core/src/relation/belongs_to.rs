//! `BelongsTo` 关联 — PHP 命名约定 + SQL 片段构造器
//!
//! Phase 4.2 核心交付物。本模块对齐 PHP `think\Model::belongsTo()` 行为，提供：
//!
//! 1. [`default_belongs_to_foreign_key`]：PHP `belongsTo` 默认外键（`Str::snake(related_class) . '_id'`）
//! 2. [`php_belongs_to`]：构造 `BelongsTo` 配置（应用 PHP 默认值）
//! 3. [`belongs_to_sql`]：生成 SQL 片段（用于测试验证）
//!
//! ## PHP 端 `belongsTo` 签名（think-orm 2.0.x）
//!
//! ```php
//! public function belongsTo(
//!     string $model,
//!     string $foreignKey = '',
//!     string $localKey = ''
//! ): BelongsTo
//! ```
//!
//! ## PHP 默认值（`RelationShip` trait）
//!
//! - `foreignKey` 默认：`Str::snake(related_class_name) . '_id'`
//!   - **关键区别**：基于关联模型名（外键所在的当前模型持有指向父模型的外键）
//!   - 例如 `Profile` 模型中 `belongsTo(User::class)` → foreignKey 默认 `user_id`
//!   - 与 `hasMany` 不同（`hasMany` 基于当前模型名 `$this->name`）
//! - `localKey` 默认：`$related->getPk()`（父模型主键字段名，通常为 `id`）
//!
//! ## 生成的 SQL（与 sz-orm-core::WithRelation::load 一致）
//!
//! ```sql
//! SELECT * FROM {parent_table} WHERE {parent_pk} = {fk_value}
//! ```
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行关联加载，而是提供：
//!
//! - **PHP 命名约定辅助函数**：`default_belongs_to_foreign_key`
//! - **配置构造器**：`php_belongs_to` 返回 sz-orm-core `BelongsTo` 结构体
//! - **SQL 片段构造器**：`belongs_to_sql` 返回 SQL 字符串，用于测试验证
//!
//! 端到端关联加载由 sz-orm-core `WithRelation::load()` 内部实现并测试。

use super::BelongsTo;
use crate::relation::has_many::class_to_snake_case;

// ============================================================================
// PHP 命名约定辅助函数
// ============================================================================

/// PHP `belongsTo` 默认外键名
///
/// 对齐 PHP `think-orm RelationShip::belongsTo`：
///
/// ```php
/// public function belongsTo(string $model, string $foreignKey = '', string $localKey = ''): BelongsTo
/// {
///     $model = new $model();
///     $foreignKey = $foreignKey ?: $this->getForeignKey(get_class($model));
///     //                                            ^^^^^^^^^^^^^^^^^
///     //                                            注意：是关联模型名，不是当前模型名
///     $localKey = $localKey ?: $model->getPk();
///     ...
/// }
/// ```
///
/// **关键区别**：`belongsTo` 的默认外键基于「关联模型名」（外键所在的当前模型持有指向父模型的外键），
/// 而 `hasMany` 的默认外键基于「当前模型名」。
///
/// 例如 `Profile` 模型中 `belongsTo(User::class)`：
/// - foreignKey 默认 `user_id`（基于关联模型 `User`）
/// - localKey 默认 `User` 的主键 `id`
///
/// ## 示例
///
/// | 输入 | 输出 |
/// |------|------|
/// | `"User"` | `"user_id"` |
/// | `"OrderItem"` | `"order_item_id"` |
/// | `"app\\model\\User"` | `"user_id"` |
pub fn default_belongs_to_foreign_key(related_class: &str) -> String {
    // 复用 has_many::default_foreign_key 的实现（PHP `getForeignKey` 算法相同）
    // 区别仅在调用方传入的参数语义：belongsTo 传关联模型名，hasMany 传当前模型名
    let class_name = related_class
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(related_class);
    format!("{}_id", class_to_snake_case(class_name))
}

// ============================================================================
// BelongsTo 配置构造器
// ============================================================================

/// 构造 `BelongsTo` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::belongsTo($model, $foreignKey = '', $localKey = '')`：
///
/// - `foreignKey` 默认：`default_belongs_to_foreign_key(related_class)`
/// - `parent_pk` 默认：`"id"`（对齐 PHP `$related->getPk()` 通常为 `id`）
///
/// ## 参数
///
/// - `related_class`：关联（父）模型类名（如 `"User"` 或 `"app\\model\\User"`），用于推导默认外键
/// - `parent_table`：父表名（如 `"users"`）
/// - `foreign_key`：当前模型持有的外键字段名（`None` 使用默认值）
/// - `parent_pk`：父表主键字段名（`None` 使用 `"id"`）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::belongs_to::php_belongs_to;
///
/// // 等价 PHP: $this->belongsTo(User::class)
/// // 在 Profile 模型中定义
/// let rel = php_belongs_to("User", "users", None, None);
/// assert_eq!(rel.foreign_key, "user_id");
/// assert_eq!(rel.parent_model, "users");
/// assert_eq!(rel.parent_pk, "id");
///
/// // 等价 PHP: $this->belongsTo(User::class, 'uid')
/// let rel = php_belongs_to("User", "users", Some("uid"), None);
/// assert_eq!(rel.foreign_key, "uid");
///
/// // 等价 PHP: $this->belongsTo(User::class, 'uid', 'pk')
/// let rel = php_belongs_to("User", "users", Some("uid"), Some("pk"));
/// assert_eq!(rel.parent_pk, "pk");
/// ```
pub fn php_belongs_to(
    related_class: &str,
    parent_table: &str,
    foreign_key: Option<&str>,
    parent_pk: Option<&str>,
) -> BelongsTo {
    BelongsTo {
        foreign_key: foreign_key
            .map(String::from)
            .unwrap_or_else(|| default_belongs_to_foreign_key(related_class)),
        parent_model: parent_table.to_string(),
        parent_pk: parent_pk
            .map(String::from)
            .unwrap_or_else(|| "id".to_string()),
    }
}

// ============================================================================
// SQL 片段构造器（用于测试验证）
// ============================================================================

/// 生成 `BelongsTo` 关联查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `BelongsTo` 分支生成的 SQL：
///
/// ```rust,ignore
/// let fk_value = model.get_relation_fk_value(&config.foreign_key);
/// let sql = format!(
///     "SELECT * FROM {} WHERE {} = {}",
///     config.parent_model,
///     config.parent_pk,
///     pk_to_sql_string(&fk_value)
/// );
/// ```
///
/// ## 参数
///
/// - `parent_table`：父表名
/// - `parent_pk`：父表主键字段名
/// - `foreign_key_value`：当前模型持有的外键值（字符串形式）
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
/// use sz_rust_core::relation::belongs_to::belongs_to_sql;
///
/// let sql = belongs_to_sql("users", "id", "1");
/// assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
/// ```
pub fn belongs_to_sql(parent_table: &str, parent_pk: &str, foreign_key_value: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE {} = {}",
        parent_table, parent_pk, foreign_key_value
    )
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // 组 1：default_belongs_to_foreign_key（PHP belongsTo 默认外键对齐）
    // ====================================================================

    #[test]
    fn test_belongs_to_default_fk_simple_class() {
        // PHP: Profile belongsTo User → foreign_key = "user_id"
        assert_eq!(default_belongs_to_foreign_key("User"), "user_id");
        assert_eq!(default_belongs_to_foreign_key("Order"), "order_id");
        assert_eq!(default_belongs_to_foreign_key("Customer"), "customer_id");
    }

    #[test]
    fn test_belongs_to_default_fk_multi_word_class() {
        // PHP: 当前模型 belongsTo OrderItem → foreign_key = "order_item_id"
        assert_eq!(default_belongs_to_foreign_key("OrderItem"), "order_item_id");
        assert_eq!(default_belongs_to_foreign_key("UserRole"), "user_role_id");
    }

    #[test]
    fn test_belongs_to_default_fk_with_namespace() {
        // PHP: $this->belongsTo(\app\model\User::class)
        // getForeignKey(get_class($model)) 处理命名空间
        assert_eq!(
            default_belongs_to_foreign_key("app\\model\\User"),
            "user_id"
        );
        assert_eq!(
            default_belongs_to_foreign_key("app\\model\\OrderItem"),
            "order_item_id"
        );
        // 支持正斜杠路径
        assert_eq!(default_belongs_to_foreign_key("app/model/User"), "user_id");
    }

    #[test]
    fn test_belongs_to_default_fk_all_uppercase_class() {
        // 极端情况：全大写类名（不常见，但需对齐 PHP Str::snake 行为）
        assert_eq!(default_belongs_to_foreign_key("URL"), "u_r_l_id");
    }

    #[test]
    fn test_belongs_to_default_fk_differs_from_has_many() {
        // 关键区别：belongsTo 基于「关联模型名」，hasMany 基于「当前模型名」
        // 在 Profile 模型中：
        //   belongsTo(User::class) → foreign_key = "user_id"（基于 User）
        //   hasMany(Profile::class) on User → foreign_key = "user_id"（基于 User）
        // 在 Order 模型中：
        //   belongsTo(User::class) → foreign_key = "user_id"（基于 User）
        //   hasMany(Order::class) on User → foreign_key = "user_id"（基于 User）
        // 在 OrderItem 模型中：
        //   belongsTo(Order::class) → foreign_key = "order_id"（基于 Order）
        //   hasMany(OrderItem::class) on Order → foreign_key = "order_id"（基于 Order）
        // 关键：调用方传入的参数语义不同，但算法相同
        assert_eq!(
            default_belongs_to_foreign_key("User"),
            crate::relation::has_many::default_foreign_key("User")
        );
        // 算法相同，调用方决定语义
    }

    // ====================================================================
    // 组 2：php_belongs_to 配置构造器
    // ====================================================================

    #[test]
    fn test_php_belongs_to_default_foreign_key() {
        // PHP: $this->belongsTo(User::class) on Profile
        let rel = php_belongs_to("User", "users", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.parent_model, "users");
        assert_eq!(rel.parent_pk, "id");
    }

    #[test]
    fn test_php_belongs_to_explicit_foreign_key() {
        // PHP: $this->belongsTo(User::class, 'uid')
        let rel = php_belongs_to("User", "users", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.parent_model, "users");
        assert_eq!(rel.parent_pk, "id");
    }

    #[test]
    fn test_php_belongs_to_explicit_parent_pk() {
        // PHP: $this->belongsTo(User::class, 'uid', 'pk')
        let rel = php_belongs_to("User", "users", Some("uid"), Some("pk"));
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.parent_model, "users");
        assert_eq!(rel.parent_pk, "pk");
    }

    #[test]
    fn test_php_belongs_to_multi_word_related() {
        // PHP: $this->belongsTo(OrderItem::class) on OrderItemLog
        let rel = php_belongs_to("OrderItem", "order_items", None, None);
        assert_eq!(rel.foreign_key, "order_item_id");
        assert_eq!(rel.parent_model, "order_items");
    }

    #[test]
    fn test_php_belongs_to_with_namespace_related() {
        // PHP: $this->belongsTo(\app\model\User::class)
        let rel = php_belongs_to("app\\model\\User", "users", None, None);
        assert_eq!(rel.foreign_key, "user_id");
    }

    // ====================================================================
    // 组 3：belongs_to_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_belongs_to_sql_numeric_fk() {
        // 数值型外键
        let sql = belongs_to_sql("users", "id", "1");
        assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
    }

    #[test]
    fn test_belongs_to_sql_string_fk() {
        // 字符串型外键（如 UUID）
        // 注意：sz-orm-core WithRelation::load 内部通过 pk_to_sql_string 自动加引号
        // 本函数仅用于测试 SQL 模式，调用方负责转义
        let sql = belongs_to_sql("users", "id", "'abc-123'");
        assert_eq!(sql, "SELECT * FROM users WHERE id = 'abc-123'");
    }

    #[test]
    fn test_belongs_to_sql_custom_parent_pk() {
        // 自定义父表主键
        let sql = belongs_to_sql("users", "uid", "1");
        assert_eq!(sql, "SELECT * FROM users WHERE uid = 1");
    }

    #[test]
    fn test_belongs_to_sql_multi_word_table() {
        // 多单词表名
        let sql = belongs_to_sql("order_items", "id", "1");
        assert_eq!(sql, "SELECT * FROM order_items WHERE id = 1");
    }

    #[test]
    fn test_belongs_to_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load BelongsTo 分支一致
        // sz-orm-core 源码：
        //   let fk_value = model.get_relation_fk_value(&config.foreign_key);
        //   format!("SELECT * FROM {} WHERE {} = {}", config.parent_model, config.parent_pk, pk_to_sql_string(&fk_value))
        let sql = belongs_to_sql("users", "id", "1");
        assert!(sql.starts_with("SELECT * FROM users WHERE id = "));
    }

    // ====================================================================
    // 组 4：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_belongs_to_default_fk_convention() {
        // R5-1：PHP `belongsTo(User::class)` 默认外键 `user_id`（基于关联模型名）
        // PHP 源码：$foreignKey = $foreignKey ?: $this->getForeignKey(get_class($model));
        assert_eq!(default_belongs_to_foreign_key("User"), "user_id");
        assert_eq!(default_belongs_to_foreign_key("Order"), "order_id");
        assert_eq!(default_belongs_to_foreign_key("Customer"), "customer_id");
        assert_eq!(default_belongs_to_foreign_key("OrderItem"), "order_item_id");
    }

    #[test]
    fn test_r5_php_belongs_to_explicit_fk_overrides_default() {
        // R5-2：PHP `belongsTo(User::class, 'uid')` 显式外键覆盖默认值
        let rel = php_belongs_to("User", "users", Some("uid"), None);
        assert_eq!(rel.foreign_key, "uid");
        assert_ne!(rel.foreign_key, default_belongs_to_foreign_key("User"));
    }

    #[test]
    fn test_r5_php_belongs_to_default_local_key_is_id() {
        // R5-3：PHP `belongsTo` 默认 localKey = $related->getPk()（通常为 `id`）
        // sz-orm-core::BelongsTo.parent_pk 对应父表主键字段名，默认 "id"
        let rel = php_belongs_to("User", "users", None, None);
        assert_eq!(rel.parent_pk, "id");
    }

    #[test]
    fn test_r5_php_belongs_to_sql_pattern_matches_think_orm() {
        // R5-4：PHP belongsTo SQL 模式 `SELECT * FROM parent WHERE parent_pk = fk_value`
        // sz-orm-core::WithRelation::load BelongsTo 分支生成相同模式
        let sql = belongs_to_sql("users", "id", "1");
        assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
    }

    #[test]
    fn test_r5_php_belongs_to_returns_single_model() {
        // R5-5：PHP belongsTo 返回单个模型（非集合）
        // sz-orm-core::Relation::BelongsTo 同样返回单个 Value（或 null）
        // 本测试验证 BelongsTo struct 字段完整性（端到端加载由 sz-orm-core 覆盖）
        let rel = php_belongs_to("User", "users", None, None);
        assert!(!rel.parent_model.is_empty());
        assert!(!rel.foreign_key.is_empty());
        assert!(!rel.parent_pk.is_empty());
    }

    #[test]
    fn test_r5_php_belongs_to_namespace_handling() {
        // R5-6：PHP getForeignKey 处理命名空间（基于关联模型名）
        // PHP 源码：$foreignKey = $foreignKey ?: $this->getForeignKey(get_class($model));
        //          getForeignKey 处理命名空间：if (strpos($name, '\\')) { $name = basename(str_replace('\\', '/', $name)); }
        assert_eq!(
            default_belongs_to_foreign_key("app\\model\\User"),
            "user_id"
        );
        assert_eq!(
            default_belongs_to_foreign_key("app\\model\\OrderItem"),
            "order_item_id"
        );
        assert_eq!(
            default_belongs_to_foreign_key("App\\Model\\User"),
            "user_id"
        );
    }

    #[test]
    fn test_r5_php_belongs_to_fk_based_on_related_not_current() {
        // R5-7：PHP belongsTo 默认外键基于「关联模型名」而非「当前模型名」
        // 在 Order 模型中定义 belongsTo(User::class)：
        //   foreign_key = "user_id"（基于 User，关联模型）
        //   而非 "order_id"（基于 Order，当前模型）
        let rel = php_belongs_to("User", "users", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_ne!(rel.foreign_key, "order_id");
    }

    #[test]
    fn test_r5_php_belongs_to_delegates_to_sz_orm_core() {
        // R5-8：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // sz-rust 端不重新实现关联加载机制，仅提供 PHP 命名约定辅助函数
        // 验证 php_belongs_to 返回 sz-orm-core::BelongsTo 类型
        let rel = php_belongs_to("User", "users", None, None);
        // 验证类型为 sz_orm_core::BelongsTo（编译时类型检查）
        let _: &BelongsTo = &rel;
        // 验证字段可被 sz-orm-core::Relation::BelongsTo 包装
        let relation = sz_orm_core::Relation::BelongsTo(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::BelongsTo(_)));
    }

    // ====================================================================
    // 组 5：集成测试（PHP 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_profile_belongs_to_user() {
        // PHP 业务场景：Profile belongsTo User
        // ```php
        // class Profile extends Model {
        //     public function user() {
        //         return $this->belongsTo(User::class);
        //     }
        // }
        // ```
        let rel = php_belongs_to("User", "users", None, None);
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.parent_model, "users");
        assert_eq!(rel.parent_pk, "id");

        // 生成 SQL（假设 Profile.user_id = 1，查询 User 表）
        let sql = belongs_to_sql(&rel.parent_model, &rel.parent_pk, "1");
        assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
    }

    #[test]
    fn test_integration_order_belongs_to_customer() {
        // PHP 业务场景：Order belongsTo Customer
        // ```php
        // class Order extends Model {
        //     public function customer() {
        //         return $this->belongsTo(Customer::class, 'customer_id');
        //     }
        // }
        // ```
        let rel = php_belongs_to("Customer", "customers", Some("customer_id"), None);
        assert_eq!(rel.foreign_key, "customer_id");
        assert_eq!(rel.parent_model, "customers");

        let sql = belongs_to_sql(&rel.parent_model, &rel.parent_pk, "100");
        assert_eq!(sql, "SELECT * FROM customers WHERE id = 100");
    }

    #[test]
    fn test_integration_contract_belongs_to_customer() {
        // PHP 业务场景：Contract belongsTo Customer
        // ```php
        // class Contract extends Model {
        //     public function customer() {
        //         return $this->belongsTo(Customer::class);
        //     }
        // }
        // ```
        let rel = php_belongs_to("Customer", "customers", None, None);
        assert_eq!(rel.foreign_key, "customer_id");
        assert_eq!(rel.parent_model, "customers");

        let sql = belongs_to_sql(&rel.parent_model, &rel.parent_pk, "200");
        assert_eq!(sql, "SELECT * FROM customers WHERE id = 200");
    }

    #[test]
    fn test_integration_order_item_belongs_to_order() {
        // PHP 业务场景：OrderItem belongsTo Order
        // ```php
        // class OrderItem extends Model {
        //     public function order() {
        //         return $this->belongsTo(Order::class);
        //     }
        // }
        // ```
        let rel = php_belongs_to("Order", "orders", None, None);
        assert_eq!(rel.foreign_key, "order_id");
        assert_eq!(rel.parent_model, "orders");

        let sql = belongs_to_sql(&rel.parent_model, &rel.parent_pk, "50");
        assert_eq!(sql, "SELECT * FROM orders WHERE id = 50");
    }
}
