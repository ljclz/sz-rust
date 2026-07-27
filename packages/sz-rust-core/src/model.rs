//! 模型模块 — BaseModel trait + Append 字段系统
//!
//! 对齐 PHP `think\Model` + `app\common\model\szoa\BaseModel`，委托 SZ-ORM `Model`。
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name` | [`Model::table_name()`] | 表名 |
//! | `$pk` | [`Model::pk_name()`] + [`Model::pk()`] | 主键列名 + 主键值 |
//! | `$append` | [`BaseModel::append()`] | 追加虚拟字段 |
//! | `getXxxAttr` | [`BaseModel::get_appended_value()`] | 访问器（虚拟字段值） |
//! | `$field`/`$fillable` | [`ModelExt::fillable()`] | 可批量赋值字段 |
//! | `$disuse`/`$guarded` | [`ModelExt::guarded()`] | 受保护字段 |
//! | `$hidden` | [`ModelExt::hidden()`] | 序列化时隐藏字段 |
//! | `$visible` | [`ModelExt::visible()`] | 序列化时白名单字段 |
//! | `$type` | [`ModelExt::casts()`] | 字段类型转换 |
//! | `save()` | Repository 层 | 持久化由 Repository 提供（见架构说明） |
//! | `delete()` | Repository 层 | 持久化由 Repository 提供 |
//! | `startTrans()`/`commit()`/`rollback()` | Repository 层 | 事务由 Repository 提供 |
//!
//! ## 架构决策：Model 与 Repository 分离
//!
//! PHP `think\Model` 采用 Active Record 模式，Model 既是数据载体也是持久化行为。
//! Rust SZ-ORM 采用 Data Mapper + Repository 模式：
//! - [`BaseModel`] trait 只描述模型元数据（表名/主键/字段/append）
//! - 持久化动作（save/delete/事务）由 `sz_orm_core::repository::Repository` 提供
//!
//! 这符合 Rust 的设计哲学：分离数据描述与副作用行为，避免 Model trait 承载过多职责。
//! 业务模型通过实现 [`BaseModel`] trait 获得元数据描述能力，
//! 通过注入 `Repository` 获得持久化能力。
//!
//! ## Append 字段系统
//!
//! 对齐 PHP `$append = ['status_text']` + `getStatusTextAttr($value, $data)`：
//!
//! ```ignore
//! use sz_rust_core::model::BaseModel;
//! use serde_json::json;
//!
//! struct Customer {
//!     customer_id: i64,
//!     status: i32,
//! }
//!
//! impl BaseModel for Customer {
//!     fn append() -> Vec<&'static str> {
//!         vec!["status_text"]
//!     }
//!
//!     fn get_appended_value(&self, field: &str) -> Option<serde_json::Value> {
//!         match field {
//!             "status_text" => Some(json!(match self.status {
//!                 0 => "禁用",
//!                 1 => "启用",
//!                 _ => "未知",
//!             })),
//!             _ => None,
//!         }
//!     }
//! }
//! ```

#![forbid(unsafe_code)]

use serde_json::Value;
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader};

/// BaseModel trait — 对齐 PHP `app\common\model\szoa\BaseModel`
///
/// 组合 SZ-ORM 的 [`Model`] + [`ModelExt`] + [`RelationLoader`]，并补充：
/// 1. **Append 字段系统**（对齐 PHP `$append` + `getXxxAttr`）
/// 2. **带 append 的序列化**（[`Self::to_json_with_append`]）
///
/// ## 元数据属性继承
///
/// BaseModel 通过组合 SZ-ORM trait 自动获得以下能力：
/// - **表名**：[`Model::table_name()`]（对齐 PHP `$name`）
/// - **主键**：[`Model::pk_name()`] + [`Model::pk()`]（对齐 PHP `$pk`）
/// - **可填充字段**：[`ModelExt::fillable()`]（对齐 PHP `$field`/`$fillable`）
/// - **受保护字段**：[`ModelExt::guarded()`]（对齐 PHP `$disuse`/`$guarded`）
/// - **隐藏字段**：[`ModelExt::hidden()`]（对齐 PHP `$hidden`）
/// - **可见字段**：[`ModelExt::visible()`]（对齐 PHP `$visible`）
/// - **类型转换**：[`ModelExt::casts()`]（对齐 PHP `$type`）
///
/// ## 持久化方法
///
/// PHP `save()`/`delete()`/`startTrans()`/`commit()`/`rollback()` 由
/// `sz_orm_core::repository::Repository` 提供，不在 BaseModel trait 中定义。
/// 详见模块文档「架构决策：Model 与 Repository 分离」。
pub trait BaseModel: Model + ModelExt + RelationLoader + Send + Sync + 'static {
    // ==================== Append 字段系统 ====================

    /// 追加字段列表（对齐 PHP `$append = ['status_text']`）
    ///
    /// 序列化时自动追加这些虚拟字段，配合 [`Self::get_appended_value()`] 提供具体值。
    ///
    /// ## 默认实现
    ///
    /// 返回空 Vec（无追加字段），业务模型按需重写。
    fn append() -> Vec<&'static str> {
        Vec::new()
    }

    /// 获取追加字段的值（对齐 PHP `getXxxAttr($value, $data)`）
    ///
    /// 默认返回 `None`。业务模型重写此方法，根据当前模型数据计算虚拟字段值。
    ///
    /// ## 参数
    ///
    /// - `field`：字段名（来自 [`Self::append()`] 列表）
    ///
    /// ## 返回
    ///
    /// - `Some(Value)`：字段值
    /// - `None`：字段不存在或无法计算
    fn get_appended_value(&self, _field: &str) -> Option<Value> {
        None
    }

    /// 序列化为 JSON（包含 append 字段）
    ///
    /// 先调用 [`ModelExt::to_json()`] 获取基础 JSON，然后追加 [`Self::append()`]
    /// 中定义的虚拟字段（通过 [`Self::get_appended_value()`] 获取值）。
    ///
    /// ## 字段顺序
    ///
    /// 基础字段在前，append 字段在后（对齐 PHP `array_merge` 行为）。
    /// append 字段之间的顺序由 [`Self::append()`] 返回的 Vec 顺序决定。
    ///
    /// ## PHP 行为对齐
    ///
    /// - append 字段**始终输出**（无访问器返回 `null`，对齐 PHP `Conversion.php` 第 292 行
    ///   `$item[$name] = $this->getAttr($name)`，`getAttr` 无访问器时返回 `null`）
    /// - append 字段**绕过 hidden 过滤**（PHP bug 复刻：`appendAttrToArray` 直接赋值，
    ///   不检查 `$hidden`，见 `Conversion.php` 第 291-296 行）
    ///
    /// ## 缓存说明
    ///
    /// 此方法走 `get_appended_value`（独立路径，**不带访问器缓存**）。
    /// 若需走访问器缓存（对齐 PHP `getAttr` 缓存机制），请实现 [`Appendable`] trait
    /// 并使用 [`Appendable::to_json_with_append_cached()`]。
    fn to_json_with_append(&self) -> Value {
        let mut json = self.to_json();
        if let Value::Object(ref mut map) = json {
            for field in Self::append() {
                // PHP 行为：append 字段始终输出（None → null）
                let value = self.get_appended_value(field).unwrap_or(Value::Null);
                map.insert(field.to_string(), value);
            }
        }
        json
    }
}

// ============================================================================
// 访问器 / 修改器系统 — 对齐 PHP `getAttr` / `setAttr` / `getXxxAttr` / `setXxxAttr`
//
// PHP 源码依据：
// - `vendor/topthink/think-orm/src/model/concern/Attribute.php` 第 367-540 行
// - 命名规则：`Str::studly($name)` → `getXxxAttr` / `setXxxAttr`
// - 访问器缓存：`$this->get[$fieldName]`，修改器失效同名字段缓存
// - 修改器 null + data 已修改 → 提前返回（Attribute.php 第 379-381 行）
// - 修改器第二参数 = `array_merge($this->data, $data)`
// - 访问器优先于 `$type` 类型转换
// ============================================================================

/// 修改器返回结果 — 对齐 PHP `setXxxAttr` 返回值语义
///
/// PHP `setXxxAttr($value, $data)` 返回值的三种情况：
/// 1. 返回非 null 值 → 写入 `$this->data[$name]`
/// 2. 返回 null 且未修改 `$this->data` → 写入 null
/// 3. 返回 null 且已修改 `$this->data` → 提前返回，不写入当前字段
///
/// Rust 中用 `MutatorResult` 显式表达：
/// - `Value(v)`：对应情况 1
/// - `Skip`：对应情况 3（修改器内部已通过 `data_map_mut` 修改 data）
///
/// 情况 2 在 Rust 中通过 `Some(MutatorResult::Value(Value::Null))` 表达。
#[derive(Debug, Clone, PartialEq)]
pub enum MutatorResult {
    /// 修改器返回具体值，写入 `$this->data[$name]`
    Value(Value),
    /// 修改器返回 null 且内部已修改 data，跳过默认赋值
    /// （对应 PHP `Attribute.php` 第 379-381 行提前返回分支）
    Skip,
}

/// 访问器 trait — 对齐 PHP `getAttr` / `getXxxAttr`
///
/// ## PHP 对齐
///
/// | PHP 方法 | Rust 等价 | 说明 |
/// |----------|----------|------|
/// | `getAttr($name)` | [`Self::get_attr()`] | 入口方法，含缓存 |
/// | `getData($name)` | [`Self::get_data()`] | 取原始字段值（不触发访问器） |
/// | `getXxxAttr($value, $data)` | [`Self::accessor_for()`] | 业务模型按字段派发 |
/// | `getRealFieldName($name)` | [`Self::real_field_name()`] | 字段名归一化 |
/// | `__isset($name)` | [`Self::has_attr()`] | 触发访问器判 null |
/// | `$this->data` | [`Self::data_map()`] | 原始字段数组 |
/// | `$this->get` | [`Self::accessor_cache()`] | 访问器结果缓存 |
///
/// ## 缓存机制
///
/// - 首次 `get_attr(field)` 触发访问器，结果缓存到 `accessor_cache`
/// - 同名字段被 `set_attr` 时失效对应缓存
/// - **PHP bug 复刻**：不同名字段修改不失效派生字段缓存
///   （如 `set_attr("status", ...)` 不失效 `status_text` 缓存）
pub trait Accessor {
    /// 取原始字段数组（对应 PHP `$this->data`）
    fn data_map(&self) -> &HashMap<String, Value>;

    /// 取可变原始字段数组
    fn data_map_mut(&mut self) -> &mut HashMap<String, Value>;

    /// 取访问器缓存（对应 PHP `$this->get`）
    fn accessor_cache(&self) -> &HashMap<String, Value>;

    /// 取可变访问器缓存
    fn accessor_cache_mut(&mut self) -> &mut HashMap<String, Value>;

    /// 字段名归一化（对应 PHP `getRealFieldName`）
    ///
    /// PHP 默认 `$strict=true, $convertNameToCamel=false` → 原样返回。
    /// 业务模型按需重写以支持 snake_case ↔ camelCase 转换。
    fn real_field_name(&self, name: &str) -> String {
        name.to_string()
    }

    /// 业务模型重写：按字段名派发到具体访问器
    ///
    /// ## 参数
    ///
    /// - `field`：归一化后的字段名
    /// - `value`：原始字段值（来自 `data_map`，可能为 `None`）
    ///
    /// ## 返回
    ///
    /// 访问器计算后的值（对应 PHP `getXxxAttr($value, $this->data)` 返回值）
    ///
    /// ## 默认实现
    ///
    /// 返回原始字段值（或 `Value::Null`），等价于 PHP 无访问器时走 `$type` / 关联 / 原值分支。
    fn accessor_for(&self, field: &str, value: Option<&Value>) -> Value {
        let _ = field;
        value.cloned().unwrap_or(Value::Null)
    }

    /// 入口方法 — 对应 PHP `getAttr($name)`
    ///
    /// 执行流程（对齐 `Attribute.php` 第 497-540 行）：
    /// 1. 字段名归一化
    /// 2. 缓存命中 → 直接返回
    /// 3. 取原始字段值
    /// 4. 派发到 `accessor_for`
    /// 5. 写入缓存
    fn get_attr(&mut self, name: &str) -> Value {
        let field = self.real_field_name(name);

        // 1. 缓存命中（对应 PHP $this->get[$fieldName]）
        if let Some(cached) = self.accessor_cache().get(&field) {
            return cached.clone();
        }

        // 2. 取原始值（对应 PHP getData，不抛异常）
        let value = self.data_map().get(&field);

        // 3. 派发到具体访问器
        let result = self.accessor_for(&field, value);

        // 4. 写入缓存（对应 PHP $this->get[$fieldName] = $value）
        self.accessor_cache_mut().insert(field, result.clone());

        result
    }

    /// 取原始字段值（不触发访问器）— 对应 PHP `getData($name)`
    fn get_data(&self, name: &str) -> Option<&Value> {
        let field = self.real_field_name(name);
        self.data_map().get(&field)
    }

    /// 检测字段是否存在 — 对应 PHP `__isset($name)`
    ///
    /// **PHP 行为复刻**：`isset($model->field)` 触发访问器执行并缓存结果。
    fn has_attr(&mut self, name: &str) -> bool {
        !self.get_attr(name).is_null()
    }
}

/// 修改器 trait — 对齐 PHP `setAttr` / `setXxxAttr`
///
/// ## PHP 对齐
///
/// | PHP 方法 | Rust 等价 | 说明 |
/// |----------|----------|------|
/// | `setAttr($name, $value, $data)` | [`Self::set_attr()`] | 入口方法 |
/// | `setXxxAttr($value, $data)` | [`Self::mutator_for()`] | 业务模型按字段派发 |
/// | `setAttrs($data)` | [`Self::set_attrs()`] | 批量赋值 |
///
/// ## 修改器第二参数 `merged_data`
///
/// PHP `setXxxAttr($value, $data)` 中 `$data = array_merge($this->data, $data)`，
/// 即「当前模型数据 + 外部批量数据」的合并。Rust 中 [`Self::mutator_for()`]
/// 第三参数 `merged_data` 保留此语义。
///
/// ## 优先级
///
/// 1. 方法修改器 `mutator_for` → 若返回 `Some` 则使用其结果
/// 2. 无修改器 → 原样写入 `data_map`
///
/// `$type` 类型转换 / 关联属性 / `__toString` 由各功能模块分别实现，当前阶段仅支持方法修改器。
pub trait Mutator: Accessor {
    /// 业务模型重写：按字段名派发到具体修改器
    ///
    /// ## 参数
    ///
    /// - `field`：归一化后的字段名
    /// - `value`：被设置的值（引用，修改器按需 clone）
    /// - `merged_data`：`data_map` + 外部 `data` 的合并（对应 PHP `array_merge`）
    ///
    /// ## 返回
    ///
    /// - `None`：无修改器，原样写入 data
    /// - `Some(MutatorResult::Value(v))`：写入 `v` 到 data
    /// - `Some(MutatorResult::Skip)`：跳过默认赋值（对应 PHP null + data modified 提前返回）
    fn mutator_for(
        &mut self,
        field: &str,
        value: &Value,
        merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult>;

    /// 入口方法 — 对应 PHP `setAttr($name, $value, $data)`
    ///
    /// 执行流程（对齐 `Attribute.php` 第 367-395 行）：
    /// 1. 字段名归一化
    /// 2. 构造 `merged_data`（`data_map` + 外部 `data`）
    /// 3. 派发到 `mutator_for`
    /// 4. 处理 `Skip` / `Value` / `None` 三种结果
    /// 5. 失效同名字段访问器缓存（对应 PHP `unset($this->get[$name])`）
    fn set_attr(&mut self, name: &str, value: Value, data: Option<&HashMap<String, Value>>) {
        let field = self.real_field_name(name);

        // 1. 构造 merged_data（对应 PHP array_merge($this->data, $data)）
        let merged_data = if let Some(d) = data {
            let mut m = self.data_map().clone();
            m.extend(d.clone());
            m
        } else {
            self.data_map().clone()
        };

        // 2. 派发到具体修改器
        let result = self.mutator_for(&field, &value, &merged_data);

        // 3. 处理结果
        match result {
            // PHP 第 379-381 行：修改器返回 null + 修改了 data → 提前返回
            Some(MutatorResult::Skip) => {
                self.accessor_cache_mut().remove(&field);
            }
            // 修改器返回具体值，写入 data
            Some(MutatorResult::Value(v)) => {
                self.data_map_mut().insert(field.clone(), v);
                self.accessor_cache_mut().remove(&field);
            }
            // 无修改器，原样写入 data（对应 PHP 第 394 行 $this->data[$name] = $value）
            None => {
                self.data_map_mut().insert(field.clone(), value);
                self.accessor_cache_mut().remove(&field);
            }
        }
    }

    /// 批量赋值 — 对应 PHP `setAttrs($data)`
    ///
    /// 对每个字段调用 `set_attr`，第三参数传完整 `data`（使修改器能感知批量上下文）。
    fn set_attrs(&mut self, data: &HashMap<String, Value>) {
        let fields: Vec<(String, Value)> =
            data.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (field, value) in fields {
            self.set_attr(&field, value, Some(data));
        }
    }
}

// ============================================================================
// Append 字段系统 — 动态 append + 走访问器缓存
//
// PHP 源码依据：
// - `vendor/topthink/think-orm/src/model/concern/Conversion.php` 第 88-97 行（append 方法）
// - `vendor/topthink/think-orm/src/model/concern/Conversion.php` 第 234-236 行（toArray 中 append 处理）
// - `vendor/topthink/think-orm/src/model/concern/Conversion.php` 第 291-296 行（appendAttrToArray 直接赋值，绕过 hidden）
// - `vendor/topthink/think-orm/src/model/concern/Attribute.php` 第 475-486 行（getAttr 入口，带缓存）
// ============================================================================

/// 动态 Append 状态 — 对齐 PHP `$this->append` 实例属性
///
/// PHP 中 `$append` 是实例属性，可通过 `append()` 方法动态修改：
/// - `append($fields)` 默认覆盖（`Conversion.php` 第 90-94 行）
/// - `append($fields, true)` 合并（`Conversion.php` 第 91-93 行）
///
/// Rust 中通过 `AppendState` 持有动态状态，`None` 表示使用静态 [`BaseModel::append()`]。
///
/// ## 鲜视达项目实际用法
///
/// 项目内 12 个模型使用静态 `$append` 声明，**零动态 `->append()` 调用**。
/// 此结构提供框架能力补全，业务模型按需使用。
#[derive(Debug, Clone, Default)]
pub struct AppendState {
    /// 动态 append 字段列表（`None` 表示使用静态默认）
    dynamic: Option<Vec<String>>,
}

impl AppendState {
    /// 创建空状态（使用静态默认）
    pub fn new() -> Self {
        Self::default()
    }

    /// 覆盖动态 append（对齐 PHP `$model->append($fields)` 默认行为）
    pub fn replace(&mut self, fields: Vec<String>) {
        self.dynamic = Some(fields);
    }

    /// 合并到动态 append（对齐 PHP `$model->append($fields, true)`）
    pub fn merge(&mut self, fields: Vec<String>) {
        match &mut self.dynamic {
            Some(existing) => {
                for field in fields {
                    if !existing.contains(&field) {
                        existing.push(field);
                    }
                }
            }
            None => {
                self.dynamic = Some(fields);
            }
        }
    }

    /// 获取动态 append 字段（如果有）
    pub fn dynamic_fields(&self) -> Option<&Vec<String>> {
        self.dynamic.as_ref()
    }
}

/// Appendable trait — 对齐 PHP `append()` + `getAttr()` 统一派发
///
/// 业务模型实现此 trait 获得：
/// 1. **动态 append 能力**（覆盖/合并，对齐 PHP `Conversion.php` 第 88-97 行）
/// 2. **append 字段走 `get_attr` 缓存**（对齐 PHP `Conversion.php` 第 292 行 +
///    `Attribute.php` 第 475-486 行）
///
/// ## 与 BaseModel 的关系
///
/// - [`BaseModel::to_json_with_append()`]：无缓存版本，走 `get_appended_value`（独立路径）
/// - [`Self::to_json_with_append_cached()`]：带缓存版本，走 `get_attr`（统一路径，完全对齐 PHP）
///
/// 业务模型按需实现 `Appendable` 获得完整 PHP 对齐能力。
///
/// ## PHP 对齐
///
/// | PHP 方法 | Rust 等价 | 说明 |
/// |----------|----------|------|
/// | `$this->append` | [`AppendState`] | 动态 append 状态 |
/// | `append($fields)` | [`Self::append_dyn()`] | 覆盖模式（默认） |
/// | `append($fields, true)` | [`Self::append_merge()`] | 合并模式 |
/// | `toArray()` 中 append 循环 | [`Self::to_json_with_append_cached()`] | 走 getAttr 缓存 |
pub trait Appendable: BaseModel + Accessor {
    /// 取动态 append 状态
    fn append_state(&self) -> &AppendState;

    /// 取可变动态 append 状态
    fn append_state_mut(&mut self) -> &mut AppendState;

    /// 动态 append（覆盖模式，对齐 PHP `$model->append($fields)`）
    ///
    /// 默认覆盖静态 [`BaseModel::append()`]，返回 `&mut Self` 支持链式调用
    ///（对齐 PHP `Conversion.php` 第 96 行 `return $this`）。
    fn append_dyn(&mut self, fields: Vec<String>) -> &mut Self {
        self.append_state_mut().replace(fields);
        self
    }

    /// 动态 append（合并模式，对齐 PHP `$model->append($fields, true)`）
    ///
    /// PHP 语义（`Conversion.php` 第 91-93 行）：`array_merge($this->append, $fields)`。
    /// 首次合并时 `$this->append` 为静态默认值，需保留；后续合并累加到当前动态字段。
    fn append_merge(&mut self, fields: Vec<String>) -> &mut Self {
        if self.append_state().dynamic_fields().is_none() {
            // 首次合并：初始化为「静态 append + fields」（去重）
            let mut combined: Vec<String> = Self::append().iter().map(|s| s.to_string()).collect();
            for field in fields {
                if !combined.contains(&field) {
                    combined.push(field);
                }
            }
            self.append_state_mut().replace(combined);
        } else {
            // 已有动态字段：合并到现有
            self.append_state_mut().merge(fields);
        }
        self
    }

    /// 获取生效的 append 字段列表
    ///
    /// 优先级：动态 append > 静态 [`BaseModel::append()`]
    /// 对齐 PHP `$this->append`（动态覆盖后静态失效）
    fn effective_append(&self) -> Vec<String> {
        match self.append_state().dynamic_fields() {
            Some(dyn_fields) => dyn_fields.clone(),
            None => Self::append().iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 序列化为 JSON（包含 append 字段，走访问器缓存）
    ///
    /// 对齐 PHP `toArray()` 第 234-236 行 + `Attribute.php` `getAttr`：
    /// 1. 先取基础 `to_json`（已应用 hidden 过滤）
    /// 2. 对每个生效 append 字段调用 `get_attr`（带缓存）
    /// 3. append 字段始终输出（无访问器返回 `null`，对齐 PHP 第 292 行）
    /// 4. append 字段绕过 hidden 过滤（PHP bug 复刻，对齐第 291-296 行）
    fn to_json_with_append_cached(&mut self) -> Value {
        let mut json = self.to_json();
        if let Value::Object(ref mut map) = json {
            let fields = self.effective_append();
            for field in fields {
                // PHP 行为：append 字段始终走 getAttr（带缓存）
                let value = self.get_attr(&field);
                map.insert(field, value);
            }
        }
        json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use sz_orm_core::Value as OrmValue;
    use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields};

    // ====================================================================
    // Mock 模型：无 append 字段
    // ====================================================================

    struct UserWithoutAppend {
        user_id: i64,
        username: String,
        password: String,
    }

    impl Model for UserWithoutAppend {
        type PrimaryKey = i64;

        fn table_name() -> &'static str {
            "sz_user"
        }

        fn pk_name() -> &'static str {
            "user_id"
        }

        fn pk(&self) -> Self::PrimaryKey {
            self.user_id
        }

        fn set_pk(&mut self, pk: Self::PrimaryKey) {
            self.user_id = pk;
        }

        fn timestamp_fields() -> Option<TimestampFields> {
            None
        }

        fn soft_delete_field() -> Option<&'static str> {
            None
        }
    }

    impl ModelExt for UserWithoutAppend {
        fn columns() -> Vec<&'static str> {
            vec!["user_id", "username", "password"]
        }

        fn fillable() -> Vec<&'static str> {
            vec!["username", "password"]
        }

        fn guarded() -> Vec<&'static str> {
            vec!["user_id"]
        }

        fn hidden() -> Vec<&'static str> {
            vec!["password"]
        }

        fn get_column_value(&self, column: &str) -> Option<OrmValue> {
            match column {
                "user_id" => Some(OrmValue::I64(self.user_id)),
                "username" => Some(OrmValue::String(self.username.clone())),
                "password" => Some(OrmValue::String(self.password.clone())),
                _ => None,
            }
        }

        fn from_value(&mut self, _map: HashMap<String, OrmValue>) {
            // 测试用：空实现
        }
    }

    impl RelationLoader for UserWithoutAppend {
        fn get_relation(&self, _name: &str) -> Option<&OrmValue> {
            None
        }

        fn set_relation_data(&mut self, _name: &str, _data: OrmValue) {}

        fn get_relation_fk_value(&self, _fk_name: &str) -> String {
            String::new()
        }
    }

    impl BaseModel for UserWithoutAppend {}

    // ====================================================================
    // Mock 模型：有 append 字段
    // ====================================================================

    struct CustomerWithAppend {
        customer_id: i64,
        status: i32,
        name: String,
    }

    impl Model for CustomerWithAppend {
        type PrimaryKey = i64;

        fn table_name() -> &'static str {
            "szoa_customer"
        }

        fn pk_name() -> &'static str {
            "customer_id"
        }

        fn pk(&self) -> Self::PrimaryKey {
            self.customer_id
        }

        fn set_pk(&mut self, pk: Self::PrimaryKey) {
            self.customer_id = pk;
        }
    }

    impl ModelExt for CustomerWithAppend {
        fn columns() -> Vec<&'static str> {
            vec!["customer_id", "status", "name"]
        }

        fn fillable() -> Vec<&'static str> {
            vec!["status", "name"]
        }

        fn guarded() -> Vec<&'static str> {
            vec!["customer_id"]
        }

        fn get_column_value(&self, column: &str) -> Option<OrmValue> {
            match column {
                "customer_id" => Some(OrmValue::I64(self.customer_id)),
                "status" => Some(OrmValue::I32(self.status)),
                "name" => Some(OrmValue::String(self.name.clone())),
                _ => None,
            }
        }

        fn from_value(&mut self, _map: HashMap<String, OrmValue>) {}
    }

    impl RelationLoader for CustomerWithAppend {
        fn get_relation(&self, _name: &str) -> Option<&OrmValue> {
            None
        }

        fn set_relation_data(&mut self, _name: &str, _data: OrmValue) {}

        fn get_relation_fk_value(&self, _fk_name: &str) -> String {
            String::new()
        }
    }

    impl BaseModel for CustomerWithAppend {
        fn append() -> Vec<&'static str> {
            vec!["status_text"]
        }

        fn get_appended_value(&self, field: &str) -> Option<Value> {
            match field {
                "status_text" => Some(json!(match self.status {
                    0 => "禁用",
                    1 => "启用",
                    _ => "未知",
                })),
                _ => None,
            }
        }
    }

    // ====================================================================
    // 元数据属性测试（name/pk/fillable/guarded/hidden）
    // ====================================================================

    #[test]
    fn test_base_model_table_name() {
        // 对齐 PHP $name = 'customer'
        assert_eq!(UserWithoutAppend::table_name(), "sz_user");
        assert_eq!(CustomerWithAppend::table_name(), "szoa_customer");
    }

    #[test]
    fn test_base_model_pk_name() {
        // 对齐 PHP $pk = 'customer_id'
        assert_eq!(UserWithoutAppend::pk_name(), "user_id");
        assert_eq!(CustomerWithAppend::pk_name(), "customer_id");
    }

    #[test]
    fn test_base_model_fillable() {
        // 对齐 PHP $field/$fillable
        assert_eq!(UserWithoutAppend::fillable(), vec!["username", "password"]);
        assert_eq!(CustomerWithAppend::fillable(), vec!["status", "name"]);
    }

    #[test]
    fn test_base_model_guarded() {
        // 对齐 PHP $disuse/$guarded
        assert_eq!(UserWithoutAppend::guarded(), vec!["user_id"]);
        assert_eq!(CustomerWithAppend::guarded(), vec!["customer_id"]);
    }

    #[test]
    fn test_base_model_hidden() {
        // 对齐 PHP $hidden
        assert_eq!(UserWithoutAppend::hidden(), vec!["password"]);
        assert_eq!(CustomerWithAppend::hidden(), Vec::<&str>::new());
    }

    #[test]
    fn test_base_model_pk_value() {
        let user = UserWithoutAppend {
            user_id: 42,
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        assert_eq!(user.pk(), 42);
    }

    // ====================================================================
    // Append 字段系统测试
    // ====================================================================

    #[test]
    fn test_base_model_append_default_empty() {
        // 默认 append() 返回空 Vec
        assert_eq!(UserWithoutAppend::append(), Vec::<&str>::new());
    }

    #[test]
    fn test_base_model_append_with_status_text() {
        // 对齐 PHP $append = ['status_text']
        assert_eq!(CustomerWithAppend::append(), vec!["status_text"]);
    }

    #[test]
    fn test_base_model_get_appended_value_default_none() {
        let user = UserWithoutAppend {
            user_id: 1,
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        // 默认 get_appended_value 返回 None
        assert_eq!(user.get_appended_value("any_field"), None);
    }

    #[test]
    fn test_base_model_get_appended_value_status_text() {
        // 对齐 PHP getStatusTextAttr($value, $data)
        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 0,
            name: "Alice Corp".to_string(),
        };
        assert_eq!(
            customer.get_appended_value("status_text"),
            Some(json!("禁用"))
        );

        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 1,
            name: "Alice Corp".to_string(),
        };
        assert_eq!(
            customer.get_appended_value("status_text"),
            Some(json!("启用"))
        );

        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 99,
            name: "Alice Corp".to_string(),
        };
        assert_eq!(
            customer.get_appended_value("status_text"),
            Some(json!("未知"))
        );

        // 未知字段返回 None
        assert_eq!(customer.get_appended_value("unknown_field"), None);
    }

    // ====================================================================
    // to_json_with_append 序列化测试
    // ====================================================================

    #[test]
    fn test_base_model_to_json_without_append() {
        // 无 append 的模型：to_json_with_append 等同于 to_json
        let user = UserWithoutAppend {
            user_id: 1,
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        let json = user.to_json_with_append();

        // password 字段被 hidden 隐藏
        assert_eq!(json["user_id"], 1);
        assert_eq!(json["username"], "alice");
        assert!(json.get("password").is_none(), "password 应被 hidden 隐藏");
        assert!(json.get("status_text").is_none(), "无 append 字段");
    }

    #[test]
    fn test_base_model_to_json_with_append() {
        // 有 append 的模型：to_json_with_append 在基础字段后追加虚拟字段
        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 1,
            name: "Alice Corp".to_string(),
        };
        let json = customer.to_json_with_append();

        // 基础字段
        assert_eq!(json["customer_id"], 1);
        assert_eq!(json["status"], 1);
        assert_eq!(json["name"], "Alice Corp");

        // append 字段
        assert_eq!(json["status_text"], "启用");
    }

    #[test]
    fn test_base_model_to_json_append_field_order() {
        // append 字段在基础字段之后
        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 0,
            name: "Test".to_string(),
        };
        let json = customer.to_json_with_append();

        if let Value::Object(map) = json {
            let keys: Vec<&String> = map.keys().collect();
            // 基础字段在前，append 字段在后
            let customer_id_pos = keys.iter().position(|k| *k == "customer_id").unwrap();
            let status_pos = keys.iter().position(|k| *k == "status").unwrap();
            let name_pos = keys.iter().position(|k| *k == "name").unwrap();
            let status_text_pos = keys.iter().position(|k| *k == "status_text").unwrap();

            assert!(
                customer_id_pos < status_text_pos,
                "customer_id 应在 status_text 之前"
            );
            assert!(status_pos < status_text_pos, "status 应在 status_text 之前");
            assert!(name_pos < status_text_pos, "name 应在 status_text 之前");
        } else {
            panic!("to_json_with_append 应返回 JSON Object");
        }
    }

    // ====================================================================
    // PHP 一致性测试（R5 硬约束：PHP/Rust 行为对比）
    // ====================================================================

    #[test]
    fn test_php_consistency_model_name_aligns_php_name_property() {
        // PHP: protected $name = 'customer';
        // Rust: Model::table_name() 返回表名
        assert_eq!(CustomerWithAppend::table_name(), "szoa_customer");
    }

    #[test]
    fn test_php_consistency_model_pk_aligns_php_pk_property() {
        // PHP: protected $pk = 'customer_id';
        // Rust: Model::pk_name() 返回主键列名
        assert_eq!(CustomerWithAppend::pk_name(), "customer_id");
    }

    #[test]
    fn test_php_consistency_model_append_aligns_php_append_property() {
        // PHP: protected $append = ['status_text'];
        // Rust: BaseModel::append() 返回追加字段列表
        assert_eq!(CustomerWithAppend::append(), vec!["status_text"]);
    }

    #[test]
    fn test_php_consistency_model_hidden_aligns_php_hidden_property() {
        // PHP: protected $hidden = ['password'];
        // Rust: ModelExt::hidden() 返回隐藏字段列表
        // 序列化时 password 字段不应出现
        let user = UserWithoutAppend {
            user_id: 1,
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        let json = user.to_json_with_append();
        assert!(
            json.get("password").is_none(),
            "password 应被 hidden 隐藏（对齐 PHP $hidden）"
        );
    }

    #[test]
    fn test_php_consistency_get_xxx_attr_aligns_php_accessor() {
        // PHP: getStatusTextAttr($value, $data) 根据状态返回文本
        // Rust: get_appended_value("status_text") 返回对应的文本
        let test_cases = vec![(0i32, "禁用"), (1, "启用"), (99, "未知")];

        for (status, expected) in test_cases {
            let customer = CustomerWithAppend {
                customer_id: 1,
                status,
                name: "Test".to_string(),
            };
            assert_eq!(
                customer.get_appended_value("status_text"),
                Some(json!(expected)),
                "status={} 应返回 '{}'",
                status,
                expected
            );
        }
    }

    #[test]
    fn test_php_consistency_serialization_includes_append_fields() {
        // PHP: 序列化时自动追加 $append 中的虚拟字段
        // Rust: to_json_with_append() 在基础字段后追加 append 字段
        let customer = CustomerWithAppend {
            customer_id: 1,
            status: 1,
            name: "Alice Corp".to_string(),
        };
        let json = customer.to_json_with_append();

        // 验证基础字段
        assert_eq!(json["customer_id"], 1);
        assert_eq!(json["status"], 1);
        assert_eq!(json["name"], "Alice Corp");

        // 验证 append 字段
        assert_eq!(json["status_text"], "启用");
    }

    // ====================================================================
    // 访问器 / 修改器系统测试
    // ====================================================================

    /// 测试用模型：实现 Accessor + Mutator
    struct AccessorTestModel {
        data: HashMap<String, Value>,
        get_cache: HashMap<String, Value>,
    }

    impl AccessorTestModel {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
                get_cache: HashMap::new(),
            }
        }

        fn with_data(mut self, key: &str, value: Value) -> Self {
            self.data.insert(key.to_string(), value);
            self
        }
    }

    impl Accessor for AccessorTestModel {
        fn data_map(&self) -> &HashMap<String, Value> {
            &self.data
        }

        fn data_map_mut(&mut self) -> &mut HashMap<String, Value> {
            &mut self.data
        }

        fn accessor_cache(&self) -> &HashMap<String, Value> {
            &self.get_cache
        }

        fn accessor_cache_mut(&mut self) -> &mut HashMap<String, Value> {
            &mut self.get_cache
        }

        /// 模拟 PHP `getStatusTextAttr($value, $data)`
        /// - status=0 → "禁用"
        /// - status=1 → "启用"
        /// - 其他 → "未知"
        /// - 不存在的字段 → Value::Null（对齐 PHP $append 无访问器时返回 null）
        fn accessor_for(&self, field: &str, value: Option<&Value>) -> Value {
            match field {
                "status_text" => {
                    let status = self
                        .data
                        .get("status")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-1);
                    json!(match status {
                        0 => "禁用",
                        1 => "启用",
                        _ => "未知",
                    })
                }
                "rentarea_ids" => {
                    // 模拟 PHP getRentareaIdsAttr：逗号分隔字符串 → int 数组
                    let raw = value.and_then(|v| v.as_str()).unwrap_or("");
                    if raw.is_empty() {
                        json!([])
                    } else {
                        let arr: Vec<Value> = raw
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| s.parse::<i64>().ok())
                            .map(Value::from)
                            .collect();
                        json!(arr)
                    }
                }
                "contract_price" => {
                    // 模拟 PHP getContractPriceAttr：float 强转，null → 0
                    // PHP (float)$value 会把字符串 "100.5" 转 100.5，Rust 需显式 parse
                    let price = value
                        .and_then(|v| {
                            if v.is_null() {
                                Some(0.0)
                            } else if let Some(f) = v.as_f64() {
                                Some(f)
                            } else if let Some(s) = v.as_str() {
                                s.parse::<f64>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0.0);
                    json!(price)
                }
                _ => value.cloned().unwrap_or(Value::Null),
            }
        }
    }

    impl Mutator for AccessorTestModel {
        /// 模拟 PHP setRentareaIdsAttr：数组 → 逗号分隔字符串
        /// 模拟 PHP setSpecialAttr（返回 Skip）：内部修改其他字段，跳过默认赋值
        fn mutator_for(
            &mut self,
            field: &str,
            value: &Value,
            merged_data: &HashMap<String, Value>,
        ) -> Option<MutatorResult> {
            match field {
                "rentarea_ids" => {
                    let arr: Vec<String> = match value {
                        Value::Array(items) => items
                            .iter()
                            .filter_map(|v| {
                                let s = match v {
                                    Value::String(s) => s.trim().to_string(),
                                    _ => v.to_string(),
                                };
                                if s.is_empty() {
                                    None
                                } else {
                                    Some(s)
                                }
                            })
                            .collect(),
                        _ => return Some(MutatorResult::Value(Value::String(String::new()))),
                    };
                    Some(MutatorResult::Value(Value::String(arr.join(","))))
                }
                "field_b" => {
                    // 模拟 PHP setFieldBAttr($value, $data)：使用 merged_data 中的 field_a
                    let field_a = merged_data
                        .get("field_a")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let val_b = value.as_i64().unwrap_or(0);
                    Some(MutatorResult::Value(json!(format!(
                        "{}_{}",
                        field_a, val_b
                    ))))
                }
                "special_attr" => {
                    // 模拟 PHP null + data modified 提前返回
                    self.data.insert("field_a".to_string(), json!("A"));
                    self.data.insert("field_b".to_string(), json!("B"));
                    Some(MutatorResult::Skip)
                }
                _ => None,
            }
        }
    }

    #[test]
    fn test_accessor_basic_status_text() {
        // 对齐 PHP getStatusTextAttr($value, $data)
        let mut model = AccessorTestModel::new().with_data("status", json!(0));
        assert_eq!(model.get_attr("status_text"), json!("禁用"));

        let mut model = AccessorTestModel::new().with_data("status", json!(1));
        assert_eq!(model.get_attr("status_text"), json!("启用"));

        let mut model = AccessorTestModel::new().with_data("status", json!(99));
        assert_eq!(model.get_attr("status_text"), json!("未知"));
    }

    #[test]
    fn test_accessor_real_field_value() {
        // 对齐 PHP getRentareaIdsAttr：逗号分隔字符串 → int 数组
        let mut model = AccessorTestModel::new().with_data("rentarea_ids", json!("1,2,3"));
        assert_eq!(model.get_attr("rentarea_ids"), json!([1, 2, 3]));

        let mut model = AccessorTestModel::new().with_data("rentarea_ids", json!(""));
        assert_eq!(model.get_attr("rentarea_ids"), json!([]));
    }

    #[test]
    fn test_accessor_float_coercion() {
        // 对齐 PHP getContractPriceAttr：null → 0.0，字符串数字 → float
        let mut model = AccessorTestModel::new().with_data("contract_price", Value::Null);
        assert_eq!(model.get_attr("contract_price"), json!(0.0));

        let mut model = AccessorTestModel::new().with_data("contract_price", json!("100.5"));
        assert_eq!(model.get_attr("contract_price"), json!(100.5));
    }

    #[test]
    fn test_accessor_cache_hit() {
        // 对齐 PHP $this->get[$fieldName] 缓存机制
        let mut model = AccessorTestModel::new().with_data("status", json!(1));
        let v1 = model.get_attr("status_text");
        // 修改 status 不失效 status_text 缓存（PHP bug 复刻）
        model.data.insert("status".to_string(), json!(0));
        let v2 = model.get_attr("status_text");
        assert_eq!(v1, v2, "缓存命中，访问器不重新执行");
        assert_eq!(v1, json!("启用"));
    }

    #[test]
    fn test_accessor_cache_invalidation_on_set_same_field() {
        // 对齐 PHP unset($this->get[$name])：setAttr 同名字段失效缓存
        let mut model = AccessorTestModel::new().with_data("status", json!(1));
        let v1 = model.get_attr("status_text");
        assert_eq!(v1, json!("启用"));

        // set_attr 同名字段（status），失效 status 缓存
        // 注意：status_text 缓存不受影响（PHP bug 复刻）
        model.set_attr("status", json!(0), None);

        // 但 status_text 的缓存还在，不会重新计算
        let v2 = model.get_attr("status_text");
        assert_eq!(v2, json!("启用"), "status_text 缓存未失效（PHP bug 复刻）");
    }

    #[test]
    fn test_mutator_basic_array_to_string() {
        // 对齐 PHP setRentareaIdsAttr：数组 → 逗号分隔字符串
        let mut model = AccessorTestModel::new();
        model.set_attr("rentarea_ids", json!([1, 2, 3]), None);
        assert_eq!(model.data.get("rentarea_ids"), Some(&json!("1,2,3")));

        // 空数组
        model.set_attr("rentarea_ids", json!([]), None);
        assert_eq!(model.data.get("rentarea_ids"), Some(&json!("")));
    }

    #[test]
    fn test_mutator_skip_php_bug_replication() {
        // 对齐 PHP Attribute.php 第 379-381 行：
        // 修改器返回 null + 已修改 data → 提前返回，不写入当前字段
        let mut model = AccessorTestModel::new();
        model.set_attr("special_attr", json!("X"), None);

        // special_attr 未被写入
        assert!(
            !model.data.contains_key("special_attr"),
            "special_attr 应被跳过（PHP bug 复刻）"
        );
        // field_a / field_b 已被修改器写入
        assert_eq!(model.data.get("field_a"), Some(&json!("A")));
        assert_eq!(model.data.get("field_b"), Some(&json!("B")));
    }

    #[test]
    fn test_mutator_merged_data() {
        // 对齐 PHP array_merge($this->data, $data)：
        // 修改器第二参数是合并后的 data
        let mut model = AccessorTestModel::new();
        model.data.insert("field_a".to_string(), json!(1));

        let mut batch = HashMap::new();
        batch.insert("field_a".to_string(), json!(100));
        batch.insert("field_b".to_string(), json!(2));

        model.set_attrs(&batch);

        // field_b 修改器使用 merged_data 中的 field_a=100（batch 覆盖 model.data）
        assert_eq!(model.data.get("field_b"), Some(&json!("100_2")));
    }

    #[test]
    fn test_set_attrs_batch() {
        // 对齐 PHP setAttrs：批量赋值
        let mut model = AccessorTestModel::new();
        let mut batch = HashMap::new();
        batch.insert("status".to_string(), json!(1));
        batch.insert("name".to_string(), json!("Alice"));

        model.set_attrs(&batch);

        assert_eq!(model.data.get("status"), Some(&json!(1)));
        assert_eq!(model.data.get("name"), Some(&json!("Alice")));
    }

    #[test]
    fn test_has_attr_triggers_accessor() {
        // 对齐 PHP __isset：触发访问器执行
        let mut model = AccessorTestModel::new().with_data("status", json!(1));
        assert!(model.has_attr("status_text"), "status_text 应存在");

        // 无访问器且字段不存在 → 返回 Null → has_attr 返回 false
        assert!(!model.has_attr("nonexistent"), "不存在的字段应返回 false");
    }

    #[test]
    fn test_get_data_returns_raw_value() {
        // 对齐 PHP getData：不触发访问器
        let model = AccessorTestModel::new().with_data("rentarea_ids", json!("1,2,3"));
        // get_data 返回原始字符串，不是数组
        assert_eq!(model.get_data("rentarea_ids"), Some(&json!("1,2,3")));
    }

    #[test]
    fn test_accessor_for_unknown_field_returns_null() {
        // 对齐 PHP $append 字段无访问器时返回 null
        let mut model = AccessorTestModel::new();
        let v = model.get_attr("nonexistent_field");
        assert!(v.is_null(), "未知字段应返回 Null");
    }

    #[test]
    fn test_real_field_name_default_identity() {
        // 对齐 PHP 默认 $strict=true, $convertNameToCamel=false：原样返回
        let model = AccessorTestModel::new();
        assert_eq!(model.real_field_name("status_text"), "status_text");
        assert_eq!(model.real_field_name("user_id"), "user_id");
    }

    // ====================================================================
    // PHP 一致性测试（R5 硬约束：PHP/Rust 行为对比）
    // ====================================================================

    #[test]
    fn test_php_consistency_accessor_cache_asymmetric_invalidation() {
        // PHP 行为：setAttr("status", ...) 不失效 status_text 缓存
        // 来源：Attribute.php 第 394 行 unset($this->get[$name]) 中 $name 是被 set 的字段名
        let mut model = AccessorTestModel::new().with_data("status", json!(1));

        // 触发 status_text 访问器，缓存结果
        let v1 = model.get_attr("status_text");
        assert_eq!(v1, json!("启用"));

        // 修改 status 字段
        model.set_attr("status", json!(0), None);

        // 再次读取 status_text：缓存命中，仍是旧值
        let v2 = model.get_attr("status_text");
        assert_eq!(
            v2,
            json!("启用"),
            "PHP bug 复刻：status_text 缓存未失效，仍返回旧值"
        );
    }

    #[test]
    fn test_php_consistency_mutator_skip_with_data_modification() {
        // PHP 行为：修改器返回 null + 已修改 data → 提前返回
        // 来源：Attribute.php 第 379-381 行
        let mut model = AccessorTestModel::new();
        model.set_attr("special_attr", json!("X"), None);

        // 验证 PHP 行为：
        // 1. special_attr 未被写入
        assert!(
            !model.data.contains_key("special_attr"),
            "special_attr 应被跳过"
        );
        // 2. 修改器内部写入的 field_a / field_b 存在
        assert_eq!(model.data.get("field_a"), Some(&json!("A")));
        assert_eq!(model.data.get("field_b"), Some(&json!("B")));
    }

    #[test]
    fn test_php_consistency_mutator_receives_merged_data() {
        // PHP 行为：setXxxAttr($value, array_merge($this->data, $data))
        // 来源：Attribute.php 第 377 行
        let mut model = AccessorTestModel::new();
        model.data.insert("field_a".to_string(), json!(1));

        // 批量 setAttrs 时，field_b 修改器能读到 batch 中的 field_a
        let mut batch = HashMap::new();
        batch.insert("field_a".to_string(), json!(100));
        batch.insert("field_b".to_string(), json!(2));
        model.set_attrs(&batch);

        // merged_data 中 field_a=100（batch 覆盖 model.data）
        // field_b 修改器返回 "100_2"
        assert_eq!(
            model.data.get("field_b"),
            Some(&json!("100_2")),
            "修改器应使用 merged_data 中的 field_a=100"
        );
    }

    #[test]
    fn test_php_consistency_append_field_without_accessor_returns_null() {
        // PHP 行为：$append 字段无对应访问器 → 序列化输出 null
        // 来源：Conversion.php 第 280-296 行 + Attribute.php 第 525 行
        let mut model = AccessorTestModel::new();
        let v = model.get_attr("nonexistent_append_field");
        assert!(v.is_null(), "PHP 行为复刻：$append 字段无访问器应返回 null");
    }

    #[test]
    fn test_php_consistency_isset_triggers_accessor() {
        // PHP 行为：__isset 触发访问器执行
        // 来源：Model.php 第 977-980 行
        let mut model = AccessorTestModel::new().with_data("status", json!(1));

        // has_attr 应触发访问器，并缓存结果
        assert!(model.has_attr("status_text"));

        // 验证缓存：再次 get_attr 应命中缓存（同值）
        let v = model.get_attr("status_text");
        assert_eq!(v, json!("启用"));
    }

    #[test]
    fn test_php_consistency_accessor_overrides_raw_value() {
        // PHP 行为：访问器优先于原始值
        // 来源：Attribute.php 第 520-528 行
        // 真实字段 contract_price 的访问器把 null → 0.0
        let mut model = AccessorTestModel::new().with_data("contract_price", Value::Null);

        // 原始值是 Null
        assert_eq!(model.get_data("contract_price"), Some(&Value::Null));

        // 访问器返回 0.0（覆盖原始 Null）
        assert_eq!(model.get_attr("contract_price"), json!(0.0));
    }

    #[test]
    fn test_php_consistency_set_attrs_preserves_batch_context() {
        // PHP 行为：setAttrs 中每个字段都能感知完整批量数据
        // 来源：Attribute.php 第 351-357 行
        let mut model = AccessorTestModel::new();

        let mut batch = HashMap::new();
        batch.insert("field_a".to_string(), json!(50));
        batch.insert("field_b".to_string(), json!(99));

        model.set_attrs(&batch);

        // field_b 修改器读到 merged_data 中 field_a=50
        assert_eq!(model.data.get("field_b"), Some(&json!("50_99")));
        // field_a 原样写入（无修改器）
        assert_eq!(model.data.get("field_a"), Some(&json!(50)));
    }

    // ====================================================================
    // Append 字段系统测试
    // ====================================================================

    /// 测试用模型：实现完整 BaseModel + Accessor + Appendable
    ///
    /// 持有 `data: HashMap` + `get_cache: HashMap` + `append_state: AppendState`
    /// 模拟 PHP `$this->data` + `$this->get` + `$this->append` 三大实例状态。
    struct AppendableTestModel {
        data: HashMap<String, Value>,
        get_cache: HashMap<String, Value>,
        append_state: AppendState,
    }

    impl AppendableTestModel {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
                get_cache: HashMap::new(),
                append_state: AppendState::new(),
            }
        }

        fn with_data(mut self, key: &str, value: Value) -> Self {
            self.data.insert(key.to_string(), value);
            self
        }
    }

    impl Model for AppendableTestModel {
        type PrimaryKey = i64;

        fn table_name() -> &'static str {
            "test_appendable"
        }

        fn pk_name() -> &'static str {
            "id"
        }

        fn pk(&self) -> Self::PrimaryKey {
            self.data.get("id").and_then(|v| v.as_i64()).unwrap_or(0)
        }

        fn set_pk(&mut self, pk: Self::PrimaryKey) {
            self.data.insert("id".to_string(), json!(pk));
        }
    }

    impl ModelExt for AppendableTestModel {
        fn columns() -> Vec<&'static str> {
            vec![
                "id",
                "status",
                "name",
                "password",
                "add_time",
                "sales_initial",
                "sales_actual",
            ]
        }

        fn fillable() -> Vec<&'static str> {
            vec![
                "status",
                "name",
                "password",
                "add_time",
                "sales_initial",
                "sales_actual",
            ]
        }

        fn guarded() -> Vec<&'static str> {
            vec!["id"]
        }

        fn hidden() -> Vec<&'static str> {
            // password 在 hidden 中（对齐 PHP $hidden）
            vec!["password"]
        }

        fn get_column_value(&self, column: &str) -> Option<OrmValue> {
            match column {
                "id" => self
                    .data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .map(OrmValue::I64),
                "status" => self
                    .data
                    .get("status")
                    .and_then(|v| v.as_i64())
                    .map(|i| OrmValue::I32(i as i32)),
                "name" => self
                    .data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| OrmValue::String(s.to_string())),
                "password" => self
                    .data
                    .get("password")
                    .and_then(|v| v.as_str())
                    .map(|s| OrmValue::String(s.to_string())),
                "add_time" => self
                    .data
                    .get("add_time")
                    .and_then(|v| v.as_i64())
                    .map(OrmValue::I64),
                "sales_initial" => self
                    .data
                    .get("sales_initial")
                    .and_then(|v| v.as_i64())
                    .map(OrmValue::I64),
                "sales_actual" => self
                    .data
                    .get("sales_actual")
                    .and_then(|v| v.as_i64())
                    .map(OrmValue::I64),
                _ => None,
            }
        }

        fn from_value(&mut self, _map: HashMap<String, OrmValue>) {}
    }

    impl RelationLoader for AppendableTestModel {
        fn get_relation(&self, _name: &str) -> Option<&OrmValue> {
            None
        }
        fn set_relation_data(&mut self, _name: &str, _data: OrmValue) {}
        fn get_relation_fk_value(&self, _fk_name: &str) -> String {
            String::new()
        }
    }

    impl BaseModel for AppendableTestModel {
        fn append() -> Vec<&'static str> {
            // 静态 append：status_text（有访问器）+ no_accessor_field（无访问器）
            vec!["status_text", "no_accessor_field"]
        }
        // get_appended_value 默认返回 None
        // BaseModel::to_json_with_append 修正后：None → Value::Null
    }

    impl Accessor for AppendableTestModel {
        fn data_map(&self) -> &HashMap<String, Value> {
            &self.data
        }

        fn data_map_mut(&mut self) -> &mut HashMap<String, Value> {
            &mut self.data
        }

        fn accessor_cache(&self) -> &HashMap<String, Value> {
            &self.get_cache
        }

        fn accessor_cache_mut(&mut self) -> &mut HashMap<String, Value> {
            &mut self.get_cache
        }

        /// 模拟 PHP 访问器派发：
        /// - status_text：基于 status 字段返回中文文案（GradeOrder/Order 模式）
        /// - stat_day：基于 add_time 时间戳格式化（ArtSave/UploadFile 模式）
        /// - product_sales：sales_initial + sales_actual 求和（Product 模式）
        fn accessor_for(&self, field: &str, _value: Option<&Value>) -> Value {
            match field {
                "status_text" => {
                    let status = self
                        .data
                        .get("status")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-1);
                    json!(match status {
                        0 => "禁用",
                        1 => "启用",
                        _ => "未知",
                    })
                }
                "stat_day" => {
                    // PHP getStatDayAttr($value, $data)：基于 $data['add_time'] 格式化
                    let timestamp = self
                        .data
                        .get("add_time")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    json!(format!("day_{}", timestamp / 86400))
                }
                "product_sales" => {
                    // PHP getProductSalesAttr：sales_initial + sales_actual
                    let initial = self
                        .data
                        .get("sales_initial")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let actual = self
                        .data
                        .get("sales_actual")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    json!(initial + actual)
                }
                _ => Value::Null,
            }
        }
    }

    impl Appendable for AppendableTestModel {
        fn append_state(&self) -> &AppendState {
            &self.append_state
        }

        fn append_state_mut(&mut self) -> &mut AppendState {
            &mut self.append_state
        }
    }

    // -------------------- Append 基本功能测试 --------------------

    #[test]
    fn test_base_model_to_json_with_append_outputs_null_for_no_accessor() {
        // PHP 行为：append 字段无访问器时仍输出，值为 null
        // 对齐 Conversion.php 第 292 行 $item[$name] = $this->getAttr($name)
        let model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        let json = model.to_json_with_append();
        // AppendableTestModel 未重写 get_appended_value，默认返回 None
        // 修正后：None → Value::Null
        assert_eq!(
            json["status_text"],
            Value::Null,
            "无访问器 append 字段应输出 null"
        );
        assert_eq!(
            json["no_accessor_field"],
            Value::Null,
            "无访问器 append 字段应输出 null"
        );
    }

    #[test]
    fn test_appendable_to_json_with_append_cached_uses_accessor() {
        // PHP 行为：append 字段走 getAttr → 访问器
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        let json = model.to_json_with_append_cached();
        // status_text 走 accessor_for → "启用"
        assert_eq!(json["status_text"], "启用");
        // no_accessor_field 走 accessor_for 默认分支 → Value::Null
        assert_eq!(json["no_accessor_field"], Value::Null);
    }

    #[test]
    fn test_appendable_caches_accessor_result() {
        // PHP 行为：getAttr 缓存结果，多次调用只执行一次访问器
        // 同名字段修改不失效缓存（PHP bug 复刻）
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        let json1 = model.to_json_with_append_cached();
        assert_eq!(json1["status_text"], "启用");

        // 修改 status 不失效 status_text 缓存（PHP bug 复刻）
        model.data.insert("status".to_string(), json!(0));
        let json2 = model.to_json_with_append_cached();
        assert_eq!(
            json2["status_text"], "启用",
            "缓存命中，访问器不重新执行（PHP bug 复刻）"
        );
    }

    #[test]
    fn test_append_field_bypasses_hidden_filter() {
        // PHP bug 复刻：append 字段绕过 hidden 过滤
        // 对齐 Conversion.php 第 291-296 行 appendAttrToArray 直接赋值
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1))
            .with_data("password", json!("secret"));
        let json = model.to_json_with_append_cached();
        // password 在 hidden 中，被过滤
        assert!(json.get("password").is_none(), "password 应被 hidden 过滤");
        // status_text 是 append 字段，绕过 hidden
        assert_eq!(json["status_text"], "启用");
    }

    #[test]
    fn test_append_dyn_overrides_static_append() {
        // PHP 行为：$model->append($fields) 默认覆盖静态 $append
        // 对齐 Conversion.php 第 90-94 行
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        // 静态 append: ["status_text", "no_accessor_field"]
        // 动态覆盖为 ["dynamic_field"]
        model.append_dyn(vec!["dynamic_field".to_string()]);
        let json = model.to_json_with_append_cached();
        // status_text 不再输出（被覆盖）
        assert!(
            json.get("status_text").is_none(),
            "status_text 应被动态 append 覆盖"
        );
        // no_accessor_field 不再输出（被覆盖）
        assert!(
            json.get("no_accessor_field").is_none(),
            "no_accessor_field 应被动态 append 覆盖"
        );
        // dynamic_field 输出（走 accessor_for 默认分支 → null）
        assert_eq!(json["dynamic_field"], Value::Null);
    }

    #[test]
    fn test_append_merge_combines_with_static() {
        // PHP 行为：$model->append($fields, true) 合并到静态 $append
        // 对齐 Conversion.php 第 91-93 行
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        model.append_merge(vec!["extra_field".to_string()]);
        let json = model.to_json_with_append_cached();
        // 静态字段保留
        assert_eq!(json["status_text"], "启用");
        assert_eq!(json["no_accessor_field"], Value::Null);
        // 合并的字段输出
        assert_eq!(json["extra_field"], Value::Null);
    }

    #[test]
    fn test_append_dyn_returns_self_for_chaining() {
        // PHP 行为：append() 返回 $this，支持链式
        // 对齐 Conversion.php 第 96 行
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        // 链式调用
        model
            .append_merge(vec!["field1".to_string()])
            .append_merge(vec!["field2".to_string()]);
        let json = model.to_json_with_append_cached();
        assert!(json.get("field1").is_some(), "链式 append_merge 应生效");
        assert!(json.get("field2").is_some(), "链式 append_merge 应生效");
        // 静态字段也保留
        assert_eq!(json["status_text"], "启用");
    }

    #[test]
    fn test_effective_append_priority() {
        // 动态 append 优先于静态
        let model = AppendableTestModel::new();
        // 默认使用静态
        assert_eq!(
            model.effective_append(),
            vec!["status_text".to_string(), "no_accessor_field".to_string()]
        );

        let mut model = model;
        model.append_dyn(vec!["override".to_string()]);
        assert_eq!(model.effective_append(), vec!["override".to_string()]);
    }

    #[test]
    fn test_append_state_replace_and_merge() {
        // 直接测试 AppendState 行为
        let mut state = AppendState::new();
        assert!(state.dynamic_fields().is_none(), "初始状态无动态字段");

        state.replace(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            state.dynamic_fields().unwrap(),
            &vec!["a".to_string(), "b".to_string()]
        );

        // merge 去重
        state.merge(vec!["b".to_string(), "c".to_string()]);
        assert_eq!(
            state.dynamic_fields().unwrap(),
            &vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    // -------------------- Append PHP 一致性测试（R5 硬约束）--------------------

    #[test]
    fn test_php_consistency_status_text_pattern() {
        // PHP 模式：*_text 后缀访问器，基于状态码返回中文文案
        // 代表模型：GradeOrder, Order
        let test_cases = vec![(0i64, "禁用"), (1, "启用"), (99, "未知")];
        for (status, expected) in test_cases {
            let mut model = AppendableTestModel::new()
                .with_data("id", json!(1))
                .with_data("status", json!(status));
            let json = model.to_json_with_append_cached();
            assert_eq!(
                json["status_text"], expected,
                "status={} 应返回 '{}'",
                status, expected
            );
        }
    }

    #[test]
    fn test_php_consistency_stat_day_pattern() {
        // PHP 模式：时间戳格式化为日期
        // 代表模型：ArtSave, UploadFile
        // PHP getStatDayAttr($value, $data)：$value 为 stat_day 字段值（不存在→null），
        // $data['add_time'] 为时间戳，格式化为 "Y-m-d"
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("add_time", json!(1690000000));
        // 动态追加 stat_day（验证动态 append + 访问器派发）
        model.append_merge(vec!["stat_day".to_string()]);
        let json = model.to_json_with_append_cached();
        // 1690000000 / 86400 = 19560（天）
        assert_eq!(json["stat_day"], "day_19560");
    }

    #[test]
    fn test_php_consistency_product_sales_pattern() {
        // PHP 模式：计算字段（多字段求和）
        // 代表模型：Product::getProductSalesAttr = sales_initial + sales_actual
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("sales_initial", json!(100))
            .with_data("sales_actual", json!(50));
        // 动态追加 product_sales
        model.append_merge(vec!["product_sales".to_string()]);
        let json = model.to_json_with_append_cached();
        assert_eq!(json["product_sales"], 150);
    }

    #[test]
    fn test_php_consistency_append_always_outputs_even_without_accessor() {
        // PHP 行为：append 字段无访问器时，toArray 仍输出该字段，值为 null
        // 对齐 Conversion.php 第 292 行 $item[$name] = $this->getAttr($name)
        // getAttr 无访问器且字段不在 $data 中 → 返回 null
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        // 静态 append 包含 no_accessor_field（无访问器）
        let json = model.to_json_with_append_cached();
        // no_accessor_field 走 accessor_for 默认分支 → Value::Null
        assert_eq!(
            json["no_accessor_field"],
            Value::Null,
            "PHP 行为复刻：append 字段无访问器应输出 null"
        );
    }

    #[test]
    fn test_php_consistency_append_overrides_hidden() {
        // PHP bug 复刻：append 字段绕过 hidden 过滤
        // 对齐 Conversion.php 第 291-296 行
        // 即使 append 字段名在 hidden 列表中，仍会输出
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1))
            .with_data("password", json!("secret"));
        // password 在 hidden 中
        // status_text 是 append 字段，绕过 hidden
        let json = model.to_json_with_append_cached();
        assert!(json.get("password").is_none(), "password 应被 hidden 过滤");
        assert_eq!(json["status_text"], "启用", "append 字段应绕过 hidden");
    }

    #[test]
    fn test_php_consistency_dynamic_append_overrides_static() {
        // PHP 行为：$model->append($fields) 默认覆盖静态 $append
        // 对齐 Conversion.php 第 90-94 行
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        // 动态覆盖
        model.append_dyn(vec!["stat_day".to_string()]);
        let json = model.to_json_with_append_cached();
        // 静态 append 字段不再输出
        assert!(
            json.get("status_text").is_none(),
            "动态 append 应覆盖静态，status_text 不应输出"
        );
        // 动态字段输出
        assert!(json.get("stat_day").is_some(), "动态 append 字段应输出");
    }

    #[test]
    fn test_php_consistency_append_method_returns_this_for_chaining() {
        // PHP 行为：append() 返回 $this，支持链式调用
        // 对齐 Conversion.php 第 96 行 return $this
        let mut model = AppendableTestModel::new()
            .with_data("id", json!(1))
            .with_data("status", json!(1));
        // 链式调用：append_merge → append_merge
        model
            .append_merge(vec!["stat_day".to_string()])
            .append_merge(vec!["product_sales".to_string()]);
        let json = model.to_json_with_append_cached();
        // 三个 append 字段都应输出（静态 2 个 + 动态合并 2 个）
        assert_eq!(json["status_text"], "启用");
        assert!(json.get("stat_day").is_some());
        assert!(json.get("product_sales").is_some());
    }
}
