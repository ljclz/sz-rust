//! 插件自动加载（Phase 10.1）
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `helper.php` 中的 `spl_autoload_register` 回调：
//!
//! ```php
//! // vendor/zzstudio/think-addons/src/helper.php
//! spl_autoload_register(function ($class) {
//!     $class = ltrim($class, '\\');
//!     $dir = app()->getRootPath();
//!     $namespace = 'addons';
//!     if (strpos($class, $namespace) === 0) {
//!         $class = substr($class, strlen($namespace));
//!         $path = '';
//!         if (($pos = strripos($class, '\\')) !== false) {
//!             $path = str_replace('\\', '/', substr($class, 0, $pos)) . '/';
//!             $class = substr($class, $pos + 1);
//!         }
//!         $path .= str_replace('_', '/', $class) . '.php';
//!         $dir .= $namespace . $path;
//!         if (file_exists($dir)) {
//!             include $dir;
//!             return true;
//!         }
//!         return false;
//!     }
//!     return false;
//! });
//! ```
//!
//! ## 类名解析规则（对齐 PHP `get_addons_class`）
//!
//! | PHP 类名 | 文件路径 |
//! |---------|---------|
//! | `addons\operate\Plugin` | `addons/operate/Plugin.php` |
//! | `addons\operate\controller\Order` | `addons/operate/controller/Order.php` |
//! | `addons\operate\controller\admin\Order` | `addons/operate/controller/admin/Order.php` |
//! | `addons\operate\controller\admin_Order` | `addons/operate/controller/admin/Order.php`（下划线转分隔符） |
//!
//! ## 多级控制器点号分隔（对齐 PHP `get_addons_class` 中 `.` 处理）
//!
//! ```php
//! // helper.php
//! if (strpos($class, '.') !== false) {
//!     $array = explode('.', $class);
//!     $class = array_pop($array);
//!     $class = Str::studly($class);
//!     $class = implode('\\', $array) . '\\' . $class;
//! }
//! ```

use std::path::{Path, PathBuf};

use crate::error::{AddonLoaderError, AddonLoaderResult};

/// 插件自动加载器（对齐 PHP `spl_autoload_register` 回调）
///
/// ## 设计
///
/// - 持有插件根目录（对齐 PHP `app()->getRootPath() . 'addons/'`）
/// - 提供 `resolve(class)` 方法：类名 → 文件路径（不实际加载文件）
/// - 支持 PSR-0 风格的下划线转目录分隔符
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonAutoload {
    /// 插件根目录（对齐 PHP `{rootPath}/addons/`）
    addons_path: PathBuf,
}

impl AddonAutoload {
    /// 创建自动加载器
    ///
    /// - `addons_path`：插件根目录，对齐 PHP `getAddonsPath()` 返回的 `{rootPath}/addons/`
    pub fn new(addons_path: impl Into<PathBuf>) -> Self {
        Self {
            addons_path: addons_path.into(),
        }
    }

    /// 获取插件根目录
    pub fn addons_path(&self) -> &Path {
        &self.addons_path
    }

    /// 解析类名到文件路径（对齐 PHP `spl_autoload_register` 回调）
    ///
    /// ## 规则
    ///
    /// 1. 类名必须以 `addons\` 开头（对齐 PHP `strpos($class, $namespace) === 0`）
    /// 2. 命名空间分隔符 `\` 转换为目录分隔符 `/`（对齐 `str_replace('\\', '/', ...)`）
    /// 3. 类名中的下划线 `_` 转换为目录分隔符（对齐 `str_replace('_', '/', $class)`）
    /// 4. 拼接 `.php` 后缀
    /// 5. 检查文件是否存在
    ///
    /// ## 参数
    ///
    /// - `class`：完整类名（如 `addons\operate\Plugin`）
    ///
    /// ## 返回
    ///
    /// - `Ok(Some(path))`：类映射到文件且文件存在
    /// - `Ok(None)`：类名不属于 `addons\` 命名空间，或文件不存在（让其他 autoloader 处理）
    /// - `Err(_)`：路径解析失败
    pub fn resolve(&self, class: &str) -> AddonLoaderResult<Option<PathBuf>> {
        let class = class.trim_start_matches('\\');

        // 对齐 PHP `strpos($class, $namespace) === 0`
        if !class.starts_with("addons\\") {
            return Ok(None);
        }

        // 对齐 PHP `substr($class, strlen($namespace))`
        // 注意：PHP 中 namespace = 'addons'（不带 \），substr 后得到 '\operate\Plugin'
        // Rust 侧我们直接 strip "addons\" 前缀
        let stripped = &class["addons\\".len()..];

        // 对齐 PHP `strripos($class, '\\')` 找到最后一个命名空间分隔符
        let (path_part, class_part) = if let Some(pos) = stripped.rfind('\\') {
            // 有命名空间前缀：path = 命名空间部分（\ → /），class = 末段
            let path = stripped[..pos].replace('\\', "/");
            let class_name = &stripped[pos + 1..];
            (format!("{}/", path), class_name.to_string())
        } else {
            // 无命名空间前缀
            (String::new(), stripped.to_string())
        };

        // 对齐 PHP `str_replace('_', '/', $class) . '.php'`
        // 下划线转目录分隔符（PSR-0 风格）
        let class_file = format!("{}.php", class_part.replace('_', "/"));

        // 对齐 PHP `$dir .= $namespace . $path`
        let file_path = self
            .addons_path
            .join(format!("{}{}", path_part, class_file));

        if file_path.exists() {
            Ok(Some(file_path))
        } else {
            Ok(None)
        }
    }

    /// 解析控制器类名（对齐 PHP `get_addons_class($name, 'controller', $class)`）
    ///
    /// ## 多级控制器点号分隔
    ///
    /// 对齐 PHP `get_addons_class` 中 `.` 处理：
    ///
    /// ```php
    /// if (strpos($class, '.') !== false) {
    ///     $array = explode('.', $class);
    ///     $class = array_pop($array);
    ///     $class = Str::studly($class);
    ///     $class = implode('\\', $array) . '\\' . $class;
    /// }
    /// ```
    ///
    /// ## 示例
    ///
    /// - `resolve_controller("operate", "Order")` → `addons/operate/controller/Order.php`
    /// - `resolve_controller("operate", "admin.Order")` → `addons/operate/controller/admin/Order.php`
    /// - `resolve_controller("operate", "admin.sub.Order")` → `addons/operate/controller/admin/sub/Order.php`
    pub fn resolve_controller(
        &self,
        addon: &str,
        controller: &str,
    ) -> AddonLoaderResult<Option<PathBuf>> {
        let controller_class = parse_dotted_controller(controller);
        let full_class = format!("addons\\{}\\controller\\{}", addon, controller_class);
        self.resolve(&full_class)
    }

    /// 解析插件入口类（对齐 PHP `get_addons_class($name)` 默认 type='hook'）
    ///
    /// ## 示例
    ///
    /// - `resolve_plugin("operate")` → `addons/operate/Plugin.php`
    pub fn resolve_plugin(&self, addon: &str) -> AddonLoaderResult<Option<PathBuf>> {
        let full_class = format!("addons\\{}\\Plugin", addon);
        self.resolve(&full_class)
    }

    /// 强制解析类名（对齐 PHP `get_addons_class` 返回字符串而非 bool）
    ///
    /// 与 `resolve` 的区别：不检查文件是否存在，直接返回路径
    pub fn resolve_strict(&self, class: &str) -> AddonLoaderResult<PathBuf> {
        let class = class.trim_start_matches('\\');

        if !class.starts_with("addons\\") {
            return Err(AddonLoaderError::AutoloadMiss {
                class: class.to_string(),
            });
        }

        let stripped = &class["addons\\".len()..];
        let (path_part, class_part) = if let Some(pos) = stripped.rfind('\\') {
            let path = stripped[..pos].replace('\\', "/");
            let class_name = &stripped[pos + 1..];
            (format!("{}/", path), class_name.to_string())
        } else {
            (String::new(), stripped.to_string())
        };

        let class_file = format!("{}.php", class_part.replace('_', "/"));
        let file_path = self
            .addons_path
            .join(format!("{}{}", path_part, class_file));
        Ok(file_path)
    }
}

/// 解析多级控制器点号分隔（对齐 PHP `get_addons_class` 中 `.` 处理）
///
/// ## PHP 对齐
///
/// ```php
/// if (strpos($class, '.') !== false) {
///     $array = explode('.', $class);
///     $class = array_pop($array);
///     $class = Str::studly($class);
///     $class = implode('\\', $array) . '\\' . $class;
/// }
/// ```
///
/// ## 示例
///
/// - `Order` → `Order`
/// - `admin.Order` → `admin\Order`
/// - `admin.sub.Order` → `admin\sub\Order`
fn parse_dotted_controller(controller: &str) -> String {
    if !controller.contains('.') {
        return controller.to_string();
    }

    let mut parts: Vec<&str> = controller.split('.').collect();
    if parts.len() == 1 {
        return controller.to_string();
    }

    // 末段转大驼峰（对齐 PHP `Str::studly`）
    let last = parts.pop().unwrap();
    let last_studly = studly_case(last);

    // 前段保持原样（PHP 不转换），用 \ 拼回
    parts.push(&last_studly);
    parts.join("\\")
}

/// 下划线转大驼峰（对齐 PHP `Str::studly`）
///
/// ## 示例
///
/// - `order` → `Order`
/// - `user_order` → `UserOrder`
/// - `Order` → `Order`
fn studly_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 创建临时插件目录结构
    fn make_test_addons_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let addons_path = tmp.path().join("addons");

        // operate 插件
        let operate_dir = addons_path.join("operate");
        fs::create_dir_all(&operate_dir).expect("create operate dir");
        fs::write(operate_dir.join("Plugin.php"), "<?php // stub").expect("write Plugin.php");

        // operate/controller 目录
        let controller_dir = operate_dir.join("controller");
        fs::create_dir_all(&controller_dir).expect("create controller dir");
        fs::write(controller_dir.join("Order.php"), "<?php // stub").expect("write Order.php");

        // operate/controller/admin 多级目录
        let admin_dir = controller_dir.join("admin");
        fs::create_dir_all(&admin_dir).expect("create admin dir");
        fs::write(admin_dir.join("Order.php"), "<?php // stub").expect("write admin/Order.php");

        // operate/model 目录
        let model_dir = operate_dir.join("model");
        fs::create_dir_all(&model_dir).expect("create model dir");
        fs::write(model_dir.join("Customer.php"), "<?php // stub").expect("write Customer.php");

        // 下划线命名测试：admin_Order 类映射到 admin/Order.php
        // 注意：PSR-0 下划线转换在类名末段生效，所以 admin\Order 类的文件是 admin/Order.php
        // 而 admin_Order 类（无命名空间分隔）的文件也是 admin/Order.php

        tmp
    }

    #[test]
    fn test_new_autoload() {
        let loader = AddonAutoload::new("/addons");
        assert_eq!(loader.addons_path(), Path::new("/addons"));
    }

    #[test]
    fn test_resolve_plugin_class() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve("addons\\operate\\Plugin").unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, addons_path.join("operate").join("Plugin.php"));
    }

    #[test]
    fn test_resolve_controller_class() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader
            .resolve("addons\\operate\\controller\\Order")
            .unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(
            path,
            addons_path
                .join("operate")
                .join("controller")
                .join("Order.php")
        );
    }

    #[test]
    fn test_resolve_multilevel_controller_class() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader
            .resolve("addons\\operate\\controller\\admin\\Order")
            .unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(
            path,
            addons_path
                .join("operate")
                .join("controller")
                .join("admin")
                .join("Order.php")
        );
    }

    #[test]
    fn test_resolve_model_class() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve("addons\\operate\\model\\Customer").unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(
            path,
            addons_path
                .join("operate")
                .join("model")
                .join("Customer.php")
        );
    }

    #[test]
    fn test_resolve_non_addons_namespace_returns_none() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve("app\\controller\\Home").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_nonexistent_file_returns_none() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve("addons\\operate\\NonExistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_leading_backslash_stripped() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve("\\addons\\operate\\Plugin").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_controller_helper() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve_controller("operate", "Order").unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(
            path,
            addons_path
                .join("operate")
                .join("controller")
                .join("Order.php")
        );
    }

    #[test]
    fn test_resolve_controller_multilevel_dotted() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve_controller("operate", "admin.Order").unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(
            path,
            addons_path
                .join("operate")
                .join("controller")
                .join("admin")
                .join("Order.php")
        );
    }

    #[test]
    fn test_resolve_controller_three_levels_dotted() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        // 创建 admin/sub/Order.php
        let sub_dir = addons_path
            .join("operate")
            .join("controller")
            .join("admin")
            .join("sub");
        fs::create_dir_all(&sub_dir).expect("create sub dir");
        fs::write(sub_dir.join("Order.php"), "<?php // stub").expect("write sub/Order.php");

        let loader = AddonAutoload::new(&addons_path);
        let result = loader
            .resolve_controller("operate", "admin.sub.Order")
            .unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("admin"));
        assert!(path.to_string_lossy().contains("sub"));
        assert!(path.to_string_lossy().ends_with("Order.php"));
    }

    #[test]
    fn test_resolve_plugin_helper() {
        let tmp = make_test_addons_dir();
        let addons_path = tmp.path().join("addons");
        let loader = AddonAutoload::new(&addons_path);

        let result = loader.resolve_plugin("operate").unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, addons_path.join("operate").join("Plugin.php"));
    }

    #[test]
    fn test_resolve_strict_addons_class() {
        let loader = AddonAutoload::new("/addons");
        let path = loader.resolve_strict("addons\\operate\\Plugin").unwrap();
        assert_eq!(path, PathBuf::from("/addons/operate/Plugin.php"));
    }

    #[test]
    fn test_resolve_strict_controller_class() {
        let loader = AddonAutoload::new("/addons");
        let path = loader
            .resolve_strict("addons\\operate\\controller\\admin\\Order")
            .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/addons/operate/controller/admin/Order.php")
        );
    }

    #[test]
    fn test_resolve_strict_non_addons_returns_error() {
        let loader = AddonAutoload::new("/addons");
        let result = loader.resolve_strict("app\\Home");
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::AutoloadMiss { class } => {
                assert_eq!(class, "app\\Home");
            }
            other => panic!("expected AutoloadMiss, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_dotted_controller_simple() {
        assert_eq!(parse_dotted_controller("Order"), "Order");
    }

    #[test]
    fn test_parse_dotted_controller_two_levels() {
        assert_eq!(parse_dotted_controller("admin.Order"), "admin\\Order");
    }

    #[test]
    fn test_parse_dotted_controller_three_levels() {
        assert_eq!(
            parse_dotted_controller("admin.sub.Order"),
            "admin\\sub\\Order"
        );
    }

    #[test]
    fn test_parse_dotted_controller_studly_conversion() {
        // 末段应该转大驼峰（对齐 PHP Str::studly）
        assert_eq!(
            parse_dotted_controller("admin.user_order"),
            "admin\\UserOrder"
        );
    }

    #[test]
    fn test_parse_dotted_controller_no_dot_passthrough() {
        assert_eq!(parse_dotted_controller("user_order"), "user_order");
        // 注意：不带点号时不做 studly 转换（PHP 原始行为）
    }

    #[test]
    fn test_studly_case_basic() {
        assert_eq!(studly_case("order"), "Order");
    }

    #[test]
    fn test_studly_case_with_underscore() {
        assert_eq!(studly_case("user_order"), "UserOrder");
    }

    #[test]
    fn test_studly_case_already_studly() {
        assert_eq!(studly_case("Order"), "Order");
    }

    #[test]
    fn test_studly_case_empty() {
        assert_eq!(studly_case(""), "");
    }

    #[test]
    fn test_studly_case_multiple_underscores() {
        assert_eq!(studly_case("a_b_c"), "ABC");
    }

    #[test]
    fn test_clone_eq() {
        let l1 = AddonAutoload::new("/addons");
        let l2 = l1.clone();
        assert_eq!(l1, l2);
    }

    #[test]
    fn test_resolve_with_trailing_backslash_in_class() {
        // PHP 行为：class 末尾不会有 \，但测试健壮性
        let loader = AddonAutoload::new("/addons");
        let result = loader.resolve_strict("addons\\operate\\Plugin\\").unwrap();
        // 末尾 \ 会被 rfind 处理，path_part = "operate/Plugin/"，class_part = ""
        assert!(result.to_string_lossy().ends_with(".php"));
    }
}
