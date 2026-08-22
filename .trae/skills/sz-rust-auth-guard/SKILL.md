---
name: sz-rust-auth-guard
description: 鉴权守卫检查 — 确保所有需要登录的接口都有正确的中间件保护。修改 controller/router 时触发。
tools: [cargo-clippy, grep]
agentMode: auto
---

# 鉴权守卫检查（sz-rust）

## 触发条件

- 新增路由或 controller
- 修改路由注册代码
- 修改中间件配置

## 检查步骤

1. 列出所有新注册的路由
2. 确认非公开路由有 `auth_middleware` 保护
3. 检查公开白名单（`PUBLIC_PATHS`）无过度放行

## 通过标准

- 所有 `/api/` 业务接口有鉴权中间件
- 公开接口仅在 `PUBLIC_PATHS` 精确列表中
- 无 `path.starts_with()` 前缀绕过漏洞
- 管理员接口有额外的 role 检查

## 安全边界

```rust
// ❌ 危险：前缀匹配绕过
if path.starts_with("/api/auth/") { skip_auth() }

// ✅ 正确：精确匹配
if PUBLIC_PATHS.contains(&path) { skip_auth() }
```
