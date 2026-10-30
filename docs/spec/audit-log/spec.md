# 审计日志持久化（P5）需求规格

> 版本：1.0
> 日期：2026-08-08
> 关联：sso-refresh-token（v0.6.2）

## 1. 背景

### 1.1 问题陈述

SSO 中心的关键操作（登录、撤销、降级、ticket 生成等）需要审计日志记录，用于安全合规、故障排查和行为分析。

### 1.2 范围

- ✅ 审计事件结构化记录（AuditEvent）
- ✅ 审计存储抽象（AuditStore trait）
- ✅ 内存存储实现（MemoryAuditStore）
- ✅ 关键操作自动记录审计
- ✅ 审计日志查询 API
- ❌ 不做日志聚合 / ELK 集成（由部署层处理）
- ❌ 不做日志告警 / SIEM 集成

## 2. 功能需求

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-1.1 | `AuditEvent` 结构体包含事件类型、用户ID、设备ID、时间戳、详情 | P0 |
| FR-1.2 | `AuditEventType` 枚举覆盖所有关键操作 | P0 |
| FR-1.3 | `AuditStore` trait 定义 save / query 方法 | P0 |
| FR-1.4 | `MemoryAuditStore` 内存实现 | P0 |
| FR-2.1 | login 操作自动记录审计 | P0 |
| FR-2.2 | revoke_all 操作自动记录审计 | P0 |
| FR-2.3 | degrade 操作自动记录审计 | P0 |
| FR-2.4 | ticket_generate / ticket_exchange 自动记录审计 | P0 |
| FR-3.1 | `query_audit(user_id, limit)` 查询用户审计事件 | P1 |

## 3. 数据模型

```rust
pub enum AuditEventType {
    Login,
    Logout,
    Revoke,
    RevokeAll,
    RevokeDevice,
    Degrade,
    ClearDegradation,
    TicketGenerate,
    TicketExchange,
    RefreshRotated,
    ReuseDetected,
    DeviceRegistered,
    DeviceEvicted,
}

pub struct AuditEvent {
    pub event_type: AuditEventType,
    pub user_id: Option<i64>,
    pub device_id: Option<String>,
    pub timestamp: i64,
    pub detail: Option<String>,
}

#[async_trait]
pub trait AuditStore: Send + Sync {
    async fn save(&self, event: AuditEvent) -> Result<()>;
    async fn query(&self, user_id: i64, limit: usize) -> Result<Vec<AuditEvent>>;
}
```

## 4. API 概览

```rust
// SsoService 新增
pub async fn query_audit(&self, user_id: i64, limit: usize) -> Result<Vec<AuditEvent>>;
```