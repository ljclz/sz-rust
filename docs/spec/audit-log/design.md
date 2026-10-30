# 审计日志持久化（P5）技术设计

> 版本：1.0
> 日期：2026-08-08
> 关联：spec.md

## 1. 架构概览

```
┌──────────────┐    record_audit()    ┌──────────────┐
│  SsoService  │ ──────────────────→ │  AuditStore  │
│  (业务操作)  │                     │  (trait)     │
│              │                     │              │
│  login()     │                     │  MemoryAudit │
│  revoke()    │                     │  Store       │
│  degrade()   │                     │              │
│  ticket()    │                     └──────────────┘
└──────────────┘
```

## 2. 核心组件

### 2.1 AuditEventType 枚举

覆盖 13 种关键操作：Login / Logout / Revoke / RevokeAll / RevokeDevice / Degrade / ClearDegradation / TicketGenerate / TicketExchange / RefreshRotated / ReuseDetected / DeviceRegistered / DeviceEvicted

### 2.2 AuditEvent 结构体

```rust
pub struct AuditEvent {
    pub event_type: AuditEventType,
    pub user_id: Option<i64>,
    pub device_id: Option<String>,
    pub timestamp: i64,
    pub detail: Option<String>,
}
```

### 2.3 AuditStore trait

```rust
#[async_trait]
pub trait AuditStore: Send + Sync {
    async fn save(&self, event: AuditEvent) -> Result<()>;
    async fn query(&self, user_id: i64, limit: usize) -> Result<Vec<AuditEvent>>;
}
```

### 2.4 MemoryAuditStore

- 内部使用 `Arc<parking_lot::RwLock<Vec<AuditEvent>>>`
- `save` 追加事件
- `query` 按 user_id 过滤，返回最近 limit 条（按时间倒序）

### 2.5 SsoService 集成

- `audit_store: Option<Arc<dyn AuditStore>>` 字段
- `with_audit_store(store)` 链式配置方法
- `record_audit(event_type, user_id, device_id, detail)` 内部方法（best-effort，失败仅 warn）
- `query_audit(user_id, limit)` 查询 API

## 3. 自动审计点

| 操作 | 事件类型 | 记录字段 |
|------|---------|---------|
| login / login_with_device | Login | user_id, device_id |
| revoke_all | RevokeAll | user_id |
| revoke_device | RevokeDevice | user_id, device_id |
| degrade_user | Degrade | user_id |
| generate_ticket | TicketGenerate | user_id, redirect_uri |
| exchange_ticket | TicketExchange | user_id |

## 4. 错误处理

- 审计记录为 best-effort：`record_audit` 失败仅 `tracing::warn!`，不中断业务流程
- `query_audit` 在 audit_store 未配置时返回 `InvalidConfig` 错误

## 5. 代码位置

- `packages/sz-rust-auth-facade/src/refresh.rs` — AuditEvent / AuditEventType / AuditStore / MemoryAuditStore
- `packages/sz-rust-auth-facade/src/sso.rs` — SsoService::record_audit / query_audit