# Rate Limiting 配置指南

> **模块**：`sz_rust_core::middleware::rate_limit`  
> **底层实现**：`sz-orm-limit` v1.2.1  
> **中间件链位置**：第 4 位（鉴权之前）

---

## 概述

Rate Limiting 中间件在请求到达业务逻辑之前进行频率限制，防止暴力破解、API 滥用和 DDoS 攻击。

**设计决策**：限流位于鉴权之前，避免未认证请求消耗鉴权开销。

---

## 限流算法

### SlidingWindowRateLimiter（滑动窗口）

保留窗口内所有请求的时间戳，精确计数，无边界突变问题。

```rust
use sz_orm_limit::SlidingWindowRateLimiter;

let limiter = SlidingWindowRateLimiter::new(
    100,   // max_requests: 窗口内最大请求数
    60,    // window_seconds: 窗口大小（秒）
);
```

**适用场景**：通用 API 限流，精确度要求高的场景。

### TokenBucketRateLimiter（令牌桶）

以固定速率补充令牌，允许突发流量（不超过桶容量）。

```rust
use sz_orm_limit::TokenBucketRateLimiter;

let limiter = TokenBucketRateLimiter::new(
    100,   // capacity: 桶容量（最大突发数）
    2.0,   // refill_rate: 每秒补充令牌数
);
```

**适用场景**：允许突发但长期速率受限的场景（如搜索接口）。

---

## 配置结构

```rust
use sz_rust_core::middleware::rate_limit::{RateLimitConfig, KeyExtractor};
use std::sync::Arc;

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::Ip)       // Key 提取策略
    .with_exclude_paths(vec![                   // 排除路径
        "/health".to_string(),
        "/favicon.ico".to_string(),
    ])
    .with_key_prefix("api");                    // Key 前缀
```

### Key 提取策略

| 策略 | Key 组成 | 适用场景 |
|------|---------|---------|
| `Ip`（默认） | 客户端 IP | 全局限流、防 DDoS |
| `UserId` | 已认证用户 ID | 用户级限流（需前置 Auth 中间件） |
| `IpPlusRoute` | `IP:route_path` | 路由级限流（精确到接口） |

**UserId 回退**：如果 extensions 中无 `AuthenticatedUser`（Auth 中间件未执行或白名单跳过），自动回退到 IP。

### 客户端 IP 提取优先级

```
X-Forwarded-For（取第一个） > X-Real-IP > "unknown"
```

⚠️ **安全提示**：`X-Forwarded-For` 可被客户端伪造。生产环境应通过可信反向代理（Nginx/Cloudflare）覆盖该 header。

---

## 场景配置

### 场景 1：全局限流（防滥用）

```rust
use sz_orm_limit::SlidingWindowRateLimiter;
use sz_rust_core::middleware::rate_limit::{RateLimitConfig, KeyExtractor};

let limiter = Arc::new(SlidingWindowRateLimiter::new(1000, 60)); // 1000 req/min

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::Ip)
    .with_key_prefix("global");
```

### 场景 2：登录限流（防暴力破解）

```rust
use sz_orm_limit::SlidingWindowRateLimiter;

let limiter = Arc::new(SlidingWindowRateLimiter::new(5, 300)); // 5 次/5 分钟

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::Ip)
    .with_key_prefix("login")
    .with_exclude_paths(vec![
        "/api/users/register".to_string(), // 注册不限流
    ]);
```

### 场景 3：短信发送限流

```rust
use sz_orm_limit::SlidingWindowRateLimiter;

let limiter = Arc::new(SlidingWindowRateLimiter::new(3, 3600)); // 3 次/小时

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::Ip)
    .with_key_prefix("sms");
```

### 场景 4：用户级限流

```rust
// 需前置 Auth 中间件注入 AuthenticatedUser
let limiter = Arc::new(SlidingWindowRateLimiter::new(100, 60));

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::UserId) // 按用户 ID 限流
    .with_key_prefix("user");
```

### 场景 5：路由级限流

```rust
let limiter = Arc::new(SlidingWindowRateLimiter::new(50, 60));

let config = RateLimitConfig::new(limiter)
    .with_key_extractor(KeyExtractor::IpPlusRoute) // IP + 路由
    .with_key_prefix("route");
```

---

## 响应格式

### 限流通过

响应 headers 添加：
- `X-RateLimit-Remaining: <剩余配额>`
- `X-RateLimit-Reset: <Unix 毫秒时间戳>`

### 限流拒绝（HTTP 429）

**Headers**：
- `X-RateLimit-Remaining: 0`
- `X-RateLimit-Reset: <Unix 毫秒时间戳>`
- `Retry-After: <秒数>`

**Body**（对齐 PHP `renderJson` 格式）：
```json
{
  "code": 429,
  "msg": "Too Many Requests",
  "data": {
    "retry_after_seconds": 60,
    "reset_at_ms": 1704067200000
  }
}
```

---

## 监控指标

建议在中间件中添加日志或指标上报：

```rust
// 限流命中时记录
tracing::warn!(
    key = %rate_limit_key,
    remaining = result.remaining,
    "Rate limit exceeded"
);

// Prometheus 指标（可选）
// rate_limit_hits_total{key_extractor, path}
// rate_limit_remaining{key}
```

---

## 配置示例（config/rate_limit.yaml）

```yaml
rate_limit:
  default:
    algorithm: sliding_window
    max_requests: 1000
    window_seconds: 60
    key_extractor: ip
    key_prefix: global

  login:
    algorithm: sliding_window
    max_requests: 5
    window_seconds: 300
    key_extractor: ip
    key_prefix: login

  sms:
    algorithm: sliding_window
    max_requests: 3
    window_seconds: 3600
    key_extractor: ip
    key_prefix: sms

  user:
    algorithm: token_bucket
    capacity: 100
    refill_rate: 2.0
    key_extractor: user_id
    key_prefix: user

  exclude_paths:
    - /health
    - /health/db
    - /health/cache
    - /favicon.ico
    - /api/docs
```

---

## 最佳实践

| 场景 | 推荐配置 | 说明 |
|------|---------|------|
| 全站限流 | 1000 req/min/IP | 防止全站滥用 |
| 登录接口 | 5 req/5min/IP | 防暴力破解 |
| 短信接口 | 3 req/hour/IP | 防短信轰炸 |
| 搜索接口 | TokenBucket(50, 2/s) | 允许突发 |
| 文件上传 | 10 req/min/user | 防存储滥用 |
| API 网关 | 按用户等级分级限流 | VIP 更高配额 |

---

## 故障转移策略

**Fail-open**：当限流器内部错误（如 RwLock 中毒）时，默认放行请求，避免影响业务。

```rust
// 限流器错误时记录日志并放行
match limiter.acquire(&key).await {
    Ok(result) => { /* 正常处理 */ }
    Err(e) => {
        tracing::error!("Rate limiter error: {}", e);
        // fail-open: 放行
        return Ok(next.run(req).await);
    }
}
```
