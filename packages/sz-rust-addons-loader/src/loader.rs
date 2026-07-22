//! 插件加载器（Phase 10.1 主入口）
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think\addons\Service` 的完整生命周期：
//!
//! ```php
//! // vendor/zzstudio/think-addons/src/addons/Service.php
//! class Service extends \think\Service
//! {
//!     public function register()
//!     {
//!         $this->addons_path = $this->getAddonsPath();
//!         Lang::load([...]);
//!         $this->autoload();    // ← 自动扫描插件目录
//!         $this->loadEvent();   // ← 加载钩子
//!         $this->loadService(); // ← 加载 service.ini
//!         $this->app->bind('addons', Service::class);
//!     }
//!
//!     public function boot()
//!     {
//!         // 注册路由：addons/:addon/[:controller]/[:action]
//!         $route->rule("addons/:addon/[:controller]/[:action]", $execute);
//!     }
//! }
//! ```
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 子任务 |
//! |------|---------|--------|
//! | `loader` | `Service::register/autoload/loadEvent/loadService` | 10.1 |
//! | `registry` | 插件状态管理（隐式） | 10.2 |
//! | `manifest` | `Addons::getInfo()` | 10.1 |
//! | `autoload` | `spl_autoload_register` | 10.1 |
//! | `route` | `Route::execute` | 10.3 |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::autoload::AddonAutoload;
use crate::error::{AddonLoaderError, AddonLoaderResult};
use crate::manifest::AddonManifest;
use crate::registry::AddonRegistry;

/// 插件加载器（Phase 10.1 主入口）
///
/// 对齐 PHP `think\addons\Service`，统一管理插件发现/清单/注册/路由解析。
///
/// ## 设计
///
/// - 持有插件根目录（对齐 PHP `$this->addons_path`）
/// - 持有 `AddonRegistry`（插件清单注册中心）
/// - 持有 `AddonAutoload`（类名→文件路径解析）
/// - 提供 `register()` 入口：扫描目录 + 解析清单 + 注册到 registry
/// - 提供 `parse_route(url)` 入口：URL → 控制器类名 + 文件路径
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_addons_loader::loader::AddonLoader;
///
/// let loader = AddonLoader::new("/path/to/addons");
/// let errors = loader.register().unwrap();
/// // errors 中包含解析失败的插件错误（不中断整体扫描）
///
/// let route = loader.parse_route("/addons/operate/Order/index").unwrap();
/// assert_eq!(route.addon, "operate");
/// assert_eq!(route.controller, "Order");
/// assert_eq!(route.action, "index");
/// ```
#[derive(Debug)]
pub struct AddonLoader {
    /// 插件根目录（对齐 PHP `getAddonsPath()`）
    addons_path: PathBuf,
    /// 插件注册中心（线程安全）
    registry: Arc<AddonRegistry>,
    /// 自动加载器
    autoload: AddonAutoload,
}

impl AddonLoader {
    /// 创建加载器
    ///
    /// - `addons_path`：插件根目录，对齐 PHP `{rootPath}/addons/`
    pub fn new(addons_path: impl Into<PathBuf>) -> Self {
        let addons_path = addons_path.into();
        let autoload = AddonAutoload::new(&addons_path);
        Self {
            addons_path,
            registry: Arc::new(AddonRegistry::new()),
            autoload,
        }
    }

    /// 获取插件根目录
    pub fn addons_path(&self) -> &Path {
        &self.addons_path
    }

    /// 获取注册中心引用（共享只读）
    pub fn registry(&self) -> &AddonRegistry {
        &self.registry
    }

    /// 获取自动加载器引用
    pub fn autoload(&self) -> &AddonAutoload {
        &self.autoload
    }

    /// 注册所有插件（对齐 PHP `Service::register()` 完整流程）
    ///
    /// ## 流程
    ///
    /// 1. 扫描 `addons_path` 下所有子目录（对齐 PHP `scandir`）
    /// 2. 对每个子目录解析 `Plugin.php` 清单（对齐 PHP `autoload()`）
    /// 3. 注册到 `registry`（对齐 PHP `get_addons_instance` 缓存）
    /// 4. 不加载钩子和服务绑定（Rust 侧无 PHP 反射能力，钩子由调用方显式注册）
    ///
    /// ## 错误处理
    ///
    /// - 单个插件解析失败不会中断整体扫描
    /// - 返回的 `Vec<AddonLoaderError>` 包含所有失败的插件错误
    /// - 目录读取失败返回 `ScanDir` 错误
    pub fn register(&self) -> AddonLoaderResult<Vec<AddonLoaderError>> {
        self.registry.load_from_directory(&self.addons_path)
    }

    /// 解析路由（对齐 PHP `Route::execute($addon, $controller, $action)`）
    ///
    /// ## 错误
    ///
    /// - `RouteParse`：URL 格式错误
    /// - `AddonNotFound`：插件不存在
    /// - `AddonDisabled`：插件已禁用
    /// - `ControllerNotFound`：控制器不存在
    pub fn parse_route(&self, url: &str) -> AddonLoaderResult<crate::route::AddonRoute> {
        crate::route::parse_route(url, &self.registry, &self.autoload)
    }

    /// 获取所有已注册插件名（按字母序）
    pub fn names(&self) -> Vec<String> {
        self.registry.names()
    }

    /// 获取所有已注册插件清单（按字母序）
    pub fn all_addons(&self) -> Vec<AddonManifest> {
        self.registry.all()
    }

    /// 获取所有启用的插件清单（按字母序）
    pub fn enabled_addons(&self) -> Vec<AddonManifest> {
        self.registry.enabled_addons()
    }

    /// 获取所有禁用的插件清单（按字母序）
    pub fn disabled_addons(&self) -> Vec<AddonManifest> {
        self.registry.disabled_addons()
    }

    /// 判断插件是否存在
    pub fn exists(&self, name: &str) -> bool {
        self.registry.exists(name)
    }

    /// 判断插件是否启用
    pub fn is_enabled(&self, name: &str) -> AddonLoaderResult<bool> {
        self.registry.is_enabled(name)
    }

    /// 获取插件清单
    pub fn get_manifest(&self, name: &str) -> AddonLoaderResult<AddonManifest> {
        self.registry.get(name)
    }

    /// 获取已注册插件数量
    pub fn count(&self) -> usize {
        self.registry.count()
    }

    /// 解析类名到文件路径（对齐 PHP `spl_autoload_register` 回调）
    pub fn resolve_class(&self, class: &str) -> AddonLoaderResult<Option<PathBuf>> {
        self.autoload.resolve(class)
    }

    /// 解析控制器类名到文件路径（对齐 PHP `get_addons_class` + `class_exists`）
    pub fn resolve_controller(
        &self,
        addon: &str,
        controller: &str,
    ) -> AddonLoaderResult<Option<PathBuf>> {
        self.autoload.resolve_controller(addon, controller)
    }

    /// 解析插件入口类到文件路径
    pub fn resolve_plugin(&self, addon: &str) -> AddonLoaderResult<Option<PathBuf>> {
        self.autoload.resolve_plugin(addon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_test_env() -> (tempfile::TempDir, AddonLoader) {
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
    'title' => '运营管理',
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

        // cashier 插件（status=0 禁用）
        let cashier_dir = addons_path.join("cashier");
        fs::create_dir_all(&cashier_dir).unwrap();
        fs::write(
            cashier_dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'cashier',
    'status' => 0,
];
"#,
        )
        .unwrap();

        let loader = AddonLoader::new(&addons_path);
        (tmp, loader)
    }

    #[test]
    fn test_new_loader() {
        let loader = AddonLoader::new("/addons");
        assert_eq!(loader.addons_path(), Path::new("/addons"));
        assert_eq!(loader.count(), 0);
    }

    #[test]
    fn test_register_scans_addons() {
        let (_tmp, loader) = make_test_env();
        let errors = loader.register().unwrap();
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(loader.count(), 2);
        assert!(loader.exists("operate"));
        assert!(loader.exists("cashier"));
    }

    #[test]
    fn test_register_returns_errors_for_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");
        let malformed_dir = addons_path.join("malformed");
        fs::create_dir_all(&malformed_dir).unwrap();
        // 写入无 $info 的 Plugin.php
        fs::write(malformed_dir.join("Plugin.php"), "<?php class Plugin {}").unwrap();

        let loader = AddonLoader::new(&addons_path);
        let errors = loader.register().unwrap();
        assert!(!errors.is_empty());
        assert_eq!(loader.count(), 0);
    }

    #[test]
    fn test_register_nonexistent_path() {
        let loader = AddonLoader::new("/nonexistent/path/12345");
        let result = loader.register();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_route_valid() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let route = loader.parse_route("/addons/operate/Order/index").unwrap();
        assert_eq!(route.addon, "operate");
        assert_eq!(route.controller, "Order");
        assert_eq!(route.action, "index");
        assert!(route.controller_file.is_some());
    }

    #[test]
    fn test_parse_route_dotted_controller() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let route = loader
            .parse_route("/addons/operate/admin.Order/index")
            .unwrap();
        assert_eq!(route.controller, "admin.Order");
        assert!(route.controller_file.is_some());
    }

    #[test]
    fn test_parse_route_disabled_addon() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let result = loader.parse_route("/addons/cashier/Order/index");
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::AddonDisabled(name) => assert_eq!(name, "cashier"),
            other => panic!("expected AddonDisabled, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_route_not_found_addon() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let result = loader.parse_route("/addons/ghost/Order/index");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_route_controller_not_found() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let result = loader.parse_route("/addons/operate/Ghost/index");
        assert!(result.is_err());
    }

    #[test]
    fn test_names_sorted() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let names = loader.names();
        assert_eq!(names, vec!["cashier", "operate"]);
    }

    #[test]
    fn test_all_addons_sorted() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let all = loader.all_addons();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "cashier");
        assert_eq!(all[1].name, "operate");
    }

    #[test]
    fn test_enabled_addons_filtered() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let enabled = loader.enabled_addons();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "operate");
    }

    #[test]
    fn test_disabled_addons_filtered() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let disabled = loader.disabled_addons();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].name, "cashier");
    }

    #[test]
    fn test_exists() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        assert!(loader.exists("operate"));
        assert!(loader.exists("cashier"));
        assert!(!loader.exists("ghost"));
    }

    #[test]
    fn test_is_enabled() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        assert!(loader.is_enabled("operate").unwrap());
        assert!(!loader.is_enabled("cashier").unwrap());
        assert!(loader.is_enabled("ghost").is_err());
    }

    #[test]
    fn test_get_manifest() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let manifest = loader.get_manifest("operate").unwrap();
        assert_eq!(manifest.name, "operate");
        assert_eq!(manifest.title, "运营管理");
    }

    #[test]
    fn test_get_manifest_not_found() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let result = loader.get_manifest("ghost");
        assert!(result.is_err());
    }

    #[test]
    fn test_count() {
        let (_tmp, loader) = make_test_env();
        assert_eq!(loader.count(), 0);
        loader.register().unwrap();
        assert_eq!(loader.count(), 2);
    }

    #[test]
    fn test_resolve_class() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let path = loader.resolve_class("addons\\operate\\Plugin").unwrap();
        assert!(path.is_some());
    }

    #[test]
    fn test_resolve_class_non_addons() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let path = loader.resolve_class("app\\Home").unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn test_resolve_controller() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let path = loader.resolve_controller("operate", "Order").unwrap();
        assert!(path.is_some());
    }

    #[test]
    fn test_resolve_controller_multilevel() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let path = loader.resolve_controller("operate", "admin.Order").unwrap();
        assert!(path.is_some());
    }

    #[test]
    fn test_resolve_plugin() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let path = loader.resolve_plugin("operate").unwrap();
        assert!(path.is_some());
    }

    #[test]
    fn test_registry_shared_via_arc() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        // 通过 registry() 访问的 Arc 应该指向同一份数据
        let registry = loader.registry();
        assert_eq!(registry.count(), 2);
        assert!(registry.exists("operate"));
    }

    #[test]
    fn test_autoload_accessor() {
        let (_tmp, loader) = make_test_env();
        loader.register().unwrap();

        let autoload = loader.autoload();
        assert_eq!(autoload.addons_path(), loader.addons_path());
    }
}
