//! # SZ-Rust Addons Loader — 插件加载器
//!
//! 对标 PHP `zzstudio/think-addons` 的 Rust 实现，提供插件发现/清单解析/注册/路由解析能力。
//!
//! ## PHP 对齐
//!
//! | PHP 模块 | Rust 模块 | 子任务 |
//! |----------|----------|--------|
//! | `think\addons\Service` | `loader::AddonLoader` | 10.1 |
//! | `think\Addons::getInfo()` | `manifest::parse_manifest` | 10.1 |
//! | `spl_autoload_register`（helper.php） | `autoload::AddonAutoload` | 10.1 |
//! | 隐式插件状态管理 | `registry::AddonRegistry` | 10.2 |
//! | `think\addons\Route::execute` | `route::parse_route` | 10.3 |
//! | `think\Route::resource` | `resource::build_resource_routes` | 10.3.5 |
//!
//! ## 模块结构
//!
//! ```text
//! sz-rust-addons-loader/
//! ├── Cargo.toml
//! └── src/
//!     ├── lib.rs          # 主入口（重导出）
//!     ├── error.rs        # 错误类型
//!     ├── loader.rs       # AddonLoader（对齐 Service.php）
//!     ├── manifest.rs     # 清单解析（对齐 Addons::getInfo）
//!     ├── autoload.rs     # 自动加载（对齐 spl_autoload_register）
//!     ├── registry.rs     # 注册中心（对齐插件状态管理）
//!     ├── route.rs        # 路由解析（对齐 Route::execute）
//!     └── resource.rs     # 资源路由注入（对齐 Route::resource）
//! ```
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_addons_loader::loader::AddonLoader;
//!
//! let loader = AddonLoader::new("/path/to/addons");
//! let errors = loader.register().unwrap();
//! // errors 包含解析失败的插件错误（不中断整体扫描）
//!
//! let route = loader.parse_route("/addons/operate/Order/index").unwrap();
//! assert_eq!(route.addon, "operate");
//! assert_eq!(route.controller, "Order");
//! assert_eq!(route.action, "index");
//! ```
//!
//! ## 不支持（与 PHP 差异）
//!
//! - **不实际加载 PHP 文件**：Rust 无法直接执行 PHP，仅解析 `$info` 数组
//! - **不实现钩子自动扫描**：PHP 通过反射收集 Plugin.php 非基类方法作为钩子，Rust 侧需调用方显式注册
//! - **不实现 `service.ini` 容器绑定**：Rust 侧无容器概念，由调用方根据需要处理
//! - **不实现 `_empty` 兜底方法**：对齐 PHP `is_callable([$instance, '_empty'])`，但需调用方实现

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod autoload;
/// 插件能力钩子，集成 Capability Registry。
pub mod capability_hook;
pub mod error;
#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod resource;
pub mod route;

// 重导出核心类型，对齐 PHP `think\addons` 命名空间
pub use autoload::AddonAutoload;
pub use capability_hook::{
    unregister_plugin_capabilities, validate_capability_naming, CapabilityHook,
};
pub use error::{AddonLoaderError, AddonLoaderResult};
pub use loader::AddonLoader;
pub use manifest::AddonManifest;
pub use registry::AddonRegistry;
pub use resource::{
    build_resource_routes, HttpMethod, ResourceAction, ResourceOptions, ResourceRoute,
    ResourceRouteEntry,
};
pub use route::AddonRoute;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::fs;

    /// 端到端集成测试：完整生命周期
    ///
    /// 验证 AddonLoader 的完整流程：
    /// 1. 创建临时插件目录
    /// 2. 编写 Plugin.php（含 $info 数组）
    /// 3. 创建控制器文件
    /// 4. register() 扫描并注册
    /// 5. parse_route() 解析 URL
    /// 6. 验证路由信息正确
    #[test]
    fn test_end_to_end_lifecycle() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let addons_path = tmp.path().join("addons");

        // 创建 operate 插件
        let operate_dir = addons_path.join("operate");
        fs::create_dir_all(&operate_dir).unwrap();
        fs::write(
            operate_dir.join("Plugin.php"),
            r#"<?php
namespace addons\operate;
use think\Addons;
class Plugin extends Addons {
    public $info = [
        'name' => 'operate',
        'title' => '运营管理',
        'identifier' => 'operate.addon',
        'icon' => 'fa-cog',
        'author' => 'sz',
        'version' => '1.0.0',
        'admin' => 'operate/index/index',
        'status' => 1,
    ];
    public function install() {}
    public function uninstall() {}
}
"#,
        )
        .unwrap();

        // 创建 operate/controller/Order.php
        let controller_dir = operate_dir.join("controller");
        fs::create_dir_all(&controller_dir).unwrap();
        fs::write(controller_dir.join("Order.php"), "<?php // stub").unwrap();

        // 创建 operate/controller/admin/Order.php
        let admin_dir = controller_dir.join("admin");
        fs::create_dir_all(&admin_dir).unwrap();
        fs::write(admin_dir.join("Order.php"), "<?php // stub").unwrap();

        // 创建 disabled 插件（status=0）
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

        // 1. 创建加载器
        let loader = AddonLoader::new(&addons_path);
        assert_eq!(loader.count(), 0);

        // 2. 注册所有插件
        let errors = loader.register().unwrap();
        assert!(errors.is_empty(), "register errors: {:?}", errors);
        assert_eq!(loader.count(), 2);

        // 3. 验证 operate 启用、disabled 禁用
        assert!(loader.is_enabled("operate").unwrap());
        assert!(!loader.is_enabled("disabled").unwrap());

        // 4. 验证清单字段
        let manifest = loader.get_manifest("operate").unwrap();
        assert_eq!(manifest.name, "operate");
        assert_eq!(manifest.title, "运营管理");
        assert_eq!(manifest.version, "1.0.0");
        assert!(manifest.is_enabled());

        // 5. 解析简单路由
        let route = loader.parse_route("/addons/operate/Order/index").unwrap();
        assert_eq!(route.addon, "operate");
        assert_eq!(route.controller, "Order");
        assert_eq!(route.action, "index");
        assert_eq!(route.controller_class, "addons\\operate\\controller\\Order");
        assert!(route.controller_file.is_some());
        assert!(route.controller_file.unwrap().exists());

        // 6. 解析多级控制器路由（点号分隔）
        let route2 = loader
            .parse_route("/addons/operate/admin.Order/index")
            .unwrap();
        assert_eq!(route2.controller, "admin.Order");
        assert_eq!(
            route2.controller_class,
            "addons\\operate\\controller\\admin\\Order"
        );
        assert!(route2.controller_file.is_some());

        // 7. 禁用插件应抛 AddonDisabled
        let result = loader.parse_route("/addons/disabled/Order/index");
        assert!(matches!(result, Err(AddonLoaderError::AddonDisabled(_))));

        // 8. 不存在的插件应抛 AddonNotFound
        let result = loader.parse_route("/addons/ghost/Order/index");
        assert!(matches!(result, Err(AddonLoaderError::AddonNotFound(_))));
    }

    /// 测试混合启用/禁用插件列表
    #[test]
    fn test_mixed_enabled_disabled_addons() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");

        for (name, status) in [
            ("enabled1", 1),
            ("enabled2", 1),
            ("disabled1", 0),
            ("disabled2", 0),
        ] {
            let dir = addons_path.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("Plugin.php"),
                format!(
                    r#"
public $info = [
    'name' => '{}',
    'status' => {},
];
"#,
                    name, status
                ),
            )
            .unwrap();
        }

        let loader = AddonLoader::new(&addons_path);
        loader.register().unwrap();

        assert_eq!(loader.count(), 4);
        assert_eq!(loader.enabled_addons().len(), 2);
        assert_eq!(loader.disabled_addons().len(), 2);
        assert_eq!(
            loader.names(),
            vec!["disabled1", "disabled2", "enabled1", "enabled2"]
        );
    }

    /// 测试动态状态切换
    #[test]
    fn test_dynamic_status_toggle() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");
        let dir = addons_path.join("operate");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'operate',
    'status' => 1,
];
"#,
        )
        .unwrap();

        let loader = AddonLoader::new(&addons_path);
        loader.register().unwrap();

        assert!(loader.is_enabled("operate").unwrap());

        // 动态禁用
        loader.registry().set_enabled("operate", false).unwrap();
        assert!(!loader.is_enabled("operate").unwrap());

        // 动态启用
        loader.registry().set_enabled("operate", true).unwrap();
        assert!(loader.is_enabled("operate").unwrap());
    }

    /// 测试 autoload 与 registry 协作
    #[test]
    fn test_autoload_registry_collaboration() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");
        let operate_dir = addons_path.join("operate");
        fs::create_dir_all(&operate_dir).unwrap();
        fs::write(operate_dir.join("Plugin.php"), "<?php // stub").unwrap();
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

        let controller_dir = operate_dir.join("controller");
        fs::create_dir_all(&controller_dir).unwrap();
        fs::write(controller_dir.join("Order.php"), "<?php // stub").unwrap();

        let loader = AddonLoader::new(&addons_path);
        loader.register().unwrap();

        // 通过 autoload 解析类路径
        let path = loader
            .resolve_class("addons\\operate\\controller\\Order")
            .unwrap();
        assert!(path.is_some());
        assert!(path.unwrap().exists());

        // 通过 autoload 解析插件入口类
        let plugin_path = loader.resolve_plugin("operate").unwrap();
        assert!(plugin_path.is_some());
        assert!(plugin_path.unwrap().exists());
    }

    /// 测试多个插件目录扫描
    #[test]
    fn test_multiple_addons_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");

        for name in &["operate", "cashier", "food", "erp"] {
            let dir = addons_path.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("Plugin.php"),
                format!(
                    r#"
public $info = [
    'name' => '{}',
    'status' => 1,
];
"#,
                    name
                ),
            )
            .unwrap();
        }

        let loader = AddonLoader::new(&addons_path);
        let errors = loader.register().unwrap();
        assert!(errors.is_empty());
        assert_eq!(loader.count(), 4);
        assert_eq!(loader.names(), vec!["cashier", "erp", "food", "operate"]);
    }

    /// 测试部分插件损坏时整体扫描不中断
    #[test]
    fn test_partial_failure_continues_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");

        // 好插件
        let good_dir = addons_path.join("good");
        fs::create_dir_all(&good_dir).unwrap();
        fs::write(
            good_dir.join("Plugin.php"),
            r#"
public $info = [
    'name' => 'good',
    'status' => 1,
];
"#,
        )
        .unwrap();

        // 坏插件（无 $info 数组）
        let bad_dir = addons_path.join("bad");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("Plugin.php"), "<?php class Plugin {}").unwrap();

        let loader = AddonLoader::new(&addons_path);
        let errors = loader.register().unwrap();

        // bad 插件应记录错误
        assert!(!errors.is_empty());
        // good 插件应正常注册
        assert_eq!(loader.count(), 1);
        assert!(loader.exists("good"));
        assert!(!loader.exists("bad"));
    }

    /// 测试空插件目录
    #[test]
    fn test_empty_addons_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let addons_path = tmp.path().join("addons");
        fs::create_dir_all(&addons_path).unwrap();

        let loader = AddonLoader::new(&addons_path);
        let errors = loader.register().unwrap();
        assert!(errors.is_empty());
        assert_eq!(loader.count(), 0);
    }
}
