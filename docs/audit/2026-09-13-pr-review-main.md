# PR 审查报告（2026-09-13，branch: main，range: HEAD~2..HEAD）

> 审查时点: `HEAD @ 8618a77`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 .github/workflows/coverage.yml                     | 10 ++-
 .github/workflows/release.yml                      | 35 ++++-----
 README.en.md                                       |  8 +--
 README.md                                          |  8 +--
 docs/adr/ADR-036-reliable-job-queue.md             |  2 +-
 docs/audit/doc-debt.md                             |  2 +-
 .../sz-rust-addons-admin/tests/integration_test.rs | 84 ++++++++++++++++++++--
 .../src/tenant_admin/router.rs                     | 26 ++++++-
 .../src/data_scope/ext/notifier.rs                 |  9 ++-
 scripts/audit/doc-code-consistency.js              | 28 +++++++-
 10 files changed, 166 insertions(+), 46 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

# PR 评审报告

## 总体评价

该 PR 将 `sz-rust-sz300` 及相关插件从开源版仓库迁移至企业版仓库，涉及 CI/CD 流程、文档和发布配置的调整。整体方向正确，但存在若干需要关注的问题。

---

## 最重要的问题

### 1. 【高】`if: false` 硬编码禁用 Job，缺乏可维护性

**问题**：`coverage-sz300` 和 `docker` 两个 job 使用 `if: false` 永久禁用，但保留了完整的 job 定义（包括 services、steps 等）。这不仅造成 CI 配置冗余，还容易误导后续维护者——他们可能不清楚这些 job 是否真的被禁用，或者误以为只是暂时跳过。

**建议**：使用 GitHub Actions 的 `workflow_dispatch` 输入参数或环境变量控制，或者直接删除这些 job（企业版仓库已有对应流程）。

```yaml
# 方案 A：使用输入参数控制（推荐）
on:
  workflow_dispatch:
    inputs:
      run_enterprise_jobs:
        description: 'Run enterprise-only jobs'
        type: boolean
        default: false

jobs:
  coverage-sz300:
    if: inputs.run_enterprise_jobs == true
    # ... 保留完整定义

# 方案 B：直接删除（更简洁）
# 删除 coverage-sz300 和 docker 两个 job，在 PR 描述中说明已迁移至企业版
```

---

### 2. 【高】Release 产物变更未同步更新下载验证逻辑

**问题**：Release job 中 `EXPECTED` 变量仍硬编码为 `x86_64-unknown-linux-gnu` 和 `aarch64-unknown-linux-gnu`，但未检查 `matrix.target` 是否与之一致。如果未来添加新平台（如 `armv7`），会导致验证逻辑失效。

**建议**：从 matrix 动态获取目标平台列表，避免硬编码：

```yaml
jobs:
  build:
    strategy:
      matrix:
        target: [x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu]
    outputs:
      targets: ${{ steps.set-targets.outputs.targets }}
    steps:
      - id: set-targets
        run: echo "targets=${{ join(matrix.target, ' ') }}" >> $GITHUB_OUTPUT

  release:
    needs: build
    steps:
      - name: Verify artifacts
        run: |
          VERSION=${GITHUB_REF#refs/tags/}
          for t in ${{ needs.build.outputs.targets }}; do
            f="artifacts/sz-rust-cli-${t}/sz-rust-cli-${VERSION}-${t}.tar.gz"
            test -s "$f" || { echo "::error::missing artifact for ${t}"; exit 1; }
            # ...
          done
```

---

### 3. 【中】README 中目录结构描述与实际 workspace 不一致

**问题**：README 中仍列出 `sz-rust-addons-ecommerce`、`sz-rust-addons-cms`、`sz-rust-addons-crm` 等目录，但标注为"企业版仓库交付"。这会让开源版用户困惑——他们 clone 仓库后找不到这些目录。

**建议**：在 README 中明确区分开源版和企业版的目录结构，或添加说明：

```markdown
## 目录结构

> **注意**：以下目录中标注 🔒 的模块仅在企业版仓库中提供，开源版不包含。

sz-rust/                          # workspace 根目录
├── sz-rust-core/                 # 核心库
├── sz-rust-ai-facade/            # AI 门面
├── ...
├── sz-rust-addons-loader/        # 插件加载器
├── sz-rust-observability/        # 可观测性
└── sz-rust-sz300/ 🔒             # SZ300 业务应用（企业版）
```

---

### 4. 【中】Release 流程中 `test` job 依赖未更新

**问题**：`docker` job 的 `needs: test` 依赖未移除，但 `docker` job 已被 `if: false` 禁用。虽然不影响功能，但保留无效依赖会误导维护者，且 `test` job 可能包含企业版相关的测试步骤。

**建议**：清理无效依赖，并确保 `test` job 不依赖已迁移的模块：

```yaml
  # 删除 docker job 或移除其 needs 依赖
  # 如果 test job 中包含 sz300 相关测试，也需要同步调整
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run tests
        run: |
          # 确保不包含 sz-rust-sz300 相关测试
          cargo test --workspace --exclude sz-rust-sz300
```

---

### 5. 【低】`coverage.yml` 中 P3 覆盖率范围缩减未说明原因

**问题**：P3 覆盖率从 `addons-crm`、`addons-ecommerce`、`addons-cms` 缩减为仅 `addons-loader`，但未在注释中说明这些模块的覆盖率测试是否已迁移至企业版，还是被完全移除。

**建议**：添加明确注释，并考虑将 P3 覆盖率目标调整为仅针对开源版模块：

```yaml
  coverage-p3:
    name: Coverage P3 (addons-loader/cli/mcp)
    runs-on: ubuntu-latest
    steps:
      - name: Run coverage P3
        run: |
          cargo llvm-cov \
            -p sz-rust-addons-loader \
            -p sz-rust-cli \
            -p sz-rust-mcp \
            # 注：addons-crm/ecommerce/cms 已迁移至企业版仓库
            # 企业版覆盖率测试见 enterprise/.github/workflows/coverage.yml
```

---

## 整体评分

**6.5/10**

**理由**：
- **优点**：迁移方向正确，文档更新及时，CI 配置调整基本合理
- **扣分点**：`if: false` 硬编码禁用不够优雅；Release 产物验证逻辑存在硬编码风险；README 目录结构描述可能误导用户；部分注释缺失导致维护者难以理解变更意图


## 结论
✅ 通过（无 ≥ medium 级别问题）
