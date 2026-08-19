# PR 审查报告（2026-08-19，branch: main，range: HEAD~3..HEAD）

> 审查时点: `HEAD @ 2d704ff`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 1 low）

- [low] `gate` **assertion-value**:   [WARN] packages/sz-rust-cache-facade/src/lib.rs:4775 测试 test_tag_clear_empty_no_error() 无断言宏但有 1 处 u


## 补充信息

## 变更集
```
 Cargo.lock                                         | 535 ++++++++++++++++++++-
 Cargo.toml                                         |  10 +-
 README.en.md                                       |   4 +
 README.md                                          |   4 +
 docs/audit/2026-08-19-pr-review-main.md            | 176 +++++++
 docs/audit/events.jsonl                            |   2 +
 packages/sz-rust-http-facade/src/graphql.rs        |   1 -
 packages/sz-rust-infra-facade/src/upload.rs        |   4 +-
 packages/sz-rust-infra-facade/src/upload/image.rs  |   2 +-
 packages/sz-rust-k8s-operator/Cargo.toml           |  28 ++
 packages/sz-rust-k8s-operator/src/crd.rs           | 133 +++++
 packages/sz-rust-k8s-operator/src/lib.rs           |  22 +
 packages/sz-rust-k8s-operator/src/reconcile.rs     | 288 +++++++++++
 packages/sz-rust-sz300/Cargo.toml                  |   5 +-
 .../sz-rust-sz300/src/controllers/graphql_api.rs   | 153 ++++++
 packages/sz-rust-sz300/src/controllers/mod.rs      |   4 +
 packages/sz-rust-sz300/src/controllers/wasm_api.rs | 133 +++++
 packages/sz-rust-sz300/src/router.rs               |  24 +-
 packages/sz-rust-wasm/Cargo.toml                   |  25 +
 packages/sz-rust-wasm/src/lib.rs                   | 250 ++++++++++
 20 files changed, 1778 insertions(+), 25 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）



## Code Review: sz-rust PR

### 关键问题

#### 1. 依赖爆炸与编译性能风险 🔴
`Cargo.lock` 中新增了大量重型依赖（`k8s-openapi`, `json-patch`, `jsonpath-rust`, `hyper-http-proxy` 等），但没有看到对应的 `Cargo.toml` 变更说明。`k8s-openapi` 以编译慢著称，会显著增加 CI 时间和二进制体积。

**建议**：
- 审查每个新增依赖是否必要，能否用更轻量的替代方案
- 考虑使用 `cargo deny` 或 `cargo audit` 管理依赖许可和安全
- 如果可能，将 K8s 相关功能放在独立的 feature flag 下

#### 2. 多版本依赖冲突风险 🟡
`base64` 同时存在 `0.21.7` 和 `0.22.1`，`getrandom` 同时存在 `0.2.17` 和 `0.3.4`。多版本共存会导致：
- 二进制体积增大
- 潜在的 ABI 不兼容问题
- `getrandom 0.3` 有重大 API 变更，需确认所有消费者已适配

**建议**：
```toml
# Cargo.toml - 统一版本约束
[dependencies]
base64 = "0.22"  # 明确指定，避免多版本
getrandom = "0.3"  # 如需升级则全面升级
```

运行 `cargo tree -d` 检查重复依赖并尝试统一。

#### 3. 测试质量缺陷 🟡
已发现问题：`test_tag_clear_empty_no_error()` 缺少断言但有 `use` 语句，这是一个空测试（silent pass）。

**建议**：
```rust
// 修改前（有问题）
#[test]
fn test_tag_clear_empty_no_error() {
    use some_module::some_func;  // 无断言
    // 测试逻辑缺失
}

// 修改后
#[test]
fn test_tag_clear_empty_no_error() {
    let cache = Cache::new();
    // 明确断言：空缓存执行 clear 不应 panic 且状态不变
    cache.clear().expect("clear on empty should not error");
    assert!(cache.is_empty());
}
```

#### 4. 安全边界扩展未审查 🟡
引入 `hyper-http-proxy` 和 `k8s-openapi` 意味着项目现在涉及：
- HTTP 代理通信（需审查代理认证、TLS 终止）
- K8s API 交互（需审查 RBAC、ServiceAccount 权限）
- `rustls-native-certs` 新增依赖需确认证书验证逻辑

**建议**：
- 审查所有新增依赖的安全公告（`cargo audit`）
- 确认 TLS 证书验证未被禁用
- 检查 K8s client 配置是否有适当的超时和重试策略

#### 5. 锁文件变更缺乏上下文 🟡
纯 `Cargo.lock` 变更的 PR 难以审查，无法判断哪些是有意升级、哪些是传递依赖的副作用。

**建议**：
- PR 描述中应说明：升级原因、影响的 crate、是否解决了特定 issue
- 如果是有意的依赖升级，应同时提交 `Cargo.toml` 的变更

---

### 整体评分: 4/10

| 维度 | 评分 | 说明 |
|------|------|------|
| 安全性 | 5 | 依赖扩展需进一步安全审查 |
| 性能 | 4 | 依赖膨胀风险高 |
| 可维护性 | 4 | 多版本依赖、缺少文档 |
| 测试质量 | 3 | 存在空测试 |

**结论**：此 PR 需要补充变更说明，审查新增依赖的必要性，并修复测试质量问题后再合并。


## 结论
✅ 通过（无 ≥ medium 级别问题）
