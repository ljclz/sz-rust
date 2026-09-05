// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 验证场景 — 对齐 PHP `think\Validate` 的场景机制
//!
//! 本模块实现 PHP `think\Validate` 的场景（scene）机制，
//! 支持两种场景定义方式：
//!
//! 1. **数组形式**：通过 [`Validate::register_scene`] 注册场景字段列表
//!    （对齐 PHP `$scene = ['login' => ['email']]`）
//! 2. **回调形式**：通过 [`Validate::register_scene_callback`] 注册场景回调
//!    （对齐 PHP `protected function sceneLogin() { ... }` 方法）
//!
//! ## PHP 对齐
//!
//! ### 场景应用流程（对齐 PHP `getScene`，第 1659-1669 行）
//!
//! 1. 重置 `only`/`append`/`remove` 为空
//! 2. 如果存在 `scene{Name}` 回调，调用回调（回调内部可调用 `only_mut`/
//!    `append_mut`/`remove_mut` 修改场景状态）
//! 3. 否则如果 `scene[{name}]` 数组存在，设置 `only = scene[{name}]`
//!
//! ### `hasScene` 判断（对齐 PHP，第 368-371 行）
//!
//! `isset($this->scene[$name]) || method_exists($this, 'scene' . $name)`
//!
//! Rust 实现：`scene.contains_key(name) || scene_callbacks.contains_key(name)`
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_core::validate::Validate;
//! use serde_json::json;
//!
//! let mut v = Validate::new()
//!     .rule("name", "require|max:25")
//!     .rule("email", "require|email")
//!     .rule("age", "require|between:1,120")
//!     .register_scene("login", vec!["email".to_string()])
//!     .scene("login");
//!
//! // 仅验证 email 字段
//! let data = json!({{"email": "test@example.com"}});
//! assert!(v.check(&data).is_ok());
//! ```
//!
//! ## 回调形式示例
//!
//! ```ignore
//! use sz_rust_core::validate::Validate;
//! use std::sync::Arc;
//!
//! let mut v = Validate::new()
//!     .rule("name", "require|max:25")
//!     .rule("email", "require|email")
//!     .rule("age", "require|between:1,120")
//!     .register_scene_callback("register", Arc::new(|v| {
//!         v.only_mut(vec!["name".to_string(), "email".to_string()]);
//!         v.remove_mut("name", None);
//!     }))
//!     .scene("register");
//! ```
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\Validate.php`
//!   - 第 155 行：`protected $scene = []`
//!   - 第 119 行：`protected $currentScene`
//!   - 第 354-360 行：`scene(string $name)` 方法
//!   - 第 368-371 行：`hasScene(string $name)` 方法
//!   - 第 475-477 行：`check()` 中调用 `getScene()`
//!   - 第 1659-1669 行：`getScene(string $scene)` 方法

use std::sync::Arc;

use crate::validate::Validate;

/// 场景回调类型 — 对齐 PHP `scene{Name}` 方法
///
/// 签名：`Fn(&mut Validate) + Send + Sync`
///
/// 回调内部可调用 [`Validate::only_mut`]、[`Validate::append_mut`]、
/// [`Validate::remove_mut`] 修改场景状态，对齐 PHP `sceneXxx` 方法中
/// 调用 `$this->only(...)`、`$this->append(...)`、`$this->remove(...)`。
///
/// ## PHP 对齐
///
/// ```php
/// protected function sceneLogin()
/// {
///     return $this->only(['email']);
/// }
/// ```
///
/// Rust 等价：
///
/// ```ignore
/// use std::sync::Arc;
/// use sz_rust_core::validate::scene::SceneCallback;
///
/// let cb: SceneCallback = Arc::new(|v| {
///     v.only_mut(vec!["email".to_string()]);
/// });
/// ```
pub type SceneCallback = Arc<dyn Fn(&mut Validate) + Send + Sync>;
