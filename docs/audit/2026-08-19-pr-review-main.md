# PR 审查报告（2026-08-19，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ de8c658`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 2 high / 0 medium / 1 low）

- [high] `workspace` **bare-unwrap**: 生产代码 1 处裸 unwrap（铁律 2）
- [high] `gate` **std-fs**: ❌ 铁律 4 违反：生产代码使用 std::fs，统一改为 tokio::fs
- [low] `gate` **assertion-value**:   [WARN] packages/sz-rust-cache-facade/src/lib.rs:4775 测试 test_tag_clear_empty_no_error() 无断言宏但有 1 处 u


## 补充信息

## 变更集
```
 Cargo.lock                                         | 535 ++++++++++++++++++++-
 Cargo.toml                                         |  10 +-
 README.en.md                                       |   4 +
 README.md                                          |   4 +
 packages/sz-rust-http-facade/src/graphql.rs        |   1 -
 packages/sz-rust-k8s-operator/Cargo.toml           |  28 ++
 packages/sz-rust-k8s-operator/src/crd.rs           | 133 +++++
 packages/sz-rust-k8s-operator/src/lib.rs           |  22 +
 packages/sz-rust-k8s-operator/src/reconcile.rs     | 285 +++++++++++
 packages/sz-rust-sz300/Cargo.toml                  |   5 +-
 .../sz-rust-sz300/src/controllers/graphql_api.rs   | 153 ++++++
 packages/sz-rust-sz300/src/controllers/mod.rs      |   4 +
 packages/sz-rust-sz300/src/controllers/wasm_api.rs | 133 +++++
 packages/sz-rust-sz300/src/router.rs               |  24 +-
 packages/sz-rust-wasm/Cargo.toml                   |  25 +
 packages/sz-rust-wasm/src/lib.rs                   | 250 ++++++++++
 16 files changed, 1594 insertions(+), 22 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）



## Code Review: sz-rust PR

### 🔴 关键问题清单

#### 1. [高危] 生产代码使用 `std::fs` 阻塞异步运行时 (铁律 4 违反)
**位置**: 未在当前 diff 直接体现，但静态扫描已捕获
**风险**: 在 Tokio 异步运行时中调用 `std::fs` 会阻塞 worker 线程，导致整个异步任务调度停滞，高并发下性能急剧下降甚至死锁。
**现状**: `packages/sz-rust-cache-facade/src/lib.rs` 中存在 `std::fs` 调用。
**建议**: 替换为 `tokio::fs` 异步 API。

```rust
// ❌ 错误：阻塞调用
use std::fs;
let contents = fs::read_to_string(path)?;

// ✅ 正确：异步调用
use tokio::fs;
let contents = fs::read_to_string(path).await?;
```

#### 2. [高危] 生产代码裸 `unwrap()` (铁律 2 违反)
**位置**: 生产代码逻辑中
**风险**: `unwrap()` 在 `None` 或 `Err` 时会触发 panic，导致当前任务崩溃。在生产环境中，应优雅处理错误而非直接崩溃。
**建议**: 使用 `?` 操作符向上传播错误，或使用 `unwrap_or`/`match` 提供降级逻辑。

```rust
// ❌ 错误：可能 panic
let value = config.get("key").unwrap();

// ✅ 正确：传播错误
let value = config.get("key").ok_or_else(|| Error::ConfigMissing("key"))?;

// ✅ 正确：安全降级
let value = config.get("key").cloned().unwrap_or_default();
```

#### 3. [中危] 依赖树膨胀与版本冲突
**位置**: `Cargo.lock`
**风险**: 本次变更引入了大量新依赖（`k8s-openapi`, `hyper-http-proxy`, `json-patch` 等），且出现了 `base64` (0.21.7 vs 0.22.1)、`getrandom` (0.2 vs 0.3)、`thiserror` 的多版本共存。
**影响**:
- 编译时间显著增加（尤其是 `k8s-openapi` 这种大型 crate）。
- 二进制体积膨胀。
- 潜在的依赖冲突维护成本。
**建议**:
- 检查是否真的需要引入 Kubernetes 支持，如果仅需部分功能，考虑更轻量的 crate。
- 统一 `base64` 等常用库的版本，避免多版本链接。

#### 4. [低危] 测试断言缺失
**位置**: `test_tag_clear_empty_no_error()`
**风险**: 测试函数没有断言宏（`assert!` 等），仅依赖不 panic 作为通过标准。这导致测试无法验证业务逻辑的正确性，容易漏测回归。
**建议**: 补充明确的断言逻辑。

```rust
#[tokio::test]
async fn test_tag_clear_empty_no_error() {
    let result = cache.clear_tags(&[]).await;

    // ❌ 仅依赖不 panic
    // assert!(result.is_ok());

    // ✅ 明确验证状态
    assert!(result.is_ok());
    let inner = result.unwrap();
    assert_eq!(inner.affected_count, 0);
}
```

---

### 📊 整体评分: 4/10

**评分理由**:
- **安全性 (-3)**: 违反铁律 2 和铁律 4，存在 panic 风险和阻塞异步运行时的严重隐患，这是生产环境的大忌。
- **可维护性 (-2)**: `Cargo.lock` 引入了大量重型依赖且存在版本冲突，增加了构建复杂度和长期维护负担。
- **测试质量 (-1)**: 关键逻辑测试缺乏断言，无法保证功能正确性。

**修复优先级**:
1. 立即修复 `std::fs` -> `tokio::fs` 的替换（阻塞问题是性能杀手）。
2. 消除所有生产代码的裸 `unwrap`。
3. 审查新引入依赖的必要性，裁剪依赖树。
4. 补充测试断言。


## 结论
❌ **阻塞**: 2 个 ≥ medium 级别问题，禁止合入
