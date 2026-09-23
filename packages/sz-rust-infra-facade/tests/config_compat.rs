// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T066 — 配置文件格式向后兼容验证
//!
//! 验证 v1.2.0 配置（不含 v1.3.0 新增的 5 个 section）在 v1.3.0 可正常加载，
//! 新 section 取默认值，不破坏既有字段。

use sz_rust_infra_facade::config::{
    ApiGatewaySection, AppConfig, ConfigCenterSection, DistributedTxSection, OpsApiSection,
    ServiceRegistrySection,
};

// ============================================================================
// 默认值验证 — 5 个新 section 的 Default 实现
// ============================================================================

#[test]
fn test_config_center_default() {
    let s = ConfigCenterSection::default();
    assert!(!s.enabled, "默认禁用");
    assert!(s.endpoints.is_empty());
    assert_eq!(s.namespace, "public");
    assert_eq!(s.timeout_ms, 5_000);
    assert_eq!(s.retry_count, 3);
}

#[test]
fn test_service_registry_default() {
    let s = ServiceRegistrySection::default();
    assert!(!s.enabled);
    assert_eq!(s.backend, "nacos");
    assert!(s.endpoint.is_empty());
    assert_eq!(s.namespace, "public");
    assert_eq!(s.heartbeat_interval_secs, 10);
}

#[test]
fn test_distributed_tx_default() {
    let s = DistributedTxSection::default();
    assert!(!s.enabled);
    assert_eq!(s.backend, "seata");
    assert!(s.endpoint.is_empty());
    assert_eq!(s.timeout_ms, 60_000);
    assert_eq!(s.retry_count, 3);
    assert_eq!(s.log_path, "logs/dtx");
}

#[test]
fn test_api_gateway_default() {
    let s = ApiGatewaySection::default();
    assert!(!s.enabled);
    assert!(s.routes.is_empty());
    assert_eq!(s.rate_limit_rps, 0);
    assert!(!s.cors_enabled);
    assert_eq!(s.cors_allowed_origins, vec!["*".to_string()]);
}

#[test]
fn test_ops_api_default() {
    let s = OpsApiSection::default();
    assert!(s.enabled, "运维 API 默认启用");
    assert_eq!(s.path, "/ops");
    assert!(s.auth_token.is_empty());
    assert!(s.metrics_enabled);
    assert!(s.health_check_enabled);
}

// ============================================================================
// 向后兼容核心场景 — v1.2.0 配置（仅含旧 section）在 v1.3.0 加载
// ============================================================================

/// v1.2.0 完整配置（不含任何 v1.3.0 新增 section）必须能正常反序列化，
/// 且新 section 全部取默认值。
#[test]
fn test_v1_2_config_loads_in_v1_3() {
    // 模拟 v1.2.0 时代的 config/app.yml + database.yml 等合并后的顶层 YAML
    // 注意：不包含 config_center / service_registry / distributed_tx /
    // api_gateway / ops_api 任何一个 key
    let yaml_v1_2 = r#"
app:
  default_app: api
  auto_multi_app: true
database:
  default: mysql
  connections:
    mysql:
      type: mysql
      hostname: localhost
      database: testdb
      username: root
      password: secret
      hostport: 3306
cache:
  default: memory
log:
  default: file
server:
  host: 0.0.0.0
  port: 8080
"#;

    let config: AppConfig = serde_yaml::from_str(yaml_v1_2).unwrap();

    // 旧字段正常加载
    assert_eq!(config.app.default_app, "api");
    assert!(config.app.auto_multi_app);
    assert_eq!(config.database.default, "mysql");
    assert!(config.database.connections.contains_key("mysql"));
    assert_eq!(config.cache.default, "memory");
    assert_eq!(config.log.default, "file");
    assert_eq!(config.server.port, 8080);

    // 新 section 全部取默认值（向后兼容的关键）
    assert_eq!(config.config_center, ConfigCenterSection::default());
    assert_eq!(config.service_registry, ServiceRegistrySection::default());
    assert_eq!(config.distributed_tx, DistributedTxSection::default());
    assert_eq!(config.api_gateway, ApiGatewaySection::default());
    assert_eq!(config.ops_api, OpsApiSection::default());
}

/// 空配置（v1.2.0 极简场景）也能加载，全部取默认值
#[test]
fn test_empty_config_loads_with_defaults() {
    let yaml_empty = "";
    let config: AppConfig = serde_yaml::from_str(yaml_empty).unwrap();
    assert_eq!(config.app.default_app, "index");
    assert_eq!(config.database.default, "mysql");
    assert_eq!(config.config_center, ConfigCenterSection::default());
    assert_eq!(config.service_registry, ServiceRegistrySection::default());
    assert_eq!(config.distributed_tx, DistributedTxSection::default());
    assert_eq!(config.api_gateway, ApiGatewaySection::default());
    assert_eq!(config.ops_api, OpsApiSection::default());
}

// ============================================================================
// 部分新 section 存在 — 只配置部分新 section，其余取默认值
// ============================================================================

#[test]
fn test_partial_new_sections_loads() {
    let yaml = r#"
app:
  default_app: farm
config_center:
  enabled: true
  endpoints:
    - http://nacos-1:8848
    - http://nacos-2:8848
  namespace: production
ops_api:
  enabled: true
  auth_token: ops-secret-token
  metrics_enabled: true
"#;
    let config: AppConfig = serde_yaml::from_str(yaml).unwrap();

    // 旧字段
    assert_eq!(config.app.default_app, "farm");

    // 已配置的新 section
    assert!(config.config_center.enabled);
    assert_eq!(
        config.config_center.endpoints,
        vec![
            "http://nacos-1:8848".to_string(),
            "http://nacos-2:8848".to_string()
        ]
    );
    assert_eq!(config.config_center.namespace, "production");
    // 未配置的字段取默认值
    assert_eq!(config.config_center.timeout_ms, 5_000);
    assert_eq!(config.config_center.retry_count, 3);

    assert!(config.ops_api.enabled);
    assert_eq!(config.ops_api.auth_token, "ops-secret-token");
    assert!(config.ops_api.metrics_enabled);
    // 未配置的字段取默认值
    assert_eq!(config.ops_api.path, "/ops");
    assert!(config.ops_api.health_check_enabled);

    // 未配置的新 section 取默认值
    assert_eq!(config.service_registry, ServiceRegistrySection::default());
    assert_eq!(config.distributed_tx, DistributedTxSection::default());
    assert_eq!(config.api_gateway, ApiGatewaySection::default());
}

// ============================================================================
// 全部新 section 完整配置 — 验证完整反序列化
// ============================================================================

#[test]
fn test_all_new_sections_full_config() {
    let yaml = r#"
config_center:
  enabled: true
  endpoints:
    - http://nacos:8848
  namespace: prod
  timeout_ms: 3000
  retry_count: 5
service_registry:
  enabled: true
  backend: consul
  endpoint: http://consul:8500
  namespace: services
  heartbeat_interval_secs: 5
distributed_tx:
  enabled: true
  backend: seata
  endpoint: http://seata:8091
  timeout_ms: 30000
  retry_count: 2
  log_path: /var/log/dtx
api_gateway:
  enabled: true
  routes:
    - id: r1
      path_prefix: /api/v1
      upstream: http://backend:8080
      weight: 100
  rate_limit_rps: 1000
  cors_enabled: true
  cors_allowed_origins:
    - https://example.com
ops_api:
  enabled: false
  path: /admin/ops
  auth_token: super-secret
  metrics_enabled: false
  health_check_enabled: true
"#;
    let config: AppConfig = serde_yaml::from_str(yaml).unwrap();

    // config_center
    assert!(config.config_center.enabled);
    assert_eq!(config.config_center.endpoints.len(), 1);
    assert_eq!(config.config_center.namespace, "prod");
    assert_eq!(config.config_center.timeout_ms, 3_000);
    assert_eq!(config.config_center.retry_count, 5);

    // service_registry
    assert!(config.service_registry.enabled);
    assert_eq!(config.service_registry.backend, "consul");
    assert_eq!(config.service_registry.endpoint, "http://consul:8500");
    assert_eq!(config.service_registry.namespace, "services");
    assert_eq!(config.service_registry.heartbeat_interval_secs, 5);

    // distributed_tx
    assert!(config.distributed_tx.enabled);
    assert_eq!(config.distributed_tx.backend, "seata");
    assert_eq!(config.distributed_tx.endpoint, "http://seata:8091");
    assert_eq!(config.distributed_tx.timeout_ms, 30_000);
    assert_eq!(config.distributed_tx.retry_count, 2);
    assert_eq!(config.distributed_tx.log_path, "/var/log/dtx");

    // api_gateway
    assert!(config.api_gateway.enabled);
    assert_eq!(config.api_gateway.routes.len(), 1);
    assert_eq!(config.api_gateway.routes[0].id, "r1");
    assert_eq!(config.api_gateway.routes[0].path_prefix, "/api/v1");
    assert_eq!(config.api_gateway.routes[0].upstream, "http://backend:8080");
    assert_eq!(config.api_gateway.routes[0].weight, 100);
    assert_eq!(config.api_gateway.rate_limit_rps, 1000);
    assert!(config.api_gateway.cors_enabled);
    assert_eq!(
        config.api_gateway.cors_allowed_origins,
        vec!["https://example.com".to_string()]
    );

    // ops_api
    assert!(!config.ops_api.enabled);
    assert_eq!(config.ops_api.path, "/admin/ops");
    assert_eq!(config.ops_api.auth_token, "super-secret");
    assert!(!config.ops_api.metrics_enabled);
    assert!(config.ops_api.health_check_enabled);
}

// ============================================================================
// 从目录加载 — v1.2.0 风格目录（无新 section 文件）兼容
// ============================================================================

#[tokio::test]
async fn test_load_from_dir_v1_2_style() {
    let dir = std::env::temp_dir().join("sz_rust_t066_compat_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 只写 v1.2.0 时代的配置文件，不写任何 v1.3.0 新增的 yml
    std::fs::write(
        dir.join("app.yml"),
        "default_app: api\nauto_multi_app: true\n",
    )
    .unwrap();
    std::fs::write(dir.join("server.yml"), "host: 127.0.0.1\nport: 9090\n").unwrap();

    let config = AppConfig::load_from_dir(&dir).await.unwrap();

    // 旧字段从文件加载
    assert_eq!(config.app.default_app, "api");
    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 9090);

    // 新 section 文件不存在 → 取默认值（向后兼容）
    assert_eq!(config.config_center, ConfigCenterSection::default());
    assert_eq!(config.service_registry, ServiceRegistrySection::default());
    assert_eq!(config.distributed_tx, DistributedTxSection::default());
    assert_eq!(config.api_gateway, ApiGatewaySection::default());
    assert_eq!(config.ops_api, OpsApiSection::default());

    // 清理测试产物（铁律：测试文件及时删除）
    let _ = std::fs::remove_dir_all(&dir);
}

/// 从目录加载含新 section 文件的场景
#[tokio::test]
async fn test_load_from_dir_v1_3_with_new_sections() {
    let dir = std::env::temp_dir().join("sz_rust_t066_v13_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("app.yml"), "default_app: oapc\n").unwrap();
    std::fs::write(
        dir.join("config_center.yml"),
        "enabled: true\nnamespace: staging\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("ops_api.yml"),
        "enabled: true\nauth_token: dir-token\n",
    )
    .unwrap();

    let config = AppConfig::load_from_dir(&dir).await.unwrap();

    assert_eq!(config.app.default_app, "oapc");
    assert!(config.config_center.enabled);
    assert_eq!(config.config_center.namespace, "staging");
    assert_eq!(config.config_center.timeout_ms, 5_000); // 未配置 → 默认
    assert!(config.ops_api.enabled);
    assert_eq!(config.ops_api.auth_token, "dir-token");

    // 未提供文件的新 section 取默认值
    assert_eq!(config.service_registry, ServiceRegistrySection::default());
    assert_eq!(config.distributed_tx, DistributedTxSection::default());
    assert_eq!(config.api_gateway, ApiGatewaySection::default());

    let _ = std::fs::remove_dir_all(&dir);
}

// ============================================================================
// Debug 脱敏验证 — OpsApiSection 的 auth_token 不应出现在 Debug 输出中
// ============================================================================

#[test]
fn test_ops_api_debug_redacts_auth_token() {
    let s = OpsApiSection {
        auth_token: "super-secret-token-12345".to_string(),
        ..Default::default()
    };
    let debug_str = format!("{:?}", s);
    assert!(
        !debug_str.contains("super-secret-token-12345"),
        "auth_token 不应出现在 Debug 输出中，实际: {debug_str}"
    );
    assert!(
        debug_str.contains("***"),
        "Debug 输出应包含 *** 脱敏标记，实际: {debug_str}"
    );
}

#[test]
fn test_app_config_debug_redacts_ops_api_token() {
    let mut config = AppConfig::default();
    config.ops_api.auth_token = "leaked-secret".to_string();
    let debug_str = format!("{:?}", config);
    assert!(
        !debug_str.contains("leaked-secret"),
        "AppConfig Debug 不应泄露 ops_api.auth_token，实际: {debug_str}"
    );
}
