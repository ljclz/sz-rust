---
name: sz-rust-framework-routing
description: 检测路由歧义（静态 vs 动态、尾部斜杠、正则约束遗漏）。修改 crates/framework/router 时触发。
tools: [cargo-mutants, reqwest]
agentMode: auto
---

# 路由变异测试（sz-rust framework）

## 攻击

- `/user/:id` -> `/user/:id/`（尾部斜杠）。
- 删除 `(\d+)` 约束。
- 交换 `/user/me` 和 `/user/:id` 顺序。

## 通过标准

歧义时返回明确错误（非随机匹配）。
