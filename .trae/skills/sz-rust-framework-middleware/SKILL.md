---
name: sz-rust-framework-middleware
description: 注入 panic、超时、上下文篡改，验证隔离性。新增中间件时触发。
tools: [tower, tokio-test]
agentMode: auto
---

# 中间件混沌测试（sz-rust framework）

## 攻击

- 中间件故意 panic，验证 `CatchPanicLayer` 返回 500 而非崩溃。
- sleep(10s)，验证全局超时层返回 408。
- 篡改 `extensions` 中的 `UserId`，确保类型安全。

## 通过标准

进程永不崩溃，连接数无泄漏。
