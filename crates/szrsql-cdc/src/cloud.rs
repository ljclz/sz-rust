//! 云原生部署模块 — 对应 `SzRSQL实施进度.md` P6-2。
//!
//! 在 P6-1 分布式协调（`cluster.rs`）之上，提供 Kubernetes Operator 配置生成与
//! CSI 存储接口，支持将 CDC 集群以 StatefulSet 形式部署到 K8s。
//!
//! # 核心概念
//!
//! - **K8sResourceSpec**：Kubernetes 资源规格基类（CPU/内存/存储/端口/环境变量）
//! - **CdcDeploymentSpec**：CDC 部署规格，扩展 K8sResourceSpec，关联 cluster_id 与 NodeRole
//! - **CdcStatefulSet**：有状态 CDC 节点的 StatefulSet 规格，包含 volumeClaimTemplates
//! - **CdcServiceSpec**：K8s Service 规格（ClusterIP/NodePort/LoadBalancer）
//! - **CdcConfigMap**：配置 ConfigMap，存储 CDC 任务配置
//! - **VolumeClaimTemplate**：PVC 模板，持久化 WAL 和数据
//! - **CsiVolumeSpec**：CSI 存储规格
//! - **CloudDeploymentGenerator**：部署清单生成器，一键生成完整 K8s 清单
//! - **CloudConfig**：云原生配置（镜像仓库/标签/命名空间/存储类/TLS/监控）
//!
//! # 设计要点
//!
//! 1. **纯配置生成**：不依赖 kube-rs 等 K8s 客户端库，所有规格提供 `to_yaml()` 方法
//! 2. **标准 YAML**：生成的 YAML 符合 K8s 规范（apiVersion/kind/metadata/spec）
//! 3. **StatefulSet**：使用 volumeClaimTemplates 持久化 WAL 和数据
//! 4. **ConfigMap**：存储 CDC 任务配置（cluster_id、node_role 等）
//! 5. **Service**：暴露 CDC API 端口，支持 ClusterIP/NodePort/LoadBalancer
//! 6. **TLS 与监控**：通过 pod annotations 支持 TLS 和 Prometheus 监控
//! 7. **闭包注入风格**：与 `cluster.rs` 一致，CloudConfig 可外部注入

use crate::cluster::NodeRole;
use std::collections::HashMap;

// =====================================================================
// CloudError — 云原生部署错误
// =====================================================================

/// 云原生部署错误
#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    /// 资源规格无效
    #[error("invalid resource spec: {0}")]
    InvalidResourceSpec(String),

    /// 卷声明模板无效
    #[error("invalid volume claim template: {0}")]
    InvalidVolumeClaim(String),

    /// 配置无效
    #[error("invalid cloud config: {0}")]
    InvalidConfig(String),
}

// =====================================================================
// ContainerPort — 容器端口
// =====================================================================

/// 容器端口定义
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPort {
    /// 端口名称（如 "api"）
    pub name: String,
    /// 容器端口
    pub container_port: u16,
    /// 协议（"TCP" 或 "UDP"，默认 "TCP"）
    pub protocol: String,
}

impl ContainerPort {
    /// 创建容器端口（默认 TCP 协议）
    pub fn new(name: impl Into<String>, port: u16) -> Self {
        Self {
            name: name.into(),
            container_port: port,
            protocol: "TCP".to_string(),
        }
    }

    /// 设置协议
    pub fn with_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = protocol.into();
        self
    }
}

// =====================================================================
// K8sResourceSpec — Kubernetes 资源规格基类
// =====================================================================

/// Kubernetes 资源规格 — 容器资源配置（CPU/内存/存储/端口/环境变量）
///
/// 作为 `CdcDeploymentSpec` 和 `CdcStatefulSet` 的基础规格。
#[derive(Debug, Clone)]
pub struct K8sResourceSpec {
    /// 资源名称
    pub name: String,
    /// 命名空间
    pub namespace: String,
    /// 副本数
    pub replicas: u32,
    /// 容器镜像
    pub image: String,
    /// CPU 请求（如 "500m"）
    pub cpu_request: String,
    /// CPU 限制（如 "1000m"）
    pub cpu_limit: String,
    /// 内存请求（如 "512Mi"）
    pub memory_request: String,
    /// 内存限制（如 "1Gi"）
    pub memory_limit: String,
    /// 存储大小（如 "10Gi"）
    pub storage_size: String,
    /// 环境变量
    pub env_vars: Vec<(String, String)>,
    /// 暴露端口
    pub ports: Vec<ContainerPort>,
}

impl K8sResourceSpec {
    /// 创建资源规格（使用默认资源值）
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.into(),
            replicas: 1,
            image: "szrsql/cdc".to_string(),
            cpu_request: "500m".to_string(),
            cpu_limit: "1000m".to_string(),
            memory_request: "512Mi".to_string(),
            memory_limit: "1Gi".to_string(),
            storage_size: "10Gi".to_string(),
            env_vars: Vec::new(),
            ports: Vec::new(),
        }
    }

    /// 设置副本数
    pub fn with_replicas(mut self, replicas: u32) -> Self {
        self.replicas = replicas;
        self
    }

    /// 设置镜像
    pub fn with_image(mut self, image: impl Into<String>) -> Self {
        self.image = image.into();
        self
    }

    /// 设置 CPU 请求
    pub fn with_cpu_request(mut self, cpu: impl Into<String>) -> Self {
        self.cpu_request = cpu.into();
        self
    }

    /// 设置 CPU 限制
    pub fn with_cpu_limit(mut self, cpu: impl Into<String>) -> Self {
        self.cpu_limit = cpu.into();
        self
    }

    /// 设置内存请求
    pub fn with_memory_request(mut self, mem: impl Into<String>) -> Self {
        self.memory_request = mem.into();
        self
    }

    /// 设置内存限制
    pub fn with_memory_limit(mut self, mem: impl Into<String>) -> Self {
        self.memory_limit = mem.into();
        self
    }

    /// 设置存储大小
    pub fn with_storage_size(mut self, size: impl Into<String>) -> Self {
        self.storage_size = size.into();
        self
    }

    /// 添加环境变量
    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }

    /// 添加端口
    pub fn with_port(mut self, port: ContainerPort) -> Self {
        self.ports.push(port);
        self
    }

    /// 校验规格合法性
    pub fn validate(&self) -> Result<(), CloudError> {
        if self.name.is_empty() {
            return Err(CloudError::InvalidResourceSpec("name is empty".to_string()));
        }
        if self.namespace.is_empty() {
            return Err(CloudError::InvalidResourceSpec(
                "namespace is empty".to_string(),
            ));
        }
        if self.image.is_empty() {
            return Err(CloudError::InvalidResourceSpec(
                "image is empty".to_string(),
            ));
        }
        if self.replicas == 0 {
            return Err(CloudError::InvalidResourceSpec(
                "replicas must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

// =====================================================================
// ServiceType — K8s Service 类型
// =====================================================================

/// K8s Service 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ServiceType {
    /// 集群内访问
    #[default]
    ClusterIP,
    /// 节点端口
    NodePort,
    /// 负载均衡器
    LoadBalancer,
}

impl ServiceType {
    /// 转为 K8s YAML 字符串
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceType::ClusterIP => "ClusterIP",
            ServiceType::NodePort => "NodePort",
            ServiceType::LoadBalancer => "LoadBalancer",
        }
    }
}

// =====================================================================
// CdcServiceSpec — K8s Service 规格
// =====================================================================

/// K8s Service 规格 — 暴露 CDC API 端口
#[derive(Debug, Clone)]
pub struct CdcServiceSpec {
    /// Service 名称
    pub name: String,
    /// 命名空间
    pub namespace: String,
    /// Service 类型
    pub service_type: ServiceType,
    /// 端口
    pub port: u16,
    /// 目标端口
    pub target_port: u16,
    /// Pod 选择器
    pub selector: Vec<(String, String)>,
}

impl CdcServiceSpec {
    /// 创建 Service 规格
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.into(),
            service_type: ServiceType::ClusterIP,
            port: 8080,
            target_port: 8080,
            selector: Vec::new(),
        }
    }

    /// 设置 Service 类型
    pub fn with_service_type(mut self, st: ServiceType) -> Self {
        self.service_type = st;
        self
    }

    /// 设置端口
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置目标端口
    pub fn with_target_port(mut self, port: u16) -> Self {
        self.target_port = port;
        self
    }

    /// 添加选择器
    pub fn with_selector(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.selector.push((key.into(), value.into()));
        self
    }

    /// 生成 K8s Service YAML
    pub fn to_yaml(&self) -> String {
        let mut lines = Vec::new();
        lines.push("apiVersion: v1".to_string());
        lines.push("kind: Service".to_string());
        lines.push("metadata:".to_string());
        lines.push(format!("  name: {}", yaml_scalar(&self.name)));
        lines.push(format!("  namespace: {}", yaml_scalar(&self.namespace)));
        lines.push("spec:".to_string());
        lines.push(format!("  type: {}", self.service_type.as_str()));
        // selector
        lines.push("  selector:".to_string());
        for (k, v) in &self.selector {
            lines.push(format!("    {}: {}", k, yaml_scalar(v)));
        }
        // ports
        lines.push("  ports:".to_string());
        lines.push(format!("  - port: {}", self.port));
        lines.push(format!("    targetPort: {}", self.target_port));
        lines.push("    protocol: TCP".to_string());
        lines.join("\n")
    }
}

// =====================================================================
// CdcConfigMap — 配置 ConfigMap
// =====================================================================

/// 配置 ConfigMap — 存储 CDC 任务配置
#[derive(Debug, Clone)]
pub struct CdcConfigMap {
    /// ConfigMap 名称
    pub name: String,
    /// 命名空间
    pub namespace: String,
    /// 配置键值对
    pub data: HashMap<String, String>,
}

impl CdcConfigMap {
    /// 创建 ConfigMap
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.into(),
            data: HashMap::new(),
        }
    }

    /// 添加配置项
    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    /// 生成 K8s ConfigMap YAML
    pub fn to_yaml(&self) -> String {
        let mut lines = Vec::new();
        lines.push("apiVersion: v1".to_string());
        lines.push("kind: ConfigMap".to_string());
        lines.push("metadata:".to_string());
        lines.push(format!("  name: {}", yaml_scalar(&self.name)));
        lines.push(format!("  namespace: {}", yaml_scalar(&self.namespace)));
        lines.push("data:".to_string());
        // 按 key 排序保证输出稳定
        let mut keys: Vec<&String> = self.data.keys().collect();
        keys.sort();
        for key in keys {
            let value = &self.data[key];
            lines.push(format!("  {}: {}", yaml_scalar(key), yaml_scalar(value)));
        }
        lines.join("\n")
    }
}

// =====================================================================
// VolumeClaimTemplate — PVC 模板
// =====================================================================

/// PVC 模板 — 持久化卷声明模板
#[derive(Debug, Clone)]
pub struct VolumeClaimTemplate {
    /// 卷名称
    pub name: String,
    /// 存储类
    pub storage_class: String,
    /// 访问模式（如 ["ReadWriteOnce"]）
    pub access_modes: Vec<String>,
    /// 存储大小
    pub storage_size: String,
}

impl VolumeClaimTemplate {
    /// 创建 PVC 模板
    pub fn new(name: impl Into<String>, storage_size: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            storage_class: "standard".to_string(),
            access_modes: vec!["ReadWriteOnce".to_string()],
            storage_size: storage_size.into(),
        }
    }

    /// 设置存储类
    pub fn with_storage_class(mut self, sc: impl Into<String>) -> Self {
        self.storage_class = sc.into();
        self
    }

    /// 设置访问模式
    pub fn with_access_modes(mut self, modes: Vec<String>) -> Self {
        self.access_modes = modes;
        self
    }

    /// 校验合法性
    pub fn validate(&self) -> Result<(), CloudError> {
        if self.name.is_empty() {
            return Err(CloudError::InvalidVolumeClaim("name is empty".to_string()));
        }
        if self.access_modes.is_empty() {
            return Err(CloudError::InvalidVolumeClaim(
                "access_modes is empty".to_string(),
            ));
        }
        if self.storage_size.is_empty() {
            return Err(CloudError::InvalidVolumeClaim(
                "storage_size is empty".to_string(),
            ));
        }
        Ok(())
    }
}

// =====================================================================
// CdcDeploymentSpec — CDC 部署规格
// =====================================================================

/// CDC 部署规格 — 扩展 K8sResourceSpec，关联 cluster_id 与 NodeRole
///
/// 包含 CDC 特有的挂载路径和健康检查路径。
/// `to_yaml()` 生成 K8s Deployment 清单。
#[derive(Debug, Clone)]
pub struct CdcDeploymentSpec {
    /// 基础资源规格
    pub base: K8sResourceSpec,
    /// 集群 ID（关联 P6-1 的 ClusterCoordinator）
    pub cluster_id: String,
    /// 节点角色（Leader/Follower，引用 cluster.rs 的 NodeRole）
    pub node_role: NodeRole,
    /// 配置挂载路径
    pub config_mount_path: String,
    /// 数据挂载路径
    pub data_mount_path: String,
    /// WAL 挂载路径
    pub wal_mount_path: String,
    /// 健康检查路径
    pub health_check_path: String,
    /// Pod 注解（TLS、监控等）
    pub pod_annotations: Vec<(String, String)>,
}

impl CdcDeploymentSpec {
    /// 创建 CDC 部署规格
    pub fn new(base: K8sResourceSpec, cluster_id: impl Into<String>) -> Self {
        Self {
            base,
            cluster_id: cluster_id.into(),
            node_role: NodeRole::Follower,
            config_mount_path: "/etc/cdc".to_string(),
            data_mount_path: "/var/lib/cdc/data".to_string(),
            wal_mount_path: "/var/lib/cdc/wal".to_string(),
            health_check_path: "/health".to_string(),
            pod_annotations: Vec::new(),
        }
    }

    /// 设置节点角色
    pub fn with_node_role(mut self, role: NodeRole) -> Self {
        self.node_role = role;
        self
    }

    /// 设置配置挂载路径
    pub fn with_config_mount_path(mut self, path: impl Into<String>) -> Self {
        self.config_mount_path = path.into();
        self
    }

    /// 设置数据挂载路径
    pub fn with_data_mount_path(mut self, path: impl Into<String>) -> Self {
        self.data_mount_path = path.into();
        self
    }

    /// 设置 WAL 挂载路径
    pub fn with_wal_mount_path(mut self, path: impl Into<String>) -> Self {
        self.wal_mount_path = path.into();
        self
    }

    /// 设置健康检查路径
    pub fn with_health_check_path(mut self, path: impl Into<String>) -> Self {
        self.health_check_path = path.into();
        self
    }

    /// 添加 Pod 注解
    pub fn with_pod_annotation(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.pod_annotations.push((key.into(), value.into()));
        self
    }

    /// 生成 K8s Deployment YAML
    pub fn to_yaml(&self) -> String {
        let mut lines = Vec::new();
        lines.push("apiVersion: apps/v1".to_string());
        lines.push("kind: Deployment".to_string());
        lines.push("metadata:".to_string());
        lines.push(format!("  name: {}", yaml_scalar(&self.base.name)));
        lines.push(format!(
            "  namespace: {}",
            yaml_scalar(&self.base.namespace)
        ));
        lines.push("  labels:".to_string());
        lines.push("    app: cdc".to_string());
        lines.push(format!("    cluster-id: {}", yaml_scalar(&self.cluster_id)));
        lines.push(format!("    node-role: {}", self.node_role.as_str()));
        lines.push("spec:".to_string());
        lines.push(format!("  replicas: {}", self.base.replicas));
        lines.push("  selector:".to_string());
        lines.push("    matchLabels:".to_string());
        lines.push("      app: cdc".to_string());
        lines.push(format!(
            "      cluster-id: {}",
            yaml_scalar(&self.cluster_id)
        ));
        lines.push("  template:".to_string());
        lines.push("    metadata:".to_string());
        lines.push("      labels:".to_string());
        lines.push("        app: cdc".to_string());
        lines.push(format!(
            "        cluster-id: {}",
            yaml_scalar(&self.cluster_id)
        ));
        lines.push(format!("        node-role: {}", self.node_role.as_str()));
        if !self.pod_annotations.is_empty() {
            lines.push("      annotations:".to_string());
            for (k, v) in &self.pod_annotations {
                lines.push(format!("        {}: {}", k, yaml_scalar(v)));
            }
        }
        lines.push("    spec:".to_string());
        lines.push("      containers:".to_string());
        lines.push("      - name: cdc".to_string());
        lines.push(format!("        image: {}", yaml_scalar(&self.base.image)));
        // ports
        if !self.base.ports.is_empty() {
            lines.push("        ports:".to_string());
            for port in &self.base.ports {
                lines.push(format!("        - containerPort: {}", port.container_port));
                lines.push(format!("          name: {}", yaml_scalar(&port.name)));
                lines.push(format!("          protocol: {}", port.protocol));
            }
        }
        // env
        if !self.base.env_vars.is_empty() {
            lines.push("        env:".to_string());
            for (k, v) in &self.base.env_vars {
                lines.push(format!("        - name: {}", yaml_scalar(k)));
                lines.push(format!("          value: {}", yaml_scalar(v)));
            }
        }
        // resources
        lines.push("        resources:".to_string());
        lines.push("          requests:".to_string());
        lines.push(format!("            cpu: {}", self.base.cpu_request));
        lines.push(format!("            memory: {}", self.base.memory_request));
        lines.push("          limits:".to_string());
        lines.push(format!("            cpu: {}", self.base.cpu_limit));
        lines.push(format!("            memory: {}", self.base.memory_limit));
        // volumeMounts
        lines.push("        volumeMounts:".to_string());
        lines.push("        - name: config".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&self.config_mount_path)
        ));
        lines.push("        - name: data".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&self.data_mount_path)
        ));
        lines.push("        - name: wal".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&self.wal_mount_path)
        ));
        // livenessProbe
        let probe_port = self
            .base
            .ports
            .first()
            .map(|p| p.container_port)
            .unwrap_or(8080);
        lines.push("        livenessProbe:".to_string());
        lines.push("          httpGet:".to_string());
        lines.push(format!(
            "            path: {}",
            yaml_scalar(&self.health_check_path)
        ));
        lines.push(format!("            port: {}", probe_port));
        // volumes（config volume 引用 ConfigMap）
        lines.push("      volumes:".to_string());
        lines.push("      - name: config".to_string());
        lines.push("        configMap:".to_string());
        lines.push(format!(
            "          name: {}-config",
            yaml_scalar(&self.base.name)
        ));
        lines.join("\n")
    }
}

// =====================================================================
// CdcStatefulSet — StatefulSet 规格
// =====================================================================

/// StatefulSet 规格 — 用于有状态 CDC 节点
///
/// 包含 `CdcDeploymentSpec` 的全部字段，外加 `volumeClaimTemplates`
/// 持久化 WAL 和数据卷。`to_yaml()` 生成 K8s StatefulSet 清单。
#[derive(Debug, Clone)]
pub struct CdcStatefulSet {
    /// CDC 部署规格
    pub deployment: CdcDeploymentSpec,
    /// PVC 模板列表
    pub volume_claim_templates: Vec<VolumeClaimTemplate>,
}

impl CdcStatefulSet {
    /// 创建 StatefulSet 规格
    pub fn new(deployment: CdcDeploymentSpec) -> Self {
        let base_storage = deployment.base.storage_size.clone();
        Self {
            deployment,
            volume_claim_templates: vec![
                VolumeClaimTemplate::new("data", &base_storage),
                VolumeClaimTemplate::new("wal", &base_storage),
            ],
        }
    }

    /// 添加 PVC 模板
    pub fn with_volume_claim(mut self, template: VolumeClaimTemplate) -> Self {
        self.volume_claim_templates.push(template);
        self
    }

    /// 生成 K8s StatefulSet YAML
    pub fn to_yaml(&self) -> String {
        let dep = &self.deployment;
        let base = &dep.base;
        let mut lines = Vec::new();
        lines.push("apiVersion: apps/v1".to_string());
        lines.push("kind: StatefulSet".to_string());
        lines.push("metadata:".to_string());
        lines.push(format!("  name: {}", yaml_scalar(&base.name)));
        lines.push(format!("  namespace: {}", yaml_scalar(&base.namespace)));
        lines.push("  labels:".to_string());
        lines.push("    app: cdc".to_string());
        lines.push(format!("    cluster-id: {}", yaml_scalar(&dep.cluster_id)));
        lines.push(format!("    node-role: {}", dep.node_role.as_str()));
        lines.push("spec:".to_string());
        lines.push(format!("  serviceName: {}", yaml_scalar(&base.name)));
        lines.push(format!("  replicas: {}", base.replicas));
        lines.push("  selector:".to_string());
        lines.push("    matchLabels:".to_string());
        lines.push("      app: cdc".to_string());
        lines.push(format!(
            "      cluster-id: {}",
            yaml_scalar(&dep.cluster_id)
        ));
        lines.push("  template:".to_string());
        lines.push("    metadata:".to_string());
        lines.push("      labels:".to_string());
        lines.push("        app: cdc".to_string());
        lines.push(format!(
            "        cluster-id: {}",
            yaml_scalar(&dep.cluster_id)
        ));
        lines.push(format!("        node-role: {}", dep.node_role.as_str()));
        if !dep.pod_annotations.is_empty() {
            lines.push("      annotations:".to_string());
            for (k, v) in &dep.pod_annotations {
                lines.push(format!("        {}: {}", k, yaml_scalar(v)));
            }
        }
        lines.push("    spec:".to_string());
        lines.push("      containers:".to_string());
        lines.push("      - name: cdc".to_string());
        lines.push(format!("        image: {}", yaml_scalar(&base.image)));
        // ports
        if !base.ports.is_empty() {
            lines.push("        ports:".to_string());
            for port in &base.ports {
                lines.push(format!("        - containerPort: {}", port.container_port));
                lines.push(format!("          name: {}", yaml_scalar(&port.name)));
                lines.push(format!("          protocol: {}", port.protocol));
            }
        }
        // env
        if !base.env_vars.is_empty() {
            lines.push("        env:".to_string());
            for (k, v) in &base.env_vars {
                lines.push(format!("        - name: {}", yaml_scalar(k)));
                lines.push(format!("          value: {}", yaml_scalar(v)));
            }
        }
        // resources
        lines.push("        resources:".to_string());
        lines.push("          requests:".to_string());
        lines.push(format!("            cpu: {}", base.cpu_request));
        lines.push(format!("            memory: {}", base.memory_request));
        lines.push("          limits:".to_string());
        lines.push(format!("            cpu: {}", base.cpu_limit));
        lines.push(format!("            memory: {}", base.memory_limit));
        // volumeMounts
        lines.push("        volumeMounts:".to_string());
        lines.push("        - name: config".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&dep.config_mount_path)
        ));
        lines.push("        - name: data".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&dep.data_mount_path)
        ));
        lines.push("        - name: wal".to_string());
        lines.push(format!(
            "          mountPath: {}",
            yaml_scalar(&dep.wal_mount_path)
        ));
        // livenessProbe
        let probe_port = base.ports.first().map(|p| p.container_port).unwrap_or(8080);
        lines.push("        livenessProbe:".to_string());
        lines.push("          httpGet:".to_string());
        lines.push(format!(
            "            path: {}",
            yaml_scalar(&dep.health_check_path)
        ));
        lines.push(format!("            port: {}", probe_port));
        // volumes（config volume 引用 ConfigMap）
        lines.push("      volumes:".to_string());
        lines.push("      - name: config".to_string());
        lines.push("        configMap:".to_string());
        lines.push(format!(
            "          name: {}-config",
            yaml_scalar(&base.name)
        ));
        // volumeClaimTemplates
        if !self.volume_claim_templates.is_empty() {
            lines.push("  volumeClaimTemplates:".to_string());
            for vct in &self.volume_claim_templates {
                lines.push("  - metadata:".to_string());
                lines.push(format!("      name: {}", yaml_scalar(&vct.name)));
                lines.push("    spec:".to_string());
                lines.push("      accessModes:".to_string());
                for mode in &vct.access_modes {
                    lines.push(format!("      - {}", mode));
                }
                lines.push(format!(
                    "      storageClassName: {}",
                    yaml_scalar(&vct.storage_class)
                ));
                lines.push("      resources:".to_string());
                lines.push("        requests:".to_string());
                lines.push(format!("          storage: {}", vct.storage_size));
            }
        }
        lines.join("\n")
    }
}

// =====================================================================
// CsiVolumeSpec — CSI 存储规格
// =====================================================================

/// CSI 存储规格 — 描述 CSI 驱动卷
#[derive(Debug, Clone)]
pub struct CsiVolumeSpec {
    /// CSI 驱动名称
    pub driver: String,
    /// 卷句柄
    pub volume_handle: String,
    /// 是否只读
    pub readonly: bool,
    /// 挂载选项
    pub mount_options: Vec<String>,
}

impl CsiVolumeSpec {
    /// 创建 CSI 卷规格
    pub fn new(driver: impl Into<String>, volume_handle: impl Into<String>) -> Self {
        Self {
            driver: driver.into(),
            volume_handle: volume_handle.into(),
            readonly: false,
            mount_options: Vec::new(),
        }
    }

    /// 设置只读
    pub fn with_readonly(mut self, ro: bool) -> Self {
        self.readonly = ro;
        self
    }

    /// 添加挂载选项
    pub fn with_mount_option(mut self, opt: impl Into<String>) -> Self {
        self.mount_options.push(opt.into());
        self
    }

    /// 生成 CSI 卷 YAML 片段
    pub fn to_yaml(&self) -> String {
        let mut lines = Vec::new();
        lines.push("csi:".to_string());
        lines.push(format!("  driver: {}", yaml_scalar(&self.driver)));
        lines.push(format!(
            "  volumeHandle: {}",
            yaml_scalar(&self.volume_handle)
        ));
        lines.push(format!("  readOnly: {}", self.readonly));
        if !self.mount_options.is_empty() {
            lines.push("  mountOptions:".to_string());
            for opt in &self.mount_options {
                lines.push(format!("  - {}", opt));
            }
        }
        lines.join("\n")
    }
}

// =====================================================================
// CloudConfig — 云原生配置
// =====================================================================

/// 云原生配置 — 镜像仓库、命名空间、存储类等
#[derive(Debug, Clone)]
pub struct CloudConfig {
    /// 镜像仓库地址（如 "registry.example.com"，空表示使用本地）
    pub registry: String,
    /// 镜像标签
    pub image_tag: String,
    /// 命名空间
    pub namespace: String,
    /// 存储类
    pub storage_class: String,
    /// 是否启用 TLS
    pub enable_tls: bool,
    /// 是否启用监控
    pub enable_monitoring: bool,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            registry: String::new(),
            image_tag: "latest".to_string(),
            namespace: "default".to_string(),
            storage_class: "standard".to_string(),
            enable_tls: false,
            enable_monitoring: false,
        }
    }
}

impl CloudConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置镜像仓库
    pub fn with_registry(mut self, registry: impl Into<String>) -> Self {
        self.registry = registry.into();
        self
    }

    /// 设置镜像标签
    pub fn with_image_tag(mut self, tag: impl Into<String>) -> Self {
        self.image_tag = tag.into();
        self
    }

    /// 设置命名空间
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    /// 设置存储类
    pub fn with_storage_class(mut self, sc: impl Into<String>) -> Self {
        self.storage_class = sc.into();
        self
    }

    /// 启用/禁用 TLS
    pub fn with_tls(mut self, enable: bool) -> Self {
        self.enable_tls = enable;
        self
    }

    /// 启用/禁用监控
    pub fn with_monitoring(mut self, enable: bool) -> Self {
        self.enable_monitoring = enable;
        self
    }
}

// =====================================================================
// CloudDeploymentGenerator — 部署清单生成器
// =====================================================================

/// 部署清单生成器 — 一键生成完整 K8s 清单
///
/// 根据集群 ID、节点角色、资源配置，生成 StatefulSet + Service + ConfigMap
/// 完整 K8s 清单（多文档 YAML，用 `---` 分隔）。
///
/// **使用示例**：
/// ```ignore
/// use szrsql_cdc::cloud::{CloudDeploymentGenerator, CloudConfig, K8sResourceSpec, ContainerPort};
/// use szrsql_cdc::cluster::NodeRole;
///
/// let resources = K8sResourceSpec::new("cdc-node", "szrsql")
///     .with_replicas(1)
///     .with_image("szrsql/cdc")
///     .with_port(ContainerPort::new("api", 8080))
///     .with_env_var("CLUSTER_ID", "cluster-1");
///
/// let yaml = CloudDeploymentGenerator::new("cluster-1")
///     .with_node_config(NodeRole::Leader, 3, resources)
///     .with_config(CloudConfig::default().with_monitoring(true))
///     .generate_all_yaml();
/// ```
pub struct CloudDeploymentGenerator {
    /// 集群 ID
    pub cluster_id: String,
    /// 云原生配置
    pub config: CloudConfig,
    /// 节点角色
    pub node_role: NodeRole,
    /// 资源规格
    pub resources: K8sResourceSpec,
}

impl CloudDeploymentGenerator {
    /// 创建部署清单生成器
    pub fn new(cluster_id: impl Into<String>) -> Self {
        let cluster_id = cluster_id.into();
        let config = CloudConfig::default();
        let resources = K8sResourceSpec::new(format!("{}-node", cluster_id), &config.namespace)
            .with_image("szrsql/cdc")
            .with_replicas(1)
            .with_port(ContainerPort::new("api", 8080))
            .with_env_var("CLUSTER_ID", &cluster_id);

        Self {
            cluster_id,
            config,
            node_role: NodeRole::Follower,
            resources,
        }
    }

    /// 设置节点配置
    pub fn with_node_config(
        mut self,
        node_role: NodeRole,
        replicas: u32,
        mut resources: K8sResourceSpec,
    ) -> Self {
        resources.replicas = replicas;
        self.node_role = node_role;
        self.resources = resources;
        self
    }

    /// 设置云原生配置
    pub fn with_config(mut self, config: CloudConfig) -> Self {
        // 同步命名空间到 resources
        self.resources.namespace = config.namespace.clone();
        self.config = config;
        self
    }

    /// 构建完整镜像路径（registry/image:tag）
    fn full_image(&self) -> String {
        if self.config.registry.is_empty() {
            format!("{}:{}", self.resources.image, self.config.image_tag)
        } else {
            format!(
                "{}/{}:{}",
                self.config.registry, self.resources.image, self.config.image_tag
            )
        }
    }

    /// 构建 pod 注解（TLS、监控）
    fn build_annotations(&self) -> Vec<(String, String)> {
        let mut annotations = Vec::new();
        if self.config.enable_tls {
            annotations.push(("tls.enabled".to_string(), "true".to_string()));
        }
        if self.config.enable_monitoring {
            annotations.push(("prometheus.io/scrape".to_string(), "true".to_string()));
            let port = self
                .resources
                .ports
                .first()
                .map(|p| p.container_port)
                .unwrap_or(8080);
            annotations.push(("prometheus.io/port".to_string(), port.to_string()));
        }
        annotations
    }

    /// 生成 StatefulSet 规格
    pub fn generate_statefulset(&self) -> CdcStatefulSet {
        // 构建资源规格（更新镜像为完整路径）
        let mut base = self.resources.clone();
        base.image = self.full_image();
        base.namespace = self.config.namespace.clone();

        // 构建 CDC 部署规格
        let mut deployment =
            CdcDeploymentSpec::new(base, &self.cluster_id).with_node_role(self.node_role);
        // 设置存储类到 PVC 模板
        for (key, value) in self.build_annotations() {
            deployment = deployment.with_pod_annotation(key, value);
        }

        // 构建 StatefulSet
        let mut sts = CdcStatefulSet::new(deployment);
        // 更新 PVC 模板的存储类
        for vct in &mut sts.volume_claim_templates {
            vct.storage_class = self.config.storage_class.clone();
        }
        sts
    }

    /// 生成 Service 规格
    pub fn generate_service(&self) -> CdcServiceSpec {
        let port = self
            .resources
            .ports
            .first()
            .map(|p| p.container_port)
            .unwrap_or(8080);
        CdcServiceSpec::new(format!("{}-node", self.cluster_id), &self.config.namespace)
            .with_service_type(ServiceType::ClusterIP)
            .with_port(port)
            .with_target_port(port)
            .with_selector("app", "cdc")
            .with_selector("cluster-id", &self.cluster_id)
    }

    /// 生成 ConfigMap
    pub fn generate_configmap(&self, config: HashMap<String, String>) -> CdcConfigMap {
        let mut cm = CdcConfigMap::new(
            format!("{}-node-config", self.cluster_id),
            &self.config.namespace,
        );
        // 注入 CDC 标准配置
        cm = cm.with_data("cluster_id", &self.cluster_id);
        cm = cm.with_data("node_role", self.node_role.as_str());
        cm = cm.with_data("namespace", &self.config.namespace);
        // 合并用户配置
        for (k, v) in config {
            cm.data.insert(k, v);
        }
        cm
    }

    /// 生成完整 K8s 清单（多文档 YAML，用 `---` 分隔）
    ///
    /// 输出顺序：ConfigMap → Service → StatefulSet
    pub fn generate_all_yaml(&self) -> String {
        let mut docs = Vec::new();

        // 1. ConfigMap（带默认配置）
        let mut cm_data = HashMap::new();
        cm_data.insert("log_level".to_string(), "info".to_string());
        cm_data.insert("heartbeat_interval_ms".to_string(), "10000".to_string());
        cm_data.insert("heartbeat_timeout_ms".to_string(), "30000".to_string());
        let cm = self.generate_configmap(cm_data);
        docs.push(cm.to_yaml());

        // 2. Service
        let svc = self.generate_service();
        docs.push(svc.to_yaml());

        // 3. StatefulSet
        let sts = self.generate_statefulset();
        docs.push(sts.to_yaml());

        docs.join("\n---\n")
    }
}

// =====================================================================
// YAML 辅助函数
// =====================================================================

/// 将字符串格式化为 YAML 标量，按需加引号
fn yaml_scalar(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quote = s == "true"
        || s == "false"
        || s == "null"
        || s == "yes"
        || s == "no"
        || s == "on"
        || s == "off"
        || s == "~"
        || s.parse::<i64>().is_ok()
        || s.parse::<f64>().is_ok()
        || s.contains(": ")
        || s.ends_with(':')
        || s.contains(" #")
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.starts_with('-')
        || s.starts_with('?')
        || s.starts_with('!')
        || s.starts_with('&')
        || s.starts_with('*')
        || s.starts_with('[')
        || s.starts_with(']')
        || s.starts_with('{')
        || s.starts_with('}')
        || s.starts_with('|')
        || s.starts_with('>')
        || s.starts_with('@')
        || s.starts_with('`')
        || s.starts_with('"')
        || s.starts_with('\'')
        || s.starts_with('#');
    if needs_quote {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    // 1. K8sResourceSpec 构建与验证
    // =================================================================

    #[test]
    fn k8s_resource_spec_construction() {
        let spec = K8sResourceSpec::new("cdc-node", "szrsql");
        assert_eq!(spec.name, "cdc-node");
        assert_eq!(spec.namespace, "szrsql");
        assert_eq!(spec.replicas, 1);
        assert_eq!(spec.image, "szrsql/cdc");
        assert_eq!(spec.cpu_request, "500m");
        assert_eq!(spec.cpu_limit, "1000m");
        assert_eq!(spec.memory_request, "512Mi");
        assert_eq!(spec.memory_limit, "1Gi");
        assert_eq!(spec.storage_size, "10Gi");
        assert!(spec.env_vars.is_empty());
        assert!(spec.ports.is_empty());
    }

    #[test]
    fn k8s_resource_spec_builder_methods() {
        let spec = K8sResourceSpec::new("cdc-node", "szrsql")
            .with_replicas(3)
            .with_image("custom/cdc")
            .with_cpu_request("250m")
            .with_cpu_limit("2000m")
            .with_memory_request("256Mi")
            .with_memory_limit("2Gi")
            .with_storage_size("20Gi")
            .with_env_var("LOG_LEVEL", "debug")
            .with_port(ContainerPort::new("api", 9090));
        assert_eq!(spec.replicas, 3);
        assert_eq!(spec.image, "custom/cdc");
        assert_eq!(spec.cpu_request, "250m");
        assert_eq!(spec.cpu_limit, "2000m");
        assert_eq!(spec.memory_request, "256Mi");
        assert_eq!(spec.memory_limit, "2Gi");
        assert_eq!(spec.storage_size, "20Gi");
        assert_eq!(spec.env_vars.len(), 1);
        assert_eq!(
            spec.env_vars[0],
            ("LOG_LEVEL".to_string(), "debug".to_string())
        );
        assert_eq!(spec.ports.len(), 1);
        assert_eq!(spec.ports[0].container_port, 9090);
    }

    #[test]
    fn k8s_resource_spec_validate_empty_name_fails() {
        let spec = K8sResourceSpec::new("", "szrsql");
        assert!(spec.validate().is_err());
    }

    #[test]
    fn k8s_resource_spec_validate_empty_namespace_fails() {
        let spec = K8sResourceSpec::new("cdc", "");
        assert!(spec.validate().is_err());
    }

    #[test]
    fn k8s_resource_spec_validate_empty_image_fails() {
        let mut spec = K8sResourceSpec::new("cdc", "szrsql");
        spec.image = "".to_string();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn k8s_resource_spec_validate_zero_replicas_fails() {
        let spec = K8sResourceSpec::new("cdc", "szrsql").with_replicas(0);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn k8s_resource_spec_validate_valid_passes() {
        let spec = K8sResourceSpec::new("cdc", "szrsql").with_replicas(1);
        assert!(spec.validate().is_ok());
    }

    // =================================================================
    // 2. ContainerPort 测试
    // =================================================================

    #[test]
    fn container_port_default_tcp() {
        let port = ContainerPort::new("api", 8080);
        assert_eq!(port.name, "api");
        assert_eq!(port.container_port, 8080);
        assert_eq!(port.protocol, "TCP");
    }

    #[test]
    fn container_port_with_protocol() {
        let port = ContainerPort::new("udp-port", 9090).with_protocol("UDP");
        assert_eq!(port.protocol, "UDP");
    }

    // =================================================================
    // 3. CdcDeploymentSpec 生成
    // =================================================================

    #[test]
    fn cdc_deployment_spec_construction() {
        let base = K8sResourceSpec::new("cdc-node", "szrsql");
        let dep = CdcDeploymentSpec::new(base, "cluster-1")
            .with_node_role(NodeRole::Leader)
            .with_config_mount_path("/config")
            .with_data_mount_path("/data")
            .with_wal_mount_path("/wal")
            .with_health_check_path("/healthz");
        assert_eq!(dep.cluster_id, "cluster-1");
        assert_eq!(dep.node_role, NodeRole::Leader);
        assert_eq!(dep.config_mount_path, "/config");
        assert_eq!(dep.data_mount_path, "/data");
        assert_eq!(dep.wal_mount_path, "/wal");
        assert_eq!(dep.health_check_path, "/healthz");
        assert_eq!(dep.base.name, "cdc-node");
    }

    #[test]
    fn cdc_deployment_spec_default_mount_paths() {
        let base = K8sResourceSpec::new("cdc", "szrsql");
        let dep = CdcDeploymentSpec::new(base, "cluster-1");
        assert_eq!(dep.config_mount_path, "/etc/cdc");
        assert_eq!(dep.data_mount_path, "/var/lib/cdc/data");
        assert_eq!(dep.wal_mount_path, "/var/lib/cdc/wal");
        assert_eq!(dep.health_check_path, "/health");
        assert_eq!(dep.node_role, NodeRole::Follower);
    }

    #[test]
    fn cdc_deployment_spec_to_yaml_contains_deployment() {
        let base = K8sResourceSpec::new("cdc-node", "szrsql")
            .with_replicas(2)
            .with_port(ContainerPort::new("api", 8080))
            .with_env_var("CLUSTER_ID", "cluster-1");
        let dep = CdcDeploymentSpec::new(base, "cluster-1").with_node_role(NodeRole::Leader);
        let yaml = dep.to_yaml();
        assert!(yaml.contains("apiVersion: apps/v1"));
        assert!(yaml.contains("kind: Deployment"));
        assert!(yaml.contains("name: cdc-node"));
        assert!(yaml.contains("namespace: szrsql"));
        assert!(yaml.contains("replicas: 2"));
        assert!(yaml.contains("cluster-id: cluster-1"));
        assert!(yaml.contains("node-role: leader"));
    }

    // =================================================================
    // 4. CdcServiceSpec 各类型
    // =================================================================

    #[test]
    fn cdc_service_spec_clusterip() {
        let svc = CdcServiceSpec::new("cdc-svc", "szrsql")
            .with_service_type(ServiceType::ClusterIP)
            .with_port(8080)
            .with_target_port(8080)
            .with_selector("app", "cdc");
        assert_eq!(svc.service_type, ServiceType::ClusterIP);
        assert_eq!(svc.port, 8080);
        let yaml = svc.to_yaml();
        assert!(yaml.contains("type: ClusterIP"));
        assert!(yaml.contains("port: 8080"));
        assert!(yaml.contains("targetPort: 8080"));
    }

    #[test]
    fn cdc_service_spec_nodeport() {
        let svc = CdcServiceSpec::new("cdc-svc", "szrsql")
            .with_service_type(ServiceType::NodePort)
            .with_port(30080)
            .with_target_port(8080);
        let yaml = svc.to_yaml();
        assert!(yaml.contains("type: NodePort"));
        assert!(yaml.contains("port: 30080"));
    }

    #[test]
    fn cdc_service_spec_loadbalancer() {
        let svc =
            CdcServiceSpec::new("cdc-svc", "szrsql").with_service_type(ServiceType::LoadBalancer);
        let yaml = svc.to_yaml();
        assert!(yaml.contains("type: LoadBalancer"));
    }

    #[test]
    fn cdc_service_spec_yaml_format() {
        let svc = CdcServiceSpec::new("cdc-svc", "szrsql")
            .with_selector("app", "cdc")
            .with_selector("cluster-id", "c1");
        let yaml = svc.to_yaml();
        assert!(yaml.contains("apiVersion: v1"));
        assert!(yaml.contains("kind: Service"));
        assert!(yaml.contains("metadata:"));
        assert!(yaml.contains("spec:"));
        assert!(yaml.contains("selector:"));
        assert!(yaml.contains("app: cdc"));
        assert!(yaml.contains("cluster-id: c1"));
    }

    #[test]
    fn service_type_as_str() {
        assert_eq!(ServiceType::ClusterIP.as_str(), "ClusterIP");
        assert_eq!(ServiceType::NodePort.as_str(), "NodePort");
        assert_eq!(ServiceType::LoadBalancer.as_str(), "LoadBalancer");
    }

    // =================================================================
    // 5. CdcConfigMap YAML 生成与解析
    // =================================================================

    #[test]
    fn cdc_configmap_construction() {
        let cm = CdcConfigMap::new("cdc-config", "szrsql")
            .with_data("log_level", "info")
            .with_data("cluster_id", "cluster-1");
        assert_eq!(cm.name, "cdc-config");
        assert_eq!(cm.namespace, "szrsql");
        assert_eq!(cm.data.len(), 2);
        assert_eq!(cm.data.get("log_level"), Some(&"info".to_string()));
        assert_eq!(cm.data.get("cluster_id"), Some(&"cluster-1".to_string()));
    }

    #[test]
    fn cdc_configmap_to_yaml_format() {
        let cm = CdcConfigMap::new("cdc-config", "szrsql")
            .with_data("log_level", "info")
            .with_data("port", "8080");
        let yaml = cm.to_yaml();
        assert!(yaml.contains("apiVersion: v1"));
        assert!(yaml.contains("kind: ConfigMap"));
        assert!(yaml.contains("metadata:"));
        assert!(yaml.contains("name: cdc-config"));
        assert!(yaml.contains("namespace: szrsql"));
        assert!(yaml.contains("data:"));
        assert!(yaml.contains("log_level: info"));
        // "8080" is numeric-looking, should be quoted
        assert!(yaml.contains("port: \"8080\""));
    }

    #[test]
    fn cdc_configmap_to_yaml_data_preserved() {
        let mut data = HashMap::new();
        data.insert("key1".to_string(), "value1".to_string());
        data.insert("key2".to_string(), "value2".to_string());
        let cm = CdcConfigMap {
            name: "test-cm".to_string(),
            namespace: "default".to_string(),
            data,
        };
        let yaml = cm.to_yaml();
        assert!(yaml.contains("key1: value1"));
        assert!(yaml.contains("key2: value2"));
    }

    #[test]
    fn cdc_configmap_to_yaml_empty_data() {
        let cm = CdcConfigMap::new("empty-cm", "default");
        let yaml = cm.to_yaml();
        assert!(yaml.contains("data:"));
        // Empty data should still have the data: key
        assert!(yaml.contains("kind: ConfigMap"));
    }

    // =================================================================
    // 6. VolumeClaimTemplate 验证
    // =================================================================

    #[test]
    fn volume_claim_template_construction() {
        let vct = VolumeClaimTemplate::new("data", "10Gi")
            .with_storage_class("fast-ssd")
            .with_access_modes(vec!["ReadWriteOnce".to_string()]);
        assert_eq!(vct.name, "data");
        assert_eq!(vct.storage_size, "10Gi");
        assert_eq!(vct.storage_class, "fast-ssd");
        assert_eq!(vct.access_modes, vec!["ReadWriteOnce".to_string()]);
    }

    #[test]
    fn volume_claim_template_default_values() {
        let vct = VolumeClaimTemplate::new("wal", "5Gi");
        assert_eq!(vct.storage_class, "standard");
        assert_eq!(vct.access_modes, vec!["ReadWriteOnce".to_string()]);
    }

    #[test]
    fn volume_claim_template_validate_empty_name_fails() {
        let vct = VolumeClaimTemplate::new("", "10Gi");
        assert!(vct.validate().is_err());
    }

    #[test]
    fn volume_claim_template_validate_empty_access_modes_fails() {
        let vct = VolumeClaimTemplate {
            name: "data".to_string(),
            storage_class: "standard".to_string(),
            access_modes: vec![],
            storage_size: "10Gi".to_string(),
        };
        assert!(vct.validate().is_err());
    }

    #[test]
    fn volume_claim_template_validate_empty_storage_size_fails() {
        let vct = VolumeClaimTemplate::new("data", "");
        assert!(vct.validate().is_err());
    }

    #[test]
    fn volume_claim_template_validate_valid_passes() {
        let vct = VolumeClaimTemplate::new("data", "10Gi");
        assert!(vct.validate().is_ok());
    }

    // =================================================================
    // 7. CsiVolumeSpec 构建
    // =================================================================

    #[test]
    fn csi_volume_spec_construction() {
        let csi = CsiVolumeSpec::new("disk.csi.aws.com", "vol-12345")
            .with_readonly(true)
            .with_mount_option("noatime");
        assert_eq!(csi.driver, "disk.csi.aws.com");
        assert_eq!(csi.volume_handle, "vol-12345");
        assert!(csi.readonly);
        assert_eq!(csi.mount_options, vec!["noatime".to_string()]);
    }

    #[test]
    fn csi_volume_spec_defaults() {
        let csi = CsiVolumeSpec::new("csi-driver", "handle-1");
        assert!(!csi.readonly);
        assert!(csi.mount_options.is_empty());
    }

    #[test]
    fn csi_volume_spec_to_yaml() {
        let csi = CsiVolumeSpec::new("disk.csi.aws.com", "vol-12345")
            .with_readonly(true)
            .with_mount_option("noatime");
        let yaml = csi.to_yaml();
        assert!(yaml.contains("csi:"));
        assert!(yaml.contains("driver: disk.csi.aws.com"));
        assert!(yaml.contains("volumeHandle: vol-12345"));
        assert!(yaml.contains("readOnly: true"));
        assert!(yaml.contains("mountOptions:"));
        assert!(yaml.contains("- noatime"));
    }

    // =================================================================
    // 8. CdcStatefulSet YAML 生成
    // =================================================================

    #[test]
    fn cdc_statefulset_construction() {
        let base = K8sResourceSpec::new("cdc-node", "szrsql").with_storage_size("20Gi");
        let dep = CdcDeploymentSpec::new(base, "cluster-1");
        let sts = CdcStatefulSet::new(dep);
        assert_eq!(sts.volume_claim_templates.len(), 2);
        assert_eq!(sts.volume_claim_templates[0].name, "data");
        assert_eq!(sts.volume_claim_templates[1].name, "wal");
        assert_eq!(sts.volume_claim_templates[0].storage_size, "20Gi");
    }

    #[test]
    fn cdc_statefulset_to_yaml_format() {
        let base = K8sResourceSpec::new("cdc-node", "szrsql")
            .with_replicas(3)
            .with_port(ContainerPort::new("api", 8080))
            .with_env_var("CLUSTER_ID", "cluster-1");
        let dep = CdcDeploymentSpec::new(base, "cluster-1").with_node_role(NodeRole::Leader);
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("apiVersion: apps/v1"));
        assert!(yaml.contains("kind: StatefulSet"));
        assert!(yaml.contains("metadata:"));
        assert!(yaml.contains("name: cdc-node"));
        assert!(yaml.contains("namespace: szrsql"));
        assert!(yaml.contains("spec:"));
        assert!(yaml.contains("serviceName: cdc-node"));
        assert!(yaml.contains("replicas: 3"));
        assert!(yaml.contains("cluster-id: cluster-1"));
        assert!(yaml.contains("node-role: leader"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_volume_claims() {
        let base = K8sResourceSpec::new("cdc-node", "szrsql").with_storage_size("15Gi");
        let dep = CdcDeploymentSpec::new(base, "cluster-1");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("volumeClaimTemplates:"));
        assert!(yaml.contains("name: data"));
        assert!(yaml.contains("name: wal"));
        assert!(yaml.contains("storage: 15Gi"));
        assert!(yaml.contains("accessModes:"));
        assert!(yaml.contains("- ReadWriteOnce"));
        assert!(yaml.contains("storageClassName: standard"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_resource_limits() {
        let base = K8sResourceSpec::new("cdc", "ns")
            .with_cpu_request("250m")
            .with_cpu_limit("500m")
            .with_memory_request("128Mi")
            .with_memory_limit("256Mi");
        let dep = CdcDeploymentSpec::new(base, "c1");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("resources:"));
        assert!(yaml.contains("requests:"));
        assert!(yaml.contains("cpu: 250m"));
        assert!(yaml.contains("memory: 128Mi"));
        assert!(yaml.contains("limits:"));
        assert!(yaml.contains("cpu: 500m"));
        assert!(yaml.contains("memory: 256Mi"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_ports() {
        let base = K8sResourceSpec::new("cdc", "ns")
            .with_port(ContainerPort::new("api", 8080))
            .with_port(ContainerPort::new("metrics", 9090));
        let dep = CdcDeploymentSpec::new(base, "c1");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("ports:"));
        assert!(yaml.contains("containerPort: 8080"));
        assert!(yaml.contains("name: api"));
        assert!(yaml.contains("containerPort: 9090"));
        assert!(yaml.contains("name: metrics"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_env_vars() {
        let base = K8sResourceSpec::new("cdc", "ns")
            .with_env_var("CLUSTER_ID", "cluster-1")
            .with_env_var("LOG_LEVEL", "debug");
        let dep = CdcDeploymentSpec::new(base, "c1");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("env:"));
        assert!(yaml.contains("name: CLUSTER_ID"));
        assert!(yaml.contains("value: cluster-1"));
        assert!(yaml.contains("name: LOG_LEVEL"));
        assert!(yaml.contains("value: debug"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_volume_mounts() {
        let base = K8sResourceSpec::new("cdc", "ns");
        let dep = CdcDeploymentSpec::new(base, "c1")
            .with_config_mount_path("/etc/cdc")
            .with_data_mount_path("/data")
            .with_wal_mount_path("/wal");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("volumeMounts:"));
        assert!(yaml.contains("mountPath: /etc/cdc"));
        assert!(yaml.contains("mountPath: /data"));
        assert!(yaml.contains("mountPath: /wal"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_liveness_probe() {
        let base = K8sResourceSpec::new("cdc", "ns").with_port(ContainerPort::new("api", 8080));
        let dep = CdcDeploymentSpec::new(base, "c1").with_health_check_path("/healthz");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("livenessProbe:"));
        assert!(yaml.contains("httpGet:"));
        assert!(yaml.contains("path: /healthz"));
        assert!(yaml.contains("port: 8080"));
    }

    #[test]
    fn cdc_statefulset_yaml_contains_config_volume() {
        let base = K8sResourceSpec::new("cdc-node", "ns");
        let dep = CdcDeploymentSpec::new(base, "c1");
        let sts = CdcStatefulSet::new(dep);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("volumes:"));
        assert!(yaml.contains("configMap:"));
        assert!(yaml.contains("name: cdc-node-config"));
    }

    // =================================================================
    // 9. CloudConfig 测试
    // =================================================================

    #[test]
    fn cloud_config_default() {
        let cfg = CloudConfig::default();
        assert_eq!(cfg.registry, "");
        assert_eq!(cfg.image_tag, "latest");
        assert_eq!(cfg.namespace, "default");
        assert_eq!(cfg.storage_class, "standard");
        assert!(!cfg.enable_tls);
        assert!(!cfg.enable_monitoring);
    }

    #[test]
    fn cloud_config_builder() {
        let cfg = CloudConfig::new()
            .with_registry("registry.example.com")
            .with_image_tag("v1.0")
            .with_namespace("szrsql")
            .with_storage_class("fast-ssd")
            .with_tls(true)
            .with_monitoring(true);
        assert_eq!(cfg.registry, "registry.example.com");
        assert_eq!(cfg.image_tag, "v1.0");
        assert_eq!(cfg.namespace, "szrsql");
        assert_eq!(cfg.storage_class, "fast-ssd");
        assert!(cfg.enable_tls);
        assert!(cfg.enable_monitoring);
    }

    // =================================================================
    // 10. CloudDeploymentGenerator 完整流程
    // =================================================================

    #[test]
    fn generator_new_initial_state() {
        let gen = CloudDeploymentGenerator::new("cluster-1");
        assert_eq!(gen.cluster_id, "cluster-1");
        assert_eq!(gen.node_role, NodeRole::Follower);
        assert_eq!(gen.resources.replicas, 1);
        assert_eq!(gen.resources.ports.len(), 1);
        assert_eq!(gen.resources.env_vars.len(), 1);
        assert_eq!(gen.resources.env_vars[0].0, "CLUSTER_ID");
    }

    #[test]
    fn generator_with_node_config() {
        let resources = K8sResourceSpec::new("custom-node", "szrsql")
            .with_replicas(1)
            .with_cpu_request("1000m");
        let gen = CloudDeploymentGenerator::new("cluster-1").with_node_config(
            NodeRole::Leader,
            5,
            resources,
        );
        assert_eq!(gen.node_role, NodeRole::Leader);
        assert_eq!(gen.resources.replicas, 5);
        assert_eq!(gen.resources.cpu_request, "1000m");
        assert_eq!(gen.resources.name, "custom-node");
    }

    #[test]
    fn generator_with_config() {
        let cfg = CloudConfig::new()
            .with_namespace("prod")
            .with_registry("registry.example.com")
            .with_image_tag("v2.0");
        let gen = CloudDeploymentGenerator::new("cluster-1").with_config(cfg);
        assert_eq!(gen.config.namespace, "prod");
        assert_eq!(gen.config.registry, "registry.example.com");
        assert_eq!(gen.config.image_tag, "v2.0");
        // namespace 同步到 resources
        assert_eq!(gen.resources.namespace, "prod");
    }

    #[test]
    fn generator_generate_statefulset() {
        let gen = CloudDeploymentGenerator::new("cluster-1").with_node_config(
            NodeRole::Leader,
            3,
            K8sResourceSpec::new("cdc-node", "szrsql"),
        );
        let sts = gen.generate_statefulset();
        assert_eq!(sts.deployment.cluster_id, "cluster-1");
        assert_eq!(sts.deployment.node_role, NodeRole::Leader);
        assert_eq!(sts.deployment.base.replicas, 3);
        assert_eq!(sts.volume_claim_templates.len(), 2);
    }

    #[test]
    fn generator_generate_service() {
        let gen = CloudDeploymentGenerator::new("cluster-1");
        let svc = gen.generate_service();
        assert_eq!(svc.name, "cluster-1-node");
        assert_eq!(svc.port, 8080);
        assert_eq!(svc.target_port, 8080);
        assert_eq!(svc.service_type, ServiceType::ClusterIP);
        assert!(svc.selector.iter().any(|(k, _)| k == "app"));
        assert!(svc.selector.iter().any(|(k, _)| k == "cluster-id"));
    }

    #[test]
    fn generator_generate_configmap() {
        let gen = CloudDeploymentGenerator::new("cluster-1").with_node_config(
            NodeRole::Leader,
            1,
            K8sResourceSpec::new("cdc", "ns"),
        );
        let mut data = HashMap::new();
        data.insert("custom_key".to_string(), "custom_value".to_string());
        let cm = gen.generate_configmap(data);
        assert_eq!(cm.name, "cluster-1-node-config");
        assert!(cm.data.contains_key("cluster_id"));
        assert!(cm.data.contains_key("node_role"));
        assert!(cm.data.contains_key("custom_key"));
        assert_eq!(cm.data.get("node_role"), Some(&"leader".to_string()));
    }

    #[test]
    fn generator_generate_all_yaml_multi_doc() {
        let gen = CloudDeploymentGenerator::new("cluster-1").with_node_config(
            NodeRole::Leader,
            3,
            K8sResourceSpec::new("cdc", "szrsql"),
        );
        let yaml = gen.generate_all_yaml();
        // 应包含 3 个文档，用 --- 分隔（2 个分隔符）
        let doc_count = yaml.matches("---").count();
        assert_eq!(doc_count, 2);
        // 应包含 ConfigMap、Service、StatefulSet
        assert!(yaml.contains("kind: ConfigMap"));
        assert!(yaml.contains("kind: Service"));
        assert!(yaml.contains("kind: StatefulSet"));
    }

    #[test]
    fn generator_all_yaml_order() {
        let gen = CloudDeploymentGenerator::new("c1");
        let yaml = gen.generate_all_yaml();
        let cm_pos = yaml.find("kind: ConfigMap").unwrap();
        let svc_pos = yaml.find("kind: Service").unwrap();
        let sts_pos = yaml.find("kind: StatefulSet").unwrap();
        // 顺序：ConfigMap < Service < StatefulSet
        assert!(cm_pos < svc_pos);
        assert!(svc_pos < sts_pos);
    }

    // =================================================================
    // 11. YAML 格式合法性
    // =================================================================

    #[test]
    fn yaml_contains_apiversion_kind_metadata() {
        let gen = CloudDeploymentGenerator::new("cluster-1");
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(yaml.contains("apiVersion:"));
        assert!(yaml.contains("kind:"));
        assert!(yaml.contains("metadata:"));
    }

    #[test]
    fn yaml_image_includes_registry_and_tag() {
        let cfg = CloudConfig::new()
            .with_registry("registry.example.com")
            .with_image_tag("v1.0");
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(yaml.contains("image: registry.example.com/szrsql/cdc:v1.0"));
    }

    #[test]
    fn yaml_image_without_registry() {
        let cfg = CloudConfig::new().with_image_tag("v2.0");
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(yaml.contains("image: szrsql/cdc:v2.0"));
    }

    // =================================================================
    // 12. TLS 和监控注解
    // =================================================================

    #[test]
    fn tls_annotation_when_enabled() {
        let cfg = CloudConfig::new().with_tls(true);
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(yaml.contains("annotations:"));
        assert!(yaml.contains("tls.enabled: \"true\""));
    }

    #[test]
    fn tls_annotation_absent_when_disabled() {
        let cfg = CloudConfig::new().with_tls(false);
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(!yaml.contains("tls.enabled"));
    }

    #[test]
    fn monitoring_annotation_when_enabled() {
        let cfg = CloudConfig::new().with_monitoring(true);
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(yaml.contains("prometheus.io/scrape: \"true\""));
        assert!(yaml.contains("prometheus.io/port: \"8080\""));
    }

    #[test]
    fn monitoring_annotation_absent_when_disabled() {
        let cfg = CloudConfig::new().with_monitoring(false);
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let yaml = sts.to_yaml();
        assert!(!yaml.contains("prometheus.io/scrape"));
    }

    // =================================================================
    // 13. 配置参数传递
    // =================================================================

    #[test]
    fn generator_storage_class_passed_to_pvc() {
        let cfg = CloudConfig::new().with_storage_class("fast-ssd");
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        for vct in &sts.volume_claim_templates {
            assert_eq!(vct.storage_class, "fast-ssd");
        }
        let yaml = sts.to_yaml();
        assert!(yaml.contains("storageClassName: fast-ssd"));
    }

    #[test]
    fn generator_namespace_passed_to_all_resources() {
        let cfg = CloudConfig::new().with_namespace("production");
        let gen = CloudDeploymentGenerator::new("c1").with_config(cfg);
        let sts = gen.generate_statefulset();
        let svc = gen.generate_service();
        let cm = gen.generate_configmap(HashMap::new());
        assert_eq!(sts.deployment.base.namespace, "production");
        assert_eq!(svc.namespace, "production");
        assert_eq!(cm.namespace, "production");
    }

    #[test]
    fn generator_node_role_passed_to_statefulset() {
        let gen = CloudDeploymentGenerator::new("c1").with_node_config(
            NodeRole::Follower,
            2,
            K8sResourceSpec::new("cdc", "ns"),
        );
        let sts = gen.generate_statefulset();
        assert_eq!(sts.deployment.node_role, NodeRole::Follower);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("node-role: follower"));
    }

    #[test]
    fn generator_replicas_passed_to_statefulset() {
        let gen = CloudDeploymentGenerator::new("c1").with_node_config(
            NodeRole::Leader,
            7,
            K8sResourceSpec::new("cdc", "ns"),
        );
        let sts = gen.generate_statefulset();
        assert_eq!(sts.deployment.base.replicas, 7);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("replicas: 7"));
    }

    // =================================================================
    // 14. yaml_scalar 辅助函数测试
    // =================================================================

    #[test]
    fn yaml_scalar_empty_string_quoted() {
        assert_eq!(yaml_scalar(""), "\"\"");
    }

    #[test]
    fn yaml_scalar_boolean_quoted() {
        assert_eq!(yaml_scalar("true"), "\"true\"");
        assert_eq!(yaml_scalar("false"), "\"false\"");
    }

    #[test]
    fn yaml_scalar_number_quoted() {
        assert_eq!(yaml_scalar("8080"), "\"8080\"");
        assert_eq!(yaml_scalar("42"), "\"42\"");
    }

    #[test]
    fn yaml_scalar_plain_string_unquoted() {
        assert_eq!(yaml_scalar("cluster-1"), "cluster-1");
        assert_eq!(yaml_scalar("500m"), "500m");
        assert_eq!(yaml_scalar("1Gi"), "1Gi");
        assert_eq!(yaml_scalar("info"), "info");
        assert_eq!(yaml_scalar("szrsql/cdc"), "szrsql/cdc");
    }

    // =================================================================
    // 15. 综合集成测试
    // =================================================================

    #[test]
    fn full_deployment_with_all_features() {
        let cfg = CloudConfig::new()
            .with_registry("registry.example.com")
            .with_image_tag("v1.0")
            .with_namespace("production")
            .with_storage_class("fast-ssd")
            .with_tls(true)
            .with_monitoring(true);
        let resources = K8sResourceSpec::new("cdc-node", "production")
            .with_replicas(1)
            .with_cpu_request("500m")
            .with_cpu_limit("2000m")
            .with_memory_request("1Gi")
            .with_memory_limit("4Gi")
            .with_storage_size("50Gi")
            .with_port(ContainerPort::new("api", 8080))
            .with_port(ContainerPort::new("metrics", 9090))
            .with_env_var("CLUSTER_ID", "prod-cluster")
            .with_env_var("LOG_LEVEL", "info");

        let yaml = CloudDeploymentGenerator::new("prod-cluster")
            .with_node_config(NodeRole::Leader, 5, resources)
            .with_config(cfg)
            .generate_all_yaml();

        // 验证多文档
        assert_eq!(yaml.matches("---").count(), 2);
        // 验证镜像
        assert!(yaml.contains("registry.example.com/szrsql/cdc:v1.0"));
        // 验证 TLS 和监控
        assert!(yaml.contains("tls.enabled: \"true\""));
        assert!(yaml.contains("prometheus.io/scrape: \"true\""));
        // 验证存储类
        assert!(yaml.contains("storageClassName: fast-ssd"));
        // 验证资源限制
        assert!(yaml.contains("cpu: 2000m"));
        assert!(yaml.contains("memory: 4Gi"));
        // 验证端口
        assert!(yaml.contains("containerPort: 8080"));
        assert!(yaml.contains("containerPort: 9090"));
        // 验证环境变量
        assert!(yaml.contains("name: CLUSTER_ID"));
        assert!(yaml.contains("name: LOG_LEVEL"));
        // 验证存储大小
        assert!(yaml.contains("storage: 50Gi"));
        // 验证副本数
        assert!(yaml.contains("replicas: 5"));
    }

    #[test]
    fn statefulset_with_custom_volume_claim() {
        let base = K8sResourceSpec::new("cdc", "ns");
        let dep = CdcDeploymentSpec::new(base, "c1");
        let extra_vct = VolumeClaimTemplate::new("logs", "5Gi")
            .with_storage_class("fast-ssd")
            .with_access_modes(vec!["ReadWriteMany".to_string()]);
        let sts = CdcStatefulSet::new(dep).with_volume_claim(extra_vct);
        assert_eq!(sts.volume_claim_templates.len(), 3);
        let yaml = sts.to_yaml();
        assert!(yaml.contains("name: logs"));
        assert!(yaml.contains("- ReadWriteMany"));
    }

    #[test]
    fn deployment_spec_with_pod_annotations() {
        let base = K8sResourceSpec::new("cdc", "ns");
        let dep = CdcDeploymentSpec::new(base, "c1")
            .with_pod_annotation("custom.annotation", "value")
            .with_pod_annotation("another.annotation", "data");
        let yaml = dep.to_yaml();
        assert!(yaml.contains("annotations:"));
        assert!(yaml.contains("custom.annotation: value"));
        assert!(yaml.contains("another.annotation: data"));
    }
}
