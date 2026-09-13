# PR 审查报告（2026-09-13，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ d6a1f8c`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 .github/workflows/coverage.yml | 62 ++++----------------------------------
 .github/workflows/release.yml  | 67 ++++++++++++++++--------------------------
 README.en.md                   | 10 ++++---
 README.md                      | 10 ++++---
 4 files changed, 43 insertions(+), 106 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审报告

### 最重要的潜在问题

#### 1. **`targets` job 使用 Python 生成 JSON 存在可移植性风险**（可维护性）
`release.yml` 中新增的 `targets` job 依赖 `python3` 生成 JSON 矩阵，但 GitHub Actions 的 `ubuntu-latest` 镜像虽然预装 Python，这增加了不必要的运行时依赖。更严重的是，如果未来 runner 镜像变更或 Python 版本升级导致行为变化，整个发布流水线会静默失败。

**建议**：使用 GitHub Actions 原生支持的 YAML 语法直接定义矩阵，避免运行时脚本依赖。

```yaml
targets:
  name: Resolve release targets
  runs-on: ubuntu-latest
  outputs:
    matrix: '["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]'
    space: 'x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu'
  steps:
    - id: set
      run: |
        echo "matrix=${{ toJSON(fromJSON('["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]')) }}" >> "$GITHUB_OUTPUT"
        echo "space=x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu" >> "$GITHUB_OUTPUT"
```

#### 2. **`coverage-merge` job 的 `needs` 依赖未同步更新**（可维护性）
`coverage.yml` 中 `coverage-merge` 的 `needs` 已移除 `coverage-sz300`，但 `coverage-p0`、`coverage-p1`、`coverage-p2` 的 job 定义是否仍然存在且有效？如果这些 job 也引用了已迁移的 crate，会导致覆盖率数据不完整或 job 失败。

**建议**：在 PR 中同步检查并更新所有 coverage job 的 crate 列表，确保 `needs` 依赖与实际运行的 job 完全一致。同时添加一个验证步骤：

```yaml
coverage-merge:
  name: Coverage Merge & Verify
  runs-on: ubuntu-latest
  needs: [coverage-p0, coverage-p1, coverage-p2, coverage-p3]
  steps:
    - name: Verify coverage artifacts exist
      run: |
        for f in cobertura-p0.xml cobertura-p1.xml cobertura-p2.xml cobertura-p3.xml; do
          test -s "$f" || { echo "::error::missing $f"; exit 1; }
        done
```

#### 3. **`release.yml` 中 `targets` job 的 `outputs` 未做类型校验**（安全性）
`targets` job 的输出直接用于 `build` 和 `release` job 的矩阵和断言。如果 `TARGETS` 变量被意外修改（如包含非法字符或空值），会导致构建矩阵异常或产物断言失效，且没有显式错误提示。

**建议**：在 `targets` job 中添加显式校验：

```yaml
steps:
  - id: set
    run: |
      TARGETS="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"
      # 校验格式：每个 target 必须匹配 Rust 平台三元组
      for t in $TARGETS; do
        echo "$t" | grep -qE '^[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+$' || {
          echo "::error::invalid target: $t"
          exit 1
        }
      done
      echo "space=$TARGETS" >> "$GITHUB_OUTPUT"
      json=$(printf '%s\n' $TARGETS | python3 -c 'import sys, json; print(json.dumps(sys.stdin.read().split()))')
      echo "matrix=$json" >> "$GITHUB_OUTPUT"
```

#### 4. **`coverage.yml` 中 `coverage-p3` 的注释与实际行为不一致**（可维护性）
注释提到 "addons-{crm,ecommerce,cms} 已迁移企业版仓库"，但 `coverage-p3` 的 `cargo llvm-cov` 命令仍然包含 `-p sz-rust-addons-loader`。如果 `sz-rust-addons-loader` 也依赖已迁移的 crate，会导致编译失败或覆盖率数据不完整。

**建议**：在 PR 中实际运行 `cargo llvm-cov -p sz-rust-addons-loader` 验证依赖完整性，并更新注释为实际验证过的 crate 列表：

```yaml
- name: Run coverage P3
  # 已验证：sz-rust-addons-loader 不依赖已迁移的 addons-{crm,ecommerce,cms}
  run: |
    cargo llvm-cov \
      -p sz-rust-addons-loader \
      --cobertura --output-path cobertura-p3.xml \
      --fail-under-lines ${COVERAGE_THRESHOLD}
```

#### 5. **`release.yml` 中 `build` job 的 `fail-fast: false` 可能导致资源浪费**（性能）
`fail-fast: false` 意味着即使一个平台的构建失败，其他平台仍会继续构建。虽然这有助于诊断问题，但在 CI 资源有限的情况下，如果 `x86_64` 构建失败，`aarch64` 构建仍会消耗大量时间和资源。

**建议**：根据实际需求权衡。如果希望快速失败以节省资源，可以改为 `fail-fast: true`；如果希望收集所有平台的错误信息，保留 `false` 但添加超时限制：

```yaml
strategy:
  fail-fast: false
  max-parallel: 2  # 限制并行构建数量，避免资源耗尽
  matrix:
    target: ${{ fromJSON(needs.targets.outputs.matrix) }}
```

---

### 整体评分：**6/10**

**评分理由**：
- **优点**：PR 正确识别了 sz300 迁移带来的 CI 变更，清理了无效 job，并引入了 `targets` job 作为单一来源，减少了平台名单漂移风险。
- **扣分点**：
  1. `targets` job 引入 Python 依赖，增加了不必要的运行时复杂度。
  2. 未充分验证 `coverage-p3` 的依赖完整性，注释与实际命令可能不一致。
  3. `coverage-merge` 的 `needs` 更新未同步验证其他 coverage job 的有效性。
  4. 缺少对 `targets` job 输出的显式校验，存在静默失败风险。
  5. 整体变更偏向"删除"而非"重构"，对遗留的 job 依赖关系缺乏系统性检查。


## 结论
✅ 通过（无 ≥ medium 级别问题）
