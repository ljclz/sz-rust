# 门禁阻断报告（2026-09-30，2026-10-01 更新修复状态）

- 分支 / commit：`main` @ `b44f6e26`
- 范围：`HEAD~1..HEAD`（提交 b44f6e26 "test(e2e): P2/P4 axum Router 端到端集成测试 — 接线深度补全"）
- 状态机：scanning → compile → static → security → test → integration → ai → **done（含阻塞问题）**
- 严重度阈值：medium
- 审查方式：`bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai` + 人工证据核验（`RUSTC_WRAPPER=""` 绕过 sccache 复跑关键门禁）

## 修复与复跑状态（2026-10-01 更新）

三项代码阻塞问题已全部修复并通过门禁复验（均为工作区未提交修改，尚未产生 commit SHA）：

| 门禁 | 状态 | 验证命令真实输出 |
|------|------|-----------------|
| compile（e2e_ssr_test.rs 漏 feature 门控，critical） | ✅ 已修复 | `cargo check -p sz-rust-mvc-facade --all-targets --target-dir /tmp/mvc-check-target` → EXIT=0（54.87s）；`--features ssr` → EXIT=0（2.88s） |
| sensitive-field（3 个 EXPOSED，critical） | ✅ 已修复 | `node scripts/audit/sensitive-field-audit.js` → 退出码 0，`EXPOSED：0`（SAFE_SKIP_SER 由 12 增至 15） |
| bare-unwrap（12 处生产裸 unwrap，high） | ✅ 已修复 | `python scripts/check-unwrap.py` → `AUTHORITATIVE_PROD_UNWRAP: 0` |
| 受影响 crate 编译 | ✅ 通过 | `cargo check -p sz-rust-auth-facade -p sz-rust-middleware-facade -p sz-rust-observability --all-targets --target-dir /tmp/fix-check-target` → EXIT=0（3m 20s，无 error） |
| fmt | ✅ 通过 | `cargo fmt --all --check` → EXIT=0 |

> 变更标识：上述修复位于工作区 `git status` 的 M 文件中（`packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs`、`packages/sz-rust-auth-facade/src/oauth_token_store.rs`、`packages/sz-rust-middleware-facade/src/api_signature/mod.rs`、`packages/sz-rust-middleware-facade/src/waf/owasp_ruleset.rs`、`packages/sz-rust-observability/src/drop_counter.rs`、`packages/sz-rust-observability/src/leak_detector.rs`），尚未 commit，无 commit SHA。文档未以任何形式声称"已合入"。

剩余不阻塞项：assertion-value（low）的 2 个空洞测试仍存在（工作区既有，建议后续补充断言或删除）；AI 评审因缺 key 未执行（medium）。

---

## 失败门禁 1：compile — 真实编译错误（本次提交引入，critical，✅ 已修复）

- **证据**：`packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs:9` 与 `:18`
  - `e2e_ssr_test.rs:9` `use async_trait::async_trait;` → `error[E0432]: unresolved import async_trait`
  - `e2e_ssr_test.rs:18` `use sz_rust_mvc_facade::ssr::{ComponentRenderer, RenderError, SsrConfig, SsrMiddleware};` → `error[E0432]: could not find ssr in sz_rust_mvc_facade`
- **根因**：`src/lib.rs:19` 的 `pub mod ssr` 被 `#[cfg(feature = "ssr")]` 门控，`Cargo.toml` 中 `ssr = ["async-trait"]` 且非默认 feature。同提交其他三个测试文件均有 feature 门控（`e2e_inertia_test.rs` 用 `#![cfg(feature = "inertia")]`、`e2e_waf_test.rs` 用 `#![cfg(feature = "waf")]`、`e2e_api_signature_test.rs` 用 `#![cfg(feature = "api-signature")]`），唯独 `e2e_ssr_test.rs` 漏写 `#![cfg(feature = "ssr")]`。
- **修复**：在 `e2e_ssr_test.rs` 文件头（第 5 行）补 `#![cfg(feature = "ssr")]`，与兄弟测试门控方式一致。
- **验证**：见上表（默认 features 与 `--features ssr` 均 EXIT=0）。

## 失败门禁 2：sensitive-field — 3 个 EXPOSED 敏感字段（工作区既有，critical，✅ 已修复）

- **证据**（修复前 `node scripts/audit/sensitive-field-audit.js` 输出）：
  - `packages/sz-rust-auth-facade/src/oauth_token_store.rs:194` `TokenInfo.access_token`
  - `packages/sz-rust-auth-facade/src/oauth_token_store.rs:196` `TokenInfo.refresh_token`
  - `packages/sz-rust-middleware-facade/src/api_signature/mod.rs:43` `ApiKey.secret`
- **修复**：按 AGENTS.md「敏感字段自动脱敏（`#[serde(skip_serializing)]`）」约束：
  - `TokenInfo` 增加 `#[derive(..., Serialize)]`，`access_token`/`refresh_token` 加 `#[serde(skip_serializing)]`
  - `ApiKey` 增加 `#[derive(Debug, Clone, Serialize)]`，`secret` 加 `#[serde(skip_serializing)]`
- **验证**：`node scripts/audit/sensitive-field-audit.js` → 退出码 0，`EXPOSED：0`。

## 失败门禁 3：bare-unwrap — 生产代码 12 处裸 unwrap（工作区既有，high，✅ 已修复）

- **证据**（修复前 `python scripts/check-unwrap.py`，AUTHORITATIVE_PROD_UNWRAP: 12）：
  - `sz-rust-middleware-facade/src/waf/owasp_ruleset.rs`：7 处（行 25/32/40/47/55/64/75，常量正则 `Regex::new(...).unwrap()`）
  - `sz-rust-observability/src/drop_counter.rs:94`：`self.value.take().unwrap()`
  - `sz-rust-observability/src/leak_detector.rs:137/138/167/168`：`counts.first()/counts.last().unwrap()`
- **修复**：全部改为带上下文的 `.expect("...")`（正则编译用「内置 WAF 规则正则应可编译」，take 用「TrackedResource 值已被取出过」，counts 访问用「counts.len() >= 2 已在前置守卫确认」）。
- **验证**：`python scripts/check-unwrap.py` → `AUTHORITATIVE_PROD_UNWRAP: 0`。

## 失败门禁 4：integration — 失败根因为 sccache 编译失败（high，环境性，非代码缺陷）

- **证据**（2026-09-30 复核修正）：集成门禁失败的输出中无 `panicked`/`test result: FAILED`/`error[` 匹配（INTEG_FAIL 为空），说明 `cargo test` 未运行到任何测试断言，在编译阶段即因 sccache 失败。
- **数据库环境实测可用**（修正初版"本机无 MySQL"的错误推断）：
  - MySQL 3306 端口开放，pymysql 以 root/test123 连接 `sz_orm_test` 成功，`SELECT VERSION()` → `9.6.0`
  - PostgreSQL 5432 端口开放，psycopg2 以 postgres/test123 连接 `sz_orm_test` 成功，`SELECT version()` → `PostgreSQL 18.2`
- **处置**：安装 sccache 或 `export RUSTC_WRAPPER=""` 后重跑 integration 环节即可（无需部署数据库）。

## 失败门禁 5：AI 评审 — 缺 key（medium，非代码缺陷）

- **证据**：`AI_API_KEY` 与 `CSDN_API_KEY` 均未设置。
- **处置**：设置 `AI_API_KEY`（及可选 `AI_BASE_URL`/`AI_MODEL`）后以 `--ai` 重跑；无 key 时该环节如实记录 medium，不参与阻塞判定。

## 补充：assertion-value（low，不阻塞，工作区既有，未修复）

- **证据**（`node scripts/audit/assertion-value-check.js` 退出码 1）：
  - `packages/sz-rust-sz300/tests/service_coverage_test.rs:593` `test_auth_login_empty_credentials_returns_error()`：空函数，无断言
  - `packages/sz-rust-testkit/src/tx_isolation_fixture.rs:62` `test_tx_isolation_fixture_construct()`：仅构造 PhantomData，无断言
- **建议**：补充断言或删除空洞测试（不在本次修复范围内）。

## 补充：sccache 环境导致的门禁误报

- **证据**：`.cargo/config.toml` 第 7 行 `rustc-wrapper = "sccache"`，但本机 `sccache` 未安装（`command -v sccache` → NOT_FOUND），导致 `cargo check/clippy/test` 均以 `error: could not execute process sccache rustc -vV` 失败。执行器按 critical 记录了 3 条（check/clippy/test）。经 `RUSTC_WRAPPER=""` 绕过后确认：真实编译错误仅 e2e_ssr_test.rs 一处（已修复）。本机复跑审查前建议安装 sccache 或临时 `export RUSTC_WRAPPER=""`。

## 复跑要求

1. ✅ 已修复：e2e_ssr_test.rs feature 门控（critical）
2. ✅ 已修复：3 个 EXPOSED 敏感字段（critical）
3. ✅ 已修复：12 处生产裸 unwrap（high）
4. 待处理：设置 `AI_API_KEY` 后以 `--ai` 重跑（数据库环境已实测可用，无需 `--skip-integration`）
5. 复跑命令：`RUSTC_WRAPPER="" bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai`（修复尚未提交，复跑前请先 commit 工作区修改）

## 真实性边界（审计诚实声明）

本报告各结论的证据强度分级如下：

| 结论 | 证据强度 | 说明 |
|------|---------|------|
| e2e_ssr_test.rs 编译错误存在 | **已验证**（cargo 真实输出 + 源码 file:line 核对） | `error[E0432]` x2，lib.rs:19 feature 门控，Cargo.toml `ssr = ["async-trait"]` |
| e2e_ssr 修复后默认/ssr feature 均编译通过 | **已验证**（修复后 cargo check 两次 EXIT=0） | 54.87s / 2.88s，无 error |
| 3 个 EXPOSED 敏感字段存在 | **已验证**（修复前脚本退出码 1 + 源码行核对） | oauth_token_store.rs:194/196、api_signature/mod.rs:43 |
| 敏感字段修复后 0 EXPOSED | **已验证**（修复后脚本退出码 0，EXPOSED：0） | SAFE_SKIP_SER 12→15 |
| 12 处生产裸 unwrap | **已验证**（修复前 check-unwrap.py 权威计数 + 独立枚举一致） | owasp_ruleset 7 + observability 5 |
| unwrap 修复后归零 | **已验证**（修复后 AUTHORITATIVE_PROD_UNWRAP: 0） | |
| 受影响 3 crate 编译通过 | **已验证**（`cargo check --all-targets --target-dir /tmp/fix-check-target` EXIT=0，3m20s 无 error） | |
| fmt 通过 | **已验证**（独立复跑 `cargo fmt --all --check` 退出码 0） | |
| sccache 环境故障 | **已验证**（`command -v sccache` NOT_FOUND + config.toml:7 + 执行器错误输出） | |
| 集成环境可用性 | **已验证**（pymysql/psycopg2 实连成功，MySQL 9.6.0 / PG 18.2，库存在） | 修正初版"本机无 MySQL"的错误推断 |
| clippy 真实结果 | **未验证** | 因 sccache + cargo 全局锁竞争无法运行；需在修复提交后复跑 |
| 单元测试真实结果 | **未验证** | 同上 |
| "编译错误仅 e2e_ssr_test 一处" | **高度可能但未完全验证** | 首次 check 日志被截断（tail -40，止于 "waiting for other jobs"），全量复验被并行构建的锁竞争阻塞；截断前无其他错误输出；修复后受影响 crate 针对性检查通过 |
| AI 评审内容 | **未执行** | AI_API_KEY 未设置 |