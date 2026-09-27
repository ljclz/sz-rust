# 变异测试债务清单

> 此文件记录存活变异体（surviving mutants），需限期补测杀死。
> 杀死率门禁：≥90%（spec 5.12.2）。
> 存活变异体需附补测计划，限期补齐。

## v1.5.0 新增模块变异测试基线

> 基线建立日期：2026-09-27
> 部分运行结果（120s 限时，77 变异体中部分完成）

| Crate | Feature | 总变异体 | 已杀死 | 存活 | 杀死率 | 状态 |
|-------|---------|---------|--------|------|--------|------|
| sz-rust-distributed-tx | dtx-parallel | 77 | - | - | - | 部分完成 |
| sz-rust-service-registry | gray-release | - | - | - | - | 待运行 |
| sz-rust-api-gateway | gateway-multidim | - | - | - | - | 待运行 |
| sz-rust-observability | leak-detect | - | - | - | - | 待运行 |

## 存活变异体清单

> 每条记录包含：变异体位置 / 变异类型 / 是否被杀死 / 存活测试名 / 补测计划

| 位置 | 变异类型 | 状态 | 补测计划 | 限期 |
|------|---------|------|---------|------|
| persistence.rs:107 | is_empty → true | 已杀死 | test_is_empty_false_when_non_empty | 已完成 |
| persistence.rs:197 | log_compensate → Ok(()) | 已杀死 | test_log_compensate_updates_state | 已完成 |
| persistence.rs:204 | log_fail → Ok(()) | 已杀死 | test_log_fail_updates_state | 已完成 |
| saga.rs:131 | with_timeout → Default | 已杀死 | test_with_timeout_sets_value | 已完成 |
| saga.rs:137 | timeout → None | 已杀死 | test_with_timeout_sets_value + test_timeout_default_is_none | 已完成 |
| saga.rs:240 | > → == | 存活 | v1.4.0 遗留，补偿重试边界等价 | 可接受 |
| saga.rs:240 | > → < | 存活 | v1.4.0 遗留，补偿重试边界等价 | 可接受 |
| saga.rs:240 | > → >= | 存活 | v1.4.0 遗留，补偿重试边界等价 | 可接受 |
| parallel_saga.rs:44 | with_dependencies → Default | 已杀死 | 已补测 test_dependency_graph_with_dependencies | 已完成 |

## 可接受存活的变异体

> 边界等价变异体（如 `<` → `<=`，hash 几乎不可能等于 threshold）可标记为可接受存活。

| 位置 | 变异类型 | 理由 |
|------|---------|------|
| - | - | - |

## 审批流程

1. CI 运行 `cargo mutants` 生成 `mutants.json` 报告
2. 存活变异体自动记录到此清单
3. 开发者分析存活原因，制定补测计划
4. 补测杀死后更新此清单状态
5. PR 增量变异门禁：新增/修改代码存活变异体阻断合并（spec 5.12.6）