//! `route:list` 命令 — 对齐 PHP `think route:list`
//!
//! ## PHP 对齐
//!
//! PHP `route:list` 输出所有注册的路由规则。Rust 端的路由通过 `RouterBuilder` 动态构建，
//! CLI 命令无法访问运行时路由表，因此输出预定义的应用映射和路径解析规则。
//!
//! ## 输出格式
//!
//! - `table`（默认）：表格格式
//! - `json`：JSON 格式

use crate::error::CliError;

/// 执行 route:list 命令
///
/// # 参数
///
/// - `format`：输出格式（`table` 或 `json`）
pub fn execute_route_list(format: &str) -> Result<(), CliError> {
    let routes = collect_routes();

    match format {
        "table" => print_table(&routes),
        "json" => print_json(&routes)?,
        _ => {
            return Err(CliError::Generic(format!(
                "Unsupported format: {} (supported: table, json)",
                format
            )));
        }
    }

    Ok(())
}

/// 路由信息
#[derive(Debug, Clone)]
struct RouteInfo {
    method: &'static str,
    path: &'static str,
    app: &'static str,
    controller: &'static str,
    action: &'static str,
}

/// 收集预定义路由
fn collect_routes() -> Vec<RouteInfo> {
    vec![
        RouteInfo {
            method: "GET",
            path: "/",
            app: "index",
            controller: "Index",
            action: "index",
        },
        RouteInfo {
            method: "GET",
            path: "/oapc/customer/index",
            app: "oapc",
            controller: "Customer",
            action: "index",
        },
        RouteInfo {
            method: "GET",
            path: "/admin/user/list",
            app: "admin",
            controller: "User",
            action: "list",
        },
        RouteInfo {
            method: "GET",
            path: "/api/goods/list",
            app: "api",
            controller: "Goods",
            action: "list",
        },
        RouteInfo {
            method: "POST",
            path: "/api/order/save",
            app: "api",
            controller: "Order",
            action: "save",
        },
        RouteInfo {
            method: "GET",
            path: "/farm/weight/index",
            app: "farm",
            controller: "Weight",
            action: "index",
        },
        RouteInfo {
            method: "GET",
            path: "/cashier/order/index",
            app: "cashier",
            controller: "Order",
            action: "index",
        },
        RouteInfo {
            method: "GET",
            path: "/scene/device/index",
            app: "scene",
            controller: "Device",
            action: "index",
        },
    ]
}

/// 表格格式输出（对齐 PHP route:list）
fn print_table(routes: &[RouteInfo]) {
    println!(
        "{:<6} {:<25} {:<8} {:<12} {:<10}",
        "Method", "Path", "App", "Controller", "Action"
    );
    println!("{}", "-".repeat(70));

    for route in routes {
        println!(
            "{:<6} {:<25} {:<8} {:<12} {:<10}",
            route.method, route.path, route.app, route.controller, route.action
        );
    }

    println!();
    println!("Registered apps: oapc, admin, api, farm, oapi, cashier, scene");
    println!("Default app: index | Default controller: Index | Default action: index");
}

/// JSON 格式输出
fn print_json(routes: &[RouteInfo]) -> Result<(), CliError> {
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

    let output = serde_json::to_string_pretty(&json)
        .map_err(|e| CliError::Generic(format!("JSON serialization failed: {}", e)))?;
    println!("{}", output);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_route_list_table_format() {
        let result = execute_route_list("table");
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_route_list_json_format() {
        let result = execute_route_list("json");
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_route_list_invalid_format() {
        let result = execute_route_list("xml");
        assert!(matches!(result, Err(CliError::Generic(_))));
    }

    #[test]
    fn test_collect_routes_not_empty() {
        let routes = collect_routes();
        assert!(!routes.is_empty());
    }

    #[test]
    fn test_collect_routes_contains_default_route() {
        let routes = collect_routes();
        assert!(routes.iter().any(|r| r.path == "/" && r.app == "index"));
    }

    #[test]
    fn test_collect_routes_contains_all_apps() {
        let routes = collect_routes();
        let apps: Vec<&str> = routes.iter().map(|r| r.app).collect();
        assert!(apps.contains(&"oapc"));
        assert!(apps.contains(&"admin"));
        assert!(apps.contains(&"api"));
        assert!(apps.contains(&"farm"));
        assert!(apps.contains(&"cashier"));
        assert!(apps.contains(&"scene"));
    }

    #[test]
    fn test_print_table_does_not_panic() {
        let routes = collect_routes();
        print_table(&routes);
    }

    #[test]
    fn test_print_json_valid_json() {
        let routes = collect_routes();
        let result = std::panic::catch_unwind(|| print_json(&routes));
        assert!(result.is_ok());
    }
}
