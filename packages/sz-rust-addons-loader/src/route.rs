//! 插件路由解析
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think\addons\Route::execute($addon, $controller, $action)` 的 URL 解析逻辑：
//!
//! ```php
//! // vendor/zzstudio/think-addons/src/addons/Route.php
//! public static function execute($addon = null, $controller = null, $action = null)
//! {
//!     // 1. 参数校验
//!     if (empty($addon) || empty($controller) || empty($action)) {
//!         throw new HttpException(500, 'addon can not be empty');
//!     }
//!     // 2. 读取插件信息
//!     $info = get_addons_info($addon);
//!     if (!$info) {
//!         throw new HttpException(404, 'addon %s not found');
//!     }
//!     if (!$info['status']) {
//!         throw new HttpException(500, 'addon %s is disabled');
//!     }
//!     // 3. 解析控制器类
//!     $class = get_addons_class($addon, 'controller', $controller);
//!     // 4. 实例化并调用方法
//! }
//! ```
//!
//! ## 路由规则
//!
//! 对齐 PHP `Service::boot()` 注册的路由：
//!
//! ```php
//! $route->rule("addons/:addon/[:controller]/[:action]", $execute);
//! ```
//!
//! ## 多级控制器点号分隔
//!
//! 对齐 PHP `get_addons_class` 中 `.` 处理：
//!
//! - `admin.Order` → `admin\Order`（末段 studly 转大驼峰）
//! - `admin.sub.Order` → `admin\sub\Order`

use std::path::PathBuf;

use crate::autoload::AddonAutoload;
use crate::error::{AddonLoaderError, AddonLoaderResult};
use crate::registry::AddonRegistry;

/// 解析后的插件路由信息
///
/// 对齐 PHP `Route::execute($addon, $controller, $action)` 的参数三元组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonRoute {
    /// 插件名（对齐 PHP `$addon`）
    pub addon: String,
    /// 控制器名（原始形式，可能含点号；对齐 PHP `$controller`）
    pub controller: String,
    /// 操作名（对齐 PHP `$action`）
    pub action: String,
    /// 控制器类名（解析后，对齐 PHP `get_addons_class` 返回值）
    pub controller_class: String,
    /// 控制器文件路径
    pub controller_file: Option<PathBuf>,
}

impl AddonRoute {
    /// 创建路由信息
    pub fn new(
        addon: impl Into<String>,
        controller: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        let addon = addon.into();
        let controller = controller.into();
        let action = action.into();

        // 对齐 PHP `get_addons_class($addon, 'controller', $controller)` 返回的类名
        let controller_class = build_controller_class(&addon, &controller);

        Self {
            addon,
            controller,
            action,
            controller_class,
            controller_file: None,
        }
    }

    /// 获取控制器文件路径（若已解析）
    pub fn controller_file(&self) -> Option<&std::path::Path> {
        self.controller_file.as_deref()
    }
}

/// 解析插件路由（主入口）
///
/// 对齐 PHP `Route::execute($addon, $controller, $action)` 完整流程：
///
/// 1. 参数校验（对齐 PHP `empty($addon)` 等检查）
/// 2. 插件存在性检查（对齐 PHP `get_addons_info($addon)` 返回 false）
/// 3. 插件状态检查（对齐 PHP `!$info['status']` 抛 500）
/// 4. 控制器类解析（对齐 PHP `get_addons_class($addon, 'controller', $controller)`）
/// 5. 控制器文件存在性检查（对齐 PHP `class_exists($class)`）
///
/// ## 参数
///
/// - `url`：URL 路径（如 `/addons/operate/admin.Order/index`）
/// - `registry`：插件注册中心
/// - `autoload`：自动加载器
///
/// ## 错误
///
/// - `RouteParse`：URL 格式错误
/// - `AddonNotFound`：插件不存在（对齐 PHP 404）
/// - `AddonDisabled`：插件已禁用（对齐 PHP 500）
/// - `ControllerNotFound`：控制器类或文件不存在（对齐 PHP 404）
#[tracing::instrument(skip(registry, autoload))]
pub fn parse_route(
    url: &str,
    registry: &AddonRegistry,
    autoload: &AddonAutoload,
) -> AddonLoaderResult<AddonRoute> {
    // 1. URL 解析
    let (addon, controller, action) =
        parse_url_segments(url).ok_or_else(|| AddonLoaderError::RouteParse {
            url: url.to_string(),
            reason: "URL must be /addons/<addon>/<controller>/<action>".to_string(),
        })?;

    // 2. 参数校验（对齐 PHP `empty($addon) || empty($controller) || empty($action)`）
    if addon.is_empty() || controller.is_empty() || action.is_empty() {
        return Err(AddonLoaderError::RouteParse {
            url: url.to_string(),
            reason: "addon, controller, action cannot be empty".to_string(),
        });
    }

    // 3. 插件存在性检查（对齐 PHP `if (!$info)` → 404）
    let manifest = registry.get(&addon)?;

    // 4. 插件状态检查（对齐 PHP `if (!$info['status'])` → 500）
    if !manifest.is_enabled() {
        return Err(AddonLoaderError::AddonDisabled(addon));
    }

    // 5. 构建控制器类名
    let mut route = AddonRoute::new(addon.clone(), controller, action);

    // 6. 解析控制器文件路径（对齐 PHP `get_addons_class` + `class_exists` 检查）
    let file_path = autoload.resolve_controller(&addon, &route.controller)?;

    if file_path.is_none() {
        // 对齐 PHP `HttpException(404, 'addon controller %s not found')`
        return Err(AddonLoaderError::ControllerNotFound(
            route.controller.clone(),
        ));
    }

    route.controller_file = file_path;

    Ok(route)
}

/// 解析 URL 段（对齐 PHP 路由规则 `addons/:addon/[:controller]/[:action]`）
///
/// ## 支持的 URL 格式
///
/// - `/addons/operate/Order/index` → `("operate", "Order", "index")`
/// - `addons/operate/admin.Order/index` → `("operate", "admin.Order", "index")`
/// - `/addons/operate/Order` → `("operate", "Order", "")`（action 缺失）
/// - `/addons/operate` → `("operate", "", "")`（controller/action 缺失）
///
/// ## 返回
///
/// - `Some((addon, controller, action))`：URL 以 `addons/` 开头
/// - `None`：URL 不以 `addons/` 开头
fn parse_url_segments(url: &str) -> Option<(String, String, String)> {
    // 去除前导斜杠
    let url = url.trim_start_matches('/');

    // 必须以 addons/ 开头
    if !url.starts_with("addons/") {
        return None;
    }

    let rest = &url["addons/".len()..];

    // 拆分段
    let parts: Vec<&str> = rest.split('/').collect();

    let addon = parts.first().unwrap_or(&"").to_string();
    let controller = parts.get(1).unwrap_or(&"").to_string();
    let action = parts.get(2).unwrap_or(&"").to_string();

    Some((addon, controller, action))
}

/// 构建控制器完整类名（对齐 PHP `get_addons_class($name, 'controller', $class)`）
///
/// ## PHP 对齐
///
/// PHP `get_addons_class` 中 `.` 处理逻辑：
/// - 若 `$class` 含 `.`，按 `.` 拆分，末段 `Str::studly` 转大驼峰，再用 `\` 拼回
/// - 单级时 `Str::studly(is_null($class) ? $name : $class)`
/// - type='controller' 返回 `\addons\{name}\controller\{class}`
/// - type='hook'（默认）返回 `\addons\{name}\Plugin`
///
/// ## 示例
///
/// - `build_controller_class("operate", "Order")` → `addons\operate\controller\Order`
/// - `build_controller_class("operate", "admin.Order")` → `addons\operate\controller\admin\Order`
fn build_controller_class(addon: &str, controller: &str) -> String {
    let resolved = parse_dotted_controller(controller);
    format!("addons\\{}\\controller\\{}", addon, resolved)
}

/// 解析多级控制器点号分隔（对齐 PHP `get_addons_class` 中 `.` 处理）
///
/// 直接复用 autoload 模块的同名函数。
fn parse_dotted_controller(controller: &str) -> String {
    if !controller.contains('.') {
        return controller.to_string();
    }

    let mut parts: Vec<&str> = controller.split('.').collect();
    if parts.len() == 1 {
        return controller.to_string();
    }

    let last = parts
        .pop()
        .expect("已通过 contains('.') 与 len 检查保证 parts 非空");
    let last_studly = studly_case(last);
    parts.push(&last_studly);
    parts.join("\\")
}

/// 下划线转大驼峰
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
    use std::path::PathBuf;

    fn make_test_env() -> (tempfile::TempDir, AddonRegistry, AddonAutoload) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let addons_path = tmp.path().join("addons");

        // operate 插件
        let operate_dir = addons_path.join("operate");
        fs::create_dir_all(&operate_dir).unwrap();
        fs::write(
            operate_dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'operate',
    'status' => 1,
];
"#,
        )
        .unwrap();

        // operate/controller/Order.php
        let controller_dir = operate_dir.join("controller");
        fs::create_dir_all(&controller_dir).unwrap();
        fs::write(controller_dir.join("Order.php"), "<?php // stub").unwrap();

        // operate/controller/admin/Order.php
        let admin_dir = controller_dir.join("admin");
        fs::create_dir_all(&admin_dir).unwrap();
        fs::write(admin_dir.join("Order.php"), "<?php // stub").unwrap();

        // disabled 插件（status=0）
        let disabled_dir = addons_path.join("disabled");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(
            disabled_dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'disabled',
    'status' => 0,
];
"#,
        )
        .unwrap();

        // nonexistent 插件（无控制器）
        let nonexistent_dir = addons_path.join("nonexistent");
        fs::create_dir_all(&nonexistent_dir).unwrap();
        fs::write(
            nonexistent_dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'nonexistent',
    'status' => 1,
];
"#,
        )
        .unwrap();

        // 注册插件
        let registry = AddonRegistry::new();
        let _ = registry.load_from_directory(&addons_path).unwrap();

        let autoload = AddonAutoload::new(&addons_path);

        (tmp, registry, autoload)
    }

    #[test]
    fn test_addon_route_new_simple() {
        let route = AddonRoute::new("operate", "Order", "index");
        assert_eq!(route.addon, "operate");
        assert_eq!(route.controller, "Order");
        assert_eq!(route.action, "index");
        assert_eq!(route.controller_class, "addons\\operate\\controller\\Order");
        assert!(route.controller_file.is_none());
    }

    #[test]
    fn test_addon_route_new_dotted_controller() {
        let route = AddonRoute::new("operate", "admin.Order", "index");
        assert_eq!(route.controller, "admin.Order");
        assert_eq!(
            route.controller_class,
            "addons\\operate\\controller\\admin\\Order"
        );
    }

    #[test]
    fn test_addon_route_new_three_level_dotted() {
        let route = AddonRoute::new("operate", "admin.sub.Order", "index");
        assert_eq!(
            route.controller_class,
            "addons\\operate\\controller\\admin\\sub\\Order"
        );
    }

    #[test]
    fn test_addon_route_controller_file_none() {
        let route = AddonRoute::new("a", "b", "c");
        assert!(route.controller_file().is_none());
    }

    #[test]
    fn test_parse_url_segments_full_url() {
        let result = parse_url_segments("/addons/operate/Order/index");
        assert_eq!(
            result,
            Some((
                "operate".to_string(),
                "Order".to_string(),
                "index".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_url_segments_no_leading_slash() {
        let result = parse_url_segments("addons/operate/Order/index");
        assert_eq!(
            result,
            Some((
                "operate".to_string(),
                "Order".to_string(),
                "index".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_url_segments_missing_action() {
        let result = parse_url_segments("/addons/operate/Order");
        assert_eq!(
            result,
            Some(("operate".to_string(), "Order".to_string(), "".to_string()))
        );
    }

    #[test]
    fn test_parse_url_segments_missing_controller_and_action() {
        let result = parse_url_segments("/addons/operate");
        assert_eq!(
            result,
            Some(("operate".to_string(), "".to_string(), "".to_string()))
        );
    }

    #[test]
    fn test_parse_url_segments_dotted_controller() {
        let result = parse_url_segments("/addons/operate/admin.Order/index");
        assert_eq!(
            result,
            Some((
                "operate".to_string(),
                "admin.Order".to_string(),
                "index".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_url_segments_non_addons_url() {
        let result = parse_url_segments("/api/users");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_url_segments_empty_url() {
        let result = parse_url_segments("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_url_segments_only_addons() {
        let result = parse_url_segments("/addons/");
        assert_eq!(
            result,
            Some(("".to_string(), "".to_string(), "".to_string()))
        );
    }

    #[test]
    fn test_parse_url_segments_trailing_slash() {
        let result = parse_url_segments("/addons/operate/Order/index/");
        assert_eq!(
            result,
            Some((
                "operate".to_string(),
                "Order".to_string(),
                "index".to_string()
            ))
        );
    }

    #[test]
    fn test_build_controller_class_simple() {
        let class = build_controller_class("operate", "Order");
        assert_eq!(class, "addons\\operate\\controller\\Order");
    }

    #[test]
    fn test_build_controller_class_dotted() {
        let class = build_controller_class("operate", "admin.Order");
        assert_eq!(class, "addons\\operate\\controller\\admin\\Order");
    }

    #[test]
    fn test_build_controller_class_three_levels() {
        let class = build_controller_class("operate", "admin.sub.Order");
        assert_eq!(class, "addons\\operate\\controller\\admin\\sub\\Order");
    }

    #[test]
    fn test_parse_route_valid() {
        let (_tmp, registry, autoload) = make_test_env();

        let route = parse_route("/addons/operate/Order/index", &registry, &autoload).unwrap();
        assert_eq!(route.addon, "operate");
        assert_eq!(route.controller, "Order");
        assert_eq!(route.action, "index");
        assert!(route.controller_file.is_some());
    }

    #[test]
    fn test_parse_route_dotted_controller() {
        let (_tmp, registry, autoload) = make_test_env();

        let route = parse_route("/addons/operate/admin.Order/index", &registry, &autoload).unwrap();
        assert_eq!(route.controller, "admin.Order");
        assert_eq!(
            route.controller_class,
            "addons\\operate\\controller\\admin\\Order"
        );
        assert!(route.controller_file.is_some());
        assert!(route
            .controller_file
            .unwrap()
            .to_string_lossy()
            .contains("admin"));
    }

    #[test]
    fn test_parse_route_non_addons_url() {
        let (_tmp, registry, autoload) = make_test_env();

        let result = parse_route("/api/users", &registry, &autoload);
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::RouteParse { .. } => {}
            other => panic!("expected RouteParse, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_route_empty_action() {
        let (_tmp, registry, autoload) = make_test_env();

        let result = parse_route("/addons/operate/Order", &registry, &autoload);
        assert!(result.is_err());
        // action 为空应该触发 RouteParse 错误
    }

    #[test]
    fn test_parse_route_empty_controller() {
        let (_tmp, registry, autoload) = make_test_env();

        let result = parse_route("/addons/operate", &registry, &autoload);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_route_addon_not_found() {
        let (_tmp, registry, autoload) = make_test_env();

        let result = parse_route("/addons/ghost/Order/index", &registry, &autoload);
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::AddonNotFound(name) => assert_eq!(name, "ghost"),
            other => panic!("expected AddonNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_route_addon_disabled() {
        let (_tmp, registry, autoload) = make_test_env();

        let result = parse_route("/addons/disabled/Order/index", &registry, &autoload);
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::AddonDisabled(name) => assert_eq!(name, "disabled"),
            other => panic!("expected AddonDisabled, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_route_controller_not_found() {
        let (_tmp, registry, autoload) = make_test_env();

        // nonexistent 插件无控制器
        let result = parse_route("/addons/nonexistent/Ghost/index", &registry, &autoload);
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::ControllerNotFound(name) => assert_eq!(name, "Ghost"),
            other => panic!("expected ControllerNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_route_controller_file_resolved() {
        let (_tmp, registry, autoload) = make_test_env();

        let route = parse_route("/addons/operate/Order/index", &registry, &autoload).unwrap();
        let file = route.controller_file.unwrap();
        assert!(file.exists());
        assert!(file.to_string_lossy().ends_with("Order.php"));
    }

    #[test]
    fn test_parse_route_multilevel_controller_file_resolved() {
        let (_tmp, registry, autoload) = make_test_env();

        let route = parse_route("/addons/operate/admin.Order/index", &registry, &autoload).unwrap();
        let file = route.controller_file.unwrap();
        assert!(file.exists());
        assert!(file.to_string_lossy().contains("admin"));
        assert!(file.to_string_lossy().ends_with("Order.php"));
    }

    #[test]
    fn test_parse_route_no_leading_slash() {
        let (_tmp, registry, autoload) = make_test_env();

        let route = parse_route("addons/operate/Order/index", &registry, &autoload).unwrap();
        assert_eq!(route.addon, "operate");
    }

    #[test]
    fn test_addon_route_clone_eq() {
        let r1 = AddonRoute::new("a", "b", "c");
        let r2 = r1.clone();
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_addon_route_with_controller_file() {
        let mut route = AddonRoute::new("a", "b", "c");
        route.controller_file = Some(PathBuf::from("/addons/a/controller/B.php"));
        assert_eq!(
            route.controller_file(),
            Some(std::path::Path::new("/addons/a/controller/B.php"))
        );
    }

    // 直接测试 AddonManifest 的 status 字段对路由的影响
    #[test]
    fn test_route_status_check_reflects_manifest() {
        let (_tmp, registry, autoload) = make_test_env();

        // operate 启用
        assert!(registry.is_enabled("operate").unwrap());

        // disabled 禁用
        assert!(!registry.is_enabled("disabled").unwrap());

        // 动态启用 disabled
        registry.set_enabled("disabled", true).unwrap();
        assert!(registry.is_enabled("disabled").unwrap());

        // disabled 启用后仍会因控制器不存在而失败（ControllerNotFound 而非 AddonDisabled）
        let result = parse_route("/addons/disabled/Order/index", &registry, &autoload);
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::ControllerNotFound(_) => {}
            AddonLoaderError::AddonDisabled(_) => {
                panic!("should be ControllerNotFound after enabling")
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }
}
