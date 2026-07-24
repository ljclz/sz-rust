//! 插件清单解析（Phase 10.1）
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think\Addons::getInfo()` 的行为：
//!
//! ```php
//! // vendor/zzstudio/think-addons/src/Addons.php:85-110
//! final public function getInfo(): array
//! {
//!     // 1. 先读缓存
//!     $info = Config::get($this->addon_info, []);
//!     if (empty($info)) {
//!         // 2. 合并子类 $info 属性与 info.ini 文件
//!         $info = $this->info;
//!         $info_file = $this->addon_path . 'info.ini';
//!         if (is_file($info_file)) {
//!             $info = array_merge($info, parse_ini_file($info_file, true, INI_SCANNER_TYPED));
//!         }
//!     }
//!     // 3. 自动注入 url 字段
//!     $_info['url'] = addons_url();
//!     Config::set($info, $this->addon_info);
//!     return $info;
//! }
//! ```
//!
//! ## Plugin.php 清单字段
//!
//! 对齐 PHP 插件入口文件的 `$info` 数组：
//!
//! | 字段 | 类型 | 说明 |
//! |------|------|------|
//! | `name` | string | 插件标识（与目录名一致） |
//! | `title` | string | 插件标题 |
//! | `identifier` | string | 插件唯一标识符 |
//! | `icon` | string | 图标路径 |
//! | `author` | string | 作者 |
//! | `version` | string | 版本号 |
//! | `admin` | string | 后台管理 URL |
//! | `status` | int | 状态（1=启用，0=禁用） |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AddonLoaderError, AddonLoaderResult};

/// 插件清单信息（对齐 PHP `think\Addons::getInfo()` 返回的 `$info` 数组）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonManifest {
    /// 插件标识（对齐 `$info['name']`）
    pub name: String,
    /// 插件标题（对齐 `$info['title']`）
    pub title: String,
    /// 唯一标识符（对齐 `$info['identifier']`）
    pub identifier: String,
    /// 图标路径（对齐 `$info['icon']`）
    pub icon: String,
    /// 作者（对齐 `$info['author']`）
    pub author: String,
    /// 版本号（对齐 `$info['version']`）
    pub version: String,
    /// 后台管理 URL（对齐 `$info['admin']`）
    pub admin: String,
    /// 状态：1=启用，0=禁用（对齐 `$info['status']`，被 `Route::execute` 检查）
    pub status: i64,
    /// 插件目录绝对路径（Rust 侧额外字段，用于后续加载文件）
    #[serde(skip)]
    pub addon_path: PathBuf,
}

impl AddonManifest {
    /// 创建空清单（用于测试）
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: String::new(),
            identifier: String::new(),
            icon: String::new(),
            author: String::new(),
            version: String::new(),
            admin: String::new(),
            status: 0,
            addon_path: PathBuf::new(),
        }
    }

    /// 判断插件是否启用（对齐 PHP `Route::execute` 中 `if (!$info['status'])` 检查）
    pub fn is_enabled(&self) -> bool {
        self.status != 0
    }

    /// 获取插件入口文件路径（对齐 PHP `ucfirst($name) . '.php'`，但 Rust 侧统一使用 `Plugin.php`）
    pub fn plugin_file(&self) -> PathBuf {
        self.addon_path.join("Plugin.php")
    }

    /// 获取 info.ini 文件路径（对齐 PHP `info.ini`）
    pub fn info_ini_file(&self) -> PathBuf {
        self.addon_path.join("info.ini")
    }

    /// 获取 config.php 文件路径（对齐 PHP `config.php`）
    pub fn config_php_file(&self) -> PathBuf {
        self.addon_path.join("config.php")
    }

    /// 获取 service.ini 文件路径（对齐 PHP `service.ini`）
    pub fn service_ini_file(&self) -> PathBuf {
        self.addon_path.join("service.ini")
    }

    /// 获取视图目录路径（对齐 PHP `{addon_path}/view/`）
    pub fn view_dir(&self) -> PathBuf {
        self.addon_path.join("view")
    }

    /// 获取控制器目录路径（对齐 PHP `{addon_path}/controller/`）
    pub fn controller_dir(&self) -> PathBuf {
        self.addon_path.join("controller")
    }

    /// 获取模型目录路径（对齐 PHP `{addon_path}/model/`）
    pub fn model_dir(&self) -> PathBuf {
        self.addon_path.join("model")
    }
}

/// 从插件目录解析清单（对齐 PHP `getInfo()` 流程）
///
/// ## 解析顺序（对齐 PHP）
///
/// 1. 读取 `Plugin.php`，提取 `$info` 数组（对齐子类 `$info` 属性）
/// 2. 读取 `info.ini`（若存在），合并覆盖（对齐 `array_merge`）
/// 3. 设置 `addon_path` 为插件目录绝对路径
///
/// ## PHP Plugin.php 解析
///
/// 由于 Rust 无法直接执行 PHP，本函数通过简单的字符串扫描解析 `$info = [...]` 数组，
/// 支持字符串值（单引号/双引号）和整数值。
///
/// ## 错误
///
/// - `ManifestParse`：Plugin.php 不存在或 `$info` 数组格式错误
/// - `ReadFile`：文件读取失败
#[tracing::instrument]
pub fn parse_manifest(addon_path: &Path) -> AddonLoaderResult<AddonManifest> {
    let plugin_file = addon_path.join("Plugin.php");
    if !plugin_file.exists() {
        return Err(AddonLoaderError::ManifestParse {
            addon: addon_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>")
                .to_string(),
            reason: format!("Plugin.php not found in {}", addon_path.display()),
        });
    }

    let plugin_content =
        std::fs::read_to_string(&plugin_file).map_err(|e| AddonLoaderError::ReadFile {
            path: plugin_file.display().to_string(),
            source: e,
        })?;

    // 解析 Plugin.php 中的 $info 数组
    let mut info =
        parse_php_info_array(&plugin_content).ok_or_else(|| AddonLoaderError::ManifestParse {
            addon: addon_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>")
                .to_string(),
            reason: "$info array not found or malformed in Plugin.php".to_string(),
        })?;

    // 合并 info.ini（若存在）
    let info_ini_path = addon_path.join("info.ini");
    if info_ini_path.exists() {
        let ini_content =
            std::fs::read_to_string(&info_ini_path).map_err(|e| AddonLoaderError::ReadFile {
                path: info_ini_path.display().to_string(),
                source: e,
            })?;
        let ini_map = parse_simple_ini(&ini_content);
        for (key, value) in ini_map {
            info.insert(key, value);
        }
    }

    // 构建清单（对齐 PHP getInfo 返回字段）
    let name = addon_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let manifest = build_manifest_from_info(&name, addon_path.to_path_buf(), info)?;

    Ok(manifest)
}

/// 构建 AddonManifest 从 info map（内部辅助函数）
fn build_manifest_from_info(
    fallback_name: &str,
    addon_path: PathBuf,
    info: BTreeMap<String, PhpValue>,
) -> AddonLoaderResult<AddonManifest> {
    let get_string =
        |key: &str| -> String { info.get(key).map(|v| v.as_string()).unwrap_or_default() };
    let get_int = |key: &str| -> i64 { info.get(key).map(|v| v.as_int()).unwrap_or(0) };

    // 对齐 PHP `Route::execute` 中的 status 检查：假值会抛 500
    let status = get_int("status");

    Ok(AddonManifest {
        name: get_string("name").if_empty(fallback_name),
        title: get_string("title"),
        identifier: get_string("identifier"),
        icon: get_string("icon"),
        author: get_string("author"),
        version: get_string("version"),
        admin: get_string("admin"),
        status,
        addon_path,
    })
}

/// 简单的 PHP 值类型（用于解析 $info 数组）
#[derive(Debug, Clone, PartialEq)]
enum PhpValue {
    /// 字符串值（单引号/双引号）
    Str(String),
    /// 整数值
    Int(i64),
    /// 布尔值
    Bool(bool),
}

impl PhpValue {
    /// 转字符串（对齐 PHP 字符串上下文转换）
    fn as_string(&self) -> String {
        match self {
            PhpValue::Str(s) => s.clone(),
            PhpValue::Int(i) => i.to_string(),
            PhpValue::Bool(b) => {
                if *b {
                    "1".to_string()
                } else {
                    "".to_string()
                }
            }
        }
    }

    /// 转整数（对齐 PHP 整数上下文转换）
    fn as_int(&self) -> i64 {
        match self {
            PhpValue::Str(s) => s.parse().unwrap_or(0),
            PhpValue::Int(i) => *i,
            PhpValue::Bool(b) => {
                if *b {
                    1
                } else {
                    0
                }
            }
        }
    }
}

/// 解析 Plugin.php 中的 `$info = [...]` 数组（对齐 PHP 子类 `$info` 属性）
///
/// ## 支持语法
///
/// ```php
/// public $info = [
///     'name' => 'operate',
///     'title' => '运营管理',
///     'status' => 1,
/// ];
/// ```
///
/// ## 解析策略
///
/// 1. 使用正则匹配 `$info\s*=\s*\[` 找到数组起始位置
/// 2. 从起始括号开始扫描，平衡括号匹配找到数组结束位置
/// 3. 逐行解析 `key => value` 对
/// 4. 支持字符串（单引号/双引号）、整数、布尔值
fn parse_php_info_array(content: &str) -> Option<BTreeMap<String, PhpValue>> {
    // 查找 $info = [ 的位置
    let info_regex = regex::Regex::new(r#"\$info\s*=\s*\["#).ok()?;
    let cap = info_regex.find(content)?;
    let array_start = cap.end() - 1; // 指向 '['

    // 平衡括号扫描找到匹配的 ']'
    let bytes = content.as_bytes();
    let mut depth = 0i32;
    let mut array_end = None;
    let mut in_string = false;
    let mut string_char = b'\0';
    let mut escape = false;

    for (i, &c) in bytes.iter().enumerate().skip(array_start) {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            if c == b'\\' {
                escape = true;
            } else if c == string_char {
                in_string = false;
            }
            continue;
        }

        match c {
            b'\'' | b'"' => {
                in_string = true;
                string_char = c;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    array_end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }

    let array_end = array_end?;
    let array_body = &content[array_start + 1..array_end];

    // 逐行解析 key => value
    let mut map = BTreeMap::new();
    parse_php_array_body(array_body, &mut map);
    Some(map)
}

/// 解析 PHP 数组体（`key => value, key => value,` 格式）
fn parse_php_array_body(body: &str, map: &mut BTreeMap<String, PhpValue>) {
    let mut chars = body.chars().peekable();
    let mut current_key: Option<String> = None;
    let mut buffer = String::new();

    while let Some(&c) = chars.peek() {
        match c {
            // 跳过空白和注释
            ' ' | '\t' | '\n' | '\r' | ',' => {
                chars.next();
            }
            // 字符串键或值
            '\'' | '"' => {
                let quote = c;
                chars.next(); // 消费引号
                let mut value = String::new();
                let mut escaped = false;
                while let Some(&cc) = chars.peek() {
                    if escaped {
                        match cc {
                            'n' => value.push('\n'),
                            't' => value.push('\t'),
                            'r' => value.push('\r'),
                            '\\' => value.push('\\'),
                            '\'' => value.push('\''),
                            '"' => value.push('"'),
                            _ => value.push(cc),
                        }
                        escaped = false;
                        chars.next();
                        continue;
                    }
                    if cc == '\\' {
                        escaped = true;
                        chars.next();
                        continue;
                    }
                    if cc == quote {
                        chars.next();
                        break;
                    }
                    value.push(cc);
                    chars.next();
                }

                // 检查后面是否跟着 =>
                skip_whitespace(&mut chars);
                if chars.peek() == Some(&'=') {
                    chars.next();
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        current_key = Some(value);
                    }
                } else {
                    if let Some(key) = current_key.take() {
                        map.insert(key, PhpValue::Str(value));
                    }
                }
            }
            // 数字
            '0'..='9' | '-' => {
                let mut num = String::new();
                while let Some(&cc) = chars.peek() {
                    if cc.is_ascii_digit() || cc == '-' || cc == '+' {
                        num.push(cc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(n) = num.parse::<i64>() {
                    if let Some(key) = current_key.take() {
                        map.insert(key, PhpValue::Int(n));
                    }
                }
            }
            // true/false/null
            't' | 'f' | 'n' => {
                let mut word = String::new();
                while let Some(&cc) = chars.peek() {
                    if cc.is_alphabetic() {
                        word.push(cc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = match word.as_str() {
                    "true" => PhpValue::Bool(true),
                    "false" => PhpValue::Bool(false),
                    "null" => PhpValue::Str(String::new()),
                    _ => {
                        // 未知单词，跳过
                        continue;
                    }
                };
                if let Some(key) = current_key.take() {
                    map.insert(key, value);
                }
            }
            // 标识符（可能是类常量或常量名）
            _ if c.is_alphabetic() || c == '_' => {
                let mut word = String::new();
                while let Some(&cc) = chars.peek() {
                    if cc.is_alphanumeric() || cc == '_' {
                        word.push(cc);
                        chars.next();
                    } else {
                        break;
                    }
                }
                buffer.push_str(&word);
            }
            _ => {
                chars.next();
            }
        }
    }

    let _ = buffer; // 避免 unused 警告
}

/// 跳过空白字符
fn skip_whitespace<I: Iterator<Item = char>>(iter: &mut std::iter::Peekable<I>) {
    while let Some(&c) = iter.peek() {
        if c.is_whitespace() {
            iter.next();
        } else {
            break;
        }
    }
}

/// 解析简单 INI 文件（对齐 PHP `parse_ini_file` 的扁平化行为）
///
/// ## 支持语法
///
/// ```ini
/// name = operate
/// title = "运营管理"
/// status = 1
/// ```
///
/// ## 不支持
///
/// - 分区（`[section]`）— 简化处理，仅返回扁平 key-value
/// - 转义序列（除引号包裹的字符串外）
fn parse_simple_ini(content: &str) -> BTreeMap<String, PhpValue> {
    let mut map = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw_value = line[eq_pos + 1..].trim();

            let value = if (raw_value.starts_with('"') && raw_value.ends_with('"'))
                || (raw_value.starts_with('\'') && raw_value.ends_with('\''))
            {
                PhpValue::Str(raw_value[1..raw_value.len() - 1].to_string())
            } else if raw_value == "true" {
                PhpValue::Bool(true)
            } else if raw_value == "false" {
                PhpValue::Bool(false)
            } else if let Ok(n) = raw_value.parse::<i64>() {
                PhpValue::Int(n)
            } else {
                PhpValue::Str(raw_value.to_string())
            };

            map.insert(key, value);
        }
    }

    map
}

/// String 扩展：空字符串时使用 fallback
trait IfEmpty {
    fn if_empty(self, fallback: &str) -> Self;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> Self {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 创建临时 PHP Plugin.php 文件用于测试
    fn make_test_plugin_php(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new()
            .suffix(".php")
            .tempfile()
            .expect("create temp file");
        file.write_all(content.as_bytes()).expect("write content");
        file
    }

    #[test]
    fn test_addon_manifest_new() {
        let manifest = AddonManifest::new("operate");
        assert_eq!(manifest.name, "operate");
        assert_eq!(manifest.title, "");
        assert_eq!(manifest.status, 0);
        assert!(!manifest.is_enabled());
    }

    #[test]
    fn test_is_enabled_status_zero() {
        let mut manifest = AddonManifest::new("test");
        manifest.status = 0;
        assert!(!manifest.is_enabled());
    }

    #[test]
    fn test_is_enabled_status_one() {
        let mut manifest = AddonManifest::new("test");
        manifest.status = 1;
        assert!(manifest.is_enabled());
    }

    #[test]
    fn test_is_enabled_status_two() {
        let mut manifest = AddonManifest::new("test");
        manifest.status = 2;
        assert!(manifest.is_enabled());
    }

    #[test]
    fn test_plugin_file_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(
            manifest.plugin_file(),
            PathBuf::from("/addons/operate/Plugin.php")
        );
    }

    #[test]
    fn test_info_ini_file_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(
            manifest.info_ini_file(),
            PathBuf::from("/addons/operate/info.ini")
        );
    }

    #[test]
    fn test_config_php_file_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(
            manifest.config_php_file(),
            PathBuf::from("/addons/operate/config.php")
        );
    }

    #[test]
    fn test_service_ini_file_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(
            manifest.service_ini_file(),
            PathBuf::from("/addons/operate/service.ini")
        );
    }

    #[test]
    fn test_view_dir_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(manifest.view_dir(), PathBuf::from("/addons/operate/view"));
    }

    #[test]
    fn test_controller_dir_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(
            manifest.controller_dir(),
            PathBuf::from("/addons/operate/controller")
        );
    }

    #[test]
    fn test_model_dir_path() {
        let mut manifest = AddonManifest::new("operate");
        manifest.addon_path = PathBuf::from("/addons/operate");
        assert_eq!(manifest.model_dir(), PathBuf::from("/addons/operate/model"));
    }

    #[test]
    fn test_php_value_string_conversion() {
        let s = PhpValue::Str("hello".to_string());
        assert_eq!(s.as_string(), "hello");
        assert_eq!(s.as_int(), 0);

        let i = PhpValue::Int(42);
        assert_eq!(i.as_string(), "42");
        assert_eq!(i.as_int(), 42);

        let b = PhpValue::Bool(true);
        assert_eq!(b.as_string(), "1");
        assert_eq!(b.as_int(), 1);

        let b2 = PhpValue::Bool(false);
        assert_eq!(b2.as_string(), "");
        assert_eq!(b2.as_int(), 0);
    }

    #[test]
    fn test_parse_php_info_array_basic() {
        let php = r#"<?php
namespace addons\operate;
use think\Addons;
class Plugin extends Addons {
    public $info = [
        'name' => 'operate',
        'title' => '运营管理',
        'status' => 1,
    ];
    public function install() {}
    public function uninstall() {}
}
"#;
        let info = parse_php_info_array(php);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.get("name").unwrap().as_string(), "operate");
        assert_eq!(info.get("title").unwrap().as_string(), "运营管理");
        assert_eq!(info.get("status").unwrap().as_int(), 1);
    }

    #[test]
    fn test_parse_php_info_array_double_quotes() {
        let php = r#"
public $info = [
    "name" => "test",
    "version" => "1.0.0",
];
"#;
        let info = parse_php_info_array(php);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.get("name").unwrap().as_string(), "test");
        assert_eq!(info.get("version").unwrap().as_string(), "1.0.0");
    }

    #[test]
    fn test_parse_php_info_array_no_info() {
        let php = r#"<?php
namespace addons\test;
class Plugin {
    public function install() {}
}
"#;
        assert!(parse_php_info_array(php).is_none());
    }

    #[test]
    fn test_parse_php_info_array_with_bool() {
        let php = r#"
public $info = [
    'enabled' => true,
    'debug' => false,
];
"#;
        let info = parse_php_info_array(php);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.get("enabled").unwrap().as_string(), "1");
        assert_eq!(info.get("debug").unwrap().as_string(), "");
    }

    #[test]
    fn test_parse_php_info_array_negative_int() {
        let php = r#"
public $info = [
    'order' => -5,
];
"#;
        let info = parse_php_info_array(php);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.get("order").unwrap().as_int(), -5);
    }

    #[test]
    fn test_parse_simple_ini_basic() {
        let ini = r#"
name = operate
title = "运营管理"
status = 1
# 注释
; 分号注释
"#;
        let map = parse_simple_ini(ini);
        assert_eq!(map.get("name").unwrap().as_string(), "operate");
        assert_eq!(map.get("title").unwrap().as_string(), "运营管理");
        assert_eq!(map.get("status").unwrap().as_int(), 1);
    }

    #[test]
    fn test_parse_simple_ini_bool() {
        let ini = "enabled = true\ndebug = false";
        let map = parse_simple_ini(ini);
        assert_eq!(map.get("enabled").unwrap().as_string(), "1");
        assert_eq!(map.get("debug").unwrap().as_string(), "");
    }

    #[test]
    fn test_parse_simple_ini_empty() {
        let map = parse_simple_ini("");
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_manifest_missing_plugin_file() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let result = parse_manifest(tmp.path());
        assert!(result.is_err());
        match result.unwrap_err() {
            AddonLoaderError::ManifestParse { .. } => {}
            other => panic!("expected ManifestParse, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_manifest_valid_plugin() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let plugin_path = tmp.path().join("Plugin.php");
        let php_content = r#"<?php
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
"#;
        std::fs::write(&plugin_path, php_content).expect("write Plugin.php");

        let result = parse_manifest(tmp.path());
        assert!(result.is_ok());
        let manifest = result.unwrap();
        assert_eq!(manifest.name, "operate");
        assert_eq!(manifest.title, "运营管理");
        assert_eq!(manifest.identifier, "operate.addon");
        assert_eq!(manifest.icon, "fa-cog");
        assert_eq!(manifest.author, "sz");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.admin, "operate/index/index");
        assert_eq!(manifest.status, 1);
        assert!(manifest.is_enabled());
    }

    #[test]
    fn test_parse_manifest_disabled_status() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let plugin_path = tmp.path().join("Plugin.php");
        let php_content = r#"
public $info = [
    'name' => 'disabled',
    'status' => 0,
];
"#;
        std::fs::write(&plugin_path, php_content).expect("write Plugin.php");

        let result = parse_manifest(tmp.path());
        assert!(result.is_ok());
        let manifest = result.unwrap();
        assert_eq!(manifest.name, "disabled");
        assert!(!manifest.is_enabled());
    }

    #[test]
    fn test_parse_manifest_with_info_ini_merge() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let plugin_path = tmp.path().join("Plugin.php");
        let php_content = r#"
public $info = [
    'name' => 'operate',
    'version' => '1.0.0',
];
"#;
        std::fs::write(&plugin_path, php_content).expect("write Plugin.php");

        // info.ini 覆盖 version
        let info_ini = tmp.path().join("info.ini");
        std::fs::write(&info_ini, "version = 2.0.0\nauthor = sz").expect("write info.ini");

        let result = parse_manifest(tmp.path());
        assert!(result.is_ok());
        let manifest = result.unwrap();
        assert_eq!(manifest.version, "2.0.0"); // 被 ini 覆盖
        assert_eq!(manifest.author, "sz"); // 来自 ini
    }

    #[test]
    fn test_parse_manifest_malformed_info() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let plugin_path = tmp.path().join("Plugin.php");
        let php_content = "<?php class Plugin {}";
        std::fs::write(&plugin_path, php_content).expect("write Plugin.php");

        let result = parse_manifest(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_if_empty_trait() {
        assert_eq!("hello".to_string().if_empty("fallback"), "hello");
        assert_eq!("".to_string().if_empty("fallback"), "fallback");
    }

    #[test]
    fn test_skip_whitespace() {
        let mut iter = "  hello".chars().peekable();
        skip_whitespace(&mut iter);
        assert_eq!(iter.peek(), Some(&'h'));
    }

    #[test]
    fn test_make_test_plugin_php() {
        let file = make_test_plugin_php("<?php echo 'hi';");
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("echo"));
    }

    #[test]
    fn test_manifest_serde() {
        let manifest = AddonManifest::new("test");
        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: AddonManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, deserialized);
    }

    #[test]
    fn test_manifest_clone_eq() {
        let m1 = AddonManifest::new("test");
        let m2 = m1.clone();
        assert_eq!(m1, m2);
    }
}
