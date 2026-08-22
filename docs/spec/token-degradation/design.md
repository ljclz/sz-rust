# Token 降级机制（P3）技术设计文档

> 版本：1.0  
> 日期：2026-08-08  
> 关联：spec.md

## 1. 架构概览

### 1.1 组件关系

```
SsoService
├── issuer: RefreshTokenIssuer（签发）
├── verifier: RefreshTokenVerifier（校验）
├── revoker: RefreshTokenRevoker（撤销）
├── device_store: Option<Arc<dyn DeviceSessionStore>>（P1）
├── degradation_store: Option<Arc<dyn DegradationStore>>（P3 新增）
└── user_auth: Arc<dyn UserAuthService>
```

### 1.2 降级流程

```
validate(access_token)
  │
  ├─ verifier.verify_access(token) → claims
  │
  ├─ apply_degradation(&mut claims)  ← P3 新增
  │   ├─ 若 claims.device_id 存在 → 查设备级降级
  │   ├─ 若无设备级 → 查用户级降级
  │   └─ 用降级后的 roles/permissions 覆盖（子集过滤）
  │
  ├─ best_effort_update_device_active(&claims)
  │
  └─ Ok(claims)
```

### 1.3 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 降级存储方式 | 外部 trait（DegradationStore） | 与黑名单/版本号一致，支持 Memory/Redis |
| 降级应用时机 | validate 返回前 | 不修改 token 本身，客户端无感知 |
| 降级方向 | 只能降级不能提权 | 安全铁律：降级后的 roles/permissions 必须是原集的子集 |
| 降级查询失败 | best-effort（返回原始 claims） | 不阻断正常请求 |
| 设备级 vs 用户级 | 设备级优先 | 更细粒度的控制 |

## 2. 数据模型

### 2.1 DegradationEntry

```rust
/// 降级条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DegradationEntry {
    /// 降级后的角色列表（必须是 token 原有 roles 的子集）
    pub roles: Vec<String>,
    /// 降级后的权限列表（必须是 token 原有 permissions 的子集）
    pub permissions: Vec<String>,
    /// 降级过期时间（Unix 时间戳）
    pub expires_at: i64,
}
```

### 2.2 DegradationStore trait

```rust
#[async_trait::async_trait]
pub trait DegradationStore: Send + Sync {
    /// 设置用户级降级
    async fn set_user_degradation(&self, user_id: i64, entry: DegradationEntry) -> Result<(), RefreshTokenError>;
    /// 获取用户级降级（已过期返回 None）
    async fn get_user_degradation(&self, user_id: i64) -> Result<Option<DegradationEntry>, RefreshTokenError>;
    /// 清除用户级降级
    async fn clear_user_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError>;
    /// 设置设备级降级
    async fn set_device_degradation(&self, user_id: i64, device_id: &str, entry: DegradationEntry) -> Result<(), RefreshTokenError>;
    /// 获取设备级降级
    async fn get_device_degradation(&self, user_id: i64, device_id: &str) -> Result<Option<DegradationEntry>, RefreshTokenError>;
    /// 清除设备级降级
    async fn clear_device_degradation(&self, user_id: i64, device_id: &str) -> Result<(), RefreshTokenError>;
    /// 清除用户所有降级（含设备级）
    async fn clear_all_degradations(&self, user_id: i64) -> Result<(), RefreshTokenError>;
}
```

### 2.3 MemoryDegradationStore

```rust
pub struct MemoryDegradationStore {
    // 用户级：user_id → DegradationEntry
    user_entries: Arc<RwLock<HashMap<i64, DegradationEntry>>>,
    // 设备级：(user_id, device_id) → DegradationEntry
    device_entries: Arc<RwLock<HashMap<(i64, String), DegradationEntry>>>,
}
```

- `get_user_degradation` 检查 `expires_at > now`，过期返回 None 并惰性清除
- `get_device_degradation` 同理

## 3. SsoService 扩展

### 3.1 新增字段

```rust
pub struct SsoService {
    // ... 现有字段 ...
    degradation_store: Option<Arc<dyn DegradationStore>>,
}
```

### 3.2 配置方法

```rust
pub fn with_degradation_store(&mut self, store: Arc<dyn DegradationStore>) -> &mut Self {
    self.degradation_store = Some(store);
    self
}
```

### 3.3 降级 API

```rust
/// 用户级降级
pub async fn degrade_user(
    &self,
    user_id: i64,
    roles: Vec<String>,
    permissions: Vec<String>,
    ttl_secs: u64,
) -> Result<(), RefreshTokenError> {
    let store = self.degradation_store.as_ref()
        .ok_or_else(|| RefreshTokenError::InvalidConfig("degradation store not configured".into()))?;
    let entry = DegradationEntry {
        roles,
        permissions,
        expires_at: chrono::Utc::now().timestamp() + ttl_secs as i64,
    };
    store.set_user_degradation(user_id, entry).await?;
    tracing::info!(user_id, ttl_secs, "user degraded");
    Ok(())
}

/// 清除用户降级
pub async fn clear_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError> {
    let store = self.degradation_store.as_ref()
        .ok_or_else(|| RefreshTokenError::InvalidConfig("degradation store not configured".into()))?;
    store.clear_all_degradations(user_id).await?;
    tracing::info!(user_id, "user degradation cleared");
    Ok(())
}

/// 查询降级状态
pub async fn get_degradation(&self, user_id: i64) -> Result<Option<DegradationEntry>, RefreshTokenError> {
    let store = self.degradation_store.as_ref()
        .ok_or_else(|| RefreshTokenError::InvalidConfig("degradation store not configured".into()))?;
    store.get_user_degradation(user_id).await
}

/// 设备级降级
pub async fn degrade_device(
    &self,
    user_id: i64,
    device_id: &str,
    roles: Vec<String>,
    permissions: Vec<String>,
    ttl_secs: u64,
) -> Result<(), RefreshTokenError> {
    let store = self.degradation_store.as_ref()
        .ok_or_else(|| RefreshTokenError::InvalidConfig("degradation store not configured".into()))?;
    let entry = DegradationEntry {
        roles,
        permissions,
        expires_at: chrono::Utc::now().timestamp() + ttl_secs as i64,
    };
    store.set_device_degradation(user_id, device_id, entry).await?;
    tracing::info!(user_id, device_id, ttl_secs, "device degraded");
    Ok(())
}

/// 清除设备降级
pub async fn clear_device_degradation(
    &self,
    user_id: i64,
    device_id: &str,
) -> Result<(), RefreshTokenError> {
    let store = self.degradation_store.as_ref()
        .ok_or_else(|| RefreshTokenError::InvalidConfig("degradation store not configured".into()))?;
    store.clear_device_degradation(user_id, device_id).await?;
    tracing::info!(user_id, device_id, "device degradation cleared");
    Ok(())
}
```

### 3.4 apply_degradation 内部方法

```rust
/// 应用降级到 claims（best-effort，失败不阻断）
async fn apply_degradation(&self, claims: &mut SsoClaims) {
    let Some(ref store) = self.degradation_store else { return };
    let Some(user_id) = claims.user_id else { return };

    // 优先查设备级降级
    let entry = if let Some(ref device_id) = claims.device_id {
        match store.get_device_degradation(user_id, device_id).await {
            Ok(Some(e)) => Some(e),
            Ok(None) => match store.get_user_degradation(user_id).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to query user degradation");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "failed to query device degradation");
                None
            }
        }
    } else {
        match store.get_user_degradation(user_id).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "failed to query user degradation");
                None
            }
        }
    };

    if let Some(entry) = entry {
        // 子集过滤：只保留 token 原有 roles 中存在于降级 roles 的部分
        claims.roles.retain(|r| entry.roles.contains(r));
        claims.permissions.retain(|p| entry.permissions.contains(p));
    }
}
```

### 3.5 validate 变更

```rust
pub async fn validate(&self, access_token: &str) -> Result<SsoClaims, RefreshTokenError> {
    let mut claims = self.verifier.verify_access(access_token).await?;
    self.apply_degradation(&mut claims).await;  // P3 新增
    self.best_effort_update_device_active(&claims).await;
    Ok(claims)
}

pub async fn validate_with_renewal(&self, access_token: &str) -> Result<(SsoClaims, Option<RenewedToken>), RefreshTokenError> {
    let mut claims = self.verifier.verify_access(access_token).await?;
    // ... 续期逻辑 ...
    self.apply_degradation(&mut claims).await;  // P3 新增
    self.best_effort_update_device_active(&claims).await;
    Ok((claims, renewed))
}
```

### 3.6 revoke_all 联动

```rust
pub async fn revoke_all(&self, user_id: i64) -> Result<(), RefreshTokenError> {
    self.revoker.revoke_all(user_id).await?;

    // 清除设备会话
    if let Some(ref store) = self.device_store {
        // ... 现有逻辑 ...
    }

    // P3 新增：清除降级映射
    if let Some(ref store) = self.degradation_store {
        if let Err(e) = store.clear_all_degradations(user_id).await {
            tracing::warn!(error = %e, "failed to clear degradations on revoke_all");
        }
    }

    Ok(())
}
```

### 3.7 revoke_device 联动

```rust
pub async fn revoke_device(&self, user_id: i64, device_id: &str) -> Result<(), RefreshTokenError> {
    // ... 现有逻辑 ...

    // P3 新增：清除设备级降级
    if let Some(ref store) = self.degradation_store {
        if let Err(e) = store.clear_device_degradation(user_id, device_id).await {
            tracing::warn!(error = %e, "failed to clear device degradation on revoke_device");
        }
    }

    Ok(())
}
```

## 4. axum 端点扩展

### 4.1 新增路由

| 方法 | 路径 | 功能 |
|------|------|------|
| POST | /sso/degrade/user | 用户级降级 |
| POST | /sso/degrade/device | 设备级降级 |
| DELETE | /sso/degrade/user/:user_id | 清除用户降级 |
| DELETE | /sso/degrade/device/:user_id/:device_id | 清除设备降级 |
| GET | /sso/degrade/:user_id | 查询降级状态 |

### 4.2 请求/响应

```rust
// POST /sso/degrade/user
#[derive(Deserialize)]
struct DegradeUserRequest {
    user_id: i64,
    roles: Vec<String>,
    permissions: Vec<String>,
    ttl_secs: u64,
}

// POST /sso/degrade/device
#[derive(Deserialize)]
struct DegradeDeviceRequest {
    user_id: i64,
    device_id: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    ttl_secs: u64,
}
```

## 5. 任务分解

| 任务 | 内容 | 优先级 |
|------|------|--------|
| T0 | DegradationEntry 结构体 | P0 |
| T1 | DegradationStore trait | P0 |
| T2 | MemoryDegradationStore 实现 | P0 |
| T3 | SsoService 新增 degradation_store 字段 + with_degradation_store | P0 |
| T4 | degrade_user / clear_degradation / get_degradation API | P0 |
| T5 | degrade_device / clear_device_degradation API | P1 |
| T6 | apply_degradation 内部方法 | P0 |
| T7 | validate / validate_with_renewal 集成降级 | P0 |
| T8 | revoke_all 联动清除降级 | P0 |
| T9 | revoke_device 联动清除设备降级 | P1 |
| T10 | 模块导出 | P0 |
| T11 | axum 端点扩展 | P1 |
| T12 | 单元测试（degradation_store + apply_degradation） | P0 |
| T13 | 单元测试（SsoService 降级 API） | P0 |
| T14 | 集成测试 | P0 |
| T15 | 全量门禁（workspace test + clippy + sz-pay） | P0 |

## 6. 风险分析

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 降级查询拖慢 validate | 低 | p99 +0.5ms | Memory HashMap O(1)；Redis 可加本地缓存 |
| 降级后权限不足导致业务异常 | 中 | 用户看到 403 | 降级是预期行为，业务层应处理权限不足 |
| 降级映射泄漏（未清除） | 低 | 用户权限永久降低 | TTL 自动过期 + revoke_all 联动清除 |
| 并发降级与 validate 竞争 | 低 | 短暂不一致 | RwLock 保证读写安全 |