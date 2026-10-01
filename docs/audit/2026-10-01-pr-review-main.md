# PR 审查报告（2026-10-01，branch: main，range: 6abd9ab7^..6abd9ab7）

> 审查时点: `HEAD @ a6e007c0`（报告为时点快照；后续新提交不在本报告范围内）
> 人工注记（2026-10-01 复核）：本报告 AI 评审使用云知声 Unisound `u2-flash` 模型（`AI_BASE_URL=https://maas-api.unisound.com/v1`，`--no-ai-cache` 强制新鲜生成）。AI 第 2 点称「PR 仅新增报告文档、无代码变更」与事实不符（本 range diff 含 9 个文件的修复+文档）；第 5 点称「AI 评审未执行」系引用 09-30 报告旧文本，本次 AI 已实际执行。AI 第 4 点（`#[derive(Debug)]` 仍可能打印敏感字段）为有效改进建议，已在后续提交中落实：`oauth_token_store.rs` 的 `TokenInfo` 与 `api_signature/mod.rs` 的 `ApiKey` 改为自定义脱敏 `Debug`（敏感字段输出 `<redacted>`），并新增 `test_token_info_debug_redacts_secrets` / `test_api_key_debug_redacts_secret` 两条防泄漏测试。本报告唯一 low 项（`service_coverage_test.rs` 空凭据测试为空洞测试）已在后续提交中彻底消除：空凭据校验下沉为纯函数 `credentials_non_empty` 并补 4 断言单元测试，同时清理 `sz-rust-testkit` 中 `tx_isolation_fixture.rs` 的 `PhantomData` 空洞测试（改为 Send+Sync 编译期约束断言）。断言审计 `assertion-value-check.js` 复跑 0 ERROR。

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 1 low → 已全部关闭）

- [low] `gate` **assertion-value**:   [ERROR] packages/sz-rust-sz300/tests/service_coverage_test.rs:593 测试 test_auth_login_empty_credentials_returns_err
  - **处置（2026-10-01）**：空洞测试已删除，空凭据校验下沉为纯函数 `credentials_non_empty`（`packages/sz-rust-sz300/src/controllers/auth.rs`），新增 `test_credentials_non_empty_rejects_empty`（4 断言）。`node scripts/audit/assertion-value-check.js` 复跑 0 ERROR。


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

## 评审意见

### 最重要的潜在问题

1. **空洞测试未修复，测试形同虚设**
   已发现问题清单明确指出 `test_auth_login_empty_credentials_returns_error` 为空函数，无任何断言。该测试无法验证登录空凭据是否返回错误，属于无效测试。文档仅建议“后续补充”，但未在本次 PR 中处理，导致门禁仍存在 low 级缺陷。

2. **文档声称的修复未包含在 PR diff 中，存在误导风险**
   报告详细描述了 compile、sensitive-field、bare-unwrap 等修复，但 PR 仅新增了这份报告文档，没有任何代码变更。读者可能误以为修复已合入，实际却仍停留在工作区。若该 PR 被合并，代码与文档将长期不一致。

3. **裸 `unwrap` 改为 `expect` 并未消除 panic 风险**
   文档将 12 处 `unwrap` 替换为 `expect`，只是提供了错误上下文，程序仍会在异常时直接 panic。对于生产代码，更健壮的做法是返回 `Result` 或使用 `ok_or_else` 传播错误，避免单点故障导致整个服务崩溃。

4. **敏感字段脱敏不完整，可能通过 `Debug` 输出泄露**
   报告仅对 `access_token`、`refresh_token`、`secret` 添加了 `#[serde(skip_serializing)]`，但未考虑 `Debug` 派生。若这些类型被 `#[derive(Debug)]` 覆盖，日志或调试输出仍会打印明文敏感字段，违反最小暴露原则。

5. **门禁状态机记录不一致，AI 评审未执行却标记为 done**
   报告状态机显示 `ai → done`，但正文明确说明“AI 评审因缺 key 未执行（medium）”。这种矛盾会误导后续审计人员，使其认为 AI 评审已通过，实际却存在未覆盖的检查项。

---

### 具体修改建议

#### 1. 修复空洞测试

```rust
// packages/sz-rust-sz300/tests/service_coverage_test.rs:593
#[test]
fn test_auth_login_empty_credentials_returns_error() {
    // 假设存在 auth_login 函数，返回 Result
    let result = auth_login("", "");
    assert!(result.is_err(), "空凭据应返回 Err，实际得到 Ok");

    // 若需要验证具体错误类型，可进一步断言
    // assert!(matches!(result, Err(AuthError::EmptyCredentials)));
}
```

若该测试无法补充有效断言，应直接删除，避免虚假的测试通过。

#### 2. 在报告中明确标注修复状态

在文档头部增加醒目标记：

```markdown
> **注意**：本报告所述修复均位于工作区未提交修改中，尚未产生 commit SHA，也未包含在当前 PR 的 diff 中。请勿将本报告视为修复已合入的证据。
```

#### 3. 使用错误传播替代 `expect`

```rust
// 修复前（drop_counter.rs:94）
let value = self.value.take().unwrap();

// 修复后（推荐）
let value = self.value.take().ok_or_else(|| {
    anyhow::anyhow!("TrackedResource 值已被取出过")
})?;
```

若函数签名不允许返回 `Result`，至少应使用 `expect` 并确保前置条件不可变，但更建议重构为可返回错误的形式。

> **处置（2026-10-01）**：已评估并闭合。`TrackedResource::take(mut self)` 按值消费自身，`value` 仅在 `new` 中置为 `Some`，`get` 不转移所有权，因此 `None` 分支在类型层面不可达——`expect` 属不变式断言而非可恢复错误路径。强行改为 `Result` 会污染公开 API 且调用方（仅测试 `drop_counter.rs:157`）无错误处理场景。已在 `drop_counter.rs` 补充不变式注释说明。生产代码其余位置无裸 `unwrap`（`exporters.rs` 中的 `unwrap` 均在 `#[cfg(test)]` 测试模块内）。

#### 4. 手动实现脱敏的 `Debug`

```rust
impl fmt::Debug for TokenInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenInfo")
            .field("access_token", &"***")
            .field("refresh_token", &"***")
            .field("user_id", &self.user_id)
            .finish()
    }
}
```

同时，若这些字段需要反序列化，应使用 `#[serde(skip_deserializing)]` 或 `#[serde(default)]` 防止外部输入覆盖。

#### 5. 修正状态机记录

将状态机中的 `ai` 状态改为 `blocked` 或 `pending`，并在报告中明确：

```markdown
| ai | ⚠️ 未执行（缺少 AI_API_KEY） | 设置 key 后重跑 |
```

---

### 整体评分

**4 / 10**

- 报告内容详实，对门禁失败原因的分析有参考价值。
- 但作为 PR，仅添加文档而未包含实际修复，且文档自身存在误导性描述和未解决的测试缺陷，无法满足“可合入”标准。


## 结论
✅ 通过（无 ≥ medium 级别问题）
