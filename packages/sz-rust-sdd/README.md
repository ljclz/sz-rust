# sz-rust-sdd

> Spec-Driven Development Agent 编排引擎

## 概述

通过四阶段流水线（Spec → Design → Task → Coding）将自然语言需求转化为可编译代码，每阶段间设 HITL（Human-in-the-Loop）审查闸门。阶段顺序固定不可跳过/并行。

## 模块

| 模块 | 说明 |
|------|------|
| `session` | 会话管理（UUID v4 / 原子写 / 并发限制 8） |
| `artifact` | 产物版本化存储（版本递增不覆盖 / diff / export） |
| `trace` | 编排轨迹（JSONL 追加写 / 查询 ≤ 2s） |
| `redactor` | 脱敏组合（SourceCodeRedactor + JSON 字段脱敏） |
| `orchestrator` | 编排核心 + 状态机（10 状态）+ 事务一致性 |
| `stage` | 四阶段执行器（Spec/Design/Task/Coding）+ 校验器 |
| `review` | 审查闸门（决策校验 / 幂等）+ 通知器（重试 3 次） |
| `legacy` | 存量分析器（目录扫描 / 风格检测 / 隐式约定） |
| `rules` | 铁律校验器（5 铁律） |
| `capability` | capability 守卫（8 SDD capability + check-only） |
| `prompt` | Prompt 构建器（Tera 模板） |
| `cli` | CLI 7 子命令 |

## CLI

```bash
sdd init      --requirement <text> --target <crate-path>  # 创建会话，启动 Spec
sdd resume    --session <id>                               # 恢复中断会话
sdd review    --session <id> --decision <pass|rollback|redo>  # 提交审查决策
sdd list                                                  # 列出所有会话
sdd export    --session <id> --stage <s> --version <n> --output <path>  # 导出产物
sdd trace     --session <id>                               # 查询编排轨迹
sdd config    --provider <p> --model <m> --rules-path <path>  # 配置热更新
```

## 铁律约束

- `#![forbid(unsafe_code)]`
- 禁止 `std::fs`（统一 `tokio::fs`）
- 禁止 crate 级 `#![allow(dead_code)]`
- 所有 `async fn` 必须 `Send + 'static`
- 敏感字段脱敏（API key / 密码 / token / 私钥 / PEM / bearer / AWS secret）

## 依赖

- `sz-rust-ai-facade` 1.2.0（AI 调用 facade）
- `sz-rust-mcp` 1.2.0（MCP 工具桥接）
- `sz-rust-capability` 1.2.0（capability 注册表）
- `sz-rust-rag` 1.2.0（源码脱敏）
- `tera` 1.20（prompt 模板渲染）

## 验证

```bash
cargo test -p sz-rust-sdd                    # 157 tests（128 lib + 29 E2E）
cargo clippy -p sz-rust-sdd --all-targets    # 0 warnings
cargo fmt --check -p sz-rust-sdd             # OK
cargo build -p sz-rust-sdd --bin sdd         # bin target
```

## 许可证

Apache-2.0（开源许可）