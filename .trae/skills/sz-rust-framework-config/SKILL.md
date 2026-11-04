---
name: sz-rust-framework-config
description: 强制脱敏敏感字段，检测静态文件目录遍历。修改 config 或 static 服务时触发。
tools: [serde, regex]
agentMode: manual
---

# 配置审计（sz-rust framework）

## 强制脱敏

扫描所有 `struct`，若字段含 `password`/`secret` 且未加 `#[serde(skip_serializing)]`，AI 自动补全。

## 目录遍历

拦截 `..` 路径，返回 403。
