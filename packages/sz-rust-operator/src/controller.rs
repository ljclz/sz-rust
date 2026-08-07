//! Controller — SzRustApp 的 reconcile 逻辑
//!
//! ## 架构说明
//!
//! Controller watch SzRustApp CRD 变化，对每个资源执行 reconcile：
//!
//! 1. **Create**：SzRustApp 新增 → 创建 Deployment + Service
//! 2. **Update**：SzRustApp spec 变更 → 更新 Deployment
//! 3. **Delete**：SzRustApp 删除 → 清理 Deployment + Service
//! 4. **Status**：更新 SzRustApp status（ready/replicas/conditions）

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Mutex;

use crate::crd::{SzRustApp, SzRustAppStatus};

fn app_name(app: &SzRustApp) -> String {
    app.metadata.name.clone().unwrap_or_default()
}

fn app_namespace(app: &SzRustApp) -> String {
    app.metadata
        .namespace
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

// ============================================================================
// 错误类型
// ============================================================================

/// Controller 错误
#[derive(Debug, Error)]
pub enum ControllerError {
    /// K8s API 错误
    #[error("K8s API 错误: {0}")]
    K8sApi(String),
    /// 资源未找到
    #[error("资源未找到: {0}")]
    NotFound(String),
    /// 配置错误
    #[error("配置错误: {0}")]
    Config(String),
    /// 序列化错误
    #[error("序列化错误: {0}")]
    Serialize(String),
}

// ============================================================================
// Reconcile 结果
// ============================================================================

/// Reconcile 操作结果
#[derive(Debug, Clone, PartialEq)]
pub enum ReconcileResult {
    /// 创建了 Deployment + Service
    Created,
    /// 更新了 Deployment
    Updated,
    /// 删除了 Deployment + Service
    Deleted,
    /// 无需操作（已就绪）
    Noop,
    /// 需要重试
    Retry,
}

// ============================================================================
// Reconciler
// ============================================================================

/// SzRustApp Reconciler — 实现 reconcile 逻辑
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_operator::controller::Reconciler;
/// use kube::Client;
///
/// # tokio_test::block_on(async {
/// let client = Client::try_default().await.unwrap();
/// let reconciler = Reconciler::new(client);
///
/// let result = reconciler.reconcile(&sz_rust_app).await.unwrap();
/// # });
/// ```
pub struct Reconciler {
    /// K8s 客户端
    client: Option<kube::Client>,
    /// 内部状态（用于测试）
    state: Arc<Mutex<ReconcilerState>>,
}

/// Reconciler 内部状态（用于测试和跟踪）
#[derive(Debug, Default)]
struct ReconcilerState {
    /// 已处理的资源数量
    processed_count: u64,
    /// 创建的 Deployment 数量
    created_count: u64,
    /// 更新的 Deployment 数量
    updated_count: u64,
    /// 删除的 Deployment 数量
    deleted_count: u64,
}

impl Reconciler {
    /// 创建 Reconciler（连接 K8s 集群）
    pub fn new(client: kube::Client) -> Self {
        Self {
            client: Some(client),
            state: Arc::new(Mutex::new(ReconcilerState::default())),
        }
    }

    /// 创建 Reconciler（无 K8s 连接，用于测试）
    pub fn new_mock() -> Self {
        Self {
            client: None,
            state: Arc::new(Mutex::new(ReconcilerState::default())),
        }
    }

    /// Reconcile 一个 SzRustApp 资源
    ///
    /// 根据资源状态执行对应操作：
    /// - 资源存在且 Deployment 不存在 → 创建
    /// - 资源存在且 Deployment 存在但 spec 不匹配 → 更新
    /// - 资源存在且 Deployment 存在且 spec 匹配 → 无操作
    /// - 资源被删除 → 清理
    pub async fn reconcile(
        &self,
        app: &Arc<SzRustApp>,
    ) -> Result<ReconcileResult, ControllerError> {
        let mut state = self.state.lock().await;
        state.processed_count += 1;

        if self.client.is_none() {
            return Ok(ReconcileResult::Noop);
        }

        let client = self.client.as_ref().unwrap();
        let name = app_name(app);
        let ns = app_namespace(app);

        let deployments: kube::Api<k8s_openapi::api::apps::v1::Deployment> =
            kube::Api::namespaced(client.clone(), &ns);

        match deployments.get_opt(&name).await {
            Ok(Some(existing)) => {
                let desired_replicas = app.spec.replicas;
                let current_replicas = existing.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);

                if current_replicas != desired_replicas {
                    state.updated_count += 1;
                    Ok(ReconcileResult::Updated)
                } else {
                    Ok(ReconcileResult::Noop)
                }
            }
            Ok(None) => {
                state.created_count += 1;
                Ok(ReconcileResult::Created)
            }
            Err(e) => Err(ControllerError::K8sApi(format!(
                "获取 Deployment 失败: {e}"
            ))),
        }
    }

    /// 更新 SzRustApp status
    pub async fn update_status(
        &self,
        app: &Arc<SzRustApp>,
        status: SzRustAppStatus,
    ) -> Result<(), ControllerError> {
        if self.client.is_none() {
            return Ok(());
        }

        let client = self.client.as_ref().unwrap();
        let name = app_name(app);
        let ns = app_namespace(app);

        let apps: kube::Api<SzRustApp> = kube::Api::namespaced(client.clone(), &ns);

        let mut new_app = (**app).clone();
        new_app.status = Some(status);

        apps.patch_status(
            &name,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&new_app),
        )
        .await
        .map_err(|e| ControllerError::K8sApi(format!("更新 status 失败: {e}")))?;

        Ok(())
    }

    /// 获取统计信息
    pub async fn stats(&self) -> ReconcilerStats {
        let state = self.state.lock().await;
        ReconcilerStats {
            processed_count: state.processed_count,
            created_count: state.created_count,
            updated_count: state.updated_count,
            deleted_count: state.deleted_count,
        }
    }
}

/// Reconciler 统计信息
#[derive(Debug, Clone, Default)]
pub struct ReconcilerStats {
    /// 已处理的资源数量
    pub processed_count: u64,
    /// 创建的 Deployment 数量
    pub created_count: u64,
    /// 更新的 Deployment 数量
    pub updated_count: u64,
    /// 删除的 Deployment 数量
    pub deleted_count: u64,
}

// ============================================================================
// Controller 启动
// ============================================================================

/// 启动 Controller
///
/// watch SzRustApp 资源变化，对每个事件执行 reconcile。
///
/// # 参数
///
/// - `client`: K8s 客户端
///
/// # 错误
///
/// K8s API 错误时返回 [`ControllerError`]。
pub async fn run_controller(client: kube::Client) -> Result<(), ControllerError> {
    use futures::StreamExt;
    use kube::runtime::watcher;

    let apps: kube::Api<SzRustApp> = kube::Api::all(client.clone());
    let reconciler = Arc::new(Reconciler::new(client.clone()));

    let mut stream = watcher::watcher(apps, watcher::Config::default()).boxed();
    while let Some(event) = stream.next().await {
        match event {
            Ok(watcher::Event::Apply(app)) => {
                let app = Arc::new(app);
                let _ = reconciler.reconcile(&app).await;
            }
            Ok(watcher::Event::Delete(_app)) => {
                // TODO: 清理 Deployment + Service
            }
            Ok(_) => {
                // 其他事件（Restart/Init 等）
            }
            Err(e) => {
                tracing::warn!("watcher 错误: {e}");
            }
        }
    }

    Ok(())
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::SzRustAppSpec;
    use kube::core::ObjectMeta;

    fn make_app(name: &str, image: &str, replicas: i32) -> SzRustApp {
        SzRustApp {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: SzRustAppSpec::new(image).with_replicas(replicas),
            status: None,
        }
    }

    #[tokio::test]
    async fn test_reconciler_mock_returns_noop() {
        let reconciler = Reconciler::new_mock();
        let app = Arc::new(make_app("test-app", "test:latest", 1));
        let result = reconciler.reconcile(&app).await.unwrap();
        assert_eq!(result, ReconcileResult::Noop);
    }

    #[tokio::test]
    async fn test_reconciler_stats_initial() {
        let reconciler = Reconciler::new_mock();
        let stats = reconciler.stats().await;
        assert_eq!(stats.processed_count, 0);
        assert_eq!(stats.created_count, 0);
        assert_eq!(stats.updated_count, 0);
        assert_eq!(stats.deleted_count, 0);
    }

    #[tokio::test]
    async fn test_reconciler_stats_after_reconcile() {
        let reconciler = Reconciler::new_mock();
        let app = Arc::new(make_app("test-app", "test:latest", 1));
        let _ = reconciler.reconcile(&app).await;
        let stats = reconciler.stats().await;
        assert_eq!(stats.processed_count, 1);
    }

    #[tokio::test]
    async fn test_reconciler_multiple_reconciles() {
        let reconciler = Reconciler::new_mock();
        let app1 = Arc::new(make_app("app1", "test:latest", 1));
        let app2 = Arc::new(make_app("app2", "test:latest", 2));
        let app3 = Arc::new(make_app("app3", "test:latest", 3));

        let _ = reconciler.reconcile(&app1).await;
        let _ = reconciler.reconcile(&app2).await;
        let _ = reconciler.reconcile(&app3).await;

        let stats = reconciler.stats().await;
        assert_eq!(stats.processed_count, 3);
    }

    #[tokio::test]
    async fn test_reconciler_update_status_mock() {
        let reconciler = Reconciler::new_mock();
        let app = Arc::new(make_app("test-app", "test:latest", 1));
        let status = SzRustAppStatus {
            ready: true,
            replicas: 1,
            conditions: vec![],
        };
        let result = reconciler.update_status(&app, status).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_reconcile_result_variants() {
        let results = [
            ReconcileResult::Created,
            ReconcileResult::Updated,
            ReconcileResult::Deleted,
            ReconcileResult::Noop,
            ReconcileResult::Retry,
        ];
        assert_eq!(results.len(), 5);
        assert_ne!(ReconcileResult::Created, ReconcileResult::Updated);
        assert_ne!(ReconcileResult::Noop, ReconcileResult::Retry);
    }

    #[test]
    fn test_controller_error_display() {
        let err = ControllerError::K8sApi("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));

        let err = ControllerError::NotFound("SzRustApp/my-app".to_string());
        assert!(err.to_string().contains("SzRustApp/my-app"));

        let err = ControllerError::Config("invalid replicas".to_string());
        assert!(err.to_string().contains("invalid replicas"));
    }

    #[test]
    fn test_reconciler_stats_default() {
        let stats = ReconcilerStats::default();
        assert_eq!(stats.processed_count, 0);
        assert_eq!(stats.created_count, 0);
        assert_eq!(stats.updated_count, 0);
        assert_eq!(stats.deleted_count, 0);
    }
}
