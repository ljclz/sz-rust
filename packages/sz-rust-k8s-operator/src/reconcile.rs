// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Reconcile 逻辑 — 根据 Sz300App CRD 创建/更新 Deployment + Service

use crate::crd::Sz300App;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar as K8sEnvVar, PodSpec, PodTemplateSpec, Service, ServicePort,
    ServiceSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, ResourceExt};
use thiserror::Error;

/// Reconcile 错误
#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error("K8s API 错误: {0}")]
    Kube(#[from] kube::Error),
    #[error("缺少 spec")]
    NoSpec,
}

/// Reconcile 结果
#[derive(Debug, Clone)]
pub struct ReconcileResult {
    pub deployment_created: bool,
    pub deployment_updated: bool,
    pub service_created: bool,
    pub service_updated: bool,
}

/// Reconcile — 根据 Sz300App CRD 确保 Deployment + Service 存在且配置正确
// TODO: 可测性改进建议 — 引入 trait ReconcileApi 抽象 Api<Deployment> 与 Api<Service>，便于 mock 错误传播（reconcile 的 Err(e) => return Err(e.into()) 分支未被测试覆盖）
pub async fn reconcile(
    app: &Sz300App,
    deploy_api: &Api<Deployment>,
    service_api: &Api<Service>,
) -> Result<ReconcileResult, ReconcileError> {
    let name = app.name_any();
    let spec = app.spec.clone();
    let labels = labels_for(&name);

    let mut result = ReconcileResult {
        deployment_created: false,
        deployment_updated: false,
        service_created: false,
        service_updated: false,
    };

    let desired_deployment = build_deployment(&name, &spec, &labels);
    match deploy_api.get(&name).await {
        Ok(_) => {
            deploy_api
                .patch(&name, &patch_params(), &patch_strategy(&desired_deployment))
                .await?;
            result.deployment_updated = true;
        }
        Err(kube::Error::Api(_)) => {
            deploy_api
                .create(&create_params(), &desired_deployment)
                .await?;
            result.deployment_created = true;
        }
        Err(e) => return Err(e.into()),
    }

    let desired_service = build_service(&name, &spec, &labels);
    match service_api.get(&name).await {
        Ok(_) => {
            service_api
                .patch(&name, &patch_params(), &patch_strategy(&desired_service))
                .await?;
            result.service_updated = true;
        }
        Err(kube::Error::Api(_)) => {
            service_api
                .create(&create_params(), &desired_service)
                .await?;
            result.service_created = true;
        }
        Err(e) => return Err(e.into()),
    }

    Ok(result)
}

fn labels_for(name: &str) -> std::collections::BTreeMap<String, String> {
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("app".into(), name.into());
    labels.insert("managed-by".into(), "sz-rust-operator".into());
    labels
}

fn build_deployment(
    name: &str,
    spec: &crate::crd::Sz300AppSpec,
    labels: &std::collections::BTreeMap<String, String>,
) -> Deployment {
    let env_vars: Vec<K8sEnvVar> = spec
        .env
        .iter()
        .map(|e| K8sEnvVar {
            name: e.name.clone(),
            value: Some(e.value.clone()),
            ..Default::default()
        })
        .collect();

    let container = Container {
        name: name.to_string(),
        image: Some(spec.image.clone()),
        ports: Some(vec![ContainerPort {
            container_port: spec.port,
            ..Default::default()
        }]),
        env: if env_vars.is_empty() {
            None
        } else {
            Some(env_vars)
        },
        ..Default::default()
    };

    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(spec.replicas),
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels.clone()),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn build_service(
    name: &str,
    spec: &crate::crd::Sz300AppSpec,
    labels: &std::collections::BTreeMap<String, String>,
) -> Service {
    Service {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(labels.clone()),
            ports: Some(vec![ServicePort {
                port: spec.port,
                target_port: Some(IntOrString::Int(spec.port)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn patch_params() -> kube::api::PatchParams {
    kube::api::PatchParams::apply("sz-rust-operator")
}

fn create_params() -> kube::api::PostParams {
    kube::api::PostParams::default()
}

fn patch_strategy<T: serde::Serialize>(obj: &T) -> kube::api::Patch<serde_json::Value> {
    kube::api::Patch::Apply(
        serde_json::to_value(obj)
            .expect("patch_strategy: serialize manually-constructed CRD object"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{EnvVar, Sz300App, Sz300AppSpec};

    fn make_app(name: &str, image: &str, replicas: i32, port: i32) -> Sz300App {
        Sz300App::new(
            name,
            Sz300AppSpec {
                image: image.into(),
                replicas,
                port,
                env: vec![],
            },
        )
    }

    #[test]
    fn test_build_deployment() {
        let app = make_app("my-app", "sz300:v1", 3, 8080);
        let labels = labels_for("my-app");
        let deploy = build_deployment("my-app", &app.spec, &labels);
        assert_eq!(deploy.metadata.name.as_deref(), Some("my-app"));
        assert_eq!(deploy.spec.as_ref().unwrap().replicas, Some(3));
        let containers = &deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers;
        assert_eq!(containers[0].image.as_deref(), Some("sz300:v1"));
        assert_eq!(
            containers[0].ports.as_ref().unwrap()[0].container_port,
            8080
        );
    }

    #[test]
    fn test_build_deployment_with_env() {
        let app = Sz300App::new(
            "env-app",
            Sz300AppSpec {
                image: "img:v1".into(),
                replicas: 1,
                port: 8080,
                env: vec![EnvVar {
                    name: "DB_URL".into(),
                    value: "mysql://x".into(),
                }],
            },
        );
        let labels = labels_for("env-app");
        let deploy = build_deployment("env-app", &app.spec, &labels);
        let containers = &deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers;
        let env = containers[0].env.as_ref().unwrap();
        assert_eq!(env[0].name, "DB_URL");
        assert_eq!(env[0].value.as_deref(), Some("mysql://x"));
    }

    #[test]
    fn test_build_service() {
        let app = make_app("svc-app", "img:v1", 1, 9090);
        let labels = labels_for("svc-app");
        let svc = build_service("svc-app", &app.spec, &labels);
        assert_eq!(svc.metadata.name.as_deref(), Some("svc-app"));
        let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports[0].port, 9090);
    }

    #[test]
    fn test_labels_for() {
        let labels = labels_for("test");
        assert_eq!(labels.get("app").unwrap(), "test");
        assert_eq!(labels.get("managed-by").unwrap(), "sz-rust-operator");
    }

    #[test]
    fn test_reconcile_result_default() {
        let r = ReconcileResult {
            deployment_created: false,
            deployment_updated: false,
            service_created: false,
            service_updated: false,
        };
        assert!(!r.deployment_created);
        assert!(!r.deployment_updated);
    }

    // === 边界与极端测试 ===

    #[test]
    fn test_build_deployment_replicas_zero() {
        let app = make_app("app", "img:v1", 0, 8080);
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        assert_eq!(deploy.spec.as_ref().unwrap().replicas, Some(0));
    }

    #[test]
    fn test_build_deployment_replicas_negative() {
        let app = make_app("app", "img:v1", -1, 8080);
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        assert_eq!(deploy.spec.as_ref().unwrap().replicas, Some(-1));
    }

    #[test]
    fn test_build_deployment_replicas_max() {
        let app = make_app("app", "img:v1", i32::MAX, 8080);
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        assert_eq!(deploy.spec.as_ref().unwrap().replicas, Some(i32::MAX));
    }

    #[test]
    fn test_build_deployment_env_empty() {
        let app = make_app("app", "img:v1", 1, 8080);
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        let containers = &deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers;
        assert!(containers[0].env.is_none());
    }

    #[test]
    fn test_build_deployment_env_duplicate_keys() {
        let app = Sz300App::new(
            "app",
            Sz300AppSpec {
                image: "img:v1".into(),
                replicas: 1,
                port: 8080,
                env: vec![
                    EnvVar {
                        name: "KEY".into(),
                        value: "v1".into(),
                    },
                    EnvVar {
                        name: "KEY".into(),
                        value: "v2".into(),
                    },
                ],
            },
        );
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        let env = deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn test_build_deployment_env_long_value() {
        let long_val = "x".repeat(1024 * 1024);
        let app = Sz300App::new(
            "app",
            Sz300AppSpec {
                image: "img:v1".into(),
                replicas: 1,
                port: 8080,
                env: vec![EnvVar {
                    name: "LONG".into(),
                    value: long_val.clone(),
                }],
            },
        );
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        let env = deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        assert_eq!(env[0].value.as_ref().unwrap().len(), 1024 * 1024);
    }

    #[test]
    fn test_build_deployment_env_special_chars() {
        let special = "a\nb$c d";
        let app = Sz300App::new(
            "app",
            Sz300AppSpec {
                image: "img:v1".into(),
                replicas: 1,
                port: 8080,
                env: vec![EnvVar {
                    name: "S".into(),
                    value: special.into(),
                }],
            },
        );
        let labels = labels_for("app");
        let deploy = build_deployment("app", &app.spec, &labels);
        let env = deploy
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        assert_eq!(env[0].value.as_deref(), Some("a\nb$c d"));
    }

    #[test]
    fn test_build_service_port_one() {
        let app = make_app("app", "img:v1", 1, 1);
        let labels = labels_for("app");
        let svc = build_service("app", &app.spec, &labels);
        let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports[0].port, 1);
    }

    #[test]
    fn test_build_service_port_max() {
        let app = make_app("app", "img:v1", 1, 65535);
        let labels = labels_for("app");
        let svc = build_service("app", &app.spec, &labels);
        let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports[0].port, 65535);
    }

    #[test]
    fn test_build_service_port_negative() {
        let app = make_app("app", "img:v1", 1, -1);
        let labels = labels_for("app");
        let svc = build_service("app", &app.spec, &labels);
        let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();
        assert_eq!(ports[0].port, -1);
    }

    #[test]
    fn test_labels_for_consistency() {
        let l1 = labels_for("test");
        let l2 = labels_for("test");
        assert_eq!(l1, l2);
        assert_eq!(l1.get("app").unwrap(), "test");
        assert_eq!(l1.get("managed-by").unwrap(), "sz-rust-operator");
    }
}
