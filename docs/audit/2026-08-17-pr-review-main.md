# PR 审查报告（2026-08-17，branch: main，range: HEAD~7..HEAD）

> 审查时点: `HEAD @ 8b08c14`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 1 medium / 1 low）

- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: docs/audit/2026-08-17-pr-review-main.md:11: trailing whitespace. +- [medium] `diff` **whitespace-error**: 空白/冲突标记错误: scripts/collect-baseline.ps1:46: trailing whitespace. +          docs/audit/2026-08-17-pr-review-main.md:209: trailing whitespace.
- [low] `gate` **assertion-value**:   [WARN] packages/sz-rust-cache-facade/src/lib.rs:4775 测试 test_tag_clear_empty_no_error() 无断言宏但有 1 处 u


## 补充信息

## 变更集
```
 .github/workflows/ci.yml                           |  73 ++++++-
 .github/workflows/coverage.yml                     | 241 ++++++++++++++++-----
 docs/audit/2026-08-17-pr-review-main.md            | 237 ++++++++++++++++++++
 docs/audit/coverage-priority.json                  | 167 +++++++++-----
 docs/audit/events.jsonl                            |   5 +
 .../sz-rust-ai-facade/tests/facade_error_test.rs   |   9 +-
 packages/sz-rust-mcp/src/lib.rs                    |   4 +-
 packages/sz-rust-orm-facade/src/jobs.rs            |   2 +-
 scripts/audit/pr-review.sh                         |   9 +-
 scripts/collect-baseline.ps1                       |   6 +-
 10 files changed, 628 insertions(+), 125 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）



## PR 评审报告：sz-rust CI 覆盖率增强

### 核心问题清单

#### 1. [安全/流程] Commit Message 动态调整覆盖率阈值 — 可绕过门禁
**严重度：High**

通过 commit message 中的 `[coverage-stage:S1]` 等标记动态降低覆盖率阈值（30%/50%/70%），任何开发者都可以在推送低覆盖率代码时附加该标记绕过 85% 门禁。

```yaml
# 当前：任何人可绕过
case "$MSG" in
  *"[coverage-stage:S1]"*) THRESHOLD=30 ;;
  ...
esac
```

**建议**：覆盖率阶段应通过 protected branch 规则或仓库设置控制，而非 commit message。如果必须保留渐进策略，应限制为仅特定分支（如 `develop`）或仅 maintainer 可使用：

```yaml
- name: Parse coverage stage from commit message
  id: cov-stage
  run: |
    THRESHOLD=${COVERAGE_THRESHOLD:-85}
    # 仅允许 develop 分支使用渐进阈值，main 分支强制 85%
    if [[ "${{ github.ref }}" == "refs/heads/main" ]]; then
      THRESHOLD=85
    else
      MSG="${{ github.event.head_commit.message }}"
      case "$MSG" in
        *"[coverage-stage:S1]"*) THRESHOLD=30 ;;
        *"[coverage-stage:S2]"*) THRESHOLD=50 ;;
        *"[coverage-stage:S3]"*) THRESHOLD=70 ;;
      esac
    fi
    echo "threshold=$THRESHOLD" >> "$GITHUB_OUTPUT"
```

---

#### 2. [可靠性] `--ignore-tests-in-target` 标志可能不存在
**严重度：Medium**

`cargo llvm-cov` 标准 CLI 中无 `--ignore-tests-in-target` 标志。该标志会导致 CI 直接报错退出。

```yaml
# 当前（可能失败）
cargo llvm-cov -p sz-rust-sz300 \
  --ignore-tests-in-target \   # ← 该标志不存在
  --cobertura --output-path cobertura-sz300-db.xml \
  -- --ignored --test-threads=1
```

**建议**：移除该标志，改用 `--ignore-run-fail` 或通过 test 属性过滤：

```yaml
- name: Run sz300 DB integration coverage
  run: |
    cargo llvm-cov -p sz-rust-sz300 \
      --cobertura --output-path cobertura-sz300-db.xml \
      --features db-integration \
      -- --ignored --test-threads=1
```

如果目标是排除测试函数本身的覆盖率（而非测试代码调用的业务逻辑），应使用 `--ignore-filename-regex` 或在 `Cargo.toml` 中配置：

```toml
# Cargo.toml
[profile.test]
# 不推荐：无法按 crate 粒度控制
```

更推荐的做法是在 `llvm-cov` 中使用 `--ignore-run-fail` 配合区域排除注释。

---

#### 3. [可维护性] DB 集成测试运行策略不精确
**严重度：Medium**

`--ignored` 会运行所有被 `#[ignore]` 标记的测试，不仅限于 DB 测试。如果项目中存在其他原因被 ignore 的测试（如慢测试、外部依赖测试），也会被一并执行，导致结果污染。

**建议**：使用自定义 test 标记精确过滤：

```rust
// 测试代码中
#[test]
#[cfg_attr(not(feature = "db-integration"), ignore)]
#[ignore]  // 移除通用 ignore，改用 feature gate
fn db_integration_test() { ... }
```

```yaml
# CI 中
cargo llvm-cov -p sz-rust-sz300 \
  --features db-integration \
  --cobertura --output-path cobertura-sz300-db.xml \
  -- --test-threads=1
```

---

#### 4. [测试有效性] `test_tag_clear_empty_no_error()` 无断言
**严重度：Low**（已在问题清单中标注）

```rust
// packages/sz-rust-cache-facade/src/lib.rs:4775
#[test]
fn test_tag_clear_empty_no_error() {
    let mut cache = Cache::new();
    cache.clear_tags(&[]);
    // 缺少断言！测试永远通过
}
```

**建议**：添加有意义的断言：

```rust
#[test]
fn test_tag_clear_empty_no_error() {
    let mut cache = Cache::new();
    cache.clear_tags(&[]);
    // 验证空操作后 cache 状态不变
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}
```

---

#### 5. [代码质量] Trailing Whitespace 与冲突标记残留
**严重度：Low**

`docs/audit/2026-08-17-pr-review-main.md` 和 `scripts/collect-baseline.ps1` 存在 trailing whitespace，审计文档中甚至出现 diff 冲突标记样式的文本残留。

**建议**：运行 `cargo fmt` 和 `clippy` 前，先清理文档和脚本：

```bash
# 清理 trailing whitespace
find . -type f \( -name "*.md" -o -name "*.ps1" -o -name "*.sh" \) \
  -exec sed -i 's/[[:space:]]*$//' {} +
```

---

### 整体评分：6/10

| 维度 | 评分 | 说明 |
|------|------|------|
| 设计意图 | 8 | 分片并行 + 合并 + per-crate 门禁，方向正确 |
| 安全性 | 5 | commit message 绕过阈值是明显漏洞 |
| 可靠性 | 6 | `--ignore-tests-in-target` 标志存疑，DB 测试策略粗糙 |
| 可维护性 | 6 | 两套 coverage workflow（ci.yml + coverage.yml）职责重叠，需明确分工 |
| 代码质量 | 7 | trailing whitespace 等小问题，无重大逻辑错误 |

**修复优先级**：问题 1（安全绕过）> 问题 2（CI 可能失败）> 问题 3（测试策略）> 问题 4/5（质量）。


## 结论
❌ **阻塞**: 1 个 ≥ medium 级别问题，禁止合入
