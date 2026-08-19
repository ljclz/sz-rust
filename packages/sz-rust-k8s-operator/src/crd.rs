//! Sz300App CRD 定义

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sz300App 自定义资源 — 声明式管理 sz300 应用部署
///
/// Operator watch 此资源变化，自动创建/更新对应的 Deployment + Service。
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(group = "sz-rust.dev", version = "v1", kind = "Sz300App", namespaced)]
#[serde(rename_all = "camelCase")]
pub struct Sz300AppSpec {
    /// 容器镜像
    pub image: String,
    /// 副本数
    #[serde(default = "default_replicas")]
    pub replicas: i32,
    /// 服务端口
    #[serde(default = "default_port")]
    pub port: i32,
    /// 环境变量（可选）
    #[serde(default)]
    pub env: Vec<EnvVar>,
}

/// 环境变量键值对
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

fn default_replicas() -> i32 {
    1
}

fn default_port() -> i32 {
    8080
}

impl Default for Sz300AppSpec {
    fn default() -> Self {
        Self {
            image: "sz300-server:latest".into(),
            replicas: default_replicas(),
            port: default_port(),
            env: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crd_spec_default() {
        let spec = Sz300AppSpec::default();
        assert_eq!(spec.image, "sz300-server:latest");
        assert_eq!(spec.replicas, 1);
        assert_eq!(spec.port, 8080);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_crd_spec_with_env() {
        let spec = Sz300AppSpec {
            image: "my-app:v2".into(),
            replicas: 3,
            port: 9090,
            env: vec![
                EnvVar {
                    name: "DATABASE_URL".into(),
                    value: "mysql://localhost".into(),
                },
                EnvVar {
                    name: "REDIS_URL".into(),
                    value: "redis://localhost".into(),
                },
            ],
        };
        assert_eq!(spec.image, "my-app:v2");
        assert_eq!(spec.replicas, 3);
        assert_eq!(spec.port, 9090);
        assert_eq!(spec.env.len(), 2);
        assert_eq!(spec.env[0].name, "DATABASE_URL");
    }

    #[test]
    fn test_crd_spec_serialize() {
        let spec = Sz300AppSpec {
            image: "test:v1".into(),
            replicas: 2,
            port: 8080,
            env: vec![],
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["image"], "test:v1");
        assert_eq!(json["replicas"], 2);
        assert_eq!(json["port"], 8080);
    }

    #[test]
    fn test_crd_spec_deserialize() {
        let json =
            r#"{"image":"app:v1","replicas":5,"port":3000,"env":[{"name":"KEY","value":"VAL"}]}"#;
        let spec: Sz300AppSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.image, "app:v1");
        assert_eq!(spec.replicas, 5);
        assert_eq!(spec.port, 3000);
        assert_eq!(spec.env.len(), 1);
        assert_eq!(spec.env[0].name, "KEY");
        assert_eq!(spec.env[0].value, "VAL");
    }

    #[test]
    fn test_crd_spec_deserialize_with_defaults() {
        let json = r#"{"image":"app:v1"}"#;
        let spec: Sz300AppSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.image, "app:v1");
        assert_eq!(spec.replicas, 1);
        assert_eq!(spec.port, 8080);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_crd_kind_and_api_version() {
        let spec = Sz300AppSpec::default();
        let crd = Sz300App::new("test-app", spec);
        assert_eq!(crd.metadata.name.as_deref(), Some("test-app"));
    }
}
