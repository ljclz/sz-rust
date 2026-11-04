// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件注册中心
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think\addons` 中隐式的插件注册表：
//!
//! - `helper.php` 中 `get_addons_instance($name)` 的 `static $_addons = []` 单例缓存
//! - `Service::loadEvent()` 中扫描插件目录构建钩子映射
//! - `Service::loadService()` 中扫描插件目录 + `service.ini` 构建容器绑定
//! - `Route::execute` 中通过 `get_addons_info($addon)` 检查插件状态
//!
//! ## 设计
//!
//! - 线程安全的插件注册表（`RwLock<HashMap<String, AddonManifest>>`）
//! - 支持注册/查询/卸载插件
//! - 支持状态管理（enabled/disabled，对齐 PHP `$info['status']`）
//! - 支持按名称排序迭代（对齐 PHP `scandir` 顺序）

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;

use crate::error::{AddonLoaderError, AddonLoaderResult};
use crate::manifest::AddonManifest;

/// 插件注册中心
///
/// 对齐 PHP `think\addons\Service` 中隐式的插件状态管理。
///
/// ## 线程安全
///
/// 使用 `RwLock<HashMap<String, AddonManifest>>` 实现，支持多读单写。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_addons_loader::registry::AddonRegistry;
/// use sz_rust_addons_loader::manifest::AddonManifest;
///
/// let registry = AddonRegistry::new();
/// let manifest = AddonManifest::new("operate");
/// registry.register(manifest).unwrap();
///
/// assert!(registry.exists("operate"));
/// assert_eq!(registry.count(), 1);
/// ```
#[derive(Debug)]
pub struct AddonRegistry {
    /// 插件清单映射（key = 插件名）
    manifests: RwLock<HashMap<String, AddonManifest>>,
}

impl AddonRegistry {
    /// 创建空注册中心
    pub fn new() -> Self {
        Self {
            manifests: RwLock::new(HashMap::new()),
        }
    }

    /// 注册插件（对齐 PHP `get_addons_instance` 缓存到 `$_addons[$name]`）
    ///
    /// ## 错误
    ///
    /// - `AddonNotFound`：插件名已存在（不覆盖）
    #[tracing::instrument(skip(self, manifest))]
    pub fn register(&self, manifest: AddonManifest) -> AddonLoaderResult<()> {
        let mut manifests = self.manifests.write();
        if manifests.contains_key(&manifest.name) {
            return Err(AddonLoaderError::AddonNotFound(format!(
                "addon '{}' already registered",
                manifest.name
            )));
        }
        manifests.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// 强制注册或更新插件（对齐 PHP `array_merge` 覆盖行为）
    #[tracing::instrument(skip(self, manifest))]
    pub fn upsert(&self, manifest: AddonManifest) {
        let mut manifests = self.manifests.write();
        manifests.insert(manifest.name.clone(), manifest);
    }

    /// 注销插件（对齐 PHP `unset($_addons[$name])`）
    ///
    /// ## 返回
    ///
    /// - `Ok(Some(manifest))`：插件已存在并已移除
    /// - `Ok(None)`：插件不存在
    #[tracing::instrument(skip(self))]
    pub fn unregister(&self, name: &str) -> AddonLoaderResult<Option<AddonManifest>> {
        let mut manifests = self.manifests.write();
        Ok(manifests.remove(name))
    }

    /// 查询插件清单（对齐 PHP `get_addons_info($name)`）
    ///
    /// ## 错误
    ///
    /// - `AddonNotFound`：插件不存在
    pub fn get(&self, name: &str) -> AddonLoaderResult<AddonManifest> {
        let manifests = self.manifests.read();
        manifests
            .get(name)
            .cloned()
            .ok_or_else(|| AddonLoaderError::AddonNotFound(name.to_string()))
    }

    /// 尝试查询插件清单（不返回错误）
    pub fn try_get(&self, name: &str) -> Option<AddonManifest> {
        let manifests = self.manifests.read();
        manifests.get(name).cloned()
    }

    /// 判断插件是否存在（对齐 PHP `class_exists` 检查）
    pub fn exists(&self, name: &str) -> bool {
        let manifests = self.manifests.read();
        manifests.contains_key(name)
    }

    /// 判断插件是否启用（对齐 PHP `Route::execute` 中 `!$info['status']` 检查）
    ///
    /// ## 返回
    ///
    /// - `Ok(true)`：插件存在且 status != 0
    /// - `Ok(false)`：插件存在但 status == 0
    /// - `Err(AddonNotFound)`：插件不存在
    pub fn is_enabled(&self, name: &str) -> AddonLoaderResult<bool> {
        let manifests = self.manifests.read();
        let manifest = manifests
            .get(name)
            .ok_or_else(|| AddonLoaderError::AddonNotFound(name.to_string()))?;
        Ok(manifest.is_enabled())
    }

    /// 设置插件状态（对齐 PHP 修改 `$info['status']`）
    ///
    /// ## 参数
    ///
    /// - `name`：插件名
    /// - `enabled`：true=1（启用），false=0（禁用）
    #[tracing::instrument(skip(self))]
    pub fn set_enabled(&self, name: &str, enabled: bool) -> AddonLoaderResult<()> {
        let mut manifests = self.manifests.write();
        let manifest = manifests
            .get_mut(name)
            .ok_or_else(|| AddonLoaderError::AddonNotFound(name.to_string()))?;
        manifest.status = if enabled { 1 } else { 0 };
        Ok(())
    }

    /// 获取所有已注册的插件名（按字母序，对齐 PHP `scandir` 排序）
    pub fn names(&self) -> Vec<String> {
        let manifests = self.manifests.read();
        let mut names: Vec<String> = manifests.keys().cloned().collect();
        names.sort();
        names
    }

    /// 获取所有已注册的插件清单（按名字序）
    pub fn all(&self) -> Vec<AddonManifest> {
        let manifests = self.manifests.read();
        let mut list: Vec<AddonManifest> = manifests.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// 获取所有启用的插件清单（按名字序）
    pub fn enabled_addons(&self) -> Vec<AddonManifest> {
        let manifests = self.manifests.read();
        let mut list: Vec<AddonManifest> = manifests
            .values()
            .filter(|m| m.is_enabled())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// 获取所有禁用的插件清单（按名字序）
    pub fn disabled_addons(&self) -> Vec<AddonManifest> {
        let manifests = self.manifests.read();
        let mut list: Vec<AddonManifest> = manifests
            .values()
            .filter(|m| !m.is_enabled())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// 获取已注册插件数量
    pub fn count(&self) -> usize {
        let manifests = self.manifests.read();
        manifests.len()
    }

    /// 清空注册中心
    #[tracing::instrument(skip(self))]
    pub fn clear(&self) {
        let mut manifests = self.manifests.write();
        manifests.clear();
    }

    /// 从插件目录批量加载并注册（对齐 PHP `Service::loadService` 扫描逻辑）
    ///
    /// ## 扫描规则
    ///
    /// 对齐 PHP `scandir($addons_path)`：
    ///
    /// 1. 扫描 `addons_path` 下的所有子目录
    /// 2. 跳过 `.` `..` 及文件项
    /// 3. 子目录必须包含 `Plugin.php`（对齐 PHP `is_file($addonDir . ucfirst($name) . '.php')`，Rust 侧统一使用 `Plugin.php`）
    /// 4. 解析 `Plugin.php` 中的 `$info` 数组
    /// 5. 注册到注册中心
    ///
    /// ## 错误处理
    ///
    /// - 单个插件解析失败不会中断整体扫描，但会记录到返回的错误列表
    /// - 目录读取失败返回 `ScanDir` 错误
    #[tracing::instrument(skip(self))]
    pub async fn load_from_directory(
        &self,
        addons_path: &Path,
    ) -> AddonLoaderResult<Vec<AddonLoaderError>> {
        let mut errors = Vec::new();

        let entries =
            tokio::fs::read_dir(addons_path)
                .await
                .map_err(|e| AddonLoaderError::ScanDir {
                    path: addons_path.display().to_string(),
                    source: e,
                })?;

        let mut entries = entries;
        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|e| AddonLoaderError::ScanDir {
                    path: addons_path.display().to_string(),
                    source: e,
                })?
        {
            let path = entry.path();
            // 仅处理目录
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // 跳过隐藏目录（以 . 开头）
            if name.starts_with('.') {
                continue;
            }

            // 解析插件清单
            match crate::manifest::parse_manifest(&path).await {
                Ok(manifest) => {
                    self.upsert(manifest);
                }
                Err(e) => {
                    errors.push(e);
                }
            }
        }

        Ok(errors)
    }

    /// 获取插件文件路径（对齐 PHP `getAddonsPath() . $name . DIRECTORY_SEPARATOR`）
    pub fn addon_path(&self, name: &str) -> AddonLoaderResult<PathBuf> {
        let manifests = self.manifests.read();
        let manifest = manifests
            .get(name)
            .ok_or_else(|| AddonLoaderError::AddonNotFound(name.to_string()))?;
        Ok(manifest.addon_path.clone())
    }
}

impl Default for AddonRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest(name: &str, status: i64) -> AddonManifest {
        let mut m = AddonManifest::new(name);
        m.status = status;
        m
    }

    #[test]
    fn test_new_empty() {
        let registry = AddonRegistry::new();
        assert_eq!(registry.count(), 0);
        assert!(registry.names().is_empty());
    }

    #[test]
    fn test_register_success() {
        let registry = AddonRegistry::new();
        let manifest = make_manifest("operate", 1);
        assert!(registry.register(manifest).is_ok());
        assert_eq!(registry.count(), 1);
        assert!(registry.exists("operate"));
    }

    #[test]
    fn test_register_duplicate_fails() {
        let registry = AddonRegistry::new();
        let manifest = make_manifest("operate", 1);
        registry.register(manifest).unwrap();

        let manifest2 = make_manifest("operate", 0);
        let result = registry.register(manifest2);
        assert!(result.is_err());
        // 原状态保持不变
        assert!(registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_upsert_overwrites() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        registry.upsert(make_manifest("operate", 0));
        assert!(!registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_unregister_existing() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        let result = registry.unregister("operate").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "operate");
        assert!(!registry.exists("operate"));
    }

    #[test]
    fn test_unregister_nonexistent() {
        let registry = AddonRegistry::new();
        let result = registry.unregister("ghost").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_existing() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        let manifest = registry.get("operate").unwrap();
        assert_eq!(manifest.name, "operate");
        assert_eq!(manifest.status, 1);
    }

    #[test]
    fn test_get_nonexistent_returns_error() {
        let registry = AddonRegistry::new();
        let result = registry.get("ghost");
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::AddonNotFound(name) => assert_eq!(name, "ghost"),
            other => panic!("expected AddonNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_try_get_existing() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        let manifest = registry.try_get("operate");
        assert!(manifest.is_some());
        assert_eq!(manifest.unwrap().name, "operate");
    }

    #[test]
    fn test_try_get_nonexistent() {
        let registry = AddonRegistry::new();
        let manifest = registry.try_get("ghost");
        assert!(manifest.is_none());
    }

    #[test]
    fn test_exists_true() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        assert!(registry.exists("operate"));
    }

    #[test]
    fn test_exists_false() {
        let registry = AddonRegistry::new();
        assert!(!registry.exists("operate"));
    }

    #[test]
    fn test_is_enabled_enabled() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        assert!(registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_is_enabled_disabled() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 0)).unwrap();
        assert!(!registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_is_enabled_nonexistent() {
        let registry = AddonRegistry::new();
        let result = registry.is_enabled("ghost");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_enabled_true() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 0)).unwrap();
        assert!(!registry.is_enabled("operate").unwrap());

        registry.set_enabled("operate", true).unwrap();
        assert!(registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_set_enabled_false() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("operate", 1)).unwrap();
        assert!(registry.is_enabled("operate").unwrap());

        registry.set_enabled("operate", false).unwrap();
        assert!(!registry.is_enabled("operate").unwrap());
    }

    #[test]
    fn test_set_enabled_nonexistent() {
        let registry = AddonRegistry::new();
        let result = registry.set_enabled("ghost", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_names_sorted() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("cashier", 1)).unwrap();
        registry.register(make_manifest("operate", 1)).unwrap();
        registry.register(make_manifest("food", 1)).unwrap();

        let names = registry.names();
        assert_eq!(names, vec!["cashier", "food", "operate"]);
    }

    #[test]
    fn test_all_sorted() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("cashier", 1)).unwrap();
        registry.register(make_manifest("operate", 1)).unwrap();

        let all = registry.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "cashier");
        assert_eq!(all[1].name, "operate");
    }

    #[test]
    fn test_enabled_addons_filtered() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("enabled1", 1)).unwrap();
        registry.register(make_manifest("disabled1", 0)).unwrap();
        registry.register(make_manifest("enabled2", 1)).unwrap();

        let enabled = registry.enabled_addons();
        assert_eq!(enabled.len(), 2);
        assert_eq!(enabled[0].name, "enabled1");
        assert_eq!(enabled[1].name, "enabled2");
    }

    #[test]
    fn test_disabled_addons_filtered() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("enabled1", 1)).unwrap();
        registry.register(make_manifest("disabled1", 0)).unwrap();
        registry.register(make_manifest("disabled2", 0)).unwrap();

        let disabled = registry.disabled_addons();
        assert_eq!(disabled.len(), 2);
        assert_eq!(disabled[0].name, "disabled1");
        assert_eq!(disabled[1].name, "disabled2");
    }

    #[test]
    fn test_count() {
        let registry = AddonRegistry::new();
        assert_eq!(registry.count(), 0);
        registry.register(make_manifest("a", 1)).unwrap();
        assert_eq!(registry.count(), 1);
        registry.register(make_manifest("b", 1)).unwrap();
        assert_eq!(registry.count(), 2);
        registry.unregister("a").unwrap();
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_clear() {
        let registry = AddonRegistry::new();
        registry.register(make_manifest("a", 1)).unwrap();
        registry.register(make_manifest("b", 1)).unwrap();
        assert_eq!(registry.count(), 2);

        registry.clear();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_default_equals_new() {
        let r1 = AddonRegistry::default();
        let r2 = AddonRegistry::new();
        assert_eq!(r1.count(), r2.count());
    }

    #[tokio::test]
    async fn test_load_from_directory_empty() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let registry = AddonRegistry::new();
        let errors = registry.load_from_directory(tmp.path()).await.unwrap();
        assert!(errors.is_empty());
        assert_eq!(registry.count(), 0);
    }

    #[tokio::test]
    async fn test_load_from_directory_nonexistent() {
        let registry = AddonRegistry::new();
        let result = registry
            .load_from_directory(Path::new("/nonexistent/path/12345"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_from_directory_with_valid_plugin() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        // 创建 operate 插件目录
        let operate_dir = tmp.path().join("operate");
        std::fs::create_dir_all(&operate_dir).unwrap();
        let plugin_php = operate_dir.join("Plugin.php");
        std::fs::write(
            &plugin_php,
            r#"
public $info = [
    'name' => 'operate',
    'title' => '运营',
    'status' => 1,
];
"#,
        )
        .unwrap();

        // 创建 cashier 插件目录
        let cashier_dir = tmp.path().join("cashier");
        std::fs::create_dir_all(&cashier_dir).unwrap();
        let plugin_php2 = cashier_dir.join("Plugin.php");
        std::fs::write(
            &plugin_php2,
            r#"
public $info = [
    'name' => 'cashier',
    'status' => 1,
];
"#,
        )
        .unwrap();

        let registry = AddonRegistry::new();
        let errors = registry.load_from_directory(tmp.path()).await.unwrap();
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(registry.count(), 2);
        assert!(registry.exists("operate"));
        assert!(registry.exists("cashier"));
    }

    #[tokio::test]
    async fn test_load_from_directory_skips_files() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        // 创建文件（应被跳过）
        std::fs::write(tmp.path().join("BaseController.php"), "<?php // stub").unwrap();
        // 创建普通目录但无 Plugin.php（会记录错误）
        std::fs::create_dir(tmp.path().join("empty_dir")).unwrap();

        let registry = AddonRegistry::new();
        let errors = registry.load_from_directory(tmp.path()).await.unwrap();
        // empty_dir 无 Plugin.php → 错误
        assert!(!errors.is_empty());
        assert_eq!(registry.count(), 0);
    }

    #[tokio::test]
    async fn test_load_from_directory_skips_hidden() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        tokio::fs::create_dir(tmp.path().join(".hidden"))
            .await
            .unwrap();

        let registry = AddonRegistry::new();
        let errors = registry.load_from_directory(tmp.path()).await.unwrap();
        // .hidden 目录应被跳过，不产生错误
        assert!(errors.is_empty());
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_addon_path_existing() {
        let registry = AddonRegistry::new();
        let mut manifest = make_manifest("operate", 1);
        manifest.addon_path = PathBuf::from("/addons/operate");
        registry.register(manifest).unwrap();

        let path = registry.addon_path("operate").unwrap();
        assert_eq!(path, PathBuf::from("/addons/operate"));
    }

    #[test]
    fn test_addon_path_nonexistent() {
        let registry = AddonRegistry::new();
        let result = registry.addon_path("ghost");
        assert!(result.is_err());
    }

    #[test]
    fn test_concurrent_read_access() {
        let registry = std::sync::Arc::new(AddonRegistry::new());
        registry.register(make_manifest("operate", 1)).unwrap();

        let registry2 = registry.clone();
        let handle = std::thread::spawn(move || {
            // 并发读
            registry2.try_get("operate")
        });

        let local_result = registry.try_get("operate");
        let remote_result = handle.join().unwrap();

        assert!(local_result.is_some());
        assert!(remote_result.is_some());
    }

    #[test]
    fn test_concurrent_write_access() {
        let registry = std::sync::Arc::new(AddonRegistry::new());

        let registry2 = registry.clone();
        let handle = std::thread::spawn(move || registry2.register(make_manifest("operate", 1)));

        let local_result = registry.register(make_manifest("cashier", 1));
        let remote_result = handle.join().unwrap();

        assert!(local_result.is_ok());
        assert!(remote_result.is_ok());
        assert_eq!(registry.count(), 2);
    }
}
