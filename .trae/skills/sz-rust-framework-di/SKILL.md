---
name: sz-rust-framework-di
description: 检测服务容器循环依赖和递归深度。修改 crates/framework/container 时触发。
tools: [mockall, loom]
agentMode: auto
---

# DI 循环依赖检测（sz-rust framework）

## 攻击

- A 依赖 B，B 依赖 A，验证返回 `CyclicDependency`。
- 10 层深度依赖，验证递归上限（16）返回 `Err`。

## 通过标准

无 Panic，无死锁。
