# PR 审查报告（2026-08-16，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ bff78e4`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 CHANGELOG.md                                       | 13 +++++
 docs/audit/doc-debt.md                             |  2 +-
 packages/sz-rust-capability/src/builtin.rs         |  6 +-
 packages/sz-rust-capability/src/registry.rs        |  6 +-
 packages/sz-rust-core/src/api_version.rs           | 67 +++++++++++++++-------
 packages/sz-rust-core/src/container/tests.rs       |  5 +-
 packages/sz-rust-core/src/mem_pool.rs              |  6 +-
 .../sz-rust-examples/src/bin/bench_axum_native.rs  |  8 ++-
 packages/sz-rust-examples/src/bin/bench_sz_rust.rs |  8 ++-
 packages/sz-rust-examples/src/bin/blog_demo.rs     | 17 ++++--
 .../sz-rust-examples/src/bin/ecommerce_demo.rs     | 27 ++++++---
 packages/sz-rust-examples/src/bin/iot_demo.rs      | 31 ++++++----
 packages/sz-rust-mcp/src/lib.rs                    | 41 +++++++++----
 .../src/handler_as_middleware.rs                   | 61 +++++++++++++++-----
 .../src/ip_access_control.rs                       |  8 ++-
 .../sz-rust-sz300/src/middleware/role_guard.rs     | 13 +++--
 scripts/perf-compare/benchmarks/actix/src/main.rs  |  4 +-
 scripts/perf-compare/benchmarks/axum/src/main.rs   |  8 +--
 scripts/perf-compare/benchmarks/poem/src/main.rs   |  4 +-
 .../perf-compare/benchmarks/sz-rust/src/main.rs    |  8 +--
 20 files changed, 237 insertions(+), 106 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）



<!DOCTYPE markdown>
# Code Review: sz-rust unwrap 专项清偿

## 1. 总体评价

本次 PR 是一次典型的**技术债务清偿（Tech Debt Paydown）**，目标明确且执行彻底。将 51 处裸 `unwrap()` 替换为语义更明确的 `expect()` 或针对锁中毒的恢复逻辑，显著提升了生产代码的健壮性和可观测性。附带修复的测试断言漂移（8802→3306）也体现了对细节的关注。

**评分：8/10**

- **加分项**：验证充分（1597 tests passed, 0 clippy warnings），文档债务闭环（DB-2026-08-16-04 RESOLVED），锁中毒处理方案具有高级 Rust 特征。
- **扣分项**：锁中毒的“静默恢复”策略在所有场景下是否均安全存疑；并发测试的错误定位信息粒度不足；Diff 截断导致无法审查 perf-compare 的具体变更。

---

## 2. 关键问题与修改建议

### 🔴 P0: 锁中毒恢复策略的潜在风险 (Safety/Correctness)

**问题描述**：
PR 将 13 处 `lock().unwrap()` 统一替换为 `unwrap_or_else(|e| e.into_inner())`。
虽然这避免了 panic，但 `Mutex::into_inner()` 会**忽略中毒状态**直接返回内部数据。如果锁中毒是因为 Panic 发生在“数据更新到一半”的时刻（例如先修改了字段 A，在修改字段 B 时 Panic），此时数据结构处于**不一致（Invariant Broken）**状态。静默读取并继续使用这些脏数据，可能导致后续业务逻辑出现极难排查的 Bug。

**修改建议**：
建议区分**关键业务状态锁**和**普通缓存/计数器锁**。
- 对于普通缓存：当前方案可接受。
- 对于关键状态：建议记录错误日志后，视情况决定是否 Panic 或返回 Error，而不是直接 `into_inner()`。

```rust
// 建议修改示例：增加日志记录，便于排查中毒原因
match lock_result {
    Ok(guard) => guard,
    Err(poisoned) => {
        // 记录中毒事件，保留现场信息
        tracing::error!("Mutex poisoned in {}, recovering lock...", module_path!());
        poisoned.into_inner()
    }
}
```

### 🟡 P1: 并发测试错误定位信息不足 (Maintainability)

**问题描述**：
在 `registry.rs` 的并发测试中，`h.await.unwrap()` 被替换为 `h.await.expect("并发注册任务应成功")`。
当 50 个并发任务中有 1 个失败（Panic 或 Cancel）时，通用的错误消息无法帮助开发者快速定位是哪一个迭代（Iteration）或哪一个任务上下文出了问题。

**修改建议**：
在 `expect` 消息中包含任务索引或唯一标识。

```rust
// 修改前
for h in handles {
    h.await.expect("并发注册任务应成功");
}

// 建议修改后
for (i, h) in handles.into_iter().enumerate() {
    h.await.expect(&format!("并发注册任务 #{} 执行失败", i));
}
```

### 🟡 P1: HTTP 响应体解析的测试健壮性 (Robustness)

**问题描述**：
在 `api_version.rs` 测试中，`response.into_body().collect().await` 的结果被 `expect("响应体读取失败")` 处理。
虽然这在测试中是合理的（失败即报错），但 `Hyper` 的 `collect()` 错误可能包含连接重置、超时等具体网络原因。如果测试环境不稳定（Flaky Test），直接 Panic 会掩盖网络层的不稳定性。

**修改建议**：
保持现状即可，但在 CI 配置中应确保测试超时时间充足。如果此处经常报错，建议改为断言具体的错误类型，而不是直接 Expect。当前 PR 的改法符合测试惯例，**无需强制修改**，仅作提示。

### 🟢 P2: 文档与代码的一致性 (Process)

**问题描述**：
`doc-debt.md` 更新及时，但 `CHANGELOG.md` 中提到的 `perf-compare` 基准测试的 11 处修改在提供的 Diff 中不可见（被截断）。

**修改建议**：
请确认 `perf-compare` 中的修改仅仅是 `unwrap` -> `expect`，没有伴随逻辑变更。基准测试代码通常对 Panic 敏感，确保 `expect` 的消息能准确反映基准测试失败的原因（如 "Redis connection failed" vs "Startup timeout"）。

---

## 3. 总结

这是一次高质量的维护性 PR。它展示了团队对代码质量的严格要求（铁律 2）和执行力。

- **通过条件**：确认 `mem_pool` 和 `core` 中的锁中毒恢复逻辑不会掩盖数据竞争或逻辑错误（即确认那些锁保护的数据在 Panic 后仍然是安全的或可丢弃的）。
- **后续行动**：建议在 Codebase 中建立 `SafeMutex` 或封装通用的锁处理宏，避免未来重复编写 `unwrap_or_else(|e| e.into_inner())` 这种样板代码，同时统一日志记录行为。


## 结论
✅ 通过（无 ≥ medium 级别问题）
