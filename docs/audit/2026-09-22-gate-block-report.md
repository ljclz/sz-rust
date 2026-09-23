# 门禁阻断报告（2026-09-22）

- 分支 / commit：`main` @ `1dc852f`
- 范围：全量 15 门禁 + AI 评审，`origin/main..HEAD`（8 个未推送提交，19 文件，+3184/−12）
- 失败门禁：**static · clippy（门禁 4，critical）** + assertion-value（门禁 10，gate ERROR ×6）
- 状态机：`scanning → compile ✓ → static(fmt ✓) → static(clippy ✗) → unwrap ✓ → security ✓ → test ✓ → integration(跳过) → ai ✓ → done`
- 事件：`ReviewBlocked`（docs/audit/events.jsonl，blocking 3：critical ×2 + medium ×1；另有 low ×3）
- 审查报告（执行器产出）：docs/audit/2026-09-22-pr-review-main.md

## 阻塞问题（≥ medium）

### 1. [critical] clippy `-D warnings` 下编译失败：unused variable `version_id`

- 证据：`packages/sz-rust-marketplace/tests/service_integration.rs:475`
  - `let version_id = service.publish(...)` 绑定后未消费（该测试为 `#[ignore]` 的 `test_service_install_not_approved_version`，469 行起）
  - 同文件 158/269/299/334/369 行的同名绑定均有消费，475 行为漏网孤绑定
- 复现输出（`cargo clippy -p sz-rust-marketplace --all-targets` 实跑）：
  `warning: unused variable: version_id --> tests\service_integration.rs:475:9`
  （`-D warnings` 下提升为 error；`cargo check` 无 `-D` 故 compile 门禁放行，属 clippy 特有拦截）
- medium 级联项「build failed, waiting for other jobs」为同一错误的并行任务噪音，修复后自动消除

### 2. [low→gate ERROR] 6 个无断言测试（铁律 10/23）

- `packages/sz-rust-cli/src/cmd/serve.rs:444` `test_build_router_with_tenant_disabled()`
- `packages/sz-rust-cli/src/cmd/serve.rs:450` `test_build_router_with_tenant_enabled()`
- `packages/sz-rust-cli/src/cmd/serve.rs:456` `test_build_router_with_data_scope_disabled()`
- `packages/sz-rust-cli/src/cmd/serve.rs:462` `test_build_router_with_data_scope_enabled()`
- `packages/sz-rust-cli/src/console.rs:357` `test_print_list_output_empty()`
- `packages/sz-rust-cli/src/console.rs:363` `test_print_list_output_with_commands()`
- 来源：`1dc852f`（test(cli): 覆盖率提升至 90.00%）——为覆盖率补测但无断言，覆盖率数字不构成回归保护，正是断言价值门禁针对的模式

## 建议修复

1. `service_integration.rs:475`：`let version_id` → `let _version_id`（publish 调用本身是测试场景必需，仅 id 未消费）
2. 6 个空洞测试补真实断言：
   - serve.rs 4 个：参照 `tests/admin_serve_wiring.rs` 的 oneshot 请求模式，断言 with/without 中间件的路由行为差异（如 tenant 中间件开启时请求获得租户上下文处理、关闭时直通）
   - console.rs 2 个：让 `print_list_output` 返回可断言的字符串/状态，或捕获输出断言内容
3. 复跑：`bash scripts/audit/pr-review.sh --range origin/main..HEAD --ai --ai-timeout 300`（同 diff AI 走缓存可忽略；修复后 diff 变化自然重评）

## AI 评审交叉核验（5 条，评分 4/10，deepseek-v4-flash）

| AI 意见 | 复核结论 |
|---|---|
| 1. [Critical] 编译错误阻塞 | ✅ 实锤（service_integration.rs:475，与门禁一致） |
| 2. [Medium] 无断言测试 | ✅ 实锤（6 处，与门禁一致） |
| 3. [Medium] 真实 PG 集成测试风险 | ⚠️ 设计取舍：真实 DB 集成是本项目既定路线（jobs_integration MySQL 先例 + 门禁 12 精神）；测试隔离建议（sqlx::test/事务回滚）值得后续评估，不阻塞 |
| 4. [Medium] cargo_checker 测试跑真实 cargo | ❌ 基本不成立：真实 cargo 测试已 `#[ignore]`（cargo_checker.rs:114，注明 CI Check job 覆盖），AI 未注意到该注解 |
| 5. [Low] sqlx 双版本风险 | ❌ 不成立：根 Cargo.toml:197 已声明 workspace 级 `sqlx = "0.9"`，cli Cargo.toml:80 的 `sqlx = { workspace = true }` 解析到同一统一版本；AI 建议的「根声明统一版本」即现状 |

采纳率 2/5（两条均为门禁独立捕获的真实问题），与 deepseek-v4-flash 历史可采纳性观察（约 2/5）一致。

## 复跑要求

修复上述 2 项后从 static 环节重跑全量（建议直接整轮 `--ai` 复核）；sz300 两条 low gate-skipped 为 1614e84 开源/企业版分离的既定记录，无需处理。
