# ADR-027: SDD Agent 四阶段编排

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-sdd-agent/src/orchestrator.rs`, `packages/sz-rust-sdd-agent/src/phase.rs`, `packages/sz-rust-sdd-agent/src/hitl.rs`

## 背景

P0-2 缺口：规格驱动开发（SDD）需要自动化编排 Spec→Design→Task→Coding 四阶段，支持 HITL 闸门和编译修复循环。

## 决策

1. **PhaseAgent trait**：每个阶段实现 `async fn execute(&self, input: PhaseInput) -> Result<PhaseOutput>`
2. **阶段状态机**：Spec→Design→Task→Coding 严格顺序，每阶段产出传递给下一阶段
3. **session.jsonl 持久化**：每个阶段完成后追加 JSONL 记录，支持断点续跑
4. **HITL 闸门**：`AwaitingHitl` 事件暂停编排，等待用户确认/修改/补充/中止
5. **Compile-Fix 循环**：Coding 阶段编译失败后自动修复，最多重试 N 次

## 替代方案

- **DAG 编排**：灵活但复杂，SDD 四阶段是线性流程不需要 DAG
- **Actor 模型**：消息传递开销大，阶段间数据传递不直观

## Bug 定位提示

- `orchestrator.rs:89` — `event_buses` HashMap 管理，注意锁粒度
- `phase.rs:63` — `event_bus` Arc 共享，确保不跨 await 持锁
- `hitl.rs` — HITL 等待逻辑，超时 30 分钟保存状态
- `coding_agent.rs:156` — Compile-Fix 循环入口，检查 max_retries

## 影响

- RAG 知识库集成到 Design/Coding 阶段（降级安全）
- 16 个 Skill 映射到 CodeChangeSummary 字段
- 铁律检查器（IronLawChecker）在 Design 阶段标注触发