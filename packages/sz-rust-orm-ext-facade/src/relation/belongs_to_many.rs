//! `BelongsToMany` 关联 — PHP 命名约定 + SQL 片段构造器
//!
//! 本模块对齐 PHP `think\Model::belongsToMany()` 行为，提供：
//!
//! 1. [`default_junction_table`]：PHP 默认中间表名（`current_snake + '_' + related_snake`）
//! 2. [`default_current_fk`]：PHP 默认 localKey（当前模型 FK，对应 sz-orm-core `foreign_key`）
//! 3. [`default_related_fk`]：PHP 默认 foreignKey（关联模型 FK，对应 sz-orm-core `other_key`）
//! 4. [`php_belongs_to_many`]：构造 `BelongsToMany` 配置（应用 PHP 默认值）
//! 5. [`belongs_to_many_sql`]：生成 SQL 片段（用于测试验证）
//!
//! ## PHP 端 `belongsToMany` 签名（think-orm 2.0.x）
//!
//! ```php
//! public function belongsToMany(
//!     string $model,
//!     string $middle = '',
//!     string $foreignKey = '',
//!     string $localKey = ''
//! ): BelongsToMany
//! ```
//!
//! ## PHP 默认值（`RelationShip` trait，think-orm 2.0 第 521-531 行）
//!
//! ```php
//! $model      = $this->parseModel($model);
//! $name       = Str::snake(class_basename($model));
//! $middle     = $middle ?: Str::snake($this->name) . '_' . $name;
//! $foreignKey = $foreignKey ?: $name . '_id';
//! $localKey   = $localKey ?: $this->getForeignKey($this->name);
//! ```
//!
//! - `$middle` 默认：`Str::snake($this->name) . '_' . Str::snake(class_basename($model))`
//!   - **顺序**：当前模型名在前，关联模型名在后（**非字母序**，think-orm 2.0 行为）
//!   - 例如 `User` belongsToMany `Role` → `user_role`
//! - `$foreignKey` 默认：`Str::snake(class_basename($model)) . '_id'`（**关联模型** FK）
//!   - 例如 `Role` → `role_id`（中间表中指向 `Role` 的列）
//! - `$localKey` 默认：`$this->getForeignKey($this->name)` = `Str::snake($this->name) . '_id'`（**当前模型** FK）
//!   - 例如 `User` → `user_id`（中间表中指向 `User` 的列）
//!
//! ## sz-orm-core vs PHP 命名映射（**关键差异**）
//!
//! sz-orm-core 与 PHP think-orm 2.0 的命名**相反**：
//!
//! | sz-orm-core 字段 | PHP 参数       | 含义                                       |
//! |------------------|----------------|--------------------------------------------|
//! | `junction_table` | `$middle`      | 中间表名                                   |
//! | `foreign_key`    | `$localKey`    | 中间表中指向**当前模型**主键的列           |
//! | `other_key`      | `$foreignKey`  | 中间表中指向**目标模型**主键的列           |
//! | `target_model`   | （显式传入）   | 目标表名                                   |
//! | `target_pk`      | （显式传入）   | 目标表主键字段名                           |
//!
//! 此命名反转要求 [`php_belongs_to_many`] 在构造 `BelongsToMany` 时必须正确映射：
//! - `foreign_key` ← PHP `$localKey`（当前模型 FK）
//! - `other_key` ← PHP `$foreignKey`（关联模型 FK）
//!
//! ## 生成的 SQL（与 sz-orm-core::WithRelation::load 一致）
//!
//! ```sql
//! SELECT t.* FROM {target_model} t
//! INNER JOIN {junction_table} j ON t.{target_pk} = j.{other_key}
//! WHERE j.{foreign_key} = {current_pk_value}
//! ```
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行关联加载，而是提供：
//!
//! - **PHP 命名约定辅助函数**：`default_junction_table` / `default_current_fk` / `default_related_fk`
//! - **配置构造器**：`php_belongs_to_many` 返回 sz-orm-core `BelongsToMany` 结构体
//! - **SQL 片段构造器**：`belongs_to_many_sql` 返回 SQL 字符串，用于测试验证
//!
//! 端到端关联加载由 sz-orm-core `WithRelation::load()` 内部实现并测试。

use super::BelongsToMany;
use crate::relation::has_many::{class_to_snake_case, default_foreign_key};

// ============================================================================
// PHP 命名约定辅助函数
// ============================================================================

/// 提取类名最后一段（对齐 PHP `class_basename`）
///
/// PHP `class_basename("app\\model\\User")` → `"User"`
fn class_basename(class: &str) -> &str {
    class.rsplit(['\\', '/']).next().unwrap_or(class)
}

/// PHP `belongsToMany` 默认中间表名
///
/// 对齐 PHP `think-orm 2.0` `RelationShip::belongsToMany` 第 525 行：
///
/// ```php
/// $name   = Str::snake(class_basename($model));
/// $middle = $middle ?: Str::snake($this->name) . '_' . $name;
/// ```
///
/// **顺序**：当前模型名（snake 化）+ `_` + 关联模型名（snake 化）
///
/// **注意**：think-orm 2.0 不使用字母序排序，当前模型在前。
/// 此行为与 think-orm 3.0+（使用 `getPivotTableName` 字母序排序）不同。
///
/// ## 示例
///
/// | 当前模型 | 关联模型 | 输出 |
/// |---------|---------|------|
/// | `"User"` | `"Role"` | `"user_role"` |
/// | `"User"` | `"OrderItem"` | `"user_order_item"` |
/// | `"app\\model\\User"` | `"app\\model\\Role"` | `"user_role"` |
/// | `"Role"` | `"User"` | `"role_user"`（顺序敏感） |
pub fn default_junction_table(current_class: &str, related_class: &str) -> String {
    let current_name = class_basename(current_class);
    let related_name = class_basename(related_class);
    format!(
        "{}_{}",
        class_to_snake_case(current_name),
        class_to_snake_case(related_name)
    )
}

/// PHP `belongsToMany` 默认 localKey（当前模型 FK）
///
/// 对应 sz-orm-core `BelongsToMany.foreign_key`（中间表中指向当前模型主键的列）。
///
/// 对齐 PHP `$localKey = $localKey ?: $this->getForeignKey($this->name)`：
///
/// ```php
/// protected function getForeignKey(string $name): string
/// {
///     if (strpos($name, '\\')) {
///         $name = class_basename($name);
///     }
///     return Str::snake($name) . '_id';
/// }
/// ```
///
/// ## 示例
///
/// | 当前模型 | 输出 |
/// |---------|------|
/// | `"User"` | `"user_id"` |
/// | `"app\\model\\User"` | `"user_id"` |
/// | `"OrderItem"` | `"order_item_id"` |
pub fn default_current_fk(current_class: &str) -> String {
    // 复用 has_many::default_foreign_key（PHP `getForeignKey` 算法相同）
    default_foreign_key(current_class)
}

/// PHP `belongsToMany` 默认 foreignKey（关联模型 FK）
///
/// 对应 sz-orm-core `BelongsToMany.other_key`（中间表中指向目标模型主键的列）。
///
/// 对齐 PHP `$foreignKey = $foreignKey ?: $name . '_id'` 其中
/// `$name = Str::snake(class_basename($model))`：
///
/// ## 示例
///
/// | 关联模型 | 输出 |
/// |---------|------|
/// | `"Role"` | `"role_id"` |
/// | `"app\\model\\Role"` | `"role_id"` |
/// | `"OrderItem"` | `"order_item_id"` |
pub fn default_related_fk(related_class: &str) -> String {
    let related_name = class_basename(related_class);
    format!("{}_id", class_to_snake_case(related_name))
}

// ============================================================================
// BelongsToMany 配置构造器
// ============================================================================

/// 构造 `BelongsToMany` 配置（应用 PHP 默认值）
///
/// 对齐 PHP `think\Model::belongsToMany($model, $middle = '', $foreignKey = '', $localKey = '')`：
///
/// - `junction_table` 默认：`default_junction_table(current_class, related_class)`
/// - `foreign_key`（当前模型 FK）默认：`default_current_fk(current_class)`
/// - `other_key`（关联模型 FK）默认：`default_related_fk(related_class)`
/// - `target_pk` 默认：`"id"`（对齐 PHP `(new $model)->getPk()` 通常为 `id`）
///
/// ## 参数（sz-orm-core 字段名，便于与 `BelongsToMany` 结构体对应）
///
/// - `current_class`：当前模型类名（如 `"User"` 或 `"app\\model\\User"`），用于推导默认 junction_table 与 foreign_key
/// - `related_class`：关联模型类名（如 `"Role"` 或 `"app\\model\\Role"`），用于推导默认 junction_table 与 other_key
/// - `target_table`：目标表名（如 `"roles"`）
/// - `junction_table`：中间表名（`None` 使用默认值）
/// - `foreign_key`：中间表中指向当前模型的 FK（`None` 使用默认值）
/// - `other_key`：中间表中指向关联模型的 FK（`None` 使用默认值）
/// - `target_pk`：目标表主键字段名（`None` 使用 `"id"`）
///
/// ## PHP ↔ sz-orm-core 命名映射
///
/// | PHP 参数       | sz-orm-core 字段   | 本函数参数        |
/// |----------------|--------------------|-------------------|
/// | `$middle`      | `junction_table`   | `junction_table`  |
/// | `$localKey`    | `foreign_key`      | `foreign_key`     |
/// | `$foreignKey`  | `other_key`        | `other_key`       |
///
/// **关键**：PHP `$foreignKey` 对应 sz-orm-core `other_key`（不是 `foreign_key`），
/// PHP `$localKey` 对应 sz-orm-core `foreign_key`（不是 `local_key`）。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::belongs_to_many::php_belongs_to_many;
///
/// // 等价 PHP: $this->belongsToMany(Role::class)
/// // 在 User 模型中定义
/// let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
/// assert_eq!(rel.junction_table, "user_role");
/// assert_eq!(rel.foreign_key, "user_id");    // 当前模型 FK
/// assert_eq!(rel.other_key, "role_id");      // 关联模型 FK
/// assert_eq!(rel.target_model, "roles");
/// assert_eq!(rel.target_pk, "id");
///
/// // 等价 PHP: $this->belongsToMany(Role::class, 'user_role_pivot')
/// let rel = php_belongs_to_many("User", "Role", "roles", Some("user_role_pivot"), None, None, None);
/// assert_eq!(rel.junction_table, "user_role_pivot");
///
/// // 等价 PHP: $this->belongsToMany(Role::class, '', 'rid', 'uid')
/// // PHP $foreignKey='rid' → sz-orm-core other_key='rid'
/// // PHP $localKey='uid'   → sz-orm-core foreign_key='uid'
/// let rel = php_belongs_to_many("User", "Role", "roles", None, Some("uid"), Some("rid"), None);
/// assert_eq!(rel.foreign_key, "uid");
/// assert_eq!(rel.other_key, "rid");
/// ```
pub fn php_belongs_to_many(
    current_class: &str,
    related_class: &str,
    target_table: &str,
    junction_table: Option<&str>,
    foreign_key: Option<&str>,
    other_key: Option<&str>,
    target_pk: Option<&str>,
) -> BelongsToMany {
    BelongsToMany {
        junction_table: junction_table
            .map(String::from)
            .unwrap_or_else(|| default_junction_table(current_class, related_class)),
        foreign_key: foreign_key
            .map(String::from)
            .unwrap_or_else(|| default_current_fk(current_class)),
        other_key: other_key
            .map(String::from)
            .unwrap_or_else(|| default_related_fk(related_class)),
        target_model: target_table.to_string(),
        target_pk: target_pk
            .map(String::from)
            .unwrap_or_else(|| "id".to_string()),
    }
}

// ============================================================================
// SQL 片段构造器（用于测试验证）
// ============================================================================

/// 生成 `BelongsToMany` 关联查询 SQL 片段
///
/// 对齐 sz-orm-core `WithRelation::load()` 中 `BelongsToMany` 分支生成的 SQL：
///
/// ```rust,ignore
/// // JOIN 条件：目标表 t 的主键 = 中间表 j 的 other_key
/// // 过滤条件：中间表 j 的 foreign_key = 当前模型主键
/// let sql = format!(
///     "SELECT t.* FROM {} t INNER JOIN {} j ON t.{} = j.{} WHERE j.{} = {}",
///     config.target_model,
///     config.junction_table,
///     config.target_pk,
///     config.other_key,
///     config.foreign_key,
///     pk_str
/// );
/// ```
///
/// ## 参数
///
/// - `target_table`：目标表名（如 `"roles"`）
/// - `junction_table`：中间表名（如 `"user_role"`）
/// - `target_pk`：目标表主键字段名（如 `"id"`）
/// - `other_key`：中间表中指向目标模型的 FK（如 `"role_id"`）
/// - `foreign_key`：中间表中指向当前模型的 FK（如 `"user_id"`）
/// - `current_pk_value`：当前模型主键值（字符串形式）
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
/// use sz_rust_core::relation::belongs_to_many::belongs_to_many_sql;
///
/// let sql = belongs_to_many_sql("roles", "user_role", "id", "role_id", "user_id", "1");
/// assert_eq!(sql, "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1");
/// ```
pub fn belongs_to_many_sql(
    target_table: &str,
    junction_table: &str,
    target_pk: &str,
    other_key: &str,
    foreign_key: &str,
    current_pk_value: &str,
) -> String {
    format!(
        "SELECT t.* FROM {} t INNER JOIN {} j ON t.{} = j.{} WHERE j.{} = {}",
        target_table, junction_table, target_pk, other_key, foreign_key, current_pk_value
    )
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // 组 1：default_junction_table（PHP belongsToMany 默认中间表对齐）
    // ====================================================================

    #[test]
    fn test_default_junction_table_simple_classes() {
        // PHP: User belongsToMany Role → middle = "user_role"
        // 当前模型名在前，关联模型名在后（非字母序）
        assert_eq!(default_junction_table("User", "Role"), "user_role");
        assert_eq!(default_junction_table("User", "Order"), "user_order");
        assert_eq!(
            default_junction_table("Customer", "Category"),
            "customer_category"
        );
    }

    #[test]
    fn test_default_junction_table_multi_word_classes() {
        // PHP: User belongsToMany OrderItem → middle = "user_order_item"
        assert_eq!(
            default_junction_table("User", "OrderItem"),
            "user_order_item"
        );
        assert_eq!(default_junction_table("OrderItem", "Tag"), "order_item_tag");
    }

    #[test]
    fn test_default_junction_table_with_namespace() {
        // PHP: $this->belongsToMany(\app\model\Role::class)
        // class_basename 提取最后一段
        assert_eq!(
            default_junction_table("app\\model\\User", "app\\model\\Role"),
            "user_role"
        );
        // 支持正斜杠路径
        assert_eq!(
            default_junction_table("app/model/User", "app/model/Role"),
            "user_role"
        );
    }

    #[test]
    fn test_default_junction_table_order_sensitive() {
        // 关键：think-orm 2.0 中间表名顺序敏感（非字母序）
        // User belongsToMany Role → user_role
        // Role belongsToMany User → role_user
        assert_eq!(default_junction_table("User", "Role"), "user_role");
        assert_eq!(default_junction_table("Role", "User"), "role_user");
        assert_ne!(
            default_junction_table("User", "Role"),
            default_junction_table("Role", "User")
        );
    }

    // ====================================================================
    // 组 2：default_current_fk / default_related_fk
    // ====================================================================

    #[test]
    fn test_default_current_fk_simple_class() {
        // PHP: $localKey = $this->getForeignKey($this->name) → user_id
        assert_eq!(default_current_fk("User"), "user_id");
        assert_eq!(default_current_fk("Order"), "order_id");
        assert_eq!(default_current_fk("Customer"), "customer_id");
    }

    #[test]
    fn test_default_current_fk_multi_word_class() {
        // PHP: OrderItem → order_item_id
        assert_eq!(default_current_fk("OrderItem"), "order_item_id");
        assert_eq!(default_current_fk("UserRole"), "user_role_id");
    }

    #[test]
    fn test_default_current_fk_with_namespace() {
        // PHP: getForeignKey 处理命名空间
        assert_eq!(default_current_fk("app\\model\\User"), "user_id");
        assert_eq!(default_current_fk("app\\model\\OrderItem"), "order_item_id");
        assert_eq!(default_current_fk("app/model/User"), "user_id");
    }

    #[test]
    fn test_default_related_fk_simple_class() {
        // PHP: $foreignKey = $name . '_id'（$name = Str::snake(class_basename($model))）
        assert_eq!(default_related_fk("Role"), "role_id");
        assert_eq!(default_related_fk("Order"), "order_id");
        assert_eq!(default_related_fk("Category"), "category_id");
    }

    #[test]
    fn test_default_related_fk_multi_word_class() {
        // PHP: OrderItem → order_item_id
        assert_eq!(default_related_fk("OrderItem"), "order_item_id");
        assert_eq!(default_related_fk("ProductCategory"), "product_category_id");
    }

    #[test]
    fn test_default_related_fk_with_namespace() {
        // PHP: class_basename 处理命名空间
        assert_eq!(default_related_fk("app\\model\\Role"), "role_id");
        assert_eq!(default_related_fk("app\\model\\OrderItem"), "order_item_id");
        assert_eq!(default_related_fk("app/model/Role"), "role_id");
    }

    #[test]
    fn test_default_current_fk_equals_default_foreign_key_algorithm() {
        // default_current_fk 与 has_many::default_foreign_key 算法相同（均调用 getForeignKey）
        assert_eq!(
            default_current_fk("User"),
            crate::relation::has_many::default_foreign_key("User")
        );
        assert_eq!(
            default_current_fk("OrderItem"),
            crate::relation::has_many::default_foreign_key("OrderItem")
        );
    }

    #[test]
    fn test_default_related_fk_differs_from_default_current_fk_semantics() {
        // 算法相同（都基于类名 → snake + _id），但调用方传入的语义不同
        // 在 User belongsToMany Role 场景下：
        //   default_current_fk("User") = "user_id"（当前模型 FK）
        //   default_related_fk("Role") = "role_id"（关联模型 FK）
        assert_eq!(default_current_fk("User"), "user_id");
        assert_eq!(default_related_fk("Role"), "role_id");
        assert_ne!(default_current_fk("User"), default_related_fk("Role"));
    }

    // ====================================================================
    // 组 3：php_belongs_to_many 配置构造器
    // ====================================================================

    #[test]
    fn test_php_belongs_to_many_all_defaults() {
        // PHP: $this->belongsToMany(Role::class) on User
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        assert_eq!(rel.junction_table, "user_role");
        assert_eq!(rel.foreign_key, "user_id"); // 当前模型 FK
        assert_eq!(rel.other_key, "role_id"); // 关联模型 FK
        assert_eq!(rel.target_model, "roles");
        assert_eq!(rel.target_pk, "id");
    }

    #[test]
    fn test_php_belongs_to_many_explicit_junction_table() {
        // PHP: $this->belongsToMany(Role::class, 'user_role_pivot')
        let rel = php_belongs_to_many(
            "User",
            "Role",
            "roles",
            Some("user_role_pivot"),
            None,
            None,
            None,
        );
        assert_eq!(rel.junction_table, "user_role_pivot");
        assert_eq!(rel.foreign_key, "user_id"); // 仍使用默认
        assert_eq!(rel.other_key, "role_id"); // 仍使用默认
    }

    #[test]
    fn test_php_belongs_to_many_explicit_foreign_key() {
        // PHP: $this->belongsToMany(Role::class, '', '', 'uid')
        // PHP $localKey='uid' → sz-orm-core foreign_key='uid'
        let rel = php_belongs_to_many("User", "Role", "roles", None, Some("uid"), None, None);
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.other_key, "role_id"); // 仍使用默认
    }

    #[test]
    fn test_php_belongs_to_many_explicit_other_key() {
        // PHP: $this->belongsToMany(Role::class, '', 'rid')
        // PHP $foreignKey='rid' → sz-orm-core other_key='rid'
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, Some("rid"), None);
        assert_eq!(rel.foreign_key, "user_id"); // 仍使用默认
        assert_eq!(rel.other_key, "rid");
    }

    #[test]
    fn test_php_belongs_to_many_explicit_target_pk() {
        // 显式指定目标表主键
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, Some("rid"));
        assert_eq!(rel.target_pk, "rid");
    }

    #[test]
    fn test_php_belongs_to_many_all_explicit() {
        // PHP: $this->belongsToMany(Role::class, 'pivot', 'rid', 'uid')
        // 映射：$middle='pivot' → junction_table='pivot'
        //       $foreignKey='rid' → other_key='rid'
        //       $localKey='uid'   → foreign_key='uid'
        let rel = php_belongs_to_many(
            "User",
            "Role",
            "roles",
            Some("pivot"),
            Some("uid"),
            Some("rid"),
            Some("pk"),
        );
        assert_eq!(rel.junction_table, "pivot");
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.other_key, "rid");
        assert_eq!(rel.target_model, "roles");
        assert_eq!(rel.target_pk, "pk");
    }

    #[test]
    fn test_php_belongs_to_many_multi_word_classes() {
        // PHP: OrderItem belongsToMany Tag
        let rel = php_belongs_to_many("OrderItem", "Tag", "tags", None, None, None, None);
        assert_eq!(rel.junction_table, "order_item_tag");
        assert_eq!(rel.foreign_key, "order_item_id"); // 当前模型 FK
        assert_eq!(rel.other_key, "tag_id"); // 关联模型 FK
    }

    #[test]
    fn test_php_belongs_to_many_with_namespace() {
        // PHP: $this->belongsToMany(\app\model\Role::class) on \app\model\User
        let rel = php_belongs_to_many(
            "app\\model\\User",
            "app\\model\\Role",
            "roles",
            None,
            None,
            None,
            None,
        );
        assert_eq!(rel.junction_table, "user_role");
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.other_key, "role_id");
    }

    // ====================================================================
    // 组 4：belongs_to_many_sql SQL 片段构造器
    // ====================================================================

    #[test]
    fn test_belongs_to_many_sql_numeric_pk() {
        // 数值型主键
        let sql = belongs_to_many_sql("roles", "user_role", "id", "role_id", "user_id", "1");
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1"
        );
    }

    #[test]
    fn test_belongs_to_many_sql_string_pk() {
        // 字符串型主键（如 UUID）
        // 注意：sz-orm-core WithRelation::load 内部通过 pk_to_sql_string 自动加引号
        // 本函数仅用于测试 SQL 模式，调用方负责转义
        let sql = belongs_to_many_sql(
            "roles",
            "user_role",
            "id",
            "role_id",
            "user_id",
            "'abc-123'",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 'abc-123'"
        );
    }

    #[test]
    fn test_belongs_to_many_sql_custom_junction() {
        // 自定义中间表
        let sql = belongs_to_many_sql("roles", "pivot_table", "id", "role_id", "user_id", "1");
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN pivot_table j ON t.id = j.role_id WHERE j.user_id = 1"
        );
    }

    #[test]
    fn test_belongs_to_many_sql_custom_keys() {
        // 自定义外键（current FK = uid, related FK = rid, target_pk = pk）
        let sql = belongs_to_many_sql("roles", "user_role", "pk", "rid", "uid", "1");
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.pk = j.rid WHERE j.uid = 1"
        );
    }

    #[test]
    fn test_belongs_to_many_sql_multi_word_tables() {
        // 多单词表名
        let sql = belongs_to_many_sql(
            "order_items",
            "order_item_tag",
            "id",
            "tag_id",
            "order_item_id",
            "30",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM order_items t INNER JOIN order_item_tag j ON t.id = j.tag_id WHERE j.order_item_id = 30"
        );
    }

    #[test]
    fn test_belongs_to_many_sql_aligns_sz_orm_core_pattern() {
        // 验证 SQL 模式与 sz-orm-core::WithRelation::load BelongsToMany 分支一致
        // sz-orm-core 源码：
        //   format!(
        //     "SELECT t.* FROM {} t INNER JOIN {} j ON t.{} = j.{} WHERE j.{} = {}",
        //     config.target_model, config.junction_table, config.target_pk,
        //     config.other_key, config.foreign_key, pk_str
        //   );
        let sql = belongs_to_many_sql("roles", "user_role", "id", "role_id", "user_id", "1");
        assert!(sql.starts_with(
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = "
        ));
    }

    // ====================================================================
    // 组 5：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_belongs_to_many_default_junction_table_convention() {
        // R5-1：PHP `belongsToMany(Role::class)` 默认中间表 `user_role`
        // PHP 源码：$middle = $middle ?: Str::snake($this->name) . '_' . $name;
        // 注意：当前模型在前（非字母序，think-orm 2.0 行为）
        assert_eq!(default_junction_table("User", "Role"), "user_role");
        assert_eq!(default_junction_table("Order", "Product"), "order_product");
        assert_eq!(
            default_junction_table("Customer", "Category"),
            "customer_category"
        );
    }

    #[test]
    fn test_r5_php_belongs_to_many_default_local_key_is_current_model_fk() {
        // R5-2：PHP `belongsToMany` 默认 localKey = getForeignKey($this->name)
        // 对应 sz-orm-core::BelongsToMany.foreign_key（中间表中指向当前模型的列）
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        assert_eq!(rel.foreign_key, "user_id"); // 当前模型 FK
        assert_eq!(rel.foreign_key, default_current_fk("User"));
    }

    #[test]
    fn test_r5_php_belongs_to_many_default_foreign_key_is_related_model_fk() {
        // R5-3：PHP `belongsToMany` 默认 foreignKey = $name . '_id'（$name = snake(related)）
        // 对应 sz-orm-core::BelongsToMany.other_key（中间表中指向关联模型的列）
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        assert_eq!(rel.other_key, "role_id"); // 关联模型 FK
        assert_eq!(rel.other_key, default_related_fk("Role"));
    }

    #[test]
    fn test_r5_php_belongs_to_many_naming_mapping_reversed() {
        // R5-4：关键差异 — PHP 与 sz-orm-core 命名反转
        // PHP $foreignKey → sz-orm-core other_key（关联模型 FK）
        // PHP $localKey   → sz-orm-core foreign_key（当前模型 FK）
        // 验证：在 User belongsToMany Role 场景下
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        // sz-orm-core foreign_key 应为 user_id（当前模型 FK，对应 PHP localKey）
        assert_eq!(rel.foreign_key, "user_id");
        // sz-orm-core other_key 应为 role_id（关联模型 FK，对应 PHP foreignKey）
        assert_eq!(rel.other_key, "role_id");
    }

    #[test]
    fn test_r5_php_belongs_to_many_explicit_overrides_default() {
        // R5-5：显式参数覆盖默认值
        let rel = php_belongs_to_many(
            "User",
            "Role",
            "roles",
            Some("pivot"),
            Some("uid"),
            Some("rid"),
            Some("pk"),
        );
        assert_eq!(rel.junction_table, "pivot");
        assert_eq!(rel.foreign_key, "uid");
        assert_eq!(rel.other_key, "rid");
        assert_eq!(rel.target_pk, "pk");
    }

    #[test]
    fn test_r5_php_belongs_to_many_sql_pattern_matches_think_orm() {
        // R5-6：PHP belongsToMany SQL 模式（INNER JOIN 中间表）
        // sz-orm-core::WithRelation::load BelongsToMany 分支生成相同模式
        let sql = belongs_to_many_sql("roles", "user_role", "id", "role_id", "user_id", "1");
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1"
        );
    }

    #[test]
    fn test_r5_php_belongs_to_many_namespace_handling() {
        // R5-7：PHP class_basename + getForeignKey 处理命名空间
        let rel = php_belongs_to_many(
            "app\\model\\User",
            "app\\model\\Role",
            "roles",
            None,
            None,
            None,
            None,
        );
        assert_eq!(rel.junction_table, "user_role");
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.other_key, "role_id");
    }

    #[test]
    fn test_r5_php_belongs_to_many_order_sensitive_junction() {
        // R5-8：think-orm 2.0 中间表名顺序敏感（非字母序）
        // User belongsToMany Role → user_role
        // Role belongsToMany User → role_user
        // 这与 think-orm 3.0+（字母序）行为不同
        let user_to_role = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        let role_to_user = php_belongs_to_many("Role", "User", "users", None, None, None, None);
        assert_eq!(user_to_role.junction_table, "user_role");
        assert_eq!(role_to_user.junction_table, "role_user");
        assert_ne!(user_to_role.junction_table, role_to_user.junction_table);
    }

    #[test]
    fn test_r5_php_belongs_to_many_delegates_to_sz_orm_core() {
        // R5-9：sz-rust 端复用 sz-orm-core::WithRelation::load 进行端到端关联加载
        // sz-rust 端不重新实现关联加载机制，仅提供 PHP 命名约定辅助函数
        // 验证 php_belongs_to_many 返回 sz-orm-core::BelongsToMany 类型
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        // 验证类型为 sz_orm_core::BelongsToMany（编译时类型检查）
        let _: &BelongsToMany = &rel;
        // 验证字段可被 sz-orm-core::Relation::BelongsToMany 包装
        let relation = sz_orm_core::Relation::BelongsToMany(rel.clone());
        assert!(matches!(relation, sz_orm_core::Relation::BelongsToMany(_)));
    }

    // ====================================================================
    // 组 6：集成测试（PHP 业务场景）
    // ====================================================================

    #[test]
    fn test_integration_user_belongs_to_many_roles() {
        // PHP 业务场景：User belongsToMany Role
        // ```php
        // class User extends Model {
        //     public function roles() {
        //         return $this->belongsToMany(Role::class);
        //     }
        // }
        // ```
        let rel = php_belongs_to_many("User", "Role", "roles", None, None, None, None);
        assert_eq!(rel.junction_table, "user_role");
        assert_eq!(rel.foreign_key, "user_id");
        assert_eq!(rel.other_key, "role_id");
        assert_eq!(rel.target_model, "roles");
        assert_eq!(rel.target_pk, "id");

        // 生成 SQL（假设 User.id = 1，查询关联的 Role）
        let sql = belongs_to_many_sql(
            &rel.target_model,
            &rel.junction_table,
            &rel.target_pk,
            &rel.other_key,
            &rel.foreign_key,
            "1",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1"
        );
    }

    #[test]
    fn test_integration_order_belongs_to_many_products() {
        // PHP 业务场景：Order belongsToMany Product
        // ```php
        // class Order extends Model {
        //     public function products() {
        //         return $this->belongsToMany(Product::class);
        //     }
        // }
        // ```
        let rel = php_belongs_to_many("Order", "Product", "products", None, None, None, None);
        assert_eq!(rel.junction_table, "order_product");
        assert_eq!(rel.foreign_key, "order_id");
        assert_eq!(rel.other_key, "product_id");

        let sql = belongs_to_many_sql(
            &rel.target_model,
            &rel.junction_table,
            &rel.target_pk,
            &rel.other_key,
            &rel.foreign_key,
            "100",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM products t INNER JOIN order_product j ON t.id = j.product_id WHERE j.order_id = 100"
        );
    }

    #[test]
    fn test_integration_customer_belongs_to_many_categories() {
        // PHP 业务场景：Customer belongsToMany Category
        let rel = php_belongs_to_many("Customer", "Category", "categories", None, None, None, None);
        assert_eq!(rel.junction_table, "customer_category");
        assert_eq!(rel.foreign_key, "customer_id");
        assert_eq!(rel.other_key, "category_id");

        let sql = belongs_to_many_sql(
            &rel.target_model,
            &rel.junction_table,
            &rel.target_pk,
            &rel.other_key,
            &rel.foreign_key,
            "50",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM categories t INNER JOIN customer_category j ON t.id = j.category_id WHERE j.customer_id = 50"
        );
    }

    #[test]
    fn test_integration_order_item_belongs_to_many_tags() {
        // PHP 业务场景：OrderItem belongsToMany Tag（多单词类名）
        let rel = php_belongs_to_many("OrderItem", "Tag", "tags", None, None, None, None);
        assert_eq!(rel.junction_table, "order_item_tag");
        assert_eq!(rel.foreign_key, "order_item_id");
        assert_eq!(rel.other_key, "tag_id");

        let sql = belongs_to_many_sql(
            &rel.target_model,
            &rel.junction_table,
            &rel.target_pk,
            &rel.other_key,
            &rel.foreign_key,
            "30",
        );
        assert_eq!(
            sql,
            "SELECT t.* FROM tags t INNER JOIN order_item_tag j ON t.id = j.tag_id WHERE j.order_item_id = 30"
        );
    }
}
