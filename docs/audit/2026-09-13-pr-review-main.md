# PR 审查报告（2026-09-13，branch: main，range: HEAD~2..HEAD）

> 审查时点: `HEAD @ ae27e57`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 1 medium / 5 low）

- [low] `gate` **doc-code-consistency**: ❌ 发现 129 处幻影交付声称（违反铁律 23），请核实：crate 是否应存在、是否需标注企业版
- [low] `gate` **assertion-value**: ❌ 发现 4 个无断言测试（断言价值门禁，铁律 10/23），请补充断言或删除空洞测试
- [low] `gate` **adr-code-consistency**: ❌ 发现 1 处 ADR 引用代码不存在（ADR 漂移，铁律 14），请修正 ADR 或补齐代码
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程
- [medium] `ai` **missing-key**: AI_API_KEY 未设置（或旧变量 CSDN_API_KEY），AI 评审跳过（设置后重跑 --ai）


## 补充信息

## 变更集
```
 docs/audit/doc-debt.md                             |  1 +
 ...211\247\350\241\214\346\214\207\345\215\227.md" |  6 ++++--
 .../src/services/user_service.rs                   |  2 ++
 packages/sz-rust-cli/src/cmd/admin.rs              |  5 ++++-
 .../sz-rust-examples/src/bin/multi_tenant_demo.rs  |  9 +++++----
 scripts/audit/pr-review.sh                         | 23 ++++++++++++++++++++--
 6 files changed, 37 insertions(+), 9 deletions(-)
```


## 结论
❌ **阻塞**: 1 个 ≥ medium 级别问题，禁止合入
