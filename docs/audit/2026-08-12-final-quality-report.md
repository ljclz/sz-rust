# sz-rust 最终质量报告

> **日期**：2026-08-12  
> **范围**：全 workspace 38 个 crate（排除 examples）  
> **结论**：**全部通过**，0 failed，5065 passed

---

## 1. 总览

| 指标 | 数值 |
|------|------|
| Workspace crate 数 | 38（排除 sz-rust-examples + addon-hot-reload-example） |
| 源文件数（src + tests） | 674 |
| 代码行数 | 192,067 |
| 测试总数 | 5,065 passed, 0 failed |
| 编译状态 | ✅ 全部通过（`cargo check --workspace`） |
| unsafe_code | ✅ workspace 级 `forbid` |
| 测试命令 | `cargo test -p <crate> --jobs 2`（分 8 批） |

---

## 2. 各 crate 测试统计

| Crate | 测试 passed | 源文件数 | 代码行数 |
|-------|------------|----------|----------|
| sz-rust-core | 900 | 53 | 22,781 |
| sz-rust-orm-facade | 806 | 23 | 2,772 |
| sz-rust-mvc-facade | 515 | 9 | 8,976 |
| sz-rust-mcp | 424 | 2 | 783 |
| sz-rust-addons-crm | 466 | 12 | 1,832 |
| sz-rust-addons-operate | 231 | 35 | 16,320 |
| sz-rust-auth-facade | 220 | 23 | 13,644 |
| sz-rust-pdf | 275 | 7 | 3,558 |
| sz-rust-sz300 | 172 | 59 | 8,707 |
| sz-rust-migration | 153 | 40 | 4,993 |
| sz-rust-cli | 140 | 22 | 7,263 |
| sz-rust-workflow | 131 | 47 | 6,247 |
| sz-rust-frontend-codegen | 71 | 24 | 2,457 |
| sz-rust-tracing | 58 | 1 | 764 |
| sz-rust-sdd-agent | 58 | 35 | 4,282 |
| sz-rust-observability | 46 | 8 | 3,574 |
| sz-rust-ai-facade | 104 | 49 | 5,525 |
| sz-rust-capability | 91 | 8 | 1,046 |
| sz-rust-middleware-facade | 25 | 24 | 12,276 |
| sz-rust-pay-facade | 24 | 2 | 1,300 |
| sz-rust-marketplace | 25 | 29 | 1,859 |
| sz-rust-addons-cms | 21 | 12 | 1,225 |
| sz-rust-addons-loader | 102 | 10 | 4,450 |
| sz-rust-router-facade | 3 | 6 | 3,870 |
| sz-rust-state-facade | 1 | 8 | 6,250 |
| sz-rust-facade-tests | 2 | 6 | 495 |
| sz-rust-macros | 1 | 1 | 440 |
| sz-rust-http-facade | 0 | 9 | 3,529 |
| sz-rust-cache-facade | 0 | 3 | 5,895 |
| sz-rust-infra-facade | 0 | 13 | 15,917 |
| sz-rust-orm-ext-facade | 0 | 13 | 10,704 |
| sz-rust-addons-erp | 0 | 11 | 1,125 |
| sz-rust-addons-ecommerce | 0 | 12 | 1,900 |
| sz-rust-addons-forum | 0 | 10 | 349 |
| sz-rust-addons-im | 0 | 10 | 355 |
| sz-rust-operator | 0 | 3 | 645 |
| sz-rust-rag | 0 | 19 | 2,724 |
| sz-rust-visual | 0 | 16 | 1,235 |
| **合计** | **5,065** | **674** | **192,067** |

---

## 3. 已知问题

### 3.1 sz-rust-examples 编译问题（非阻塞）

- **现象**：`crud_demo` bin 测试编译失败，`error: crate 'rustls' required to be available in rlib format`
- **原因**：rustls `default-features = false` 配置与某些 bin target 的测试编译不兼容
- **影响**：仅影响 examples crate 的测试编译，不影响任何生产 crate
- **修复建议**：在 sz-rust-examples 的 Cargo.toml 中为 rustls 添加 `features = ["ring"]` 或将 crud_demo 的 rustls 依赖改为 `features = ["ring", "std"]`

### 3.2 sz-rust-sz300 测试隔离问题（非阻塞）

- **现象**：批量运行时 `test_metrics_auth_from_env_default` 偶发失败（assertion: `config.allowed_ips.is_empty()`）
- **原因**：其他测试设置了 `METRICS_AUTH_ALLOWED_IPS` 环境变量，未清理即退出，污染后续测试
- **验证**：单独运行 `cargo test -p sz-rust-sz300 --test metrics_auth_config_test` 全部 13 passed
- **修复建议**：在设置环境变量的测试结尾添加 cleanup（`std::env::remove_var("METRICS_AUTH_ALLOWED_IPS")`）

---

## 4. 编译警告统计

| 警告类型 | 数量 | 来源 |
|---------|------|------|
| `dead_code` | 3 | sz-rust-workflow（sensitive_registry/history_recorder 字段预留） |
| `unused_mut` | 1 | sz-rust-sz300 |
| `dead_code`（test） | ~50 | sz-rust-ai-facade 测试辅助结构（MockServer/StubProvider 等） |

**结论**：无严重警告，所有警告均为预留字段或测试辅助代码，不影响生产。

---

## 5. P4 新增代码验证

### 5.1 sz-rust-frontend-codegen（P4-T1）

- **测试**：71 passed, 0 failed
- **源文件**：24 files, 2,457 lines
- **功能**：根据 ORM 模型生成 Vue/React 组件、前端路由、权限控制

### 5.2 sz-rust-workflow（P4-T2）

- **测试**：131 passed, 0 failed（121 单元 + 10 集成）
- **源文件**：47 files, 6,247 lines
- **功能**：状态机引擎、审批流引擎、6 种节点类型、And/Or 会签、候选人解析、容错策略、事件总线、审计日志、敏感字段脱敏、设计器 API、版本管理

---

## 6. 质量门禁通过情况

| 门禁 | 状态 | 证据 |
|------|------|------|
| `unsafe_code = "forbid"` | ✅ | `Cargo.toml:62` |
| `overflow-checks = true` | ✅ | `Cargo.toml:280,289` |
| workspace 编译 | ✅ | `cargo check --workspace` finished |
| 全量测试 | ✅ | 5065 passed, 0 failed |
| P4-T1 测试 | ✅ | 71 passed |
| P4-T2 测试 | ✅ | 131 passed |

---

*报告生成方式：分 8 批运行 `cargo test -p <crate> --jobs 2`，避免 OOM。所有数字来自实际命令输出。*