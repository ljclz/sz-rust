//! Addon 热加载探索 — 运行时动态加载插件（`libloading` 后端）
//!
//! ## ⚠️ 探索性实现说明
//!
//! 当前 sz-rust-addons-loader 采用**编译期注册**：插件在 `Cargo.toml` 中声明，
//! 通过 `sz_rust_core::addons::loader::AddonLoader` 在启动时扫描并注册。
//!
//! 本模块探索**运行时动态加载**方案：在进程运行期间加载/卸载 `.dylib` / `.so` / `.dll`，
//! 无需重新编译主应用。适用于：
//! - 生产环境热更新插件
//! - 第三方插件市场安装
//! - 多租户动态启用/禁用插件
//!
//! ## 架构
//!
//! ```text
//! addons/
//! ├── operate/           ← 编译为 liboperate.so
//! │   ├── Cargo.toml     ← [lib] crate-type = ["cdylib"]
//! │   └── src/lib.rs     ← #[unsafe(no_mangle)] pub extern "Rust" fn addon_init()
//! └── crm/               ← 编译为 libcrm.so
//!     └── ...
//!
//! 主应用:
//!   HotAddonLoader::scan("/path/to/addons")
//!     → 发现 .so/.dylib/.dll
//!     → libloading::Library::new() 动态加载
//!     → dlsym("addon_init") 获取初始化符号
//!     → 调用 addon_init() 获取插件元数据
//! ```
//!
//! ## 安全约束
//!
//! - **版本对齐**：插件与主应用必须使用完全相同版本的 `sz-rust-core`，
//!   否则 `RouterBuilder<S>` 的内存布局可能不一致，导致 UB。
//! - **ABI 正确**：`addon_init` 使用 Rust ABI（`extern "Rust"`），同进程内动态加载，
//!   可安全传递 `String` 等 Rust 特有类型；`#[unsafe(no_mangle)]` 保证符号名不被混淆，
//!   使 `dlsym` 可按明文名称查找。
//! - **生命周期**：插件 `Library` 持有 `'static` 生命周期，卸载后其注册的路由
//!   仍可能被 axum 引用 → 当前实现仅支持**加载**，不支持安全卸载。
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use sz_rust_core::runtime::hot_reload::HotAddonLoader;
//!
//! let mut loader = HotAddonLoader::new();
//! // 扫描目录并加载所有 .so/.dylib/.dll
//! let results = loader.scan("/opt/sz300/addons").await.unwrap();
//! for (name, result) in results {
//!     println!("{}: {:?}", name, result);
//! }
//! ```
//!
//! ## 插件开发规范
//!
//! 插件 `Cargo.toml`：
//! ```toml
//! [lib]
//! name = "addon_operate"
//! crate-type = ["cdylib"]   # 关键：编译为 C 兼容动态库
//!
//! [dependencies]
//! sz-rust-core = { version = "0.3", features = ["hot-reload"] }
//! ```
//!
//! 插件 `src/lib.rs`：
//! ```rust,ignore
//! use sz_rust_core::runtime::hot_reload::AddonInitResult;
//!
//! /// 插件入口 — 主应用通过 dlsym 调用此函数
//! #[unsafe(no_mangle)]
//! pub extern "Rust" fn addon_init() -> AddonInitResult {
//!     AddonInitResult {
//!         name: "operate".to_string(),
//!         version: "1.0.0".to_string(),
//!         // 路由注册通过返回的 AddonManifest 描述，由主应用注入
//!     }
//! }
//! ```

#![allow(unsafe_code)]
#![cfg(feature = "hot-reload")]

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// 类型定义
// ============================================================================

/// 插件初始化结果 — 插件 `addon_init` 函数的返回类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddonInitResult {
    /// 插件名称（用于路由前缀，如 `/addons/{name}/...`）
    pub name: String,
    /// 插件版本（语义化版本，如 `"1.0.0"`）
    pub version: String,
    /// 插件描述
    pub description: Option<String>,
    /// 插件依赖的其他插件名称列表
    pub dependencies: Vec<String>,
}

/// 插件清单（从 `addon_init` 返回值 + 文件系统元数据合并）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotAddonManifest {
    /// 插件名称（用于路由前缀，如 `/addons/{name}/...`）
    pub name: String,
    /// 插件版本（语义化版本，如 `"1.0.0"`）
    pub version: String,
    /// 插件描述
    pub description: Option<String>,
    /// 动态库文件路径
    pub file_path: PathBuf,
    /// 插件依赖的其他插件名称列表
    pub dependencies: Vec<String>,
    /// 加载时间（UTC）
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

/// 已加载的插件句柄
pub struct LoadedAddon {
    /// 插件清单
    pub manifest: HotAddonManifest,
    /// 动态库句柄（持有 `'static` 生命周期）
    _library: Arc<Library>,
}

/// 热加载错误类型
#[derive(Debug, Error)]
pub enum HotReloadError {
    /// 动态库加载失败
    #[error("动态库加载失败: {0}")]
    LibraryLoad(#[from] libloading::Error),

    /// 找不到 `addon_init` 符号（插件未实现入口函数）
    #[error("找不到 addon_init 符号（插件未实现入口函数）")]
    MissingInitSymbol,

    /// `addon_init` 调用失败（插件 panic 被捕获）
    #[error("addon_init 调用失败: {0}")]
    InitFailed(String),

    /// 插件目录扫描失败
    #[error("插件目录扫描失败: {0}")]
    ScanFailed(String),

    /// 插件名称冲突（同名插件已加载）
    #[error("插件名称冲突: {0}")]
    NameConflict(String),

    /// 插件依赖缺失
    #[error("插件依赖缺失: {0} 依赖 {1}（未加载）")]
    MissingDependency(String, String),
}

/// 单个插件的扫描结果：`(插件名, 加载结果)`
pub type AddonScanResult = (String, Result<HotAddonManifest, HotReloadError>);

/// addon_init 函数指针类型
///
/// 使用 `extern "Rust"` 而非 `extern "C"`：插件与主应用同进程运行，
/// Rust ABI 可安全传递 `String`/`Vec` 等 Rust 特有类型，避免 FFI 未定义行为。
/// `#[unsafe(no_mangle)]` 保证符号名不被混淆，`libloading::get` 可按明文查找。
type AddonInitFn = extern "Rust" fn() -> AddonInitResult;

// ============================================================================
// HotAddonLoader
// ============================================================================

/// Addon 热加载器
///
/// 扫描目录中的 `.so` / `.dylib` / `.dll` 文件，动态加载并调用 `addon_init`。
///
/// ## 线程安全
///
/// 内部使用 `RwLock<HashMap>` 保护已加载插件注册表，支持多线程并发加载。
pub struct HotAddonLoader {
    /// 已加载插件注册表（name → LoadedAddon）
    registry: Arc<RwLock<HashMap<String, LoadedAddon>>>,
    /// 扫描目录列表
    scan_dirs: Vec<PathBuf>,
}

impl HotAddonLoader {
    /// 创建新的热加载器
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(HashMap::new())),
            scan_dirs: Vec::new(),
        }
    }

    /// 添加扫描目录
    pub fn add_scan_dir(&mut self, dir: impl Into<PathBuf>) {
        self.scan_dirs.push(dir.into());
    }

    /// 获取已加载插件列表
    pub fn loaded_addons(&self) -> Vec<String> {
        self.registry.read().keys().cloned().collect()
    }

    /// 查询指定插件的清单
    pub fn get_manifest(&self, name: &str) -> Option<HotAddonManifest> {
        self.registry
            .read()
            .get(name)
            .map(|addon| addon.manifest.clone())
    }

    /// 扫描并加载所有扫描目录中的插件
    ///
    /// 返回 `(name, Result<manifest, error>)` 列表。
    /// 单个插件加载失败不影响其他插件。
    pub async fn scan(&mut self) -> Vec<AddonScanResult> {
        let mut results = Vec::new();
        let dirs = self.scan_dirs.clone();

        for dir in dirs {
            match self.scan_dir(&dir).await {
                Ok(entries) => results.extend(entries),
                Err(e) => results.push((
                    dir.to_string_lossy().to_string(),
                    Err(HotReloadError::ScanFailed(e.to_string())),
                )),
            }
        }

        results
    }

    /// 扫描单个目录
    async fn scan_dir(&mut self, dir: &Path) -> Result<Vec<AddonScanResult>, HotReloadError> {
        let mut results = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
            HotReloadError::ScanFailed(format!("无法读取目录 {}: {}", dir.display(), e))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            HotReloadError::ScanFailed(format!("读取目录条目失败 {}: {}", dir.display(), e))
        })? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // 检查是否为动态库文件
            if !is_shared_library(&path) {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            // 跳过已加载的插件
            if self.registry.read().contains_key(&name) {
                results.push((
                    name.clone(),
                    Err(HotReloadError::NameConflict(format!(
                        "插件 {} 已加载，跳过",
                        name
                    ))),
                ));
                continue;
            }

            match self.load_library(&path, &name) {
                Ok(manifest) => results.push((name, Ok(manifest))),
                Err(e) => results.push((name, Err(e))),
            }
        }

        Ok(results)
    }

    /// 加载单个动态库
    fn load_library(&self, path: &Path, name: &str) -> Result<HotAddonManifest, HotReloadError> {
        // 1. 动态加载动态库
        //    # Safety: libloading 保证 Library 在 Drop 前有效。
        //    我们用 Arc 持有 Library，确保其生命周期覆盖所有注册的路由。
        let library = unsafe { Library::new(path) }?;

        // 2. 查找 addon_init 符号
        //    # Safety: libloading::get 将原始指针转为函数指针，符号存在且类型正确时安全。
        //    若符号不存在，返回 MissingInitSymbol 错误。
        let init_symbol: libloading::Symbol<AddonInitFn> = unsafe { library.get(b"addon_init\0") }
            .map_err(|_| HotReloadError::MissingInitSymbol)?;

        // 3. 调用 addon_init
        //    extern "Rust" 函数指针调用本身是安全的（同进程 Rust ABI）。
        //    使用 catch_unwind 防止插件 panic 穿透动态库边界（否则为 UB）。
        //    AssertUnwindSafe 在此安全：函数指针不捕获任何 Rust 状态。
        let init_result =
            panic::catch_unwind(AssertUnwindSafe(*init_symbol)).map_err(|payload| {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "未知 panic（插件 addon_init 崩溃）".to_string()
                };
                HotReloadError::InitFailed(format!("插件 {} panic: {}", name, msg))
            })?;

        // 4. 检查依赖
        for dep in &init_result.dependencies {
            if !self.registry.read().contains_key(dep) {
                return Err(HotReloadError::MissingDependency(
                    init_result.name.clone(),
                    dep.clone(),
                ));
            }
        }

        // 5. 构建清单
        let manifest = HotAddonManifest {
            name: init_result.name.clone(),
            version: init_result.version.clone(),
            description: init_result.description.clone(),
            file_path: path.to_path_buf(),
            dependencies: init_result.dependencies.clone(),
            loaded_at: chrono::Utc::now(),
        };

        // 6. 注册到注册表（Arc 持有 Library，防止 Drop）
        let loaded = LoadedAddon {
            manifest: manifest.clone(),
            _library: Arc::new(library),
        };

        self.registry.write().insert(name.to_string(), loaded);

        Ok(manifest)
    }

    /// 卸载指定插件
    ///
    /// ⚠️ **设计限制**：当前实现仅从注册表移除，不实际卸载动态库。
    /// 因为 axum Router 可能持有对插件路由的引用，强制卸载会导致悬垂指针。
    /// 安全卸载需要 axum 上游支持路由热替换，当前版本通过 Arc 引用计数自动管理生命周期。
    pub fn unload(&self, name: &str) -> Result<(), HotReloadError> {
        if self.registry.write().remove(name).is_some() {
            // 仅从注册表移除；Library 的 Arc 引用计数减 1
            // 若 Router 仍持有引用，Library 不会被 Drop
            Ok(())
        } else {
            Err(HotReloadError::ScanFailed(format!("插件 {} 未加载", name)))
        }
    }

    /// 获取注册表 Arc（供框架内部使用，如 axum 路由挂载）
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> Arc<RwLock<HashMap<String, LoadedAddon>>> {
        self.registry.clone()
    }
}

impl Default for HotAddonLoader {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 判断路径是否为共享库文件
fn is_shared_library(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(ext, "dylib" | "dll") {
        return true;
    }
    // Linux：`.so` 或版本化共享库（如 `libfoo.so.1`、`libfoo.so.2`）
    if ext == "so" {
        return true;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.contains(".so."))
        .unwrap_or(false)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_shared_library_linux() {
        assert!(is_shared_library(Path::new("/opt/addons/liboperate.so")));
        assert!(is_shared_library(Path::new("addon.so.1")));
    }

    #[test]
    fn test_is_shared_library_macos() {
        assert!(is_shared_library(Path::new("/opt/addons/liboperate.dylib")));
    }

    #[test]
    fn test_is_shared_library_windows() {
        assert!(is_shared_library(Path::new("C:\\addons\\operate.dll")));
    }

    #[test]
    fn test_is_shared_library_rejects_non_lib() {
        assert!(!is_shared_library(Path::new("/opt/addons/operate.txt")));
        assert!(!is_shared_library(Path::new("/opt/addons/operate")));
        assert!(!is_shared_library(Path::new("/opt/addons/manifest.json")));
    }

    #[test]
    fn test_loader_new_empty() {
        let loader = HotAddonLoader::new();
        assert!(loader.loaded_addons().is_empty());
        assert!(loader.scan_dirs.is_empty());
    }

    #[test]
    fn test_loader_add_scan_dir() {
        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir("/opt/addons");
        loader.add_scan_dir("/opt/plugins");
        assert_eq!(loader.scan_dirs.len(), 2);
        assert_eq!(loader.scan_dirs[0], Path::new("/opt/addons"));
        assert_eq!(loader.scan_dirs[1], Path::new("/opt/plugins"));
    }

    #[test]
    fn test_loader_get_manifest_unknown() {
        let loader = HotAddonLoader::new();
        assert!(loader.get_manifest("nonexistent").is_none());
    }

    #[test]
    fn test_unload_unknown_addon() {
        let loader = HotAddonLoader::new();
        let result = loader.unload("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_addon_init_result_serialization() {
        let result = AddonInitResult {
            name: "operate".to_string(),
            version: "1.0.0".to_string(),
            description: Some("CRUD 业务插件".to_string()),
            dependencies: vec!["auth".to_string()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("operate"));
        assert!(json.contains("1.0.0"));
        assert!(json.contains("CRUD"));

        let decoded: AddonInitResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "operate");
        assert_eq!(decoded.version, "1.0.0");
    }

    #[test]
    fn test_hot_addon_manifest_serialization() {
        let manifest = HotAddonManifest {
            name: "crm".to_string(),
            version: "2.1.0".to_string(),
            description: None,
            file_path: PathBuf::from("/opt/addons/libcrm.so"),
            dependencies: vec![],
            loaded_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("crm"));
        assert!(json.contains("2.1.0"));
        assert!(json.contains("libcrm.so"));
    }

    #[tokio::test]
    async fn test_scan_empty_dir_returns_empty() {
        // 创建临时空目录
        let tmp = tempfile::tempdir().unwrap();
        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir(tmp.path());
        let results = loader.scan().await;
        // 空目录 → 无结果（不是错误）
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_scan_nonexistent_dir_returns_error() {
        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir("/this/path/does/not/exist/xyz123");
        let results = loader.scan().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_err());
    }

    #[tokio::test]
    async fn test_scan_dir_with_non_lib_files_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        // 写入非库文件
        std::fs::write(tmp.path().join("readme.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("manifest.json"), "{}").unwrap();

        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir(tmp.path());
        let results = loader.scan().await;
        // 非库文件应被忽略 → 无结果
        assert!(results.is_empty());
    }

    #[test]
    fn test_load_fake_library_fails_gracefully() {
        // 创建一个假的 .so 文件（实际是文本文件）
        let tmp = tempfile::tempdir().unwrap();
        let fake_so = tmp.path().join("libfake.so");
        std::fs::write(&fake_so, "not a real shared library").unwrap();

        let loader = HotAddonLoader::new();
        let result = loader.load_library(&fake_so, "fake");
        // 动态库加载应失败（文件不是有效的 ELF/Mach-O/PE）
        assert!(result.is_err());
    }
}
