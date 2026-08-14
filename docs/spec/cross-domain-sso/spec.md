# SSO 跨域单点登录（P4）需求规格

> 版本：1.0  
> 日期：2026-08-08  
> 关联：sso-refresh-token（v0.6.2）

## 1. 背景

### 1.1 问题陈述

当前 SSO 机制仅在单个域名内工作。用户在域名A登录后，访问域名B需要重新登录。跨域 SSO 允许用户在一个域名登录后，在其他域名自动登录。

### 1.2 方案

采用**重定向 + 一次性 Ticket** 模式（类似 CAS / OAuth2 授权码流程）：

```
用户 → 域名B（未登录）
域名B → 重定向到 SSO 中心（带 redirect_uri）
SSO 中心 → 检测已登录 → 生成一次性 ticket
SSO 中心 → 重定向回域名B（带 ticket）
域名B → POST SSO 中心 /ticket/exchange（用 ticket 换 token）
SSO 中心 → 返回 TokenPair，ticket 失效
域名B → 建立本地会话
```

### 1.3 范围

- ✅ 一次性 Ticket 生成与验证
- ✅ Ticket 换取 TokenPair
- ✅ Ticket TTL 自动过期
- ✅ Ticket 一次性使用（验证后立即失效）
- ✅ axum 端点（ticket 生成 + 交换）
- ❌ 不做前端重定向逻辑（由业务系统实现）
- ❌ 不做 CORS 配置（由部署层处理）

## 2. 功能需求

| ID | 需求 | 优先级 |
|----|------|--------|
| FR-1.1 | `generate_ticket(user_id, redirect_uri)` 生成一次性 ticket | P0 |
| FR-1.2 | `exchange_ticket(ticket) → TokenPair` 验证 ticket 并返回 token | P0 |
| FR-1.3 | Ticket 一次性使用：验证后立即失效 | P0 |
| FR-1.4 | Ticket TTL 自动过期（默认 30 秒） | P0 |
| FR-1.5 | `validate_ticket(ticket)` 仅验证不交换（返回 user_id） | P1 |
| FR-2.1 | axum POST /sso/ticket/generate 生成 ticket | P0 |
| FR-2.2 | axum POST /sso/ticket/exchange 用 ticket 换 token | P0 |
| FR-2.3 | axum GET /sso/ticket/validate 验证 ticket | P1 |

## 3. 非功能需求

| ID | 需求 | 指标 |
|----|------|------|
| NFR-1.1 | Ticket 生成延迟 p99 < 1ms（Memory） | 性能 |
| NFR-1.2 | Ticket 交换延迟 p99 < 2ms（Memory） | 性能 |
| NFR-2.1 | Ticket 长度 32 字节（UUID v4），不可猜测 | 安全 |
| NFR-2.2 | Ticket 存储线程安全（Send + Sync） | 并发 |

## 4. 验收标准

### AC-1: 生成 ticket
```
Given 用户已登录（user_id=1）
When 调用 generate_ticket(1, "https://b.example.com/callback")
Then 返回 ticket 字符串（非空，UUID 格式）
```

### AC-2: 交换 ticket
```
Given 已生成 ticket 绑定 user_id=1
When 调用 exchange_ticket(ticket)
Then 返回 TokenPair（access_token + refresh_token 有效）
  And ticket 已失效（再次 exchange 返回错误）
```

### AC-3: Ticket TTL 过期
```
Given ticket TTL=2秒
When 等待3秒后 exchange_ticket(ticket)
Then 返回 TicketExpired 错误
```

### AC-4: 一次性使用
```
Given ticket 已成功 exchange
When 再次 exchange_ticket(ticket)
Then 返回 TicketNotFound 错误
```

## 5. 数据模型

```rust
pub struct SsoTicket {
    pub ticket: String,
    pub user_id: i64,
    pub username: String,
    pub redirect_uri: String,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[async_trait]
pub trait TicketStore: Send + Sync {
    async fn save(&self, ticket: SsoTicket) -> Result<()>;
    async fn take(&self, ticket: &str) -> Result<Option<SsoTicket>>;  // 取出并删除
    async fn peek(&self, ticket: &str) -> Result<Option<SsoTicket>>;  // 仅查看
}
```

## 6. API 概览

```rust
// SsoService 新增
pub async fn generate_ticket(&self, user_id: i64, redirect_uri: &str) -> Result<String>;
pub async fn exchange_ticket(&self, ticket: &str) -> Result<TokenPair>;
pub async fn validate_ticket(&self, ticket: &str) -> Result<Option<SsoTicket>>;
```