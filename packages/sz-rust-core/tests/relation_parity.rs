//! 关联关系一致性测试（PHP 对比）
//!
//! 本文件验证 sz-rust 关联关系模块与 PHP `think\Model` 的行为一致性，覆盖：
//!
//! 1. **4 种基础关联**：HasMany / BelongsTo / HasOne / BelongsToMany
//! 2. **2 种多态关联**：MorphMany / MorphTo
//! 3. **关联预加载**：with() 批量 IN 查询
//! 4. **N+1 检测**：与 PHP `with()` 机制对齐
//!
//! ## 测试组织
//!
//! - 组 1：PHP `Str::snake` 命名转换对齐（`class_to_snake_case`）
//! - 组 2：PHP `getForeignKey` 默认外键对齐（`default_foreign_key` / `default_belongs_to_foreign_key`）
//! - 组 3：PHP `belongsToMany` 默认中间表对齐（`default_junction_table`）
//! - 组 4：HasMany SQL 一致性（`has_many_sql` + `php_has_many`）
//! - 组 5：BelongsTo SQL 一致性（`belongs_to_sql` + `php_belongs_to`）
//! - 组 6：HasOne SQL 一致性（`has_one_sql` + `php_has_one`）
//! - 组 7：BelongsToMany SQL 一致性（`belongs_to_many_sql` + `php_belongs_to_many`）
//! - 组 8：MorphMany SQL 一致性（`morph_many_sql` + `morph_many_in_sql` + `php_morph_many`）
//! - 组 9：MorphTo SQL 一致性（`morph_to_sql` + `php_morph_to` + `group_by_morph_type`）
//! - 组 10：with() 批量预加载 SQL 一致性（4 种关联类型 IN 查询）
//! - 组 11：PHP/Rust 行为对比（端到端场景模拟）
//! - 组 12：N+1 检测与 PHP `with()` 机制对齐
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\think-orm\src\model\concern\RelationShip.php`
//!   - 第 416-424 行：`hasOne($model, $foreignKey = '', $localKey = '')`
//!   - 第 434-444 行：`belongsTo($model, $foreignKey = '', $localKey = '')`
//!   - 第 454-462 行：`hasMany($model, $foreignKey = '', $localKey = '')`
//!   - 第 521-531 行：`belongsToMany($model, $middle = '', $foreignKey = '', $localKey = '')`
//!   - 第 703-710 行：`getForeignKey(string $name): string` → `Str::snake($name) . '_id'`
//!
//! ## R5 PHP 行为对齐验证（硬约束）
//!
//! 本测试文件验证以下 PHP 行为：
//!
//! - R5-1：PHP `Str::snake` 命名转换对齐
//! - R5-2：PHP `getForeignKey` 默认外键推导对齐
//! - R5-3：PHP `hasMany` 默认 `foreignKey` 基于当前模型名（`$this->name`）
//! - R5-4：PHP `belongsTo` 默认 `foreignKey` 基于关联模型名（`get_class($model)`）
//! - R5-5：PHP `belongsToMany` 默认中间表顺序敏感（`snake(a) + '_' + snake(b)`，非字母序）
//! - R5-6：PHP `belongsToMany` 默认 `foreignKey` 是关联模型名 + `_id`（PHP localKey 是关联模型 FK）
//! - R5-7：PHP `belongsToMany` 默认 `localKey` 是当前模型名 + `_id`（PHP foreignKey 是当前模型 FK）
//! - R5-8：PHP `morphMany` 默认 `morphType = morph . '_type'` + `foreignKey = morph . '_id'`
//! - R5-9：PHP `morphTo` 无 `$type` 参数（由子模型数据动态决定）
//! - R5-10：PHP `with()` 批量预加载 IN 查询 SQL 模式对齐
//! - R5-11：PHP N+1 问题（PHP 端不主动检测，sz-rust 端作为扩展提供检测）

use sz_rust_core::relation::{
    belongs_to, belongs_to_many, has_many, has_one, morph, n_plus_one, with,
};

// ============================================================================
// 组 1：PHP `Str::snake` 命名转换对齐
// ============================================================================

#[test]
fn test_parity_str_snake_simple_class() {
    // PHP: Str::snake("User") = "user"
    assert_eq!(has_many::class_to_snake_case("User"), "user");
    assert_eq!(has_many::class_to_snake_case("Order"), "order");
    assert_eq!(has_many::class_to_snake_case("Customer"), "customer");
}

#[test]
fn test_parity_str_snake_multi_word_class() {
    // PHP: Str::snake("OrderItem") = "order_item"
    assert_eq!(has_many::class_to_snake_case("OrderItem"), "order_item");
    assert_eq!(has_many::class_to_snake_case("UserRole"), "user_role");
    assert_eq!(
        has_many::class_to_snake_case("ContractDetail"),
        "contract_detail"
    );
}

#[test]
fn test_parity_str_snake_all_lowercase() {
    // PHP: Str::snake("user") = "user"（全小写原样返回）
    assert_eq!(has_many::class_to_snake_case("user"), "user");
    assert_eq!(has_many::class_to_snake_case("order_item"), "order_item");
}

#[test]
fn test_parity_str_snake_all_uppercase() {
    // PHP: Str::snake("URL") = "u_r_l"（极端情况，每个大写字母前插入下划线）
    assert_eq!(has_many::class_to_snake_case("URL"), "u_r_l");
}

// ============================================================================
// 组 2：PHP `getForeignKey` 默认外键对齐
// ============================================================================

#[test]
fn test_parity_has_many_default_foreign_key() {
    // PHP hasMany: foreignKey = Str::snake($this->name) . '_id'
    // User hasMany Order → foreignKey = "user_id"（基于当前模型 User）
    assert_eq!(has_many::default_foreign_key("User"), "user_id");
    assert_eq!(has_many::default_foreign_key("Order"), "order_id");
    assert_eq!(has_many::default_foreign_key("OrderItem"), "order_item_id");
}

#[test]
fn test_parity_belongs_to_default_foreign_key() {
    // PHP belongsTo: foreignKey = Str::snake(get_class($model)) . '_id'
    // Profile belongsTo User → foreignKey = "user_id"（基于关联模型 User）
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("User"),
        "user_id"
    );
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("Order"),
        "order_id"
    );
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("OrderItem"),
        "order_item_id"
    );
}

#[test]
fn test_parity_belongs_to_default_fk_with_namespace() {
    // PHP getForeignKey 处理命名空间：strpos($name, '\\') → class_basename
    // e.g., "app\model\User" → "User" → "user_id"
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("app\\model\\User"),
        "user_id"
    );
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("app/model/User"),
        "user_id"
    );
}

#[test]
fn test_parity_has_many_vs_belongs_to_default_fk_semantics() {
    // 关键差异：hasMany 基于当前模型名，belongsTo 基于关联模型名
    // 但算法相同（都调用 getForeignKey），区别在调用方传入的参数语义
    //
    // 场景：User hasMany Order（User 是父模型，Order 是子模型）
    // - hasMany 调用方传入 User（当前模型）→ foreignKey = "user_id"
    //
    // 场景：Order belongsTo User（Order 是子模型，User 是父模型）
    // - belongsTo 调用方传入 User（关联模型）→ foreignKey = "user_id"
    //
    // 两种场景下 foreignKey 都是 "user_id"，但语义不同
    assert_eq!(
        has_many::default_foreign_key("User"),
        belongs_to::default_belongs_to_foreign_key("User")
    );
}

// ============================================================================
// 组 3：PHP `belongsToMany` 默认中间表对齐
// ============================================================================

#[test]
fn test_parity_default_junction_table_order_sensitive() {
    // PHP belongsToMany: middle = Str::snake($this->name) . '_' . Str::snake(class_basename($model))
    // **顺序敏感**：当前模型在前，关联模型在后
    //
    // User belongsToMany Role → "user_role"（User 在前）
    // Role belongsToMany User → "role_user"（Role 在前）
    assert_eq!(
        belongs_to_many::default_junction_table("User", "Role"),
        "user_role"
    );
    assert_eq!(
        belongs_to_many::default_junction_table("Role", "User"),
        "role_user"
    );
}

#[test]
fn test_parity_default_junction_table_multi_word() {
    // 多单词类名
    assert_eq!(
        belongs_to_many::default_junction_table("OrderItem", "Tag"),
        "order_item_tag"
    );
}

#[test]
fn test_parity_belongs_to_many_default_fk_semantics() {
    // PHP belongsToMany:
    // - foreignKey = $name . '_id'（关联模型名 + _id，对应 sz-orm-core other_key）
    // - localKey = $this->getForeignKey($this->name)（当前模型名 + _id，对应 sz-orm-core foreign_key）
    //
    // User belongsToMany Role:
    // - foreignKey = "role_id"（关联模型 Role）
    // - localKey = "user_id"（当前模型 User）
    assert_eq!(belongs_to_many::default_related_fk("Role"), "role_id");
    assert_eq!(belongs_to_many::default_current_fk("User"), "user_id");
}

// ============================================================================
// 组 4：HasMany SQL 一致性
// ============================================================================

#[test]
fn test_parity_has_many_sql_pattern() {
    // PHP HasMany 生成的 SQL: SELECT * FROM {child} WHERE {fk} = {parent_pk}
    let sql = has_many::has_many_sql("orders", "user_id", "1");
    assert_eq!(sql, "SELECT * FROM orders WHERE user_id = 1");
}

#[test]
fn test_parity_php_has_many_default_config() {
    // PHP: $this->hasMany(Order::class) on User
    // - foreignKey 默认 "user_id"（基于当前模型 User）
    // - localKey 默认 "id"（$this->getPk()）
    let rel = has_many::php_has_many("User", "orders", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.child_pk, "id");
    assert_eq!(rel.child_model, "orders");
}

#[test]
fn test_parity_php_has_many_explicit_config() {
    // PHP: $this->hasMany(Order::class, 'uid', 'pk')
    let rel = has_many::php_has_many("User", "orders", Some("uid"), Some("pk"));
    assert_eq!(rel.foreign_key, "uid");
    assert_eq!(rel.child_pk, "pk");
}

// ============================================================================
// 组 5：BelongsTo SQL 一致性
// ============================================================================

#[test]
fn test_parity_belongs_to_sql_pattern() {
    // PHP BelongsTo 生成的 SQL: SELECT * FROM {parent} WHERE {parent_pk} = {fk_value}
    let sql = belongs_to::belongs_to_sql("users", "id", "1");
    assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
}

#[test]
fn test_parity_php_belongs_to_default_config() {
    // PHP: $this->belongsTo(User::class) on Profile
    // - foreignKey 默认 "user_id"（基于关联模型 User）
    // - parent_pk 默认 "id"（$related->getPk()）
    let rel = belongs_to::php_belongs_to("User", "users", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.parent_pk, "id");
    assert_eq!(rel.parent_model, "users");
}

#[test]
fn test_parity_php_belongs_to_explicit_config() {
    // PHP: $this->belongsTo(User::class, 'uid', 'pk')
    let rel = belongs_to::php_belongs_to("User", "users", Some("uid"), Some("pk"));
    assert_eq!(rel.foreign_key, "uid");
    assert_eq!(rel.parent_pk, "pk");
}

// ============================================================================
// 组 6：HasOne SQL 一致性
// ============================================================================

#[test]
fn test_parity_has_one_sql_pattern() {
    // PHP HasOne 生成的 SQL: SELECT * FROM {child} WHERE {fk} = {parent_pk}
    // 与 HasMany SQL 模式相同，区别仅在返回语义（HasOne 取第一行）
    let sql = has_one::has_one_sql("profiles", "user_id", "1");
    assert_eq!(sql, "SELECT * FROM profiles WHERE user_id = 1");
}

#[test]
fn test_parity_php_has_one_default_config() {
    // PHP: $this->hasOne(Profile::class) on User
    // - foreignKey 默认 "user_id"（基于当前模型 User，与 hasMany 算法相同）
    // - localKey 默认 "id"
    let rel = has_one::php_has_one("User", "profiles", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.child_pk, "id");
    assert_eq!(rel.child_model, "profiles");
}

// ============================================================================
// 组 7：BelongsToMany SQL 一致性
// ============================================================================

#[test]
fn test_parity_belongs_to_many_sql_pattern() {
    // PHP BelongsToMany 生成的 SQL:
    // SELECT t.* FROM {target} t INNER JOIN {junction} j ON t.{target_pk} = j.{other_key}
    // WHERE j.{foreign_key} = {current_pk}
    let sql = belongs_to_many::belongs_to_many_sql(
        "roles",     // target_table
        "user_role", // junction_table
        "id",        // target_pk
        "role_id",   // other_key (PHP foreignKey, 关联模型 FK)
        "user_id",   // foreign_key (PHP localKey, 当前模型 FK)
        "1",         // current_pk_value
    );
    assert_eq!(
        sql,
        "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1"
    );
}

#[test]
fn test_parity_php_belongs_to_many_default_config() {
    // PHP: $this->belongsToMany(Role::class) on User
    // - junction_table 默认 "user_role"（顺序敏感）
    // - foreign_key 默认 "user_id"（当前模型 FK，sz-orm-core foreign_key）
    // - other_key 默认 "role_id"（关联模型 FK，sz-orm-core other_key）
    // - target_pk 默认 "id"
    let rel = belongs_to_many::php_belongs_to_many(
        "User", "Role", "roles", None, // junction_table
        None, // foreign_key (current FK)
        None, // other_key (related FK)
        None, // target_pk
    );
    assert_eq!(rel.junction_table, "user_role");
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.other_key, "role_id");
    assert_eq!(rel.target_pk, "id");
    assert_eq!(rel.target_model, "roles");
}

// ============================================================================
// 组 8：MorphMany SQL 一致性
// ============================================================================

#[test]
fn test_parity_morph_many_sql_pattern() {
    // PHP MorphMany 生成的 SQL:
    // SELECT * FROM {child} WHERE {morph_type_col} = '{morph_type_val}' AND {morph_id_col} = {parent_pk}
    let sql = morph::morph_many_sql(
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
fn test_parity_morph_many_in_sql_pattern() {
    // PHP MorphMany::eagerlyResultSet 批量 IN 查询:
    // SELECT * FROM {child} WHERE {morph_id_col} IN ({v1}, {v2}, ...) AND {morph_type_col} = '{morph_type_val}'
    // WHERE 条件顺序：morphKey IN (range) 在前，morphType = type 在后
    let sql = morph::morph_many_in_sql(
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
fn test_parity_php_morph_many_default_config() {
    // PHP: $this->morphMany(Comment::class, 'commentable') on Post
    // - morph_type_column 默认 "commentable_type"
    // - morph_id_column 默认 "commentable_id"
    // - morph_type_value 默认 "Post"（get_class($this)）
    let rel = morph::php_morph_many("Post", "comments", "commentable", None, None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");
    assert_eq!(rel.morph_type_value, "Post");
    assert_eq!(rel.child_model, "comments");
}

#[test]
fn test_parity_morph_default_column_names() {
    // PHP: morphType = morph . '_type', morphId = morph . '_id'
    assert_eq!(
        morph::default_morph_type_column("commentable"),
        "commentable_type"
    );
    assert_eq!(
        morph::default_morph_id_column("commentable"),
        "commentable_id"
    );
    assert_eq!(
        morph::default_morph_type_column("imageable"),
        "imageable_type"
    );
    assert_eq!(morph::default_morph_id_column("imageable"), "imageable_id");
}

// ============================================================================
// 组 9：MorphTo SQL 一致性
// ============================================================================

#[test]
fn test_parity_morph_to_sql_pattern() {
    // PHP MorphTo 生成的 SQL:
    // SELECT * FROM {parent_table} WHERE {parent_pk} = {morph_id_value}
    let sql = morph::morph_to_sql("posts", "id", "1");
    assert_eq!(sql, "SELECT * FROM posts WHERE id = 1");
}

#[test]
fn test_parity_php_morph_to_default_config() {
    // PHP: $this->morphTo('commentable') on Comment
    // - morph_type_column 默认 "commentable_type"
    // - morph_id_column 默认 "commentable_id"
    // - 无 morph_type_value（由子模型数据动态决定）
    let rel = morph::php_morph_to("commentable", None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");
}

#[test]
fn test_parity_group_by_morph_type() {
    // PHP MorphTo::eagerlyResultSet 按 morph_type 分组:
    // $range[$result->$morphType][] = $result->$morphKey
    use serde_json::json;
    let rows = vec![
        json!({"commentable_type": "Post", "commentable_id": 1}),
        json!({"commentable_type": "Post", "commentable_id": 2}),
        json!({"commentable_type": "Video", "commentable_id": 1}),
        json!({"commentable_type": "Post", "commentable_id": 3}),
    ];
    let grouped = morph::group_by_morph_type(&rows, "commentable_type", "commentable_id");
    assert_eq!(grouped.get("Post").unwrap().len(), 3);
    assert_eq!(grouped.get("Video").unwrap().len(), 1);
    assert_eq!(
        grouped.get("Post").unwrap(),
        &vec!["1".to_string(), "2".to_string(), "3".to_string()]
    );
    assert_eq!(grouped.get("Video").unwrap(), &vec!["1".to_string()]);
}

// ============================================================================
// 组 10：with() 批量预加载 SQL 一致性
// ============================================================================

#[test]
fn test_parity_has_many_in_sql_pattern() {
    // PHP HasMany::eagerlyResultSet 批量 IN 查询:
    // SELECT * FROM {child} WHERE {fk} IN ({v1}, {v2}, ...)
    // 注：批量 IN 查询由 with 模块统一实现（对齐 PHP eagerlyResultSet）
    let sql = with::has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
    assert_eq!(sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");
}

#[test]
fn test_parity_has_one_in_sql_pattern() {
    // PHP HasOne::eagerlyResultSet 批量 IN 查询（与 HasMany 相同 SQL 模式）
    let sql = with::has_one_in_sql("profiles", "user_id", &["1", "2"]);
    assert_eq!(sql, "SELECT * FROM profiles WHERE user_id IN (1, 2)");
}

#[test]
fn test_parity_belongs_to_in_sql_pattern() {
    // PHP BelongsTo::eagerlyResultSet 批量 IN 查询:
    // SELECT * FROM {parent} WHERE {parent_pk} IN ({v1}, {v2}, ...)
    let sql = with::belongs_to_in_sql("users", "id", &["1", "2", "3"]);
    assert_eq!(sql, "SELECT * FROM users WHERE id IN (1, 2, 3)");
}

#[test]
fn test_parity_belongs_to_many_in_sql_pattern() {
    // PHP BelongsToMany::eagerlyResultSet 批量 IN 查询:
    // SELECT t.* FROM {target} t INNER JOIN {junction} j ON t.{target_pk} = j.{other_key}
    // WHERE j.{foreign_key} IN ({v1}, {v2}, ...)
    let sql = with::belongs_to_many_in_sql(
        "roles",
        "user_role",
        "id",
        "role_id",
        "user_id",
        &["1", "2"],
    );
    assert_eq!(
        sql,
        "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2)"
    );
}

#[test]
fn test_parity_with_notation_parse() {
    // PHP RelationShip::eagerlyResultSet 第 266-270 行:
    // if (strpos($relation, '.')) {
    //     [$relation, $subRelation] = explode('.', $relation, 2);
    // }
    assert_eq!(with::parse_with_notation("category"), ("category", None));
    assert_eq!(
        with::parse_with_notation("items.product"),
        ("items", Some("product"))
    );
    // 仅按第一个 . 分割
    assert_eq!(with::parse_with_notation("a.b.c"), ("a", Some("b.c")));
}

// ============================================================================
// 组 11：PHP/Rust 行为对比（端到端场景模拟）
// ============================================================================

#[test]
fn test_parity_scenario_user_has_many_orders() {
    // 场景：User hasMany Order
    // PHP: class User extends Model { public function orders() { return $this->hasMany(Order::class); } }
    //
    // 默认值推导：
    // - foreignKey = "user_id"（基于当前模型 User）
    // - localKey = "id"（$this->getPk()）
    //
    // 生成的 SQL（单条加载）:
    // SELECT * FROM orders WHERE user_id = {user.id}
    //
    // 生成的 SQL（批量预加载）:
    // SELECT * FROM orders WHERE user_id IN ({user1.id}, {user2.id}, ...)

    // 1. 配置对齐
    let rel = has_many::php_has_many("User", "orders", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.child_pk, "id");

    // 2. 单条 SQL 对齐
    let single_sql = has_many::has_many_sql("orders", "user_id", "1");
    assert_eq!(single_sql, "SELECT * FROM orders WHERE user_id = 1");

    // 3. 批量 IN SQL 对齐
    let batch_sql = with::has_many_in_sql("orders", "user_id", &["1", "2", "3"]);
    assert_eq!(batch_sql, "SELECT * FROM orders WHERE user_id IN (1, 2, 3)");
}

#[test]
fn test_parity_scenario_order_belongs_to_user() {
    // 场景：Order belongsTo User
    // PHP: class Order extends Model { public function user() { return $this->belongsTo(User::class); } }
    //
    // 默认值推导：
    // - foreignKey = "user_id"（基于关联模型 User）
    // - localKey = "id"（$related->getPk()）
    //
    // 生成的 SQL（单条加载）:
    // SELECT * FROM users WHERE id = {order.user_id}
    //
    // 生成的 SQL（批量预加载）:
    // SELECT * FROM users WHERE id IN ({order1.user_id}, {order2.user_id}, ...)

    // 1. 配置对齐
    let rel = belongs_to::php_belongs_to("User", "users", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.parent_pk, "id");

    // 2. 单条 SQL 对齐
    let single_sql = belongs_to::belongs_to_sql("users", "id", "1");
    assert_eq!(single_sql, "SELECT * FROM users WHERE id = 1");

    // 3. 批量 IN SQL 对齐
    let batch_sql = with::belongs_to_in_sql("users", "id", &["1", "2", "3"]);
    assert_eq!(batch_sql, "SELECT * FROM users WHERE id IN (1, 2, 3)");
}

#[test]
fn test_parity_scenario_user_has_one_profile() {
    // 场景：User hasOne Profile
    // PHP: class User extends Model { public function profile() { return $this->hasOne(Profile::class); } }
    //
    // 默认值推导：
    // - foreignKey = "user_id"（基于当前模型 User，与 hasMany 算法相同）
    // - localKey = "id"
    //
    // 生成的 SQL（单条加载）:
    // SELECT * FROM profiles WHERE user_id = {user.id}

    // 1. 配置对齐
    let rel = has_one::php_has_one("User", "profiles", None, None);
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.child_pk, "id");

    // 2. 单条 SQL 对齐
    let single_sql = has_one::has_one_sql("profiles", "user_id", "1");
    assert_eq!(single_sql, "SELECT * FROM profiles WHERE user_id = 1");

    // 3. 批量 IN SQL 对齐（与 HasMany 相同 SQL 模式）
    let batch_sql = with::has_one_in_sql("profiles", "user_id", &["1", "2"]);
    assert_eq!(batch_sql, "SELECT * FROM profiles WHERE user_id IN (1, 2)");
}

#[test]
fn test_parity_scenario_user_belongs_to_many_roles() {
    // 场景：User belongsToMany Role
    // PHP: class User extends Model { public function roles() { return $this->belongsToMany(Role::class); } }
    //
    // 默认值推导：
    // - junction_table = "user_role"（顺序敏感：User 在前 Role 在后）
    // - foreign_key = "user_id"（当前模型 FK，sz-orm-core foreign_key）
    // - other_key = "role_id"（关联模型 FK，sz-orm-core other_key）
    // - target_pk = "id"
    //
    // 生成的 SQL（单条加载）:
    // SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = {user.id}
    //
    // 生成的 SQL（批量预加载）:
    // SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN ({user1.id}, ...)

    // 1. 配置对齐
    let rel = belongs_to_many::php_belongs_to_many("User", "Role", "roles", None, None, None, None);
    assert_eq!(rel.junction_table, "user_role");
    assert_eq!(rel.foreign_key, "user_id");
    assert_eq!(rel.other_key, "role_id");
    assert_eq!(rel.target_pk, "id");

    // 2. 单条 SQL 对齐
    let single_sql =
        belongs_to_many::belongs_to_many_sql("roles", "user_role", "id", "role_id", "user_id", "1");
    assert_eq!(
        single_sql,
        "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id = 1"
    );

    // 3. 批量 IN SQL 对齐
    let batch_sql = with::belongs_to_many_in_sql(
        "roles",
        "user_role",
        "id",
        "role_id",
        "user_id",
        &["1", "2"],
    );
    assert_eq!(
        batch_sql,
        "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2)"
    );
}

#[test]
fn test_parity_scenario_post_morph_many_comments() {
    // 场景：Post morphMany Comment
    // PHP: class Post extends Model { public function comments() { return $this->morphMany(Comment::class, 'commentable'); } }
    //
    // 默认值推导：
    // - morph_type_column = "commentable_type"
    // - morph_id_column = "commentable_id"
    // - morph_type_value = "Post"（get_class($this)）
    //
    // 生成的 SQL（单条加载）:
    // SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = {post.id}
    //
    // 生成的 SQL（批量预加载）:
    // SELECT * FROM comments WHERE commentable_id IN ({post1.id}, ...) AND commentable_type = 'Post'

    // 1. 配置对齐
    let rel = morph::php_morph_many("Post", "comments", "commentable", None, None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");
    assert_eq!(rel.morph_type_value, "Post");

    // 2. 单条 SQL 对齐
    let single_sql = morph::morph_many_sql(
        "comments",
        "commentable_type",
        "Post",
        "commentable_id",
        "1",
    );
    assert_eq!(
        single_sql,
        "SELECT * FROM comments WHERE commentable_type = 'Post' AND commentable_id = 1"
    );

    // 3. 批量 IN SQL 对齐（WHERE 条件顺序：morph_id IN 在前，morph_type = 在后）
    let batch_sql = morph::morph_many_in_sql(
        "comments",
        "commentable_type",
        "Post",
        "commentable_id",
        &["1", "2", "3"],
    );
    assert_eq!(
        batch_sql,
        "SELECT * FROM comments WHERE commentable_id IN (1, 2, 3) AND commentable_type = 'Post'"
    );
}

#[test]
fn test_parity_scenario_comment_morph_to_post_or_video() {
    // 场景：Comment morphTo Post/Video
    // PHP: class Comment extends Model { public function commentable() { return $this->morphTo('commentable'); } }
    //
    // 默认值推导：
    // - morph_type_column = "commentable_type"
    // - morph_id_column = "commentable_id"
    // - 无 morph_type_value（由子模型数据动态决定）
    //
    // 生成的 SQL（按 morph_type 分组查询）:
    // - 对于 morph_type = "Post": SELECT * FROM posts WHERE id IN ({comment1.commentable_id}, ...)
    // - 对于 morph_type = "Video": SELECT * FROM videos WHERE id IN ({comment2.commentable_id}, ...)

    // 1. 配置对齐
    let rel = morph::php_morph_to("commentable", None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");

    // 2. 分组逻辑对齐
    use serde_json::json;
    let comments = vec![
        json!({"commentable_type": "Post", "commentable_id": 1}),
        json!({"commentable_type": "Video", "commentable_id": 1}),
        json!({"commentable_type": "Post", "commentable_id": 2}),
    ];
    let grouped = morph::group_by_morph_type(&comments, "commentable_type", "commentable_id");
    assert_eq!(
        grouped.get("Post").unwrap(),
        &vec!["1".to_string(), "2".to_string()]
    );
    assert_eq!(grouped.get("Video").unwrap(), &vec!["1".to_string()]);

    // 3. 每组生成 IN 查询 SQL
    let post_sql = morph::morph_to_sql("posts", "id", "1");
    let video_sql = morph::morph_to_sql("videos", "id", "1");
    assert_eq!(post_sql, "SELECT * FROM posts WHERE id = 1");
    assert_eq!(video_sql, "SELECT * FROM videos WHERE id = 1");
}

// ============================================================================
// 组 12：N+1 检测与 PHP `with()` 机制对齐
// ============================================================================

#[test]
fn test_parity_n_plus_one_pattern_detection() {
    // PHP N+1 模式：
    //   $users = User::select();  // 1 次
    //   foreach ($users as $user) {
    //       $orders = $user->orders;  // N 次
    //   }
    //
    // sz-rust N+1 检测：6 次相同模板查询触发告警
    let mut records = vec![n_plus_one::SqlQueryRecord::new(
        "SELECT * FROM users",
        "users",
        0,
        0,
    )];
    for i in 1..=6u64 {
        records.push(n_plus_one::SqlQueryRecord::new(
            &format!("SELECT * FROM orders WHERE user_id = {}", i),
            "orders",
            i * 100,
            i,
        ));
    }
    let config = n_plus_one::DetectionConfig::new(5, 1000);
    let alerts = n_plus_one::detect_n_plus_one(&records, &config);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].table, "orders");
    assert_eq!(alerts[0].query_count, 6);
}

#[test]
fn test_parity_with_avoids_n_plus_one() {
    // PHP with() 批量预加载避免 N+1：
    //   $users = User::with('orders')->select();
    //   // 内部通过 eagerlyResultSet() 批量 IN 查询（2 次查询）
    //
    // sz-rust N+1 检测：2 次查询（1 主表 + 1 IN 查询）不触发告警
    let records = vec![
        n_plus_one::SqlQueryRecord::new("SELECT * FROM users", "users", 0, 0),
        n_plus_one::SqlQueryRecord::new(
            "SELECT * FROM orders WHERE user_id IN (1, 2, 3, 4, 5, 6)",
            "orders",
            100,
            1,
        ),
    ];
    let config = n_plus_one::DetectionConfig::new(5, 1000);
    let alerts = n_plus_one::detect_n_plus_one(&records, &config);
    assert_eq!(alerts.len(), 0); // 不触发告警
}

#[test]
fn test_parity_suggest_with_usage_format() {
    // sz-rust suggest_with_usage 生成 PHP with() 使用建议
    let suggestion = n_plus_one::suggest_with_usage("orders", 6);
    assert!(suggestion.contains("with('orders')"));
    assert!(suggestion.contains("6 queries"));
    assert!(suggestion.contains("N+1 problem"));
}

#[test]
fn test_parity_n_plus_one_detector_integration() {
    // sz-rust NPlusOneDetector 集成场景：累积记录 + 批量分析
    let mut detector = n_plus_one::NPlusOneDetector::default();
    // 模拟 N+1 模式
    detector.record("SELECT * FROM users", "users", 0);
    for i in 1..=6u64 {
        detector.record(
            &format!("SELECT * FROM orders WHERE user_id = {}", i),
            "orders",
            i * 100,
        );
    }
    let alerts = detector.detect();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].table, "orders");
    assert_eq!(alerts[0].query_count, 6);
}

#[test]
fn test_parity_n_plus_one_different_query_no_alert() {
    // PHP 不同查询模板不构成 N+1
    let records = vec![
        n_plus_one::SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
        n_plus_one::SqlQueryRecord::new(
            "SELECT * FROM orders WHERE user_id = 2 AND status = 1",
            "orders",
            200,
            1,
        ),
        n_plus_one::SqlQueryRecord::new(
            "SELECT * FROM orders WHERE user_id = 3 AND status = 2",
            "orders",
            300,
            2,
        ),
    ];
    let config = n_plus_one::DetectionConfig::new(3, 1000);
    let alerts = n_plus_one::detect_n_plus_one(&records, &config);
    assert_eq!(alerts.len(), 0); // 不同模板不构成 N+1
}

// ============================================================================
// 组 13：结果集字段顺序一致性（PHP/Rust 行为对比）
// ============================================================================

#[test]
fn test_parity_collect_pk_values_field_order() {
    // PHP eagerlyResultSet 第 1 步：收集 $range
    //   $range = [];
    //   foreach ($resultSet as $result) {
    //       if (isset($result->$localKey)) {
    //           $range[] = $result->$localKey;
    //       }
    //   }
    //
    // sz-rust collect_pk_values 保持结果集顺序
    use serde_json::json;
    let rows = vec![
        json!({"id": 3, "name": "Charlie"}),
        json!({"id": 1, "name": "Alice"}),
        json!({"id": 2, "name": "Bob"}),
    ];
    let pks = with::collect_pk_values(&rows, "id");
    assert_eq!(pks, vec!["3".to_string(), "1".to_string(), "2".to_string()]);
}

#[test]
fn test_parity_group_by_fk_preserves_insertion_order() {
    // PHP eagerlyResultSet 第 3 步：按 $data[$pk] 分桶回填
    //   foreach ($resultSet as $result) {
    //       $pk = $result->$localKey;
    //       if (!isset($data[$pk])) { $data[$pk] = []; }
    //       $result->setRelation($relation, $this->resultSetBuild($data[$pk], clone $this->parent));
    //   }
    //
    // sz-rust group_by_fk 按外键分桶
    use serde_json::json;
    let rows = vec![
        json!({"id": 101, "user_id": 1, "name": "Order A"}),
        json!({"id": 102, "user_id": 2, "name": "Order B"}),
        json!({"id": 103, "user_id": 1, "name": "Order C"}),
    ];
    let grouped = with::group_by_fk(rows, "user_id");
    assert_eq!(grouped.get("1").unwrap().len(), 2);
    assert_eq!(grouped.get("2").unwrap().len(), 1);
    // 验证分桶内顺序保持插入顺序
    assert_eq!(grouped.get("1").unwrap()[0]["id"], 101);
    assert_eq!(grouped.get("1").unwrap()[1]["id"], 103);
}

#[test]
fn test_parity_sanitize_pk_value_sql_injection_prevention() {
    // sz-rust sanitize_pk_value 对齐 SQL 标准转义
    assert_eq!(with::sanitize_pk_value("1"), "1"); // 数值型原样返回
    assert_eq!(with::sanitize_pk_value("abc"), "'abc'"); // 字符串型加引号
    assert_eq!(with::sanitize_pk_value("a'b"), "'a''b'"); // 单引号转义为 ''
}

// ============================================================================
// 组 14：PHP 关键行为差异验证（R5 硬约束）
// ============================================================================

#[test]
fn test_r5_parity_has_many_vs_belongs_to_caller_semantics() {
    // R5-3/R5-4：PHP hasMany 默认 foreignKey 基于当前模型名，belongsTo 默认 foreignKey 基于关联模型名
    //
    // 场景 1：User hasMany Order（User 是父模型）
    // - hasMany 调用方传入 User（当前模型）→ foreignKey = "user_id"
    //
    // 场景 2：Order belongsTo User（User 是父模型）
    // - belongsTo 调用方传入 User（关联模型）→ foreignKey = "user_id"
    //
    // 两种场景下 foreignKey 都是 "user_id"，但调用方传入的参数语义不同
    let has_many_fk = has_many::default_foreign_key("User");
    let belongs_to_fk = belongs_to::default_belongs_to_foreign_key("User");
    assert_eq!(has_many_fk, belongs_to_fk); // 算法相同
    assert_eq!(has_many_fk, "user_id");
}

#[test]
fn test_r5_parity_belongs_to_many_junction_table_order_sensitive() {
    // R5-5：PHP belongsToMany 默认中间表顺序敏感（非字母序）
    // think-orm 2.0.x: Str::snake($this->name) . '_' . Str::snake(class_basename($model))
    // 当前模型在前，关联模型在后
    //
    // User belongsToMany Role → "user_role"（User 在前）
    // Role belongsToMany User → "role_user"（Role 在前）
    // 两者不同！
    let user_role = belongs_to_many::default_junction_table("User", "Role");
    let role_user = belongs_to_many::default_junction_table("Role", "User");
    assert_eq!(user_role, "user_role");
    assert_eq!(role_user, "role_user");
    assert_ne!(user_role, role_user); // 顺序敏感
}

#[test]
fn test_r5_parity_belongs_to_many_fk_naming_inversion() {
    // R5-6/R5-7：PHP↔sz-orm-core 命名反转
    //
    // PHP belongsToMany:
    // - foreignKey = $name . '_id'（关联模型名 + _id）→ sz-orm-core other_key
    // - localKey = $this->getForeignKey($this->name)（当前模型名 + _id）→ sz-orm-core foreign_key
    //
    // User belongsToMany Role:
    // - PHP foreignKey = "role_id" → sz-orm-core other_key = "role_id"
    // - PHP localKey = "user_id" → sz-orm-core foreign_key = "user_id"
    let rel = belongs_to_many::php_belongs_to_many("User", "Role", "roles", None, None, None, None);
    // sz-orm-core 字段命名
    assert_eq!(rel.foreign_key, "user_id"); // 当前模型 FK
    assert_eq!(rel.other_key, "role_id"); // 关联模型 FK
                                          // PHP 命名（反转）
    assert_eq!(belongs_to_many::default_current_fk("User"), rel.foreign_key);
    assert_eq!(belongs_to_many::default_related_fk("Role"), rel.other_key);
}

#[test]
fn test_r5_parity_morph_many_default_value_derivation() {
    // R5-8：PHP morphMany 默认 morphType = morph . '_type', foreignKey = morph . '_id'
    //
    // PHP: $this->morphMany(Comment::class, 'commentable') on Post
    // - morphType = "commentable_type"
    // - foreignKey = "commentable_id"
    // - type = "Post"（get_class($this)）
    assert_eq!(
        morph::default_morph_type_column("commentable"),
        "commentable_type"
    );
    assert_eq!(
        morph::default_morph_id_column("commentable"),
        "commentable_id"
    );

    let rel = morph::php_morph_many("Post", "comments", "commentable", None, None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");
    assert_eq!(rel.morph_type_value, "Post"); // get_class($this)
}

#[test]
fn test_r5_parity_morph_to_no_type_parameter() {
    // R5-9：PHP morphTo 无 $type 参数（由子模型数据动态决定）
    //
    // PHP: $this->morphTo('commentable') on Comment
    // - morphType = "commentable_type"
    // - morphId = "commentable_id"
    // - 无 type 参数（运行时根据 morph_type 列值路由到不同父表）
    let rel = morph::php_morph_to("commentable", None, None);
    assert_eq!(rel.morph_type_column, "commentable_type");
    assert_eq!(rel.morph_id_column, "commentable_id");
    // MorphTo 结构体无 morph_type_value 字段（由子模型数据动态决定）
}

#[test]
fn test_r5_parity_with_eagerly_result_set_in_query() {
    // R5-10：PHP with() 批量预加载 IN 查询 SQL 模式对齐
    //
    // PHP eagerlyResultSet 生成的 IN 查询:
    // - HasMany: SELECT * FROM {child} WHERE {fk} IN (v1, v2, ...)
    // - HasOne: 同 HasMany
    // - BelongsTo: SELECT * FROM {parent} WHERE {parent_pk} IN (v1, v2, ...)
    // - BelongsToMany: SELECT t.* FROM {target} t INNER JOIN {junction} j ON ... WHERE j.{fk} IN (...)
    assert_eq!(
        with::has_many_in_sql("orders", "user_id", &["1", "2", "3"]),
        "SELECT * FROM orders WHERE user_id IN (1, 2, 3)"
    );
    assert_eq!(
        with::has_one_in_sql("profiles", "user_id", &["1", "2"]),
        "SELECT * FROM profiles WHERE user_id IN (1, 2)"
    );
    assert_eq!(
        with::belongs_to_in_sql("users", "id", &["1", "2", "3"]),
        "SELECT * FROM users WHERE id IN (1, 2, 3)"
    );
    assert_eq!(
        with::belongs_to_many_in_sql(
            "roles", "user_role", "id", "role_id", "user_id", &["1", "2"]
        ),
        "SELECT t.* FROM roles t INNER JOIN user_role j ON t.id = j.role_id WHERE j.user_id IN (1, 2)"
    );
}

#[test]
fn test_r5_parity_empty_pk_values_returns_in_null() {
    // PHP 空主键值列表对齐：返回 IN (NULL) 而非跳过查询
    // PHP !empty($range) 为 false 时跳过查询，但本函数返回 SQL 字符串
    // 调用方应自行判断空列表并跳过查询
    assert_eq!(
        with::has_many_in_sql("orders", "user_id", &[]),
        "SELECT * FROM orders WHERE user_id IN (NULL)"
    );
    assert_eq!(
        with::belongs_to_in_sql("users", "id", &[]),
        "SELECT * FROM users WHERE id IN (NULL)"
    );
}

// ============================================================================
// 组 15：PHP 关键源码行为对齐（独立验证）
// ============================================================================

#[test]
fn test_r5_parity_php_get_foreign_key_algorithm() {
    // PHP RelationShip::getForeignKey 第 703-710 行:
    //   protected function getForeignKey(string $name): string {
    //       if (strpos($name, '\\')) {
    //           $name = class_basename($name);
    //       }
    //       return Str::snake($name) . '_id';
    //   }
    //
    // 验证：命名空间处理 + Str::snake + '_id' 后缀
    assert_eq!(has_many::default_foreign_key("User"), "user_id");
    assert_eq!(has_many::default_foreign_key("OrderItem"), "order_item_id");
    // 命名空间处理
    assert_eq!(has_many::default_foreign_key("app\\model\\User"), "user_id");
    assert_eq!(has_many::default_foreign_key("app/model/User"), "user_id");
}

#[test]
fn test_r5_parity_php_belongs_to_relation_name() {
    // PHP belongsTo 第 440-441 行:
    //   $trace = debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS, 2);
    //   $relation = Str::snake($trace[1]['function']);
    //
    // PHP belongsTo 使用调用方方法名作为 relation 名（用于默认外键推导）
    // 但 sz-rust 端 belongsTo 的默认外键基于关联模型名（get_class($model)）
    // 两者算法相同（getForeignKey），区别在调用方传入的参数语义
    //
    // 注：sz-rust 端不模拟 debug_backtrace，由调用方显式传入关联模型名
    // 验证算法对齐
    assert_eq!(
        belongs_to::default_belongs_to_foreign_key("User"),
        "user_id"
    );
}

#[test]
fn test_r5_parity_php_belongs_to_many_middle_default() {
    // PHP belongsToMany 第 525-526 行:
    //   $name = Str::snake(class_basename($model));
    //   $middle = $middle ?: Str::snake($this->name) . '_' . $name;
    //
    // 验证：中间表顺序敏感（当前模型在前，关联模型在后）
    assert_eq!(
        belongs_to_many::default_junction_table("User", "Role"),
        "user_role"
    );
    assert_eq!(
        belongs_to_many::default_junction_table("OrderItem", "Tag"),
        "order_item_tag"
    );
}
