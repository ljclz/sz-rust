# SSO 跨域单点登录（P4）技术设计

> 版本：1.0
> 日期：2026-08-08
> 关联：spec.md（FR-1.1 ~ FR-2.3，AC-1 ~ AC-4）

## 1. 架构概览

```
┌─────────────┐     redirect     ┌──────────────┐
│   域名B     │ ──────────────→ │  SSO 中心    │
│ (业务系统)  │                  │ (sz-rust)    │
│             │  ←── ticket ── │              │
│             │                  │              │
│             │  POST exchange   │              │
│             │ ──────────────→ │              │
│             │  ←── TokenPair ─ │              │
└─────────────┘                  └──────────────┘
```

## 2. 核心组件

### 2.1 SsoTicket

```rust
pub struct SsoTicket {
    pub ticket: String,          // UUID v4，32 字节
    pub user_id: i64,
    pub username: String,
    pub redirect_uri: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    pub created_at: i64,         // Unix 时间戳
    pub expires_at: i64,         // created_at + 30
}
```

### 2.2 TicketStore trait

```rust
#[async_trait]
pub trait TicketStore: Send + Sync {
    async fn save(&self, ticket: SsoTicket) -> Result<()>;
    async fn take(&self, ticket: &str) -> Result<Option<SsoTicket>>;  // 取出并删除
    async fn peek(&self, ticket: &str) -> Result<Option<SsoTicket>>;  // 仅查看
}
```

### 2.3 MemoryTicketStore

- 内部使用 `Arc<parking_lot::RwLock<HashMap<String, SsoTicket>>>`
- `take` 方法先读后删（原子操作）
- `peek` 方法仅读不删

### 2.4 SsoService 集成

- `ticket_store: Option<Arc<dyn TicketStore>>` 字段
- `with_ticket_store(store)` 链式配置方法
- `generate_ticket(user_id, redirect_uri)` — 生成 UUID v4 ticket，保存到 store，返回 ticket 字符串
- `exchange_ticket(ticket)` — take 操作（一次性），验证未过期，签发新 TokenPair
- `validate_ticket(ticket)` — peek 操作（不消费），返回 Option<SsoTicket>

## 3. 安全设计

- Ticket 为 UUID v4（128 位随机），不可猜测
- TTL 默认 30 秒，短窗口减少被截获风险
- 一次性使用：exchange 后立即删除，防止重放攻击
- Ticket 不携带敏感信息（不包含密码/token）

## 4. 错误处理

| 场景 | 错误类型 |
|------|---------|
| ticket_store 未配置 | `InvalidConfig` |
| ticket 不存在 | `TicketNotFound` |
| ticket 已过期 | `TicketExpired` |
| ticket 已使用 | `TicketNotFound`（已删除） |

## 5. 审计集成

- `generate_ticket` 记录 `AuditEventType::TicketGenerate`
- `exchange_ticket` 记录 `AuditEventType::TicketExchange`

## 6. 代码位置

- `packages/sz-rust-auth-facade/src/refresh.rs` — SsoTicket / TicketStore / MemoryTicketStore
- `packages/sz-rust-auth-facade/src/sso.rs` — SsoService::generate_ticket / exchange_ticket / validate_ticket