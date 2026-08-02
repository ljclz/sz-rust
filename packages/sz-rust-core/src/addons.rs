//! 插件加载模块 — 重导出 `sz-rust-addons-loader`
//!
//! H-6 修复：从 `sz-rust-addons-loader` 包重导出插件加载器全套 API，
//! 使业务层可通过 `sz_rust_core::addons::AddonLoader` 等路径访问，
//! 无需直接依赖 `sz-rust-addons-loader` 包。
//!
//! ## 对齐 PHP
//!
//! 对标 PHP `zzstudio/think-addons`，提供插件发现/清单解析/注册/路由解析能力。
//!
//! | PHP 模块 | Rust 类型 | 说明 |
//! |----------|----------|------|
//! | `think\addons\Service` | `AddonLoader` | 插件扫描与注册 |
//! | `think\Addons::getInfo()` | `AddonManifest` | 插件清单解析 |
//! | `spl_autoload_register` | `AddonAutoload` | 类自动加载 |
//! | 隐式插件状态管理 | `AddonRegistry` | 插件启用/禁用状态 |
//! | `think\addons\Route::execute` | `AddonRoute` | URL 路由解析 |
//! | `think\Route::resource` | `build_resource_routes` | RESTful 资源路由 |
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_core::addons::AddonLoader;
//!
//! let loader = AddonLoader::new("/path/to/addons");
//! let errors = loader.register().unwrap();
//! let route = loader.parse_route("/addons/operate/Order/index").unwrap();
//! assert_eq!(route.addon, "operate");
//! assert_eq!(route.controller, "Order");
//! assert_eq!(route.action, "index");
//! ```

/// 自动加载器 — 对齐 `spl_autoload_register`
pub use sz_rust_addons_loader::AddonAutoload;

/// 插件加载错误类型
pub use sz_rust_addons_loader::AddonLoaderError;

/// 插件加载 Result 别名
pub use sz_rust_addons_loader::AddonLoaderResult;

/// 插件加载器 — 对齐 `think\addons\Service`
pub use sz_rust_addons_loader::AddonLoader;

/// 插件清单 — 对齐 `think\Addons::getInfo()`
pub use sz_rust_addons_loader::AddonManifest;

/// 插件注册中心 — 对齐隐式插件状态管理
pub use sz_rust_addons_loader::AddonRegistry;

/// 插件路由解析结果 — 对齐 `think\addons\Route::execute`
pub use sz_rust_addons_loader::AddonRoute;

/// RESTful 资源路由构建函数 — 对齐 `think\Route::resource`
pub use sz_rust_addons_loader::build_resource_routes;

/// 资源路由配置选项
pub use sz_rust_addons_loader::ResourceOptions;

/// 资源路由条目
pub use sz_rust_addons_loader::ResourceRouteEntry;

/// 资源路由动作类型
pub use sz_rust_addons_loader::ResourceAction;

/// HTTP 方法枚举
pub use sz_rust_addons_loader::HttpMethod;

/// 资源路由定义
pub use sz_rust_addons_loader::ResourceRoute;
