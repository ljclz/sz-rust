# PR 审查报告（2026-10-01，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ 6abd9ab7`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 1 low）

- [low] `gate` **assertion-value**:   [ERROR] packages/sz-rust-sz300/tests/service_coverage_test.rs:593 测试 test_auth_login_empty_credentials_returns_err


## 补充信息

## 变更集
```
 docs/audit/2026-09-30-gate-block-report.md         | 107 +++++++++++++++++++++
 docs/audit/2026-09-30-pr-review-main.md            |  54 +++++++++++
 docs/audit/events.jsonl                            |   1 +
 .../sz-rust-auth-facade/src/oauth_token_store.rs   |   5 +-
 .../src/api_signature/mod.rs                       |   4 +-
 .../src/waf/owasp_ruleset.rs                       |  17 ++--
 packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs  |   1 +
 packages/sz-rust-observability/src/drop_counter.rs |   2 +-
 .../sz-rust-observability/src/leak_detector.rs     |   8 +-
 9 files changed, 185 insertions(+), 14 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审：sz-rust 门禁阻断报告

### 总体评价
本次 PR 仅新增门禁阻断报告文档，未包含代码变更。文档内容详实，但暴露了若干流程与代码质量问题。结合静态问题清单，以下列出 3-5 个最重要潜在问题及修改建议。

---

### 1. 空洞测试未修复，测试有效性不足（可维护性 / 测试质量）

**问题**
`service_coverage_test.rs:593` 的 `test_auth_login_empty_credentials_returns_error` 为空函数，无任何断言。该测试无法验证“空凭据返回错误”的行为，属于无效测试，长期维护中易产生虚假安全感。

**修改建议**
补充实际断言，或删除该测试。若函数逻辑已由其他测试覆盖，应删除；否则补充断言，例如：

```rust
// packages/sz-rust-sz300/tests/service_coverage_test.rs
#[tokio::test]
async fn test_auth_login_empty_credentials_returns_error() {
    let result = auth_login("", "").await;
    assert!(result.is_err(), "空凭据应返回错误");
    assert!(matches!(result.unwrap_err(), AuthError::InvalidCredentials));
}
```

若无法确定具体 API，至少应使用 `assert!(true)` 占位并标注 TODO，但更推荐直接删除或补全。

---

### 2. 文档声称修复但代码未提交，存在版本漂移风险（可维护性 / 流程）

**问题**
报告中的三项阻塞修复（feature 门控、敏感字段、bare-unwrap）均位于工作区未提交修改，未产生 commit SHA。文档与代码仓库状态不一致，后续合入时可能遗漏修复，或导致文档描述与实际代码不符。

**修改建议**
将修复代码与文档一同提交，或在文档中明确标注“修复未提交，需在合入前完成 commit”。示例：

```bash
git add packages/sz-rust-mvc-facade/tests/e2e_ssr_test.rs \
        packages/sz-rust-auth-facade/src/oauth_token_store.rs \
        packages/sz-rust-middleware-facade/src/api_signature/mod.rs \
        packages/sz-rust-middleware-facade/src/waf/owasp_ruleset.rs \
        packages/sz-rust-observability/src/drop_counter.rs \
        packages/sz-rust-observability/src/leak_detector.rs
git commit -m "fix: 修复门禁阻塞问题（feature 门控、敏感字段脱敏、unwrap 替换）"
```

---

### 3. 敏感字段脱敏可能破坏序列化契约（安全 / 兼容性）

**问题**
`TokenInfo` 和 `ApiKey` 增加 `#[serde(skip_serializing)]` 后，所有序列化场景（如 JSON 响应、日志）都会跳过敏感字段。若某些内部接口或调试工具依赖这些字段，将导致功能异常或难以排查问题。

**修改建议**
使用 DTO（Data Transfer Object）模式，仅在对外响应中脱敏，内部序列化保留完整字段。示例：

```rust
#[derive(Serialize)]
pub struct TokenInfoDto {
    pub access_token: Option<String>,  // 仅在授权响应中填充
    pub refresh_token: Option<String>,
    // ... 其他字段
}

impl TokenInfo {
    pub fn to_public_dto(&self) -> TokenInfoDto {
        TokenInfoDto {
            access_token: None,  // 脱敏
            refresh_token: None,
            // ...
        }
    }
}
```

若必须使用 `skip_serializing`，应确保所有消费方已适配，并在文档中注明破坏性变更。

---

### 4. bare-unwrap 修复仅替换为 expect，未根本消除 panic 风险（可靠性 / 健壮性）

**问题**
将 `unwrap()` 改为 `.expect("...")` 只是提供了错误上下文，panic 行为未变。对于 `owasp_ruleset.rs` 中的正则编译，若规则配置错误，程序仍会在启动时崩溃；`drop_counter.rs` 和 `leak_detector.rs` 中的 `unwrap` 也依赖前置条件，一旦条件不满足将直接 panic。

**修改建议**
对于可恢复错误，使用 `Result` 和 `?` 传播；对于不可恢复错误，确保前置条件绝对成立，或使用 `if let` / `match` 优雅处理。示例：

```rust
// owasp_ruleset.rs
let regex = Regex::new(pattern).map_err(|e| {
    ConfigError::InvalidRegex { pattern: pattern.to_string(), source: e }
})?;

// drop_counter.rs
if let Some(value) = self.value.take() {
    // 正常处理
} else {
    // 记录错误日志并返回默认值，而非 panic
    tracing::error!("TrackedResource value already taken");
    return None;
}
```

---

### 5. 门禁环境依赖 sccache 导致误报，影响 CI 稳定性（可维护性 / 流程）

**问题**
`.cargo/config.toml` 配置了 `rustc-wrapper = "sccache"`，但本机未安装 sccache，导致 `cargo check/clippy/test` 全部失败，产生 3 条 critical 误报。这降低了门禁的可信度，并浪费排查时间。

**修改建议**
在 CI 中显式安装 sccache，或允许通过环境变量覆盖配置。示例：

```toml
# .cargo/config.toml
[build]
rustc-wrapper = "sccache"  # 可被 RUSTC_WRAPPER="" 覆盖
```

CI 脚本中增加：

```bash
if ! command -v sccache &> /dev/null; then
    echo "sccache not found, disabling wrapper"
    export RUSTC_WRAPPER=""
fi
```

---

### 整体评分

**6 / 10**

- 文档质量高，证据充分，但修复未提交、空洞测试未处理、unwrap 修复不彻底，且存在环境配置隐患。
- 建议在合入前完成代码修复提交，并补充测试断言，否则门禁报告仅能作为记录，不能代表代码已达标。


## 结论
✅ 通过（无 ≥ medium 级别问题）
