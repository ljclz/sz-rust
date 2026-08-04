//! sz-rust 热加载 Addon 示例模板
//!
//! 本 crate 演示如何编写一个可被 `HotAddonLoader` 动态加载的 Addon 插件。
//!
//! ## 构建
//!
//! ```bash
//! cargo build --package sz-rust-addon-example --release
//! ```
//!
//! 产物路径（Windows）：
//! `target/release/sz_rust_addon_example.dll`
//!
//! ## 部署
//!
//! 将产物复制到 sz300 的 `addons/` 扫描目录：
//! ```bash
//! cp target/release/sz_rust_addon_example.dll <sz300>/addons/
//! ```
//!
//! 启动 sz300（启用 hot-reload feature）即可自动加载。
//!
//! ## 入口规范
//!
//! Addon 必须导出一个 `#[unsafe(no_mangle)] pub extern "Rust" fn addon_init()` 函数，
//! 返回 `AddonInitResult`。框架通过 `libloading` 查找该符号并调用。

use sz_rust_core::runtime::hot_reload::AddonInitResult;

/// Addon 入口函数
///
/// 框架通过 `libloading` 动态加载本动态库后，查找 `addon_init` 符号并调用，
/// 用返回值注册插件元数据（名称、版本、依赖等）。
///
/// # Safety
///
/// 此函数为动态库 FFI 边界，必须：
/// - 不 panic（panic 会被框架 `catch_unwind` 捕获并记录为加载失败）
/// - 不访问已释放的内存
/// - 返回的所有权转移给调用方
#[unsafe(no_mangle)]
pub extern "Rust" fn addon_init() -> AddonInitResult {
    AddonInitResult {
        name: "example".to_string(),
        version: "1.0.0".to_string(),
        description: Some("sz-rust 热加载 Addon 示例模板".to_string()),
        dependencies: vec![],
    }
}
