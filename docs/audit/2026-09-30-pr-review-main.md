# PR 审查报告（2026-09-30，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ b44f6e26`（报告为时点快照；后续新提交不在本报告范围内）
> 说明: 本报告由 `pr-review.sh` 自动生成，并经 sz-rust 审查流水线人工证据核验与根因修正（见下方「人工复核修正」）。2026-10-01 更新修复状态。

## 状态机
- scanning → compile → static → static → static → static → security → test → integration → ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）
- 执行器自动问题计数: 4 critical / 2 high / 1 medium / 1 low（其中 3 条 critical 为 sccache 环境误报，经人工复核修正为 1 条真实编译错误）

## 问题清单（人工复核修正后）

### 代码问题 — 本次提交引入
- [critical] `compile` **compile-error（真实，✅ 已修复-工作区未提交）**: `packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs`
  - 根因: 漏写 `#![cfg(feature = "ssr")]`（同提交 e2e_inertia/e2e_waf/e2e_api_signature 均有门控）
  - 修复: 文件头补 `#![cfg(feature = "ssr")]`（第 5 行）
  - 验证: `cargo check -p sz-rust-mvc-facade --all-targets --target-dir /tmp/mvc-check-target` → EXIT=0（54.87s）；`--features ssr` → EXIT=0（2.88s）；`cargo fmt --all --check` → EXIT=0

### 代码问题 — 工作区既有（非本次提交引入）
- [critical] `gate` **sensitive-field（✅ 已修复-工作区未提交）**: 3 个 EXPOSED 敏感字段
  - 修复前: `oauth_token_store.rs:194`（TokenInfo.access_token）、`oauth_token_store.rs:196`（TokenInfo.refresh_token）、`api_signature/mod.rs:43`（ApiKey.secret）
  - 修复: 结构体补 `#[derive(Serialize)]` + 敏感字段加 `#[serde(skip_serializing)]`
  - 验证: `node scripts/audit/sensitive-field-audit.js` → 退出码 0，`EXPOSED：0`（SAFE_SKIP_SER 12→15）
- [high] `workspace` **bare-unwrap（✅ 已修复-工作区未提交）**: 生产代码 12 处裸 unwrap（铁律 2）
  - 修复前: `waf/owasp_ruleset.rs`: 25/32/40/47/55/64/75（7 处）、`drop_counter.rs:94`、`leak_detector.rs:137/138/167/168`（5 处）
  - 修复: 全部改为 `.expect("...")`
  - 验证: `python scripts/check-unwrap.py` → `AUTHORITATIVE_PROD_UNWRAP: 0`
- [low] `gate` **assertion-value（未修复，不阻塞）**: 2 个无断言空洞测试
  - `packages/sz-rust-sz300/tests/service_coverage_test.rs:593`
  - `packages/sz-rust-testkit/src/tx_isolation_fixture.rs:62`

### 环境性失败（非代码缺陷，但使门禁无法通过）
- [critical] `check/clippy/test` **sccache 环境**: `.cargo/config.toml:7` 配置 `rustc-wrapper = "sccache"`，本机未安装 sccache，命令以 `could not execute process sccache rustc -vV` 失败（执行器按 critical 记录 3 条；人工绕过后确认真实编译错误仅 e2e_ssr_test 一处，已修复）
- [high] `integration` **integration-failure**: 失败根因为 sccache 编译失败（INTEG_FAIL 为空、无 panicked/FAILED，测试未运行到断言阶段）。数据库环境实测可用：MySQL 9.6.0（pymysql root/test123 连 sz_orm_test 成功）、PostgreSQL 18.2（psycopg2 同凭据成功）
- [medium] `ai` **missing-key**: AI_API_KEY 未设置，AI 评审跳过

## 补充信息

## 变更集
```
 .../tests/e2e_api_signature_test.rs                | 232 +++++++++++++++++++++
 .../tests/e2e_waf_test.rs                          | 214 +++++++++++++++++++
 .../sz-rust-mvc-facade/tests/e2e_inertia_test.rs   | 200 ++++++++++++++++++
 packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs  | 143 +++++++++++++
 4 files changed, 789 insertions(+)
```

## 人工复核修正
- 执行器将 compile/clippy/test 三处失败均归因于 sccache（critical）。人工以 `RUSTC_WRAPPER="" cargo check --workspace --all-targets` 复跑后确认：真实编译错误为 `e2e_ssr_test.rs` 漏写 `#![cfg(feature = "ssr")]`；截断日志显示 13 个 crate Checking 通过且无其他错误，但全量错误清点因 cargo 全局锁竞争未复跑完成（详见阻断报告「真实性边界」）。
- 二次复核修正：初版将 integration 失败归因"本机无 MySQL"系错误推断（仅验证了 mysql 客户端缺失）；实测 MySQL 9.6.0 与 PostgreSQL 18.2 均可连接，集成失败根因是 sccache 编译失败。
- 2026-10-01 更新：三项代码阻塞问题（compile/sensitive-field/bare-unwrap）已修复并通过门禁复验，详见 `docs/audit/2026-09-30-gate-block-report.md`「修复与复跑状态」。

## 结论
⚠️ **阻塞项已修复，但审查尚未最终通过**：代码级阻塞问题（compile/sensitive-field/bare-unwrap）已修复并验证；剩余 assertion-value（low，不阻塞）与 AI 评审缺 key（medium）未解决。修复尚未提交（无 commit SHA），提交并设置 `AI_API_KEY` 后应重跑 `RUSTC_WRAPPER="" bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai` 以完成最终门禁。