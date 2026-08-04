//! `optimize:route` / `optimize:config` / `optimize:schema` / `route:clear` 命令
//!
//! 对齐 PHP `think optimize:route` / `think optimize:config` / `think optimize:schema` / `think route:clear`。
//!
//! ## PHP 对齐
//!
//! PHP `optimize:route` 将路由配置编译为缓存文件（`runtime/route.php`），
//! 启动时直接加载缓存，跳过路由解析，加速分发。
//!
//! PHP `optimize:config` 将所有配置文件合并编译为缓存文件（`runtime/config.php`），
//! 启动时直接加载缓存，跳过逐文件扫描。
//!
//! PHP `optimize:schema` 扫描所有 Model，将数据表字段元数据缓存到文件
//! （`runtime/schema/`），避免运行时反复 `SHOW COLUMNS FROM` 查询。
//!
//! PHP `route:clear` 删除路由缓存文件，使下次启动重新解析路由。
//!
//! ## Rust 实现
//!
//! Rust 端路由通过代码注册（编译期确定），运行时无需解析配置文件。
//! 但为对齐 PHP 命令体系，本模块实现：
//!
//! 1. `optimize:route` — 收集路由元数据，序列化为 JSON 缓存文件
//!    （`runtime/cache/route_cache.json`），供 `route:list` 等命令快速查询
//! 2. `optimize:config` — 扫描配置目录，合并所有配置，序列化为 JSON 缓存文件
//!    （`runtime/cache/config_cache.json`），供运行时快速加载
//! 3. `optimize:schema` — 读取数据库连接配置，生成 schema 缓存索引文件
//!    （`runtime/schema_cache.json` + `runtime/schema_cache.php`），
//!    业务方运行时通过 `SchemaCache::remember_schema()` 填充具体字段信息
//! 4. `route:clear` — 删除路由缓存文件

use std::path::{Path, PathBuf};

use crate::cmd::route;
use crate::error::CliError;

/// 缓存目录（对齐 PHP `runtime/cache`）
const CACHE_DIR: &str = "runtime/cache";

/// 路由缓存文件名
const ROUTE_CACHE_FILE: &str = "route_cache.json";

/// 配置缓存文件名
const CONFIG_CACHE_FILE: &str = "config_cache.json";

/// 配置目录（对齐 PHP `config/`）
const CONFIG_DIR: &str = "config";

/// Schema 缓存文件名（对齐 PHP `think optimize:schema` 输出）
const SCHEMA_CACHE_FILE: &str = "schema_cache.json";

/// PHP 兼容的 schema 缓存索引文件名
const SCHEMA_CACHE_PHP_FILE: &str = "schema_cache.php";

/// 数据库配置文件名（对齐 PHP `config/database.php`，项目实际为 YAML）
const DATABASE_CONFIG_FILE: &str = "database.yml";

/// Schema 缓存运行时目录（对齐 PHP `runtime/`）
const RUNTIME_DIR: &str = "runtime";

/// 执行 optimize:route 命令
///
/// 收集路由元数据，序列化为 JSON 写入 `runtime/cache/route_cache.json`。
///
/// # 流程
///
/// 1. 收集预定义路由（复用 `route::collect_routes()`）
/// 2. 序列化为美化格式 JSON
/// 3. 写入缓存文件（自动创建父目录）
/// 4. 输出统计信息（路由数量、文件路径）
pub fn execute_optimize_route() -> Result<(), CliError> {
    let routes = route::collect_routes();
    let route_count = routes.len();

    // 序列化为 JSON（美化格式，便于人工审查）
    let json: Vec<serde_json::Value> = routes
        .iter()
        .map(|r| {
            serde_json::json!({
                "method": r.method,
                "path": r.path,
                "app": r.app,
                "controller": r.controller,
                "action": r.action,
            })
        })
        .collect();

    let content = serde_json::to_string_pretty(&json)
        .map_err(|e| CliError::Generic(format!("路由缓存序列化失败: {}", e)))?;

    let cache_path = get_route_cache_path();
    write_cache_file(&cache_path, &content)?;

    println!(
        "Route cache generated: {} route(s) → {}",
        route_count,
        cache_path.display()
    );
    Ok(())
}

/// 执行 optimize:config 命令
///
/// 扫描 `config/` 目录，合并所有 `.php`/`.yaml`/`.json` 配置，
/// 序列化为 JSON 写入 `runtime/cache/config_cache.json`。
///
/// # 流程
///
/// 1. 扫描 `config/` 目录
/// 2. 读取每个配置文件，解析为 JSON Value
/// 3. 以文件名（不含扩展名）为 key 合并到统一对象
/// 4. 序列化为美化格式 JSON
/// 5. 写入缓存文件
/// 6. 输出统计信息（配置项数量、文件路径）
pub fn execute_optimize_config() -> Result<(), CliError> {
    let config_dir = Path::new(CONFIG_DIR);

    if !config_dir.exists() {
        return Err(CliError::Generic(format!(
            "配置目录不存在: {}（请在项目根目录执行此命令）",
            config_dir.display()
        )));
    }

    let mut merged = serde_json::Map::new();
    let mut file_count = 0usize;

    let entries = std::fs::read_dir(config_dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // 仅处理文件（跳过子目录）
        if !path.is_file() {
            continue;
        }

        // 仅处理支持的配置文件格式
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "php" | "yaml" | "yml" | "json" | "toml") {
            continue;
        }

        // 以文件名（不含扩展名）为 key
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = std::fs::read_to_string(&path)?;
        let config_value = parse_config_file(&content, ext)?;
        merged.insert(stem, config_value);
        file_count += 1;
    }

    let content = serde_json::to_string_pretty(&serde_json::Value::Object(merged))
        .map_err(|e| CliError::Generic(format!("配置缓存序列化失败: {}", e)))?;

    let cache_path = get_config_cache_path();
    write_cache_file(&cache_path, &content)?;

    println!(
        "Config cache generated: {} file(s) → {}",
        file_count,
        cache_path.display()
    );
    Ok(())
}

/// 执行 route:clear 命令
///
/// 删除路由缓存文件 `runtime/cache/route_cache.json`。
/// 若文件不存在，输出提示但不报错。
pub fn execute_route_clear() -> Result<(), CliError> {
    let cache_path = get_route_cache_path();

    if !cache_path.exists() {
        println!("Route cache not found: {}", cache_path.display());
        println!("Nothing to clear.");
        return Ok(());
    }

    std::fs::remove_file(&cache_path)?;
    println!("Route cache cleared: {}", cache_path.display());
    Ok(())
}

/// 执行 optimize:schema 命令 — 生成数据表字段缓存文件
///
/// 对齐 PHP `think optimize:schema`：
/// - 扫描 `config/database.yml` 读取数据库连接配置与表前缀
/// - 生成 schema 缓存索引文件（`runtime/schema_cache.json`）
/// - 同时生成 PHP 兼容的 schema 缓存索引文件（`runtime/schema_cache.php`）
/// - 业务方运行时通过 `SchemaCache::remember_schema()` 填充具体字段信息
///
/// # 流程
///
/// 1. 读取 `config/database.yml`，提取所有连接名、数据库与表前缀
/// 2. 序列化为美化格式 JSON（含生成时间戳、连接列表、空 tables 数组）
/// 3. 写入 `runtime/schema_cache.json`
/// 4. 生成 PHP 兼容索引文件 `runtime/schema_cache.php`
/// 5. 输出统计信息（连接数量、文件路径）
///
/// # 说明
///
/// Rust 无法运行时反射获取所有 `Model` 实现类型，因此本命令生成的是 schema
/// 缓存索引占位文件（`tables` 为空数组），具体字段元数据由运行时
/// `SchemaCache::remember_schema()` 在首次访问表时回源加载并填充。
pub fn execute_optimize_schema() -> Result<(), CliError> {
    let (default_connection, connections) = read_database_connections()?;
    let connection_count = connections.len();
    let generated_at = chrono::Utc::now().to_rfc3339();

    // 生成 JSON 缓存索引（tables 为空，运行时由 SchemaCache 填充）
    let cache = serde_json::json!({
        "generated_at": generated_at,
        "default_connection": default_connection,
        "connections": connections,
        "tables": [],
    });

    let content = serde_json::to_string_pretty(&cache)
        .map_err(|e| CliError::Generic(format!("schema 缓存序列化失败: {}", e)))?;

    let cache_path = get_schema_cache_path();
    write_cache_file(&cache_path, &content)?;

    // 生成 PHP 兼容索引文件（对齐 PHP schema 缓存文件格式）
    let php_content = build_php_schema_index(&generated_at, &connections);
    let php_path = get_schema_cache_php_path();
    write_cache_file(&php_path, &php_content)?;

    println!(
        "Schema cache generated: {} connection(s) → {}",
        connection_count,
        cache_path.display()
    );
    Ok(())
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 获取路由缓存文件路径
pub fn get_route_cache_path() -> PathBuf {
    PathBuf::from(CACHE_DIR).join(ROUTE_CACHE_FILE)
}

/// 获取配置缓存文件路径
pub fn get_config_cache_path() -> PathBuf {
    PathBuf::from(CACHE_DIR).join(CONFIG_CACHE_FILE)
}

/// 获取 schema 缓存文件路径（`runtime/schema_cache.json`）
pub fn get_schema_cache_path() -> PathBuf {
    PathBuf::from(RUNTIME_DIR).join(SCHEMA_CACHE_FILE)
}

/// 获取 PHP 兼容 schema 缓存索引文件路径（`runtime/schema_cache.php`）
pub fn get_schema_cache_php_path() -> PathBuf {
    PathBuf::from(RUNTIME_DIR).join(SCHEMA_CACHE_PHP_FILE)
}

/// 读取数据库连接配置
///
/// 扫描 `config/database.yml`，提取所有连接名、数据库名与表前缀。
/// 返回 `(默认连接名, 连接信息列表)`。
///
/// 若配置文件不存在，返回空列表（生成空 schema 缓存索引，不报错）。
fn read_database_connections() -> Result<(String, Vec<serde_json::Value>), CliError> {
    let path = Path::new(CONFIG_DIR).join(DATABASE_CONFIG_FILE);

    if !path.exists() {
        // 配置文件不存在，返回空连接列表（对齐：optimize:schema 即使无配置也生成占位缓存）
        return Ok((String::new(), Vec::new()));
    }

    let content = std::fs::read_to_string(&path)?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| CliError::Generic(format!("数据库配置解析失败: {}", e)))?;
    let json = serde_json::to_value(yaml)
        .map_err(|e| CliError::Generic(format!("YAML→JSON 转换失败: {}", e)))?;

    let default_connection = json
        .get("default")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut connections: Vec<serde_json::Value> = Vec::new();
    if let Some(conns) = json.get("connections").and_then(|c| c.as_object()) {
        for (name, info) in conns {
            connections.push(serde_json::json!({
                "name": name,
                "database": info.get("database").and_then(|v| v.as_str()).unwrap_or(""),
                "prefix": info.get("prefix").and_then(|v| v.as_str()).unwrap_or(""),
                "type": info.get("type").and_then(|v| v.as_str()).unwrap_or(""),
            }));
        }
    }

    // 按连接名排序，保证输出稳定（便于人工审查与测试断言）
    connections.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    Ok((default_connection, connections))
}

/// 构建 PHP 兼容的 schema 缓存索引文件内容
///
/// 对齐 PHP `think optimize:schema` 输出的 schema 缓存文件格式
/// （`<?php return [...]`），作为索引占位，运行时由 `SchemaCache` 填充。
fn build_php_schema_index(generated_at: &str, connections: &[serde_json::Value]) -> String {
    let mut buf = String::new();
    buf.push_str("<?php\n");
    buf.push_str("// Schema 缓存索引 — 由 sz-rust optimize:schema 生成\n");
    buf.push_str("// 生成时间: ");
    buf.push_str(generated_at);
    buf.push('\n');
    buf.push_str("// 业务方运行时通过 SchemaCache::remember_schema() 填充具体字段信息\n\n");

    buf.push_str("return [\n");
    buf.push_str("    'generated_at' => '");
    buf.push_str(generated_at);
    buf.push_str("',\n");

    // 连接列表（空时紧凑输出 `[]`，对齐 PHP 空数组风格）
    if connections.is_empty() {
        buf.push_str("    'connections' => [],\n");
    } else {
        buf.push_str("    'connections' => [\n");
        for conn in connections {
            let name = conn["name"].as_str().unwrap_or("");
            let database = conn["database"].as_str().unwrap_or("");
            let prefix = conn["prefix"].as_str().unwrap_or("");
            buf.push_str(&format!(
                "        ['name' => '{}', 'database' => '{}', 'prefix' => '{}'],\n",
                name, database, prefix
            ));
        }
        buf.push_str("    ],\n");
    }

    // 表字段缓存占位（运行时填充）
    buf.push_str("    'tables' => [],\n");
    buf.push_str("];\n");

    buf
}

/// 写入缓存文件（自动创建父目录）
fn write_cache_file(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// 根据扩展名解析配置文件内容为 JSON Value
///
/// 支持格式：
/// - `json` — 直接解析
/// - `yaml`/`yml` — YAML 转 JSON
/// - `php`/`toml` — 提取为字符串（无法在 CLI 中安全解析 PHP，保留原始内容）
fn parse_config_file(content: &str, ext: &str) -> Result<serde_json::Value, CliError> {
    match ext {
        "json" => serde_json::from_str(content)
            .map_err(|e| CliError::Generic(format!("JSON 配置解析失败: {}", e))),
        "yaml" | "yml" => {
            // YAML 解析：使用 serde_yaml 转 JSON
            let yaml: serde_yaml::Value = serde_yaml::from_str(content)
                .map_err(|e| CliError::Generic(format!("YAML 配置解析失败: {}", e)))?;
            serde_json::to_value(yaml)
                .map_err(|e| CliError::Generic(format!("YAML→JSON 转换失败: {}", e)))
        }
        "php" | "toml" => {
            // PHP/TOML 配置无法在 CLI 中安全解析，保留为原始字符串
            // 运行时由应用自行解析
            Ok(serde_json::Value::String(content.to_string()))
        }
        _ => Ok(serde_json::Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_route_cache_path() {
        let path = get_route_cache_path();
        assert!(path.ends_with("runtime/cache/route_cache.json"));
    }

    #[test]
    fn test_get_config_cache_path() {
        let path = get_config_cache_path();
        assert!(path.ends_with("runtime/cache/config_cache.json"));
    }

    #[test]
    fn test_write_cache_file_creates_parent_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested").join("deep").join("cache.json");

        write_cache_file(&nested, r#"{"key":"value"}"#).unwrap();

        assert!(nested.exists());
        let content = std::fs::read_to_string(&nested).unwrap();
        assert_eq!(content, r#"{"key":"value"}"#);
    }

    #[test]
    fn test_parse_config_file_json() {
        let json = r#"{"name":"app","port":8080}"#;
        let value = parse_config_file(json, "json").unwrap();
        assert_eq!(value["name"], "app");
        assert_eq!(value["port"], 8080);
    }

    #[test]
    fn test_parse_config_file_yaml() {
        let yaml = "name: app\nport: 8080\n";
        let value = parse_config_file(yaml, "yaml").unwrap();
        assert_eq!(value["name"], "app");
        assert_eq!(value["port"], 8080);
    }

    #[test]
    fn test_parse_config_file_php_preserves_raw_content() {
        let php = "<?php return ['name' => 'app'];";
        let value = parse_config_file(php, "php").unwrap();
        assert!(value.is_string());
        assert!(value.as_str().unwrap().contains("<?php"));
    }

    #[test]
    fn test_parse_config_file_toml_preserves_raw_content() {
        let toml = "[server]\nport = 8080\n";
        let value = parse_config_file(toml, "toml").unwrap();
        assert!(value.is_string());
        assert!(value.as_str().unwrap().contains("[server]"));
    }

    #[test]
    fn test_parse_config_file_unsupported_returns_null() {
        let value = parse_config_file("content", "txt").unwrap();
        assert!(value.is_null());
    }

    #[test]
    fn test_parse_config_file_invalid_json() {
        let result = parse_config_file("{invalid}", "json");
        assert!(matches!(result, Err(CliError::Generic(_))));
    }

    #[test]
    fn test_parse_config_file_invalid_yaml() {
        let result = parse_config_file(":\n : bad", "yaml");
        assert!(matches!(result, Err(CliError::Generic(_))));
    }

    #[test]
    fn test_execute_optimize_route_creates_cache_file() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        execute_optimize_route().unwrap();

        let cache_path = get_route_cache_path();
        assert!(cache_path.exists());

        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.is_array());
        assert!(!json.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_execute_route_clear_removes_cache_file() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 先生成缓存
        execute_optimize_route().unwrap();
        assert!(get_route_cache_path().exists());

        // 清除缓存
        execute_route_clear().unwrap();
        assert!(!get_route_cache_path().exists());
    }

    #[test]
    fn test_execute_route_clear_nonexistent_cache() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 缓存不存在时应返回 Ok
        let result = execute_route_clear();
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_optimize_config_no_config_dir() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // config/ 目录不存在时应返回错误
        let result = execute_optimize_config();
        assert!(matches!(result, Err(CliError::Generic(_))));
    }

    #[test]
    fn test_execute_optimize_config_with_json_files() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 创建 config/ 目录及配置文件
        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("app.json"),
            r#"{"name":"test","debug":true}"#,
        )
        .unwrap();
        std::fs::write(
            config_dir.join("database.json"),
            r#"{"host":"localhost","port":5432}"#,
        )
        .unwrap();

        execute_optimize_config().unwrap();

        let cache_path = get_config_cache_path();
        assert!(cache_path.exists());

        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["app"]["name"], "test");
        assert_eq!(json["app"]["debug"], true);
        assert_eq!(json["database"]["host"], "localhost");
        assert_eq!(json["database"]["port"], 5432);
    }

    #[test]
    fn test_execute_optimize_config_with_yaml_files() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("cache.yaml"), "driver: redis\nttl: 3600\n").unwrap();

        execute_optimize_config().unwrap();

        let cache_path = get_config_cache_path();
        assert!(cache_path.exists());

        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["cache"]["driver"], "redis");
        assert_eq!(json["cache"]["ttl"], 3600);
    }

    // ---------- optimize:schema 测试 ----------

    #[test]
    fn test_get_schema_cache_path() {
        let path = get_schema_cache_path();
        assert!(path.ends_with("runtime/schema_cache.json"));
    }

    #[test]
    fn test_get_schema_cache_php_path() {
        let path = get_schema_cache_php_path();
        assert!(path.ends_with("runtime/schema_cache.php"));
    }

    #[test]
    fn test_execute_optimize_schema_no_config() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 无 config/database.yml 时也应成功生成空 schema 缓存索引
        execute_optimize_schema().unwrap();

        let cache_path = get_schema_cache_path();
        assert!(cache_path.exists());

        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.is_object());
        assert!(json["generated_at"].is_string());
        assert!(json["tables"].is_array());
        assert_eq!(json["tables"].as_array().unwrap().len(), 0);
        // 无配置时默认连接名为空、连接列表为空数组
        assert_eq!(json["default_connection"].as_str(), Some(""));
        assert!(json["connections"].is_array());
        assert_eq!(json["connections"].as_array().unwrap().len(), 0);

        // PHP 兼容索引文件也应生成
        let php_path = get_schema_cache_php_path();
        assert!(php_path.exists());
        let php_content = std::fs::read_to_string(&php_path).unwrap();
        assert!(php_content.starts_with("<?php"));
        assert!(php_content.contains("return ["));
        assert!(php_content.contains("'tables' => []"));
    }

    #[test]
    fn test_optimize_schema_generates_valid_json() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 创建 config/database.yml（含两个连接）
        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("database.yml"),
            "default: mysql\n\
             connections:\n\
             \x20 mysql:\n\
             \x20   type: mysql\n\
             \x20   database: shop\n\
             \x20   prefix: sz_\n\
             \x20 food:\n\
             \x20   type: mysql\n\
             \x20   database: food\n\
             \x20   prefix: sz_food_\n",
        )
        .unwrap();

        execute_optimize_schema().unwrap();

        let cache_path = get_schema_cache_path();
        assert!(cache_path.exists());

        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        // 基本字段校验
        assert!(json["generated_at"].is_string());
        assert_eq!(json["default_connection"].as_str(), Some("mysql"));
        assert!(json["tables"].is_array());
        assert_eq!(json["tables"].as_array().unwrap().len(), 0);

        // 连接列表按名称排序：food 在 mysql 之前
        let conns = json["connections"].as_array().unwrap();
        assert_eq!(conns.len(), 2);
        assert_eq!(conns[0]["name"].as_str(), Some("food"));
        assert_eq!(conns[0]["prefix"].as_str(), Some("sz_food_"));
        assert_eq!(conns[1]["name"].as_str(), Some("mysql"));
        assert_eq!(conns[1]["database"].as_str(), Some("shop"));
        assert_eq!(conns[1]["prefix"].as_str(), Some("sz_"));

        // PHP 索引文件包含连接信息
        let php_path = get_schema_cache_php_path();
        let php_content = std::fs::read_to_string(&php_path).unwrap();
        assert!(php_content.contains("'name' => 'mysql'"));
        assert!(php_content.contains("'prefix' => 'sz_'"));
        assert!(php_content.contains("'name' => 'food'"));
        assert!(php_content.contains("'database' => 'shop'"));
    }

    #[test]
    fn test_read_database_connections_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        // 配置文件不存在时返回空列表
        let (default, conns) = read_database_connections().unwrap();
        assert_eq!(default, "");
        assert!(conns.is_empty());
    }

    #[test]
    fn test_read_database_connections_invalid_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("database.yml"), ":\n : bad").unwrap();

        let result = read_database_connections();
        assert!(matches!(result, Err(CliError::Generic(_))));
    }

    #[test]
    fn test_build_php_schema_index_empty() {
        let content = build_php_schema_index("2026-07-31T00:00:00+00:00", &[]);
        assert!(content.starts_with("<?php"));
        assert!(content.contains("'generated_at' => '2026-07-31T00:00:00+00:00'"));
        assert!(content.contains("'connections' => []"));
        assert!(content.contains("'tables' => []"));
    }

    #[test]
    fn test_build_php_schema_index_with_connections() {
        let connections = vec![
            serde_json::json!({"name": "mysql", "database": "shop", "prefix": "sz_", "type": "mysql"}),
            serde_json::json!({"name": "food", "database": "food", "prefix": "sz_food_", "type": "mysql"}),
        ];
        let content = build_php_schema_index("2026-07-31T00:00:00+00:00", &connections);
        assert!(content.contains("'name' => 'mysql'"));
        assert!(content.contains("'database' => 'shop'"));
        assert!(content.contains("'prefix' => 'sz_'"));
        assert!(content.contains("'name' => 'food'"));
        assert!(content.contains("'prefix' => 'sz_food_'"));
    }

    #[test]
    fn test_execute_optimize_config_skips_unsupported_files() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = CwdGuard::switch(temp.path()).unwrap();

        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("app.json"), r#"{"name":"test"}"#).unwrap();
        // .txt 文件应被跳过
        std::fs::write(config_dir.join("readme.txt"), "not a config").unwrap();

        execute_optimize_config().unwrap();

        let cache_path = get_config_cache_path();
        let content = std::fs::read_to_string(&cache_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        // 只应有 app 这一个配置项
        assert_eq!(json.as_object().unwrap().len(), 1);
        assert!(json.get("app").is_some());
        assert!(json.get("readme").is_none());
    }

    // ---------- 辅助：CwdGuard（避免并行测试工作目录污染） ----------

    use std::sync::MutexGuard;

    /// RAII 守卫：在作用域结束时恢复原始工作目录并释放锁
    struct CwdGuard {
        original: Option<PathBuf>,
        _lock: MutexGuard<'static, ()>,
    }

    impl CwdGuard {
        fn switch(new_dir: &Path) -> std::io::Result<Self> {
            // 使用全局互斥锁，避免与 make 模块测试的 set_current_dir 并行竞态
            let lock = super::super::test_support::acquire_global_lock();
            let original = std::env::current_dir().ok();
            std::env::set_current_dir(new_dir)?;
            Ok(Self {
                original,
                _lock: lock,
            })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if let Some(ref orig) = self.original {
                let _ = std::env::set_current_dir(orig);
            }
        }
    }
}
