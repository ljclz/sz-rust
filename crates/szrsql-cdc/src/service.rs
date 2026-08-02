//! CDC 即服务模址‿对应 `SzRSQL实施进度.md` P6-3〿//!
//! 圀P2 任务管理（`task.rs`＿ P6-1 分布式协调（`cluster.rs`）之上，提供多租戀//! CDC 服务皀API 层，支持租户隔离、配额管理、API Key 认证、可选集群集成〿//!
//! # 核心概念
//!
//! - **TenantConfig**：租户配置（等级、配额、白名单＿//! - **TenantTier**：租户等级（Free / Pro / Enterprise），各等级有默认配额
//! - **CdcService**：CDC 服务主入口，封装 `ReplicationTaskManager`，提供多租户隔离
//! - **TenantUsage**：租户使用量统计（任务数、事件数、字节数、吞吐量＿//! - **AuthService**：简化认证服务，使用 API Key（不依赖 JWT 库）
//! - **ServiceError**：服务错误枚举，提供 `From<TaskError>` 转换
//!
//! # 设计要点
//!
//! 1. **多租户隔禀*：所有任务操作必须校骀task 归属 tenant
//! 2. **配额管理**：创建任务前检柀`max_tasks`，运行时检柀`max_throughput`
//! 3. **闭包注入模式**：`AuthService` 不依赀JWT 库，使用简區API Key
//! 4. **线程安全**：`RwLock + Arc`
//! 5. **集群集成可退*：不关联 cluster 时也能独立运血//! 6. **与现最task.rs 集成**：`CdcService` 内部委托 `ReplicationTaskManager` 执行任务操作

use crate::cluster::ClusterCoordinator;
use crate::task::{ReplicationTaskManager, TaskConfig, TaskError, TaskInfo};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// P8-4 安全加固：生成 32 字节（256 bit）随机 hex API Key。
///
/// **为什么不用 `sk_<tenant>_<counter>` 格式**：
/// - 旧格式可预测：知道 tenant_id 和签发顺序即可枚举所有 Key
/// - 新格式 `sk_<64 hex chars>`：256 bit 随机熵，暴力枚举不可行
///
/// **为什么不用 uuid crate**：避免引入新依赖，`rand` 已在 workspace 中。
fn generate_api_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("sk_{hex}")
}

// =====================================================================
// TenantTier ‿租户等级
// =====================================================================

/// 租户等级 ‿决定默认配额限制
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantTier {
    /// 免费版：3 任务 / 100 events/sec
    Free,
    /// 专业版：20 任务 / 1000 events/sec
    Pro,
    /// 企业版：100 任务 / 10000 events/sec
    Enterprise,
}

impl TenantTier {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            TenantTier::Free => "free",
            TenantTier::Pro => "pro",
            TenantTier::Enterprise => "enterprise",
        }
    }

    /// 各等级默认限制：(max_tasks, max_throughput events/sec, retention_hours)
    pub fn default_limits(self) -> (u32, u64, u32) {
        match self {
            TenantTier::Free => (3, 100, 24),
            TenantTier::Pro => (20, 1_000, 168),
            TenantTier::Enterprise => (100, 10_000, 720),
        }
    }

    /// 默认允许的source 类型白名单
    pub fn default_allowed_sources(self) -> Vec<String> {
        match self {
            TenantTier::Free => vec!["postgres".to_string(), "mysql".to_string()],
            TenantTier::Pro => vec![
                "postgres".to_string(),
                "mysql".to_string(),
                "oracle".to_string(),
                "sqlserver".to_string(),
            ],
            TenantTier::Enterprise => vec![
                "postgres".to_string(),
                "mysql".to_string(),
                "oracle".to_string(),
                "sqlserver".to_string(),
                "kafka".to_string(),
            ],
        }
    }

    /// 默认允许的target 类型白名单
    pub fn default_allowed_targets(self) -> Vec<String> {
        match self {
            TenantTier::Free => vec!["memory".to_string(), "postgres".to_string()],
            TenantTier::Pro => vec![
                "memory".to_string(),
                "postgres".to_string(),
                "mysql".to_string(),
                "kafka".to_string(),
            ],
            TenantTier::Enterprise => vec![
                "memory".to_string(),
                "postgres".to_string(),
                "mysql".to_string(),
                "oracle".to_string(),
                "kafka".to_string(),
            ],
        }
    }
}

impl std::fmt::Display for TenantTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// TenantConfig ‿租户配置
// =====================================================================

/// 租户配置 ‿描述一个租户的等级、配额与白名區
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TenantConfig {
    /// 租户 ID（唯一标识＿
    pub tenant_id: String,
    /// 租户名称（人类可读）
    pub tenant_name: String,
    /// 租户等级
    pub tier: TenantTier,
    /// 最大任务数
    pub max_tasks: u32,
    /// 最大吞吐量（events/sec＿
    pub max_throughput: u64,
    /// 允许皀source 类型白名區
    pub allowed_sources: Vec<String>,
    /// 允许皀target 类型白名區
    pub allowed_targets: Vec<String>,
    /// 事件保留时长（小时）
    pub retention_hours: u32,
    /// 创建时间戳（Unix 毫秒＿
    pub created_at: u64,
}

impl TenantConfig {
    /// 创建租户配置（自动填充该等级的默认限制）
    pub fn new(
        tenant_id: impl Into<String>,
        tenant_name: impl Into<String>,
        tier: TenantTier,
    ) -> Self {
        let (max_tasks, max_throughput, retention_hours) = tier.default_limits();
        Self {
            tenant_id: tenant_id.into(),
            tenant_name: tenant_name.into(),
            tier,
            max_tasks,
            max_throughput,
            allowed_sources: tier.default_allowed_sources(),
            allowed_targets: tier.default_allowed_targets(),
            retention_hours,
            created_at: current_millis(),
        }
    }

    /// 校验配置合法怀
    pub fn validate(&self) -> Result<(), ServiceError> {
        if self.tenant_id.is_empty() {
            return Err(ServiceError::InvalidConfig(
                "tenant_id is empty".to_string(),
            ));
        }
        if self.tenant_name.is_empty() {
            return Err(ServiceError::InvalidConfig(
                "tenant_name is empty".to_string(),
            ));
        }
        if self.max_tasks == 0 {
            return Err(ServiceError::InvalidConfig(
                "max_tasks must be > 0".to_string(),
            ));
        }
        if self.max_throughput == 0 {
            return Err(ServiceError::InvalidConfig(
                "max_throughput must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    /// 检柀source 类型是否在白名单冀
    pub fn allows_source(&self, source_type: &str) -> bool {
        self.allowed_sources.iter().any(|s| s == source_type)
    }

    /// 检柀target 类型是否在白名单冀
    pub fn allows_target(&self, target_type: &str) -> bool {
        self.allowed_targets.iter().any(|t| t == target_type)
    }
}

// =====================================================================
// ServiceError ‿服务错误
// =====================================================================

/// 服务错误枚举
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// 租户不存圀    
    #[error("tenant not found: {0}")]
    TenantNotFound(String),

    /// 租户已存圀    
    #[error("tenant already exists: {0}")]
    TenantAlreadyExists(String),

    /// 租户配额超限（任务数＿    
    #[error("tenant limit exceeded: {tenant_id} (max_tasks={max_tasks}, current={current})")]
    TenantLimitExceeded {
        tenant_id: String,
        max_tasks: u32,
        current: u32,
    },

    /// 任务不存圀    
    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// 任务已存圀    
    #[error("task already exists: {0}")]
    TaskAlreadyExists(String),

    /// 未认证（API Key 无效或缺失）
    #[error("unauthorized: invalid or missing api key")]
    Unauthorized,

    /// 无权限（任务不属于该租户＿    
    #[error("forbidden: task {task_id} does not belong to tenant {tenant_id}")]
    Forbidden { task_id: String, tenant_id: String },

    /// 吞吐量配额超陀    
    #[error("quota exceeded: throughput {current} > max {max} for tenant {tenant_id}")]
    QuotaExceeded {
        tenant_id: String,
        current: u64,
        max: u64,
    },

    /// 配置无效
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// 集群未关聀    
    #[error("cluster not associated with service")]
    ClusterNotAssociated,

    /// 集群操作错误
    #[error("cluster error: {0}")]
    Cluster(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<TaskError> for ServiceError {
    fn from(e: TaskError) -> Self {
        match e {
            TaskError::NotFound(task_id) => ServiceError::TaskNotFound(task_id),
            TaskError::AlreadyExists(task_id) => ServiceError::TaskAlreadyExists(task_id),
            TaskError::InvalidConfig(msg) => ServiceError::InvalidConfig(msg),
            other => ServiceError::Internal(other.to_string()),
        }
    }
}

impl From<crate::cluster::ClusterError> for ServiceError {
    fn from(e: crate::cluster::ClusterError) -> Self {
        ServiceError::Cluster(e.to_string())
    }
}

// =====================================================================
// TenantUsage ‿租户使用釀// =====================================================================

/// 租户使用量统讀
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TenantUsage {
    /// 当前任务敀
    pub task_count: u32,
    /// 总处理事件数
    pub total_events_processed: u64,
    /// 总处理字节数
    pub total_bytes_processed: u64,
    /// 当前吞吐量（events/sec＿
    pub current_throughput: u64,
    /// 上次重置时间戳（Unix 毫秒＿
    pub last_reset_at: u64,
}

impl TenantUsage {
    /// 创建初始使用釀
    pub fn new() -> Self {
        Self {
            last_reset_at: current_millis(),
            ..Default::default()
        }
    }
}

// =====================================================================
// CreateTaskRequest ‿创建任务请求
// =====================================================================

/// 创建任务请求 ‿由调用方提供，CdcService 校验后委所ReplicationTaskManager 执行
#[derive(Clone)]
pub struct CreateTaskRequest {
    /// 租户 ID
    pub tenant_id: String,
    /// 任务配置（引甀task.rs 皀TaskConfig＿
    pub task_config: TaskConfig,
}

impl std::fmt::Debug for CreateTaskRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateTaskRequest")
            .field("tenant_id", &self.tenant_id)
            .field("task_config", &self.task_config)
            .finish()
    }
}

// =====================================================================
// CdcService ‿CDC 服务主入叀// =====================================================================

/// CDC 服务主入叀‿多租戀CDC 服务皀API 局///
/// **内部结构**＿/// - `tenants`：租户配置表（`tenant_id -> TenantConfig`＿/// - `task_manager`：复甀P2 皀`ReplicationTaskManager`
/// - `cluster`：可选关聀P6-1 皀`ClusterCoordinator`
/// - `tenant_tasks`：租户到任务的映射（`tenant_id -> task_ids`＿/// - `usage_stats`：租户使用量统计（`tenant_id -> TenantUsage`＿///
///
/// **线程安全**：所有状态用 `RwLock` 保护，支持并发读、互斥写
pub struct CdcService {
    /// 租户配置血
    tenants: RwLock<HashMap<String, TenantConfig>>,
    /// 任务管理器（复用 P2＿
    task_manager: Arc<ReplicationTaskManager>,
    /// 可选集群协调器
    cluster: RwLock<Option<Arc<ClusterCoordinator>>>,
    /// 租户到任务的映射
    tenant_tasks: RwLock<HashMap<String, HashSet<String>>>,
    /// 租户使用量统讀
    usage_stats: RwLock<HashMap<String, TenantUsage>>,
    /// 时间戳函数（便于测试固定时间戳）
    timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    /// 累计任务创建次数（统计用＿
    total_tasks_created: AtomicU64,
}

impl CdcService {
    /// 创建 CDC 服务（使甀SystemTime 作为时间戳源＿
    pub fn new(task_manager: Arc<ReplicationTaskManager>) -> Self {
        Self::with_timestamp_fn(task_manager, Box::new(current_millis))
    }

    /// 创建 CDC 服务，注入自定义时间戳函数（便于测试固定时间戳）
    pub fn with_timestamp_fn(
        task_manager: Arc<ReplicationTaskManager>,
        timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            tenants: RwLock::new(HashMap::new()),
            task_manager,
            cluster: RwLock::new(None),
            tenant_tasks: RwLock::new(HashMap::new()),
            usage_stats: RwLock::new(HashMap::new()),
            timestamp_fn,
            total_tasks_created: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------
    // 租户管理 API
    // -----------------------------------------------------------------

    /// 注册租户
    ///
    /// # 错误
    /// - `TenantAlreadyExists`：租户已存在
    /// - `InvalidConfig`：配置不合法
    pub fn register_tenant(&self, config: TenantConfig) -> Result<(), ServiceError> {
        config.validate()?;
        let mut tenants = self.tenants.write();
        if tenants.contains_key(&config.tenant_id) {
            return Err(ServiceError::TenantAlreadyExists(config.tenant_id));
        }
        // 初始化使用量
        let tenant_id = config.tenant_id.clone();
        let mut usage = TenantUsage::new();
        usage.last_reset_at = (self.timestamp_fn)();
        self.usage_stats.write().insert(tenant_id.clone(), usage);
        self.tenant_tasks
            .write()
            .insert(tenant_id.clone(), HashSet::new());
        tenants.insert(tenant_id, config);
        Ok(())
    }

    /// 注销租户
    ///
    /// **流程**＿    /// 1. 校验租户存在
    /// 2. 停止并删除该租户所有任劀    /// 3. 移除租户配置、任务映射、使用量统计
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    pub fn unregister_tenant(&self, tenant_id: &str) -> Result<(), ServiceError> {
        // 先收集该租户的所有任劀ID
        let task_ids: Vec<String> = {
            let tenant_tasks = self.tenant_tasks.read();
            tenant_tasks
                .get(tenant_id)
                .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?
                .iter()
                .cloned()
                .collect()
        };

        // 停止并删除所有任务（忽略单个任务的失败，继续清理＿
        for task_id in &task_ids {
            let _ = self.task_manager.remove_task(task_id);
        }

        // 移除租户的所有映尀
        {
            let mut tenants = self.tenants.write();
            tenants
                .remove(tenant_id)
                .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?;
        }
        self.tenant_tasks.write().remove(tenant_id);
        self.usage_stats.write().remove(tenant_id);
        Ok(())
    }

    /// 获取租户配置
    pub fn get_tenant(&self, tenant_id: &str) -> Result<TenantConfig, ServiceError> {
        let tenants = self.tenants.read();
        tenants
            .get(tenant_id)
            .cloned()
            .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))
    }

    /// 列出所有租戀
    pub fn list_tenants(&self) -> Vec<TenantConfig> {
        let tenants = self.tenants.read();
        let mut list: Vec<TenantConfig> = tenants.values().cloned().collect();
        list.sort_by(|a, b| a.tenant_id.cmp(&b.tenant_id));
        list
    }

    /// 更新租户等级
    ///
    /// **效果**：更新等级后，自动应用新等级的默认配额限刀
    pub fn update_tenant_tier(
        &self,
        tenant_id: &str,
        new_tier: TenantTier,
    ) -> Result<(), ServiceError> {
        let mut tenants = self.tenants.write();
        let config = tenants
            .get_mut(tenant_id)
            .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?;
        config.tier = new_tier;
        let (max_tasks, max_throughput, retention_hours) = new_tier.default_limits();
        config.max_tasks = max_tasks;
        config.max_throughput = max_throughput;
        config.retention_hours = retention_hours;
        Ok(())
    }

    // -----------------------------------------------------------------
    // 任务管理 API（多租户隔离＿    // -----------------------------------------------------------------

    /// 创建任务（多租户隔离 + 配额检柀+ 白名单检查）
    ///
    /// **流程**＿    /// 1. 校验租户存在
    /// 2. 校验任务数未趀`max_tasks`
    /// 3. 校验 source/target 在白名单内（通过 task_config.target_type 判断 target＿    /// 4. 委托 `ReplicationTaskManager::create_task` 执行
    /// 5. 更新 tenant_tasks 映射 + 使用釀    ///
    /// # 返回
    /// - `Ok(task_id)`：创建成功的任务 ID
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    /// - `TenantLimitExceeded`：任务数已达上限
    /// - `InvalidConfig`：source/target 不在白名區    /// - `TaskAlreadyExists`：任劀ID 已被使用
    pub fn create_task(&self, request: CreateTaskRequest) -> Result<String, ServiceError> {
        // 1. 校验租户存在 + 配额检柀+ 白名单检查（持读锁）
        {
            let tenants = self.tenants.read();
            let config = tenants
                .get(&request.tenant_id)
                .ok_or_else(|| ServiceError::TenantNotFound(request.tenant_id.clone()))?;

            // 配额检查：当前任务敀vs max_tasks
            let current_count = self
                .tenant_tasks
                .read()
                .get(&request.tenant_id)
                .map(|s| s.len() as u32)
                .unwrap_or(0);
            if current_count >= config.max_tasks {
                return Err(ServiceError::TenantLimitExceeded {
                    tenant_id: request.tenant_id.clone(),
                    max_tasks: config.max_tasks,
                    current: current_count,
                });
            }

            // target 白名单检查（通过 task_config.target_type＿
            if !config.allows_target(&request.task_config.target_type) {
                return Err(ServiceError::InvalidConfig(format!(
                    "target type '{}' not in whitelist for tenant '{}'",
                    request.task_config.target_type, request.tenant_id
                )));
            }
        }

        // 2. 委托 ReplicationTaskManager 创建任务（持锁外，避免嵌套锁＿
        let task_id = request.task_config.task_id.clone();
        self.task_manager
            .create_task(request.task_config)
            .map_err(ServiceError::from)?;

        // 3. 更新 tenant_tasks 映射 + 使用釀
        {
            let mut tenant_tasks = self.tenant_tasks.write();
            tenant_tasks
                .entry(request.tenant_id.clone())
                .or_default()
                .insert(task_id.clone());
        }
        {
            let mut usage_stats = self.usage_stats.write();
            let usage = usage_stats.entry(request.tenant_id.clone()).or_default();
            usage.task_count = usage.task_count.saturating_add(1);
        }
        self.total_tasks_created.fetch_add(1, Ordering::SeqCst);
        Ok(task_id)
    }

    /// 删除任务（校验任务归属）
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    /// - `TaskNotFound`：任务不属于该租户或不存圀
    pub fn delete_task(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .remove_task(task_id)
            .map_err(ServiceError::from)?;
        // 更新映射 + 使用釀
        {
            let mut tenant_tasks = self.tenant_tasks.write();
            if let Some(tasks) = tenant_tasks.get_mut(tenant_id) {
                tasks.remove(task_id);
            }
        }
        {
            let mut usage_stats = self.usage_stats.write();
            if let Some(usage) = usage_stats.get_mut(tenant_id) {
                usage.task_count = usage.task_count.saturating_sub(1);
            }
        }
        Ok(())
    }

    /// 启动任务（校验归属）
    pub fn start_task(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .start_task(task_id)
            .map_err(ServiceError::from)
    }

    /// 停止任务（校验归属）
    pub fn stop_task(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .stop_task(task_id)
            .map_err(ServiceError::from)
    }

    /// 暂停任务（校验归属）
    pub fn pause_task(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .pause_task(task_id)
            .map_err(ServiceError::from)
    }

    /// 恢复任务（校验归属）
    pub fn resume_task(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .resume_task(task_id)
            .map_err(ServiceError::from)
    }

    /// 获取任务信息（校验归属）
    pub fn get_task(&self, tenant_id: &str, task_id: &str) -> Result<TaskInfo, ServiceError> {
        self.validate_task_ownership(tenant_id, task_id)?;
        self.task_manager
            .monitor_task(task_id)
            .map_err(ServiceError::from)
    }

    /// 列出租户的所有任劀    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    pub fn list_tasks(&self, tenant_id: &str) -> Result<Vec<TaskInfo>, ServiceError> {
        // 校验租户存在
        {
            let tenants = self.tenants.read();
            if !tenants.contains_key(tenant_id) {
                return Err(ServiceError::TenantNotFound(tenant_id.to_string()));
            }
        }
        // 收集该租户的所有任劀ID
        let task_ids: HashSet<String> = {
            let tenant_tasks = self.tenant_tasks.read();
            tenant_tasks.get(tenant_id).cloned().unwrap_or_default()
        };
        // 什task_manager 获取所有任务信息，过滤出属于该租户皀
        let all_tasks = self.task_manager.list_tasks();
        Ok(all_tasks
            .into_iter()
            .filter(|info| task_ids.contains(&info.task_id))
            .collect())
    }

    // -----------------------------------------------------------------
    // 配额与使用量 API
    // -----------------------------------------------------------------

    /// 获取租户使用釀
    pub fn get_usage(&self, tenant_id: &str) -> Result<TenantUsage, ServiceError> {
        // 校验租户存在
        {
            let tenants = self.tenants.read();
            if !tenants.contains_key(tenant_id) {
                return Err(ServiceError::TenantNotFound(tenant_id.to_string()));
            }
        }
        let usage_stats = self.usage_stats.read();
        Ok(usage_stats
            .get(tenant_id)
            .cloned()
            .unwrap_or_else(TenantUsage::new))
    }

    /// 检查配额（是否可创建新任务＿    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    /// - `TenantLimitExceeded`：任务数已达上限
    pub fn check_quota(&self, tenant_id: &str) -> Result<(), ServiceError> {
        let tenants = self.tenants.read();
        let config = tenants
            .get(tenant_id)
            .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?;
        let current_count = self
            .tenant_tasks
            .read()
            .get(tenant_id)
            .map(|s| s.len() as u32)
            .unwrap_or(0);
        if current_count >= config.max_tasks {
            return Err(ServiceError::TenantLimitExceeded {
                tenant_id: tenant_id.to_string(),
                max_tasks: config.max_tasks,
                current: current_count,
            });
        }
        Ok(())
    }

    /// 更新使用量（内部调用，由事件处理路径调用＿    ///
    /// **效果**＿    /// - 累加 `total_events_processed` 咀`total_bytes_processed`
    /// - 更新 `current_throughput`（简单平均：总事件数 / 已运行秒数）
    /// - 苀`current_throughput > max_throughput`，返囀`QuotaExceeded`
    ///
    /// # 参数
    /// - `tenant_id`：租戀ID
    /// - `events`：本次处理的事件敀    /// - `bytes`：本次处理的字节敀
    pub fn update_usage(
        &self,
        tenant_id: &str,
        events: u64,
        bytes: u64,
    ) -> Result<(), ServiceError> {
        // 校验租户存在
        let max_throughput = {
            let tenants = self.tenants.read();
            tenants
                .get(tenant_id)
                .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?
                .max_throughput
        };
        let now = (self.timestamp_fn)();
        let mut usage_stats = self.usage_stats.write();
        let usage = usage_stats.entry(tenant_id.to_string()).or_default();
        usage.total_events_processed = usage.total_events_processed.saturating_add(events);
        usage.total_bytes_processed = usage.total_bytes_processed.saturating_add(bytes);
        // 计算吞吐量（events/sec）：总事件数 / 已运行秒敀
        let elapsed_ms = now.saturating_sub(usage.last_reset_at);
        if elapsed_ms > 0 {
            let elapsed_sec = elapsed_ms / 1000;
            if let Some(throughput) = usage.total_events_processed.checked_div(elapsed_sec) {
                usage.current_throughput = throughput;
            }
        }
        // 检查吞吐量配额
        if usage.current_throughput > max_throughput {
            return Err(ServiceError::QuotaExceeded {
                tenant_id: tenant_id.to_string(),
                current: usage.current_throughput,
                max: max_throughput,
            });
        }
        Ok(())
    }

    /// 按保留时长定期重置使用量
    ///
    /// **效果**：若距离 `last_reset_at` 超过 `retention_hours`，重置统计计数器
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    pub fn reset_usage_if_needed(&self, tenant_id: &str) -> Result<(), ServiceError> {
        let retention_ms = {
            let tenants = self.tenants.read();
            tenants
                .get(tenant_id)
                .ok_or_else(|| ServiceError::TenantNotFound(tenant_id.to_string()))?
                .retention_hours as u64
                * 3_600_000
        };
        let now = (self.timestamp_fn)();
        let mut usage_stats = self.usage_stats.write();
        let usage = usage_stats.entry(tenant_id.to_string()).or_default();
        let elapsed = now.saturating_sub(usage.last_reset_at);
        if elapsed >= retention_ms {
            usage.total_events_processed = 0;
            usage.total_bytes_processed = 0;
            usage.current_throughput = 0;
            usage.last_reset_at = now;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // 集群集成 API（可选）
    // -----------------------------------------------------------------

    /// 关联 P6-1 集群协调噀
    pub fn with_cluster(self, cluster: Arc<ClusterCoordinator>) -> Self {
        *self.cluster.write() = Some(cluster);
        self
    }

    /// 设置集群（builder 模式，返囀Self 以支持链式调用）
    pub fn set_cluster(&self, cluster: Arc<ClusterCoordinator>) {
        *self.cluster.write() = Some(cluster);
    }

    /// 获取关联的集群（若存在）
    pub fn cluster(&self) -> Option<Arc<ClusterCoordinator>> {
        self.cluster.read().clone()
    }

    /// 将任务分配到集群节点
    ///
    /// # 返回
    /// - `Ok(node_id)`：分配到的节炀ID
    ///
    /// # 错误
    /// - `ClusterNotAssociated`：未关联集群
    /// - `Cluster`：集群分配失贀
    pub fn assign_task_to_cluster(&self, task_id: &str) -> Result<String, ServiceError> {
        let cluster = self
            .cluster
            .read()
            .clone()
            .ok_or(ServiceError::ClusterNotAssociated)?;
        cluster.assign_task(task_id, "").map_err(ServiceError::from)
    }

    /// 迁移租户的所有任务到集群其他节点
    ///
    /// # 返回
    /// - `Ok(Vec<task_id>)`：成功迁移的任务 ID 列表
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    /// - `ClusterNotAssociated`：未关联集群
    pub fn migrate_tenant_tasks(&self, tenant_id: &str) -> Result<Vec<String>, ServiceError> {
        // 校验租户存在
        {
            let tenants = self.tenants.read();
            if !tenants.contains_key(tenant_id) {
                return Err(ServiceError::TenantNotFound(tenant_id.to_string()));
            }
        }
        let cluster = self
            .cluster
            .read()
            .clone()
            .ok_or(ServiceError::ClusterNotAssociated)?;
        // 收集租户的所有任劀ID
        let task_ids: Vec<String> = {
            let tenant_tasks = self.tenant_tasks.read();
            tenant_tasks
                .get(tenant_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect()
        };
        // 对每个任务，找当前分配节点之外的其他节点迁移
        let mut migrated = Vec::new();
        for task_id in &task_ids {
            // 获取当前分配的节炀
            if let Some(current_node) = cluster.get_assignment(task_id) {
                // 选择其他节点迁移（找列表中第一个非当前节点＿
                let nodes = cluster.list_nodes();
                if let Some(target) = nodes.iter().find(|n| n.node_id != current_node) {
                    if cluster.migrate_task(task_id, &target.node_id).is_ok() {
                        migrated.push(task_id.clone());
                    }
                }
            }
        }
        Ok(migrated)
    }

    // -----------------------------------------------------------------
    // 内部辅助方法
    // -----------------------------------------------------------------

    /// 校验任务归属租户
    ///
    /// # 错误
    /// - `TenantNotFound`：租户不存在
    /// - `TaskNotFound`：任务不属于该租户或不存圀
    fn validate_task_ownership(&self, tenant_id: &str, task_id: &str) -> Result<(), ServiceError> {
        // 校验租户存在
        {
            let tenants = self.tenants.read();
            if !tenants.contains_key(tenant_id) {
                return Err(ServiceError::TenantNotFound(tenant_id.to_string()));
            }
        }
        // 校验任务归属
        let belongs = {
            let tenant_tasks = self.tenant_tasks.read();
            tenant_tasks
                .get(tenant_id)
                .map(|tasks| tasks.contains(task_id))
                .unwrap_or(false)
        };
        if !belongs {
            return Err(ServiceError::TaskNotFound(task_id.to_string()));
        }
        Ok(())
    }

    /// 获取累计任务创建次数（统计用＿
    pub fn total_tasks_created(&self) -> u64 {
        self.total_tasks_created.load(Ordering::SeqCst)
    }
}

// =====================================================================
// AuthService ‿简化认证服劀// =====================================================================

/// 简化认证服劀‿使用 API Key 进行认证（不依赖 JWT 库）
///
/// **内部结构**＿/// - `api_keys`：`api_key -> tenant_id` 的映尀///
/// **线程安全**：`RwLock<HashMap<String, String>>`
pub struct AuthService {
    /// API Key 到租户 ID 的映射
    api_keys: RwLock<HashMap<String, String>>,
}

impl AuthService {
    /// 创建认证服务
    pub fn new() -> Self {
        Self {
            api_keys: RwLock::new(HashMap::new()),
        }
    }

    /// 签发 API Key
    ///
    /// **格式（P8-4 安全加固）**：`sk_<64 hex chars>`（256 bit 随机熵）
    ///
    /// 旧格式 `sk_<tenant_id>_<counter>` 已废弃：可预测、可枚举。
    /// 新格式使用 CSPRNG 生成 32 字节随机数，暴力枚举不可行。
    ///
    /// # 返回
    /// 生成的 API Key 字符串
    pub fn issue_api_key(&self, tenant_id: &str) -> String {
        let api_key = generate_api_key();
        self.api_keys
            .write()
            .insert(api_key.clone(), tenant_id.to_string());
        api_key
    }

    /// 撤销 API Key
    ///
    /// # 错误
    /// - `Unauthorized`：API Key 不存圀
    pub fn revoke_api_key(&self, api_key: &str) -> Result<(), ServiceError> {
        let mut keys = self.api_keys.write();
        if keys.remove(api_key).is_none() {
            return Err(ServiceError::Unauthorized);
        }
        Ok(())
    }

    /// 认证 API Key，返回对应的 tenant_id
    ///
    /// # 错误
    /// - `Unauthorized`：API Key 无效
    pub fn authenticate(&self, api_key: &str) -> Result<String, ServiceError> {
        let keys = self.api_keys.read();
        keys.get(api_key).cloned().ok_or(ServiceError::Unauthorized)
    }

    /// 校验 API Key 对指定任务的访问权限
    ///
    /// **流程**＿    /// 1. 认证 API Key，得刀tenant_id
    /// 2. 校验任务属于诀tenant_id
    ///
    /// # 参数
    /// - `api_key`：API Key
    /// - `task_id`：要访问的任劀ID
    /// - `tenant_tasks`：租户到任务的映射（甀CdcService 提供＿    ///
    /// # 错误
    /// - `Unauthorized`：API Key 无效
    /// - `Forbidden`：任务不属于该租戀
    pub fn validate_access(
        &self,
        api_key: &str,
        task_id: &str,
        tenant_tasks: &RwLock<HashMap<String, HashSet<String>>>,
    ) -> Result<String, ServiceError> {
        let tenant_id = self.authenticate(api_key)?;
        // 校验任务归属
        let belongs = {
            let tenant_tasks = tenant_tasks.read();
            tenant_tasks
                .get(&tenant_id)
                .map(|tasks| tasks.contains(task_id))
                .unwrap_or(false)
        };
        if !belongs {
            return Err(ServiceError::Forbidden {
                task_id: task_id.to_string(),
                tenant_id,
            });
        }
        Ok(tenant_id)
    }

    /// 获取已签发的 API Key 数量
    pub fn api_key_count(&self) -> usize {
        self.api_keys.read().len()
    }
}

impl Default for AuthService {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 当前 Unix 毫秒
fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{ClusterConfig, ClusterCoordinator};
    use crate::schema::SchemaRegistry;
    use crate::slot::SlotManager;
    use crate::target::memory::MemoryWriter;
    use crate::target::TargetWriter;
    use crate::CdcEngine;
    use std::sync::Arc;
    use std::thread;

    // --- 测试辅助 ---

    /// 创建测试甀schema_registry + decoder + slot_mgr
    fn test_setup() -> (
        Arc<SchemaRegistry>,
        Arc<crate::decoder::RowDecoder>,
        Arc<SlotManager>,
    ) {
        let registry = Arc::new(SchemaRegistry::new());
        let decoder = Arc::new(crate::decoder::RowDecoder::new(registry.clone()));
        let slot_mgr = Arc::new(SlotManager::in_memory());
        (registry, decoder, slot_mgr)
    }

    /// 创建测试甀CdcEngine（固定时间戳 0＿
    fn test_cdc_engine() -> Arc<CdcEngine> {
        let observer_mgr = Arc::new(crate::CdcObserverManager::new());
        Arc::new(CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0)))
    }

    /// 创建测试甀task_manager
    fn test_task_manager() -> Arc<ReplicationTaskManager> {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        Arc::new(ReplicationTaskManager::new(
            slot_mgr, decoder, registry, cdc_engine,
        ))
    }

    /// 创建测试甀task config
    fn test_task_config(task_id: &str, writer: Arc<dyn TargetWriter>) -> TaskConfig {
        TaskConfig {
            task_id: task_id.to_string(),
            description: "test task".to_string(),
            table_filter: None,
            writer,
            target_type: "memory".to_string(),
            target_connection: "memory://test".to_string(),
            snapshot_first: false,
            dialect: crate::migration::Dialect::Postgres,
            backpressure_config: crate::backpressure::BackpressureConfig::default(),
        }
    }

    /// 创建测试甀CdcService（固定时间戳 0＿
    fn test_service(task_manager: Arc<ReplicationTaskManager>) -> CdcService {
        CdcService::with_timestamp_fn(task_manager, Box::new(|| 0))
    }

    // =================================================================
    // 1. TenantTier 测试
    // =================================================================

    #[test]
    fn tenant_tier_default_limits_free() {
        let (max_tasks, max_throughput, retention) = TenantTier::Free.default_limits();
        assert_eq!(max_tasks, 3);
        assert_eq!(max_throughput, 100);
        assert_eq!(retention, 24);
    }

    #[test]
    fn tenant_tier_default_limits_pro() {
        let (max_tasks, max_throughput, retention) = TenantTier::Pro.default_limits();
        assert_eq!(max_tasks, 20);
        assert_eq!(max_throughput, 1_000);
        assert_eq!(retention, 168);
    }

    #[test]
    fn tenant_tier_default_limits_enterprise() {
        let (max_tasks, max_throughput, retention) = TenantTier::Enterprise.default_limits();
        assert_eq!(max_tasks, 100);
        assert_eq!(max_throughput, 10_000);
        assert_eq!(retention, 720);
    }

    #[test]
    fn tenant_tier_as_str() {
        assert_eq!(TenantTier::Free.as_str(), "free");
        assert_eq!(TenantTier::Pro.as_str(), "pro");
        assert_eq!(TenantTier::Enterprise.as_str(), "enterprise");
    }

    #[test]
    fn tenant_tier_default_allowed_sources_by_tier() {
        // Free 只有 postgres/mysql
        let free_sources = TenantTier::Free.default_allowed_sources();
        assert!(free_sources.contains(&"postgres".to_string()));
        assert!(free_sources.contains(&"mysql".to_string()));
        assert!(!free_sources.contains(&"oracle".to_string()));
        // Enterprise 包含所最
        let ent_sources = TenantTier::Enterprise.default_allowed_sources();
        assert!(ent_sources.contains(&"postgres".to_string()));
        assert!(ent_sources.contains(&"kafka".to_string()));
    }

    #[test]
    fn tenant_tier_default_allowed_targets_by_tier() {
        let free_targets = TenantTier::Free.default_allowed_targets();
        assert!(free_targets.contains(&"memory".to_string()));
        assert!(!free_targets.contains(&"kafka".to_string()));
        let pro_targets = TenantTier::Pro.default_allowed_targets();
        assert!(pro_targets.contains(&"kafka".to_string()));
    }

    // =================================================================
    // 2. TenantConfig 测试
    // =================================================================

    #[test]
    fn tenant_config_new_uses_tier_defaults() {
        let config = TenantConfig::new("t1", "Tenant 1", TenantTier::Free);
        assert_eq!(config.tenant_id, "t1");
        assert_eq!(config.tenant_name, "Tenant 1");
        assert_eq!(config.tier, TenantTier::Free);
        assert_eq!(config.max_tasks, 3);
        assert_eq!(config.max_throughput, 100);
    }

    #[test]
    fn tenant_config_validate_rejects_empty_id() {
        let config = TenantConfig {
            tenant_id: "".to_string(),
            tenant_name: "Test".to_string(),
            tier: TenantTier::Free,
            max_tasks: 1,
            max_throughput: 100,
            allowed_sources: vec![],
            allowed_targets: vec![],
            retention_hours: 24,
            created_at: 0,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn tenant_config_validate_rejects_zero_max_tasks() {
        let config = TenantConfig {
            tenant_id: "t1".to_string(),
            tenant_name: "Test".to_string(),
            tier: TenantTier::Free,
            max_tasks: 0,
            max_throughput: 100,
            allowed_sources: vec![],
            allowed_targets: vec![],
            retention_hours: 24,
            created_at: 0,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn tenant_config_allows_source_whitelist() {
        let config = TenantConfig::new("t1", "T1", TenantTier::Free);
        assert!(config.allows_source("postgres"));
        assert!(config.allows_source("mysql"));
        assert!(!config.allows_source("oracle"));
    }

    #[test]
    fn tenant_config_allows_target_whitelist() {
        let config = TenantConfig::new("t1", "T1", TenantTier::Pro);
        assert!(config.allows_target("postgres"));
        assert!(config.allows_target("kafka"));
        assert!(!config.allows_target("nonexistent"));
    }

    // =================================================================
    // 3. ServiceError From<TaskError> 测试
    // =================================================================

    #[test]
    fn service_error_from_task_error_not_found() {
        let err = TaskError::NotFound("task-x".to_string());
        let service_err: ServiceError = err.into();
        assert!(matches!(service_err, ServiceError::TaskNotFound(_)));
    }

    #[test]
    fn service_error_from_task_error_already_exists() {
        let err = TaskError::AlreadyExists("task-1".to_string());
        let service_err: ServiceError = err.into();
        assert!(matches!(service_err, ServiceError::TaskAlreadyExists(_)));
    }

    // =================================================================
    // 4. 租户注册/注销/查询/列表测试
    // =================================================================

    #[test]
    fn register_tenant_success() {
        let service = test_service(test_task_manager());
        let config = TenantConfig::new("t1", "Tenant 1", TenantTier::Free);
        assert!(service.register_tenant(config).is_ok());
    }

    #[test]
    fn register_tenant_duplicate_fails() {
        let service = test_service(test_task_manager());
        let config = TenantConfig::new("t1", "Tenant 1", TenantTier::Free);
        service.register_tenant(config).unwrap();
        let config2 = TenantConfig::new("t1", "Tenant 1 Again", TenantTier::Pro);
        let result = service.register_tenant(config2);
        assert!(matches!(result, Err(ServiceError::TenantAlreadyExists(_))));
    }

    #[test]
    fn register_tenant_invalid_config_fails() {
        let service = test_service(test_task_manager());
        let config = TenantConfig {
            tenant_id: "".to_string(),
            tenant_name: "Test".to_string(),
            tier: TenantTier::Free,
            max_tasks: 1,
            max_throughput: 100,
            allowed_sources: vec![],
            allowed_targets: vec![],
            retention_hours: 24,
            created_at: 0,
        };
        let result = service.register_tenant(config);
        assert!(matches!(result, Err(ServiceError::InvalidConfig(_))));
    }

    #[test]
    fn unregister_tenant_success() {
        let service = test_service(test_task_manager());
        let config = TenantConfig::new("t1", "Tenant 1", TenantTier::Free);
        service.register_tenant(config).unwrap();
        assert!(service.unregister_tenant("t1").is_ok());
        // 注销后查询应失败
        assert!(matches!(
            service.get_tenant("t1"),
            Err(ServiceError::TenantNotFound(_))
        ));
    }

    #[test]
    fn unregister_nonexistent_tenant_fails() {
        let service = test_service(test_task_manager());
        let result = service.unregister_tenant("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    #[test]
    fn get_tenant_returns_config() {
        let service = test_service(test_task_manager());
        let config = TenantConfig::new("t1", "Tenant 1", TenantTier::Pro);
        service.register_tenant(config).unwrap();
        let got = service.get_tenant("t1").unwrap();
        assert_eq!(got.tenant_id, "t1");
        assert_eq!(got.tier, TenantTier::Pro);
    }

    #[test]
    fn get_nonexistent_tenant_fails() {
        let service = test_service(test_task_manager());
        let result = service.get_tenant("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    #[test]
    fn list_tenants_returns_all_sorted() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t3", "T3", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        let list = service.list_tenants();
        assert_eq!(list.len(), 3);
        // 挀tenant_id 字典序排庀
        assert_eq!(list[0].tenant_id, "t1");
        assert_eq!(list[1].tenant_id, "t2");
        assert_eq!(list[2].tenant_id, "t3");
    }

    #[test]
    fn list_tenants_empty_returns_empty() {
        let service = test_service(test_task_manager());
        let list = service.list_tenants();
        assert!(list.is_empty());
    }

    // =================================================================
    // 5. 租户等级升级测试
    // =================================================================

    #[test]
    fn update_tenant_tier_free_to_pro() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service.update_tenant_tier("t1", TenantTier::Pro).unwrap();
        let config = service.get_tenant("t1").unwrap();
        assert_eq!(config.tier, TenantTier::Pro);
        assert_eq!(config.max_tasks, 20);
        assert_eq!(config.max_throughput, 1_000);
    }

    #[test]
    fn update_tenant_tier_pro_to_enterprise() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Pro))
            .unwrap();
        service
            .update_tenant_tier("t1", TenantTier::Enterprise)
            .unwrap();
        let config = service.get_tenant("t1").unwrap();
        assert_eq!(config.tier, TenantTier::Enterprise);
        assert_eq!(config.max_tasks, 100);
        assert_eq!(config.max_throughput, 10_000);
    }

    #[test]
    fn update_tenant_tier_full_cycle() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // Free -> Pro
        service.update_tenant_tier("t1", TenantTier::Pro).unwrap();
        assert_eq!(service.get_tenant("t1").unwrap().tier, TenantTier::Pro);
        // Pro -> Enterprise
        service
            .update_tenant_tier("t1", TenantTier::Enterprise)
            .unwrap();
        assert_eq!(
            service.get_tenant("t1").unwrap().tier,
            TenantTier::Enterprise
        );
    }

    #[test]
    fn update_tenant_tier_nonexistent_fails() {
        let service = test_service(test_task_manager());
        let result = service.update_tenant_tier("nonexistent", TenantTier::Pro);
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    // =================================================================
    // 6. AuthService 测试
    // =================================================================

    #[test]
    fn auth_service_issue_api_key() {
        let auth = AuthService::new();
        let key = auth.issue_api_key("t1");
        // P8-4：新格式 sk_<64 hex>，不再包含 tenant_id
        assert!(key.starts_with("sk_"));
        assert_eq!(key.len(), 3 + 64); // "sk_" + 32 bytes hex (64 chars)
        assert!(key[3..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(auth.api_key_count(), 1);
    }

    #[test]
    fn auth_service_issue_api_key_is_random() {
        // P8-4：验证两次签发的 Key 不同（随机性）
        let auth = AuthService::new();
        let key1 = auth.issue_api_key("t1");
        let key2 = auth.issue_api_key("t1");
        assert_ne!(key1, key2, "API Keys must be random, not predictable");
    }

    #[test]
    fn auth_service_authenticate_valid_key() {
        let auth = AuthService::new();
        let key = auth.issue_api_key("t1");
        let tenant_id = auth.authenticate(&key).unwrap();
        assert_eq!(tenant_id, "t1");
    }

    #[test]
    fn auth_service_authenticate_invalid_key_fails() {
        let auth = AuthService::new();
        let result = auth.authenticate("invalid_key");
        assert!(matches!(result, Err(ServiceError::Unauthorized)));
    }

    #[test]
    fn auth_service_revoke_api_key() {
        let auth = AuthService::new();
        let key = auth.issue_api_key("t1");
        assert!(auth.revoke_api_key(&key).is_ok());
        // 撤销后认证应失败
        assert!(matches!(
            auth.authenticate(&key),
            Err(ServiceError::Unauthorized)
        ));
    }

    #[test]
    fn auth_service_revoke_nonexistent_key_fails() {
        let auth = AuthService::new();
        let result = auth.revoke_api_key("nonexistent_key");
        assert!(matches!(result, Err(ServiceError::Unauthorized)));
    }

    #[test]
    fn auth_service_issue_multiple_keys_for_same_tenant() {
        let auth = AuthService::new();
        let key1 = auth.issue_api_key("t1");
        let key2 = auth.issue_api_key("t1");
        assert_ne!(key1, key2);
        assert_eq!(auth.api_key_count(), 2);
        // 两个 key 都能认证刀t1
        assert_eq!(auth.authenticate(&key1).unwrap(), "t1");
        assert_eq!(auth.authenticate(&key2).unwrap(), "t1");
    }

    #[test]
    fn auth_service_validate_access_granted() {
        let auth = AuthService::new();
        let task_manager = test_task_manager();
        let service = test_service(task_manager);
        // 注册租户
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 创建任务
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let request = CreateTaskRequest {
            tenant_id: "t1".to_string(),
            task_config: test_task_config("task-1", writer),
        };
        service.create_task(request).unwrap();
        // 签发 API Key
        let key = auth.issue_api_key("t1");
        // 校验访问权限
        let tenant_id = auth
            .validate_access(&key, "task-1", &service.tenant_tasks)
            .unwrap();
        assert_eq!(tenant_id, "t1");
    }

    #[test]
    fn auth_service_validate_access_forbidden_for_other_tenant_task() {
        let auth = AuthService::new();
        let task_manager = test_task_manager();
        let service = test_service(task_manager);
        // 注册两个租户
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        // t1 创建任务
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        // t2 签发 API Key
        let key_t2 = auth.issue_api_key("t2");
        // t2 尝试访问 t1 的任务，应被拒绝
        let result = auth.validate_access(&key_t2, "task-t1", &service.tenant_tasks);
        assert!(matches!(result, Err(ServiceError::Forbidden { .. })));
    }

    #[test]
    fn auth_service_validate_access_invalid_key_fails() {
        let auth = AuthService::new();
        let task_manager = test_task_manager();
        let service = test_service(task_manager);
        let result = auth.validate_access("invalid_key", "task-1", &service.tenant_tasks);
        assert!(matches!(result, Err(ServiceError::Unauthorized)));
    }

    // =================================================================
    // 7. 创建任务测试（配额、白名单＿    // =================================================================

    #[test]
    fn create_task_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let request = CreateTaskRequest {
            tenant_id: "t1".to_string(),
            task_config: test_task_config("task-1", writer),
        };
        let task_id = service.create_task(request).unwrap();
        assert_eq!(task_id, "task-1");
        assert_eq!(service.total_tasks_created(), 1);
    }

    #[test]
    fn create_task_tenant_not_found_fails() {
        let service = test_service(test_task_manager());
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let request = CreateTaskRequest {
            tenant_id: "nonexistent".to_string(),
            task_config: test_task_config("task-1", writer),
        };
        let result = service.create_task(request);
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    #[test]
    fn create_task_quota_exceeded_fails() {
        let service = test_service(test_task_manager());
        // Free 等级 max_tasks=3
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 创建 3 个任务（达到上限＿
        for i in 0..3 {
            let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
            service
                .create_task(CreateTaskRequest {
                    tenant_id: "t1".to_string(),
                    task_config: test_task_config(&format!("task-{}", i), writer),
                })
                .unwrap();
        }
        // 笀4 个任务应失败
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let result = service.create_task(CreateTaskRequest {
            tenant_id: "t1".to_string(),
            task_config: test_task_config("task-3", writer),
        });
        assert!(matches!(
            result,
            Err(ServiceError::TenantLimitExceeded { .. })
        ));
    }

    #[test]
    fn create_task_target_not_in_whitelist_fails() {
        let service = test_service(test_task_manager());
        // Free 等级不允讀oracle target
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let mut config = test_task_config("task-1", writer);
        config.target_type = "oracle".to_string();
        let result = service.create_task(CreateTaskRequest {
            tenant_id: "t1".to_string(),
            task_config: config,
        });
        assert!(matches!(result, Err(ServiceError::InvalidConfig(_))));
    }

    #[test]
    fn create_task_duplicate_task_id_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        let writer2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let result = service.create_task(CreateTaskRequest {
            tenant_id: "t1".to_string(),
            task_config: test_task_config("task-1", writer2),
        });
        assert!(matches!(result, Err(ServiceError::TaskAlreadyExists(_))));
    }

    // =================================================================
    // 8. 删除任务测试（校验归属）
    // =================================================================

    #[test]
    fn delete_task_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        assert!(service.delete_task("t1", "task-1").is_ok());
        // 删除后列表应为空
        let list = service.list_tasks("t1").unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn delete_task_wrong_tenant_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        // t1 创建任务
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        // t2 尝试删除 t1 的任劀
        let result = service.delete_task("t2", "task-t1");
        assert!(matches!(result, Err(ServiceError::TaskNotFound(_))));
    }

    #[test]
    fn delete_task_tenant_not_found_fails() {
        let service = test_service(test_task_manager());
        let result = service.delete_task("nonexistent", "task-1");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    #[test]
    fn delete_task_task_not_found_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let result = service.delete_task("t1", "nonexistent-task");
        assert!(matches!(result, Err(ServiceError::TaskNotFound(_))));
    }

    // =================================================================
    // 9. 启动/停止/暂停/恢复任务测试（校验归属）
    // =================================================================

    #[test]
    fn start_task_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        assert!(service.start_task("t1", "task-1").is_ok());
    }

    #[test]
    fn start_task_wrong_tenant_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        let result = service.start_task("t2", "task-t1");
        assert!(matches!(result, Err(ServiceError::TaskNotFound(_))));
    }

    #[test]
    fn pause_resume_task_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        service.start_task("t1", "task-1").unwrap();
        service.pause_task("t1", "task-1").unwrap();
        service.resume_task("t1", "task-1").unwrap();
    }

    #[test]
    fn stop_task_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        service.start_task("t1", "task-1").unwrap();
        assert!(service.stop_task("t1", "task-1").is_ok());
    }

    #[test]
    fn pause_task_wrong_tenant_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        let result = service.pause_task("t2", "task-t1");
        assert!(matches!(result, Err(ServiceError::TaskNotFound(_))));
    }

    // =================================================================
    // 10. 获取任务信息测试
    // =================================================================

    #[test]
    fn get_task_info_success() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        let info = service.get_task("t1", "task-1").unwrap();
        assert_eq!(info.task_id, "task-1");
    }

    #[test]
    fn get_task_wrong_tenant_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        let result = service.get_task("t2", "task-t1");
        assert!(matches!(result, Err(ServiceError::TaskNotFound(_))));
    }

    // =================================================================
    // 11. 列出租户任务测试
    // =================================================================

    #[test]
    fn list_tasks_returns_tenant_tasks() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 创建 2 个任劀
        for i in 0..2 {
            let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
            service
                .create_task(CreateTaskRequest {
                    tenant_id: "t1".to_string(),
                    task_config: test_task_config(&format!("task-{}", i), writer),
                })
                .unwrap();
        }
        let list = service.list_tasks("t1").unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn list_tasks_isolates_other_tenants() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        // t1 创建 2 个任劀
        for i in 0..2 {
            let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
            service
                .create_task(CreateTaskRequest {
                    tenant_id: "t1".to_string(),
                    task_config: test_task_config(&format!("task-t1-{}", i), writer),
                })
                .unwrap();
        }
        // t2 创建 1 个任劀
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t2".to_string(),
                task_config: test_task_config("task-t2-0", writer),
            })
            .unwrap();
        // t1 的列表应只有 2 个任劀
        let list_t1 = service.list_tasks("t1").unwrap();
        assert_eq!(list_t1.len(), 2);
        // t2 的列表应只有 1 个任劀
        let list_t2 = service.list_tasks("t2").unwrap();
        assert_eq!(list_t2.len(), 1);
    }

    #[test]
    fn list_tasks_tenant_not_found_fails() {
        let service = test_service(test_task_manager());
        let result = service.list_tasks("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    // =================================================================
    // 12. 配额检查测诀    // =================================================================

    #[test]
    fn check_quota_ok_when_under_limit() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // Free max_tasks=3，未创建任何任务
        assert!(service.check_quota("t1").is_ok());
    }

    #[test]
    fn check_quota_fails_when_at_limit() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 创建 3 个任务达到上陀
        for i in 0..3 {
            let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
            service
                .create_task(CreateTaskRequest {
                    tenant_id: "t1".to_string(),
                    task_config: test_task_config(&format!("task-{}", i), writer),
                })
                .unwrap();
        }
        let result = service.check_quota("t1");
        assert!(matches!(
            result,
            Err(ServiceError::TenantLimitExceeded { .. })
        ));
    }

    #[test]
    fn check_quota_nonexistent_tenant_fails() {
        let service = test_service(test_task_manager());
        let result = service.check_quota("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    // =================================================================
    // 13. 使用量更新测诀    // =================================================================

    #[test]
    fn update_usage_increments_counters() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Enterprise))
            .unwrap();
        service.update_usage("t1", 100, 1024).unwrap();
        service.update_usage("t1", 50, 512).unwrap();
        let usage = service.get_usage("t1").unwrap();
        assert_eq!(usage.total_events_processed, 150);
        assert_eq!(usage.total_bytes_processed, 1536);
    }

    #[test]
    fn update_usage_nonexistent_tenant_fails() {
        let service = test_service(test_task_manager());
        let result = service.update_usage("nonexistent", 100, 1024);
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    #[test]
    fn get_usage_nonexistent_tenant_fails() {
        let service = test_service(test_task_manager());
        let result = service.get_usage("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    // =================================================================
    // 14. 使用量定期重置测诀    // =================================================================

    #[test]
    fn reset_usage_if_needed_resets_when_expired() {
        // 使用可控时间戳的 service
        let task_manager = test_task_manager();
        let time = Arc::new(AtomicU64::new(1000));
        let time_clone = time.clone();
        let service = CdcService::with_timestamp_fn(
            task_manager,
            Box::new(move || time_clone.load(Ordering::SeqCst)),
        );
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 累计一些使用量
        service.update_usage("t1", 100, 1024).unwrap();
        let usage = service.get_usage("t1").unwrap();
        assert_eq!(usage.total_events_processed, 100);
        // 推进时间超过 retention_hours (Free=24h = 86400000ms)
        time.store(1000 + 86_400_000 + 1, Ordering::SeqCst);
        service.reset_usage_if_needed("t1").unwrap();
        let usage = service.get_usage("t1").unwrap();
        assert_eq!(usage.total_events_processed, 0);
        assert_eq!(usage.total_bytes_processed, 0);
    }

    #[test]
    fn reset_usage_if_needed_noop_when_not_expired() {
        let task_manager = test_task_manager();
        let time = Arc::new(AtomicU64::new(1000));
        let time_clone = time.clone();
        let service = CdcService::with_timestamp_fn(
            task_manager,
            Box::new(move || time_clone.load(Ordering::SeqCst)),
        );
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service.update_usage("t1", 100, 1024).unwrap();
        // 推进时间但未超过 24h
        time.store(1000 + 3_600_000, Ordering::SeqCst); // 1 小时吀
        service.reset_usage_if_needed("t1").unwrap();
        let usage = service.get_usage("t1").unwrap();
        assert_eq!(usage.total_events_processed, 100);
    }

    // =================================================================
    // 15. 集群集成测试
    // =================================================================

    #[test]
    fn with_cluster_associates_cluster() {
        let task_manager = test_task_manager();
        let cluster = Arc::new(ClusterCoordinator::new(ClusterConfig::default()).unwrap());
        let service = test_service(task_manager).with_cluster(cluster);
        assert!(service.cluster().is_some());
    }

    #[test]
    fn set_cluster_associates_cluster() {
        let task_manager = test_task_manager();
        let service = test_service(task_manager);
        assert!(service.cluster().is_none());
        let cluster = Arc::new(ClusterCoordinator::new(ClusterConfig::default()).unwrap());
        service.set_cluster(cluster);
        assert!(service.cluster().is_some());
    }

    #[test]
    fn assign_task_to_cluster_success() {
        let task_manager = test_task_manager();
        let cluster = Arc::new(ClusterCoordinator::new(ClusterConfig::default()).unwrap());
        cluster
            .register_node("node-1", "10.0.0.1:8080", 10)
            .unwrap();
        let service = test_service(task_manager).with_cluster(cluster);
        let node_id = service.assign_task_to_cluster("task-1").unwrap();
        assert_eq!(node_id, "node-1");
    }

    #[test]
    fn assign_task_to_cluster_without_association_fails() {
        let service = test_service(test_task_manager());
        let result = service.assign_task_to_cluster("task-1");
        assert!(matches!(result, Err(ServiceError::ClusterNotAssociated)));
    }

    #[test]
    fn migrate_tenant_tasks_returns_migrated_ids() {
        let task_manager = test_task_manager();
        let cluster = Arc::new(ClusterCoordinator::new(ClusterConfig::default()).unwrap());
        // 注册两个节点
        cluster
            .register_node("node-1", "10.0.0.1:8080", 10)
            .unwrap();
        cluster
            .register_node("node-2", "10.0.0.2:8080", 10)
            .unwrap();
        let service = test_service(task_manager).with_cluster(cluster.clone());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Pro))
            .unwrap();
        // 在集群上分配任务
        cluster.assign_task("task-1", "").unwrap();
        cluster.assign_task("task-2", "").unwrap();
        // 手动将任务关联到租户（用亀migrate_tenant_tasks 找到任务＿
        {
            let mut tenant_tasks = service.tenant_tasks.write();
            tenant_tasks
                .entry("t1".to_string())
                .or_default()
                .insert("task-1".to_string());
            tenant_tasks
                .get_mut("t1")
                .unwrap()
                .insert("task-2".to_string());
        }
        // 迁移租户任务
        let migrated = service.migrate_tenant_tasks("t1").unwrap();
        assert_eq!(migrated.len(), 2);
    }

    #[test]
    fn migrate_tenant_tasks_without_cluster_fails() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        let result = service.migrate_tenant_tasks("t1");
        assert!(matches!(result, Err(ServiceError::ClusterNotAssociated)));
    }

    #[test]
    fn migrate_tenant_tasks_nonexistent_tenant_fails() {
        let task_manager = test_task_manager();
        let cluster = Arc::new(ClusterCoordinator::new(ClusterConfig::default()).unwrap());
        let service = test_service(task_manager).with_cluster(cluster);
        let result = service.migrate_tenant_tasks("nonexistent");
        assert!(matches!(result, Err(ServiceError::TenantNotFound(_))));
    }

    // =================================================================
    // 16. 注销租户清理任务测试
    // =================================================================

    #[test]
    fn unregister_tenant_cleans_up_tasks() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 创建 2 个任劀
        for i in 0..2 {
            let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
            service
                .create_task(CreateTaskRequest {
                    tenant_id: "t1".to_string(),
                    task_config: test_task_config(&format!("task-{}", i), writer),
                })
                .unwrap();
        }
        assert_eq!(service.list_tasks("t1").unwrap().len(), 2);
        // 注销租户
        service.unregister_tenant("t1").unwrap();
        // 任务应被清理（task_manager 不再有这些任务）
        assert!(service.task_manager.get_task("task-0").is_err());
        assert!(service.task_manager.get_task("task-1").is_err());
    }

    // =================================================================
    // 17. 并发安全测试
    // =================================================================

    #[test]
    fn concurrent_create_tasks_under_quota() {
        let task_manager = test_task_manager();
        let service = Arc::new(test_service(task_manager));
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Enterprise))
            .unwrap();
        // Enterprise max_tasks=100，并发创廀50 个任劀
        let mut handles = Vec::new();
        for i in 0..50 {
            let service_clone = service.clone();
            handles.push(thread::spawn(move || {
                let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
                let task_id = format!("task-{}", i);
                service_clone
                    .create_task(CreateTaskRequest {
                        tenant_id: "t1".to_string(),
                        task_config: test_task_config(&task_id, writer),
                    })
                    .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let list = service.list_tasks("t1").unwrap();
        assert_eq!(list.len(), 50);
    }

    #[test]
    fn concurrent_register_tenants() {
        let task_manager = test_task_manager();
        let service = Arc::new(test_service(task_manager));
        let mut handles = Vec::new();
        for i in 0..10 {
            let service_clone = service.clone();
            handles.push(thread::spawn(move || {
                let tenant_id = format!("t{}", i);
                service_clone
                    .register_tenant(TenantConfig::new(
                        &tenant_id,
                        format!("Tenant {}", i),
                        TenantTier::Free,
                    ))
                    .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let list = service.list_tenants();
        assert_eq!(list.len(), 10);
    }

    #[test]
    fn concurrent_register_duplicate_tenant_one_succeeds() {
        let task_manager = test_task_manager();
        let service = Arc::new(test_service(task_manager));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let service_clone = service.clone();
            handles.push(thread::spawn(move || {
                service_clone.register_tenant(TenantConfig::new("t1", "Tenant 1", TenantTier::Free))
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let successes = results.iter().filter(|r| r.is_ok()).count();
        let failures = results.iter().filter(|r| r.is_err()).count();
        // 只有一个成功，其他都是 TenantAlreadyExists
        assert_eq!(successes, 1);
        assert_eq!(failures, 4);
    }

    // =================================================================
    // 18. 综合集成测试
    // =================================================================

    #[test]
    fn full_tenant_lifecycle_with_tasks() {
        let service = test_service(test_task_manager());
        // 1. 注册 Free 租户
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        // 2. 创建任务（受 Free 限制，最夀3 个）
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-1", writer),
            })
            .unwrap();
        // 3. 启动任务
        service.start_task("t1", "task-1").unwrap();
        // 4. 更新使用釀
        service.update_usage("t1", 50, 1024).unwrap();
        let usage = service.get_usage("t1").unwrap();
        assert_eq!(usage.total_events_processed, 50);
        // 5. 升级刀Pro
        service.update_tenant_tier("t1", TenantTier::Pro).unwrap();
        assert_eq!(service.get_tenant("t1").unwrap().tier, TenantTier::Pro);
        // 6. 停止任务
        service.stop_task("t1", "task-1").unwrap();
        // 7. 注销租户（清理任务）
        service.unregister_tenant("t1").unwrap();
        assert!(matches!(
            service.get_tenant("t1"),
            Err(ServiceError::TenantNotFound(_))
        ));
    }

    #[test]
    fn multi_tenant_isolation() {
        let service = test_service(test_task_manager());
        // 注册两个租户
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Pro))
            .unwrap();
        // t1 创建任务
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t1".to_string(),
                task_config: test_task_config("task-t1", writer),
            })
            .unwrap();
        // t2 创建任务
        let writer2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        service
            .create_task(CreateTaskRequest {
                tenant_id: "t2".to_string(),
                task_config: test_task_config("task-t2", writer2),
            })
            .unwrap();
        // t1 不能访问 t2 的任劀
        assert!(matches!(
            service.get_task("t1", "task-t2"),
            Err(ServiceError::TaskNotFound(_))
        ));
        // t2 不能删除 t1 的任劀
        assert!(matches!(
            service.delete_task("t2", "task-t1"),
            Err(ServiceError::TaskNotFound(_))
        ));
        // 各自的任务列表独竀
        assert_eq!(service.list_tasks("t1").unwrap().len(), 1);
        assert_eq!(service.list_tasks("t2").unwrap().len(), 1);
    }

    #[test]
    fn usage_independent_per_tenant() {
        let service = test_service(test_task_manager());
        service
            .register_tenant(TenantConfig::new("t1", "T1", TenantTier::Free))
            .unwrap();
        service
            .register_tenant(TenantConfig::new("t2", "T2", TenantTier::Free))
            .unwrap();
        // t1 累计使用釀
        service.update_usage("t1", 100, 1024).unwrap();
        // t2 累计使用釀
        service.update_usage("t2", 200, 2048).unwrap();
        // 各自独立
        let usage_t1 = service.get_usage("t1").unwrap();
        let usage_t2 = service.get_usage("t2").unwrap();
        assert_eq!(usage_t1.total_events_processed, 100);
        assert_eq!(usage_t2.total_events_processed, 200);
    }
}
