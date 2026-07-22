//! 关联关系模块 — HasMany/BelongsTo/HasOne/BelongsToMany/Morph
//!
//! 对齐 PHP `think\Model` 的关联关系机制，基于 sz-orm-core `Relation` 枚举实现。
//!
//! ## 模块结构
//!
//! | 模块 | 内容 | 实现阶段 |
//! |------|------|---------|
//! | [`has_many`] | `HasMany` 关联 PHP 命名约定 + SQL 片段构造器 | Phase 4.1 ✅ |
//! | [`belongs_to`] | `BelongsTo` 关联 PHP 命名约定 + SQL 片段构造器 | Phase 4.2 ✅ |
//! | [`has_one`] | `HasOne` 关联 PHP 命名约定 + SQL 片段构造器 | Phase 4.3 ✅ |
//! | [`belongs_to_many`] | `BelongsToMany` 关联 PHP 命名约定 + SQL 片段构造器 | Phase 4.4 ✅ |
//! | [`with`] | 关联预加载（批量 IN 查询 + 数据处理纯函数 + 语法解析） | Phase 4.5 ✅ |
//! | [`find_with_related`] | find_with_related API（JOIN / eager_sql / 子查询 三种模式） | Phase 4.6 ✅ |
//! | [`cache`] | 关联缓存（对齐 PHP `withCache()` / `Cache::clear($tag)`） | Phase 4.7 ✅ |
//! | [`morph`] | MorphTo/MorphMany 多态关联（对齐 PHP `morphMany()` / `morphTo()`） | Phase 4.8 ✅ |
//! | [`n_plus_one`] | N+1 问题检测（SQL 计数 + 模板分组 + 告警生成） | Phase 4.9 ✅ |
//!
//! ## PHP 端关联关系对齐
//!
//! PHP `think\Model` 提供 4 种基础关联 + 2 种多态关联：
//!
//! | PHP 方法 | Rust 等价 | 说明 |
//! |---------|----------|------|
//! | `hasMany($model, $foreignKey, $localKey)` | `Relation::HasMany(HasMany)` | 一对多 |
//! | `belongsTo($model, $foreignKey, $localKey)` | `Relation::BelongsTo(BelongsTo)` | 多对一 |
//! | `hasOne($model, $foreignKey, $localKey)` | `Relation::HasOne(HasOne)` | 一对一 |
//! | `belongsToMany($model, $table, $foreignKey, $localKey)` | `Relation::BelongsToMany(BelongsToMany)` | 多对多 |
//! | `morphMany($model, $name, $type)` | `Relation::MorphMany(MorphMany)` | 多态一对多 |
//! | `morphTo($name, $type, $id)` | `Relation::MorphTo(MorphTo)` | 多态反向 |
//!
//! ## PHP 命名约定
//!
//! PHP think-orm 2.0.x 在 `RelationShip` trait 中定义默认外键命名：
//!
//! - `hasMany($model)` 默认外键：`Str::snake(class_name) . '_id'`（如 `User` → `user_id`）
//! - `belongsTo($model)` 默认外键：`Str::snake(relation_name) . '_id'`（注意是关联名而非类名）
//! - `belongsToMany($model)` 默认中间表：字母序 `snake_case(a) + '_' + snake_case(b)`
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model` trait，因此无法实现 `RelationLoader` trait 进行端到端关联加载测试。
//! sz-rust 端通过以下方式对齐 PHP 行为：
//!
//! 1. **re-export sz-orm-core 关联类型**：`Relation`/`HasMany`/`BelongsTo`/`HasOne`/
//!    `BelongsToMany`/`MorphMany`/`MorphTo`/`WithRelation`/`RelationError`/`RelationAccess`
//! 2. **PHP 命名约定辅助函数**：`class_to_snake_case` / `default_foreign_key` /
//!    `default_belongs_to_foreign_key` / `default_junction_table` /
//!    `default_current_fk` / `default_related_fk`
//! 3. **SQL 片段构造器**：`has_many_sql` / `belongs_to_sql` / `has_one_sql` /
//!    `belongs_to_many_sql`，用于测试验证 SQL 生成对齐 PHP
//! 4. **批量预加载辅助**（Phase 4.5）：`has_many_in_sql` / `has_one_in_sql` /
//!    `belongs_to_in_sql` / `belongs_to_many_in_sql` / `collect_pk_values` /
//!    `group_by_fk` / `sanitize_pk_value` / `parse_with_notation`，
//!    对齐 PHP `eagerlyResultSet` 批量 IN 查询机制
//! 5. **find_with_related API**（Phase 4.6）：re-export sz-orm-core
//!    `FindWithRelated` / `FindWithRelation` / `inspect_relation` /
//!    `find_with_related_join` / `find_with_related_eager_sql` /
//!    `find_with_related_subquery`，并提供 PHP 命名约定辅助函数
//!    `JoinMode` / `join_mode_str` / `is_one_to_one` / `php_with_join_sql` /
//!    `php_has_join_sql`，对齐 PHP `withJoin()` / `has()` 行为
//! 6. **关联缓存**（Phase 4.7）：re-export sz-orm-core
//!    `L2Cache` / `CacheKey` / `CacheKeyKind` / `L2CacheStats`，并提供
//!    PHP 命名约定辅助类型 `WithCacheConfig` / `WithCacheOption` 与辅助函数
//!    `php_with_cache_config` / `php_relation_cache_key` /
//!    `php_relation_cache_tag` / `php_relation_cache_remember` /
//!    `php_relation_cache_fetch` / `php_relation_cache_invalidate` /
//!    `php_relation_cache_delete`，对齐 PHP `withCache()` /
//!    `Cache::clear($tag)` / `Cache::delete($key)` 行为
//! 7. **多态关联**（Phase 4.8）：re-export sz-orm-core `MorphMany` / `MorphTo`，
//!    并提供 PHP 命名约定辅助函数 `default_morph_type_column` /
//!    `default_morph_id_column` 与配置构造器 `php_morph_many` / `php_morph_to`，
//!    以及 SQL 片段构造器 `morph_many_sql` / `morph_many_in_sql` / `morph_to_sql`，
//!    数据处理纯函数 `group_by_morph_type`（对齐 PHP `MorphTo::eagerlyResultSet`
//!    按 `morphType` 分组逻辑），对齐 PHP `morphMany()` / `morphTo()` 默认值
//!    推导（`morph . '_type'` / `morph . '_id'` / `get_class($this)`）
//! 8. **N+1 问题检测**（Phase 4.9）：作为 PHP 端的扩展（PHP 端不主动检测 N+1
//!    问题，开发者需自行识别并使用 `with()` 规避），提供类型 `SqlQueryRecord` /
//!    `DetectionConfig` / `NPlusOneAlert` / `NPlusOneDetector` 与函数
//!    `extract_template` / `detect_n_plus_one` / `suggest_with_usage`，
//!    通过 SQL 查询计数与模板分组识别 N+1 模式，生成 `with()` 使用建议告警
//!
//! 端到端关联加载测试由 sz-orm-core 内部 `WithRelation::load()` 覆盖，
//! sz-rust 端通过 SQL 片段构造器验证 SQL 生成对齐 PHP 行为。

// re-export sz-orm-core 关联类型（pub use model::* 已在 sz-orm-core::lib.rs 完成）
pub use sz_orm_core::{
    BelongsTo, BelongsToMany, HasMany, HasOne, MorphMany, MorphTo, Relation, RelationAccess,
    RelationError, RelationLoader, WithRelation,
};

pub mod belongs_to;
pub mod belongs_to_many;
pub mod cache;
pub mod find_with_related;
pub mod has_many;
pub mod has_one;
pub mod morph;
pub mod n_plus_one;
pub mod with;
