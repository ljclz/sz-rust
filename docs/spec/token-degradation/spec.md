# Token 降级机制（P3）需求规格

> 版本：1.0  
> 日期：2026-08-08  
> 关联：sso-refresh-token（v0.6.2）、multi-device-session（P1）

## 1. 背景

### 1.1 问题陈述

当前 SSO 机制支持 token 撤销（revoke）和版本号失效（revoke_all），但缺少**权限降级**能力：

- **权限变更场景**：用户角色从 admin 降为 user 时，已签发的 access_token 中仍携带 `roles=["admin"]`，在 token 过期前（默认 30 分钟）持续拥有管理员权限。`revoke_all` 可强制失效但要求客户端重新登录，体验差。
- **风险检测场景**：检测到异地登录、异常高频请求等可疑行为时，安全系统希望立即降低该用户 token 的权限级别（如降为只读），而非直接撤销（可能误判正常用户）。

### 1.2 目标

提供 **Token 权限降级** 机制，在不撤销 token 的前提下，动态降低其携带的权限级别。

### 1.3 范围

- ✅ 用户级权限降级（degrade_user）：设置降级后的 roles/permissions
- ✅ 设备级权限降级（degrade_device）：仅降级特定设备的权限
- ✅ validate 时自动应用降级：返回降级后的 claims
- ✅ 清除降级恢复正常权限
- ✅ 降级 TTL 自动过期
- ❌ 不修改 token 本身（不重新签发）
- ❌ 不做风险检测逻辑（由外部系统调用降级 API）

## 2. 功能需求

### 2.1 用户级降级

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-1.1 | `degrade_user(user_id, degraded_roles, degraded_permissions, ttl_secs)` 设置用户全局降级 | P0 |
| FR-1.2 | `validate` 时若用户存在降级映射，用降级后的 roles/permissions 覆盖 token 中的值 | P0 |
| FR-1.3 | `clear_degradation(user_id)` 清除用户降级，恢复正常权限 | P0 |
| FR-1.4 | 降级映射支持 TTL，到期自动清除 | P1 |
| FR-1.5 | `get_degradation(user_id)` 查询当前降级状态 | P1 |

### 2.2 设备级降级

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-2.1 | `degrade_device(user_id, device_id, degraded_roles, degraded_permissions, ttl_secs)` 仅降级特定设备 | P1 |
| FR-2.2 | `validate` 时若 token 携带 device_id 且该设备存在降级映射，优先使用设备级降级 | P1 |
| FR-2.3 | `clear_device_degradation(user_id, device_id)` 清除设备降级 | P1 |

### 2.3 降级策略

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-3.1 | 降级后的 roles 是 token 中 roles 的**子集**（不能提权） | P0 |
| FR-3.2 | 降级后的 permissions 是 token 中 permissions 的**子集**（不能提权） | P0 |
| FR-3.3 | 若降级映射中的 role 不在 token 原有 roles 中，忽略该 role | P1 |

### 2.4 与现有机制联动

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-4.1 | `revoke_all` 同时清除该用户的降级映射 | P0 |
| FR-4.2 | `revoke_device` 同时清除该设备的降级映射 | P1 |
| FR-4.3 | 降级不影响 token 的有效性（签名、过期、版本号校验仍正常通过） | P0 |

## 3. 非功能需求

| ID | 需求 | 指标 |
|----|------|------|
| NFR-1.1 | 降级查询延迟 p99 < 1ms（Memory）/ < 3ms（Redis） | 性能基线 |
| NFR-1.2 | validate 路径增加降级查询后，p99 延迟增量 < 0.5ms | 性能基线 |
| NFR-2.1 | 降级映射存储线程安全（Send + Sync） | 并发安全 |
| NFR-3.1 | 降级 API 失败不阻断 validate（best-effort，失败仅 warn） | 容错 |

## 4. 验收标准

### AC-1: 用户级降级生效
```
Given 用户已登录，access_token 携带 roles=["admin","user"], permissions=["read","write","delete"]
When 调用 degrade_user(user_id, ["user"], ["read"], 3600)
Then validate(access_token) 返回的 claims.roles == ["user"]
  And claims.permissions == ["read"]
  And token 本身仍然有效（签名、过期校验通过）
```

### AC-2: 清除降级恢复
```
Given 用户已降级，validate 返回 roles=["user"]
When 调用 clear_degradation(user_id)
Then validate(access_token) 返回的 claims.roles == ["admin","user"]（原始值）
```

### AC-3: 降级不能提权
```
Given token 携带 roles=["user"]
When 调用 degrade_user(user_id, ["admin"], [], 3600)
Then validate 返回的 claims.roles == []（admin 不在原有 roles 中，被忽略）
```

### AC-4: 降级 TTL 过期
```
Given 用户已降级，TTL=2秒
When 等待3秒后 validate(access_token)
Then 返回原始 roles/permissions（降级已过期清除）
```

### AC-5: revoke_all 清除降级
```
Given 用户已降级
When 调用 revoke_all(user_id)
Then get_degradation(user_id) 返回 None
```

### AC-6: 设备级降级优先
```
Given 用户级降级 roles=["user"]，设备级降级 roles=["guest"]
When validate 携带 device_id 的 token
Then 返回 claims.roles == ["guest"]（设备级优先）
```

### AC-7: 降级不影响 token 有效性
```
Given 用户已降级
When validate(access_token)
Then 不返回 Revoked / VersionMismatch / Expired 错误
  And 返回的 claims 其余字段（sub, exp, iss, ver, jti）不变
```

### AC-8: 降级查询失败不阻断 validate
```
Given 降级存储查询失败
When validate(access_token)
Then 返回原始 claims（降级未生效），日志输出 warn
```

## 5. 数据模型

### 5.1 DegradationEntry

```rust
pub struct DegradationEntry {
    /// 降级后的角色列表
    pub roles: Vec<String>,
    /// 降级后的权限列表
    pub permissions: Vec<String>,
    /// 降级过期时间（Unix 时间戳）
    pub expires_at: i64,
}
```

### 5.2 DegradationStore trait

```rust
#[async_trait]
pub trait DegradationStore: Send + Sync {
    /// 设置用户级降级
    async fn set_user_degradation(&self, user_id: i64, entry: DegradationEntry) -> Result<()>;
    /// 获取用户级降级（已过期返回 None）
    async fn get_user_degradation(&self, user_id: i64) -> Result<Option<DegradationEntry>>;
    /// 清除用户级降级
    async fn clear_user_degradation(&self, user_id: i64) -> Result<()>;
    /// 设置设备级降级
    async fn set_device_degradation(&self, user_id: i64, device_id: &str, entry: DegradationEntry) -> Result<()>;
    /// 获取设备级降级
    async fn get_device_degradation(&self, user_id: i64, device_id: &str) -> Result<Option<DegradationEntry>>;
    /// 清除设备级降级
    async fn clear_device_degradation(&self, user_id: i64, device_id: &str) -> Result<()>;
    /// 清除用户所有降级（含设备级）
    async fn clear_all_degradations(&self, user_id: i64) -> Result<()>;
}
```

## 6. API 概览

### 6.1 SsoService 新增方法

```rust
// 用户级降级
pub async fn degrade_user(&self, user_id: i64, roles: Vec<String>, permissions: Vec<String>, ttl_secs: u64) -> Result<()>;
pub async fn clear_degradation(&self, user_id: i64) -> Result<()>;
pub async fn get_degradation(&self, user_id: i64) -> Result<Option<DegradationEntry>>;

// 设备级降级
pub async fn degrade_device(&self, user_id: i64, device_id: &str, roles: Vec<String>, permissions: Vec<String>, ttl_secs: u64) -> Result<()>;
pub async fn clear_device_degradation(&self, user_id: i64, device_id: &str) -> Result<()>;
```

### 6.2 validate 行为变更

`validate` 和 `validate_with_renewal` 在返回 claims 前，检查降级映射并应用：
1. 若 token 携带 device_id，先查设备级降级
2. 若无设备级降级，查用户级降级
3. 应用降级：`claims.roles = degradation.roles ∩ claims.roles`（子集过滤）
4. 降级查询失败 → best-effort，返回原始 claims + warn 日志