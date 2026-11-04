# PR 审查报告（2026-08-22，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ 2fdc456`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 CHANGELOG.md | 7 +++++++
 Cargo.lock   | 3 +++
 2 files changed, 10 insertions(+)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）（缓存命中 f255c4312917dbbb）



## PR 评审报告：sz-rust

### 1. 关键问题清单

| # | 严重性 | 类别 | 问题描述 |
|---|--------|------|----------|
| 1 | 🔴 Critical | 编译错误 | `sz-rust-addons-crm` 的 `coverage_test` 编译失败：`Router<S>` 类型上调用了不存在的 `.expect()` 方法 |
| 2 | 🔴 Critical | 编译错误 | 同上，clippy 检查也触发编译失败，阻塞整个 crate 的测试 |
| 3 | 🟡 Medium | 格式 | `audit_remediation_v3_high_risk_test.rs:134` 存在 `cargo fmt` 未通过的差异 |
| 4 | 🟡 Medium | 依赖管理 | Cargo.lock 新增 `sz-rust-addons-cms` / `sz-rust-addons-crm` / `sz-rust-addons-loader` 依赖，需确认版本约束一致性 |
| 5 | 🟢 Low | 文档 | CHANGELOG 中 T3 workflow 状态为运行时快照（mutants 运行中），合并后可能过时 |

### 2. 详细分析与修改建议

#### 问题 1 & 2：`Router` 类型误用导致编译失败

**根因分析：**
`axum::routing::Router<S>` 是一个已构建完成的路由对象，它**不实现 `Future` / `Result` / `Option`**，因此没有 `.expect()` 方法。这个错误通常出现在以下场景：

```rust
// ❌ 错误写法：Router 没有 expect 方法
let router = create_router().expect("router should build");

// ❌ 错误写法：在测试中试图 await Router
let response = router.call(request).await.expect("request failed");
```

**修复方向：**

```rust
// ✅ Router 构建通常不会失败，直接赋值
let router = create_router();

// ✅ 如果是测试 HTTP 响应，应对 Response 调用 expect/status 检查
use axum::http::StatusCode;
use tower::ServiceExt; // for `oneshot` and `call`

let response = router
    .oneshot(request)
    .await
    .expect("request should succeed");

assert_eq!(response.status(), StatusCode::OK);
```

**具体步骤：**
1. 定位 `sz-rust-addons-crm` crate 中的 `tests/coverage_test.rs`（或类似路径）
2. 搜索 `.expect(` 调用链，找到作用在 `Router` 上的位置
3. 移除对 `Router` 的 `.expect()`，改为对 `Response` 或 `Result` 操作

#### 问题 3：格式不合格

**修复命令：**
```bash
cargo fmt --package sz-rust-sz300
```

**手动修复位置：**
`packages/sz-rust-sz300/tests/audit_remediation_v3_high_risk_test.rs:134`

通常是因为括号闭合 `);` 前有多余空格或缩进不一致。

#### 问题 4：新增依赖需确认

Cargo.lock diff 显示 `sz-rust-sz300` 新增了对 `sz-rust-addons-cms` 和 `sz-rust-addons-crm` 的依赖。建议确认：

```toml
# 在 sz-rust-sz300/Cargo.toml 中检查版本约束是否使用 workspace 统一版本
[dependencies]
sz-rust-addons-crm = { path = "../sz-rust-addons-crm", version = "1.2.0" }
sz-rust-addons-cms = { path = "../sz-rust-addons-cms", version = "1.2.0" }
```

确保没有硬编码版本号，而是使用 `workspace.dependencies` 统一管理。

### 3. 整体评分

**评分：4 / 10**

| 维度 | 得分 | 说明 |
|------|------|------|
| 编译通过 | 2/10 | 存在 critical 编译错误，CI 必然失败 |
| 代码质量 | 6/10 | CHANGELOG 详细，但格式未通过 |
| 安全性 | 8/10 | 未发现明显安全漏洞 |
| 可维护性 | 7/10 | 依赖关系清晰，但需确认版本管理 |

**合并建议：🚫 阻塞合并**

必须先修复 `coverage_test` 的编译错误并通过 `cargo fmt --check` 和 `cargo clippy --all-targets` 后才能合并。


## 结论
✅ 通过（无 ≥ medium 级别问题）
