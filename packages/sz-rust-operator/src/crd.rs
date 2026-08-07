//! SzRustApp CRD — 定义 sz-rust 应用的 Kubernetes 自定义资源
//!
//! ## CRD 定义
//!
//! ```yaml
//! apiVersion: sz-rust.dev/v1
//! kind: SzRustApp
//! metadata:
//!   name: my-app
//! spec:
//!   image: ghcr.io/ljclz/sz-rust:latest
//!   replicas: 3
//!   port: 8080
//!   env:
//!     DATABASE_URL: postgres://...
//!     REDIS_URL: redis://...
//! status:
//!   ready: true
//!   replicas: 3
//! ```

use kube::core::crd::CustomResourceExt;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================================
// SzRustApp CRD
// ============================================================================

/// SzRustApp 自定义资源 — 描述一个 sz-rust 应用部署
///
/// Operator watch 此资源，根据 spec 创建/更新/删除对应的 Deployment + Service。
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "sz-rust.dev",
    version = "v1",
    kind = "SzRustApp",
    namespaced,
    status = "SzRustAppStatus",
    shortname = "szapp"
)]
pub struct SzRustAppSpec {
    /// 容器镜像地址
    pub image: String,

    /// 期望副本数（默认 1）
    #[serde(default = "default_replicas")]
    pub replicas: i32,

    /// 服务端口（默认 8080）
    #[serde(default = "default_port")]
    pub port: u16,

    /// 环境变量
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,

    /// 资源限制
    #[serde(default)]
    pub resources: Option<ResourceRequirements>,

    /// 数据库配置
    #[serde(default)]
    pub database: Option<DatabaseConfig>,

    /// Redis 配置
    #[serde(default)]
    pub redis: Option<RedisConfig>,
}

/// SzRustApp 状态
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema, Default)]
pub struct SzRustAppStatus {
    /// 是否就绪
    pub ready: bool,

    /// 当前运行副本数
    pub replicas: i32,

    /// 条件列表
    #[serde(default)]
    pub conditions: Vec<SzRustAppCondition>,
}

/// 资源需求
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema, Default)]
pub struct ResourceRequirements {
    /// CPU 请求（如 "100m"）
    #[serde(default)]
    pub cpu_request: Option<String>,
    /// CPU 限制（如 "500m"）
    #[serde(default)]
    pub cpu_limit: Option<String>,
    /// 内存请求（如 "128Mi"）
    #[serde(default)]
    pub memory_request: Option<String>,
    /// 内存限制（如 "512Mi"）
    #[serde(default)]
    pub memory_limit: Option<String>,
}

/// 数据库配置
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct DatabaseConfig {
    /// 数据库连接 URL
    pub url: String,
    /// 最大连接数
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

/// Redis 配置
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct RedisConfig {
    /// Redis 连接 URL
    pub url: String,
    /// 是否启用集群模式
    #[serde(default)]
    pub cluster: bool,
}

/// 条件状态
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct SzRustAppCondition {
    /// 条件类型（如 "Ready"、"Available"）
    pub type_: String,
    /// 条件状态（"True"、"False"、"Unknown"）
    pub status: String,
    /// 上次更新时间
    #[serde(default)]
    pub last_transition_time: Option<String>,
    /// 原因
    #[serde(default)]
    pub reason: Option<String>,
    /// 消息
    #[serde(default)]
    pub message: Option<String>,
}

fn default_replicas() -> i32 {
    1
}

fn default_port() -> u16 {
    8080
}

fn default_max_connections() -> u32 {
    10
}

impl SzRustAppSpec {
    /// 创建新的 Spec
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            replicas: default_replicas(),
            port: default_port(),
            env: std::collections::BTreeMap::new(),
            resources: None,
            database: None,
            redis: None,
        }
    }

    /// 设置副本数
    pub fn with_replicas(mut self, replicas: i32) -> Self {
        self.replicas = replicas;
        self
    }

    /// 设置端口
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 添加环境变量
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

// ============================================================================
// CRD 生成
// ============================================================================

/// 生成 SzRustApp CRD 的 YAML 定义
///
/// 用于 `kubectl apply -f` 安装 CRD。
pub fn generate_crd_yaml() -> String {
    let crd = SzRustApp::crd();
    serde_yaml::to_string(&crd).unwrap_or_else(|e| format!("# CRD 序列化失败: {e}"))
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_new() {
        let spec = SzRustAppSpec::new("ghcr.io/ljclz/sz-rust:latest");
        assert_eq!(spec.image, "ghcr.io/ljclz/sz-rust:latest");
        assert_eq!(spec.replicas, 1);
        assert_eq!(spec.port, 8080);
        assert!(spec.env.is_empty());
        assert!(spec.resources.is_none());
        assert!(spec.database.is_none());
        assert!(spec.redis.is_none());
    }

    #[test]
    fn test_spec_builder() {
        let spec = SzRustAppSpec::new("my-image:v1")
            .with_replicas(3)
            .with_port(9090)
            .with_env("DATABASE_URL", "postgres://localhost/mydb")
            .with_env("REDIS_URL", "redis://localhost:6379");

        assert_eq!(spec.replicas, 3);
        assert_eq!(spec.port, 9090);
        assert_eq!(
            spec.env.get("DATABASE_URL").unwrap(),
            "postgres://localhost/mydb"
        );
        assert_eq!(spec.env.get("REDIS_URL").unwrap(), "redis://localhost:6379");
    }

    #[test]
    fn test_spec_serialization() {
        let spec = SzRustAppSpec::new("test:latest").with_replicas(2);
        let json = serde_json::to_string(&spec).unwrap();
        let decoded: SzRustAppSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.image, "test:latest");
        assert_eq!(decoded.replicas, 2);
    }

    #[test]
    fn test_spec_with_database() {
        let spec = SzRustAppSpec::new("test:latest");
        let mut spec = spec;
        spec.database = Some(DatabaseConfig {
            url: "postgres://localhost/db".to_string(),
            max_connections: 20,
        });
        assert!(spec.database.is_some());
        let db = spec.database.unwrap();
        assert_eq!(db.url, "postgres://localhost/db");
        assert_eq!(db.max_connections, 20);
    }

    #[test]
    fn test_spec_with_redis() {
        let spec = SzRustAppSpec::new("test:latest");
        let mut spec = spec;
        spec.redis = Some(RedisConfig {
            url: "redis://localhost:6379".to_string(),
            cluster: true,
        });
        assert!(spec.redis.is_some());
        let redis = spec.redis.unwrap();
        assert_eq!(redis.url, "redis://localhost:6379");
        assert!(redis.cluster);
    }

    #[test]
    fn test_status_default() {
        let status = SzRustAppStatus::default();
        assert!(!status.ready);
        assert_eq!(status.replicas, 0);
        assert!(status.conditions.is_empty());
    }

    #[test]
    fn test_condition_serialization() {
        let condition = SzRustAppCondition {
            type_: "Ready".to_string(),
            status: "True".to_string(),
            last_transition_time: Some("2026-08-06T00:00:00Z".to_string()),
            reason: Some("AllReplicasReady".to_string()),
            message: None,
        };
        let json = serde_json::to_string(&condition).unwrap();
        let decoded: SzRustAppCondition = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.type_, "Ready");
        assert_eq!(decoded.status, "True");
        assert_eq!(decoded.reason.unwrap(), "AllReplicasReady");
    }

    #[test]
    fn test_resource_requirements_default() {
        let req = ResourceRequirements::default();
        assert!(req.cpu_request.is_none());
        assert!(req.cpu_limit.is_none());
        assert!(req.memory_request.is_none());
        assert!(req.memory_limit.is_none());
    }

    #[test]
    fn test_crd_yaml_generation() {
        let yaml = generate_crd_yaml();
        assert!(yaml.contains("sz-rust.dev"));
        assert!(yaml.contains("SzRustApp"));
        assert!(yaml.contains("v1"));
    }

    #[test]
    fn test_crd_has_correct_group() {
        let crd = SzRustApp::crd();
        assert_eq!(crd.spec.group, "sz-rust.dev");
    }

    #[test]
    fn test_crd_has_correct_kind() {
        let crd = SzRustApp::crd();
        assert_eq!(crd.spec.names.kind, "SzRustApp");
    }

    #[test]
    fn test_crd_has_shortname() {
        let crd = SzRustApp::crd();
        let short_names = crd.spec.names.short_names.as_ref().unwrap();
        assert!(short_names.contains(&"szapp".to_string()));
    }
}
