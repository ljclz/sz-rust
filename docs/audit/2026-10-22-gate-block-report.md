# 门禁阻断报告（2026-10-22）

- 审查时点分支 / commit：`main` @ `dc02ac42`（`/sz-rust-review --ai` 全量审查）
- 最新 HEAD（本报告落盘时）：`815fd6dd`
- 审查范围：`HEAD~1..HEAD`（`packages/sz-rust-core/src/h2.rs`，12 行变更）
- 执行器：`bash scripts/audit/pr-review.sh --range HEAD~1..HEAD --ai --ai-timeout 300`
- 状态机：`scanning → compile → static → security → test → integration → ai → done`
- 原始报告：`docs/audit/2026-10-22-pr-review-main.md`（已由 `84cb6512` 记录）
- 事件落盘：`docs/audit/events.jsonl` → `ReviewBlocked`（commit dc02ac42，blocking 3）

## 问题清单（原始审查时点 dc02ac42：1 critical / 1 high / 1 medium / 1 low）

### 1. [critical] test-failure：sz-rust-sz300 --lib 1 个测试失败 —— ✅ 已被 `917fda95` 修复
- 失败用例：`openapi::tests::test_spec_total_endpoint_count`
- 证据：`packages/sz-rust-sz300/src/openapi.rs:483`（旧断言 33，实际 36）
- 复现输出：`assertion left == right failed: 端点总数应为 33，实际: 36`（left: 36, right: 33）
- 修复：并行提交 `917fda95` 将断言 33→36（新增插件市场 3 端点）
- 最新验证：`cargo test -p sz-rust-sz300 --lib` → **48 passed; 0 failed** ✅

### 2. [high] bare-unwrap：生产代码 6 处裸 unwrap（铁律 2）—— ❌ 当前 HEAD 仍存在
- 证据：`packages/sz-rust-alert-engine/src/silence.rs:52,64,77,90,96,102`
- 均为 `std::sync::RwLock` 的 `read()/write()` 后直接 `.unwrap()`
- 复现输出：`python scripts/check-unwrap.py` → `AUTHORITATIVE_PROD_UNWRAP: 6`（sz-rust-alert-engine: 6）
- 建议：改为带上下文的 `expect("...")`（如 `expect("silences 锁被污染")`），或按铁律 2 用 `?` 传播错误

### 3. [medium] fmt：`cargo fmt --all --check` 报 `文件名或扩展名太长 (os error 206)` —— ❌ 当前 HEAD 仍存在（门禁脚本不一致）
- 根因（已定位）：`cargo fmt --all` 在 Windows 上把本地 path 依赖（含 sz-orm 全家桶 16 包）的所有源文件路径一次性拼成 rustfmt 命令行参数；sz-orm workspace 源文件 1729 个，估算命令行约 **98,669 字符**，超过 Windows `CreateProcess` 32,767 上限 → 返回 `ERROR_FILENAME_EXCED_RANGE`（os error 206）。
- 复现证据：`cargo fmt --all --check` 在 sz-rust 与 sz-orm 仓库根均报 206；`cargo fmt -p <单个 workspace 成员> --check` 全部通过。
- **关键发现**：`.githooks/pre-commit` 已修复此问题（第 23-37 行，逐 workspace member 执行 fmt，注释明确说明原因），但 `scripts/audit/pr-review.sh:205-207` **仍使用 `cargo fmt --all --check`**，导致全量审查每次误报 fmt medium。
- 建议：将 `pr-review.sh` 的 fmt 环节改为与 `.githooks/pre-commit` 相同的逐包逻辑（`cargo metadata --no-deps` 取成员列表，循环 `cargo fmt -p "$pkg" -- --check`）。

### 4. [low] assertion-value：1 个无断言测试（铁律 10/23）—— ❌ 当前 HEAD 仍存在
- 证据：`packages/sz-rust-sz300/tests/v19_integration.rs:21` `test_integration_graphql_schema_db_buildable()` 无任何断言（仅 `let _ = format!("{:p}", &schema);`）
- 复现输出：`node scripts/audit/assertion-value-check.js` → `❌ 发现 1 个无断言测试`
- 建议：补充断言（如断言 schema 构建成功）或删除该空洞测试

## 结论

- 审查时点 `dc02ac42`：❌ **阻塞**（3 个 ≥ medium 问题，禁止合入）
- 最新 HEAD `815fd6dd`：❌ **仍阻塞**（剩余 1 high + 1 medium + 1 low；critical 已由 `917fda95` 修复）
- 最新验证：`cargo check --workspace --all-targets` ✅ exit 0；`cargo test -p sz-rust-sz300 --lib` ✅ 48 passed；unwrap 6 处 ❌；fmt 206 ❌；空洞测试 ❌