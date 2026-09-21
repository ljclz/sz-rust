# PR 审查报告（2026-09-13，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ 9e75467`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 .github/workflows/ci.yml | 47 ++++-------------------------------------------
 1 file changed, 4 insertions(+), 43 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审报告

### 最重要的潜在问题

#### 1. **CI 门禁移除导致回归风险（可维护性/流程）**
**严重度：高**

`db-integration` job 被完全移除而非保留为 `if: false` 的占位符。虽然注释说明了迁移原因，但：
- 开源版 CI 将**永久失去**对 `sz-rust-sz300` 相关代码的编译检查（即使该包不在 workspace 中，其依赖关系仍可能影响 facade 层）
- 企业版 CI 若未同步配置，将出现**测试覆盖盲区**
- 后续维护者无法从 CI 配置中快速了解该测试的存在

**建议**：保留 job 定义但显式标记为 `if: false`，并添加指向企业版仓库的链接：

```yaml
# 门禁 12：DB 集成测试（P1-TEST-02）
# sz-rust-sz300 已迁移至企业版仓库（1614e84）
# 企业版 CI 配置: https://github.com/your-org/sz-rust-enterprise/.github/workflows/ci.yml
db-integration:
  name: DB Integration Tests (moved to enterprise)
  runs-on: ubuntu-latest
  if: false  # 显式禁用，保留配置可追溯性
  services:
    mysql:
      image: mysql:9.6
      # ... 保留原有 services 配置
  steps:
    - run: echo "Skipped - moved to enterprise repo"
```

---

#### 2. **覆盖率门禁阈值可能失真（质量保障）**
**严重度：中**

`coverage` job 中移除了 `sz-rust-sz300` 的覆盖率收集，但 `--fail-under-lines` 阈值（85%）未调整。若 `sz-rust-sz300` 原本贡献了较高覆盖率，移除后：
- 剩余 workspace 包的实际覆盖率可能**低于 85%**，导致 CI 频繁失败
- 或者阈值被**悄悄放宽**，掩盖了真实覆盖率下降

**建议**：在移除后重新计算基线覆盖率，并显式声明：

```yaml
- name: Run coverage with threshold
  run: |
    # 基线覆盖率已重新计算（2024-XX-XX）：移除 sz300 后为 87.3%
    # 若低于 85% 请调整阈值或补充测试
    cargo llvm-cov \
      --workspace \
      --exclude sz-orm-macros --exclude sz-rust-macros \
      --cobertura --output-path cobertura.xml \
      --fail-under-lines 85
```

---

#### 3. **`cobertura-merger.js` 脚本可能产生无效输入（健壮性）**
**严重度：中**

合并脚本现在只接收 workspace 报告，但 `cobertura-sz300-db.xml` 的移除未检查脚本是否支持**单文件输入**。若脚本假设至少两个输入文件，将导致 CI 失败。

**建议**：在 CI 中增加防御性检查：

```yaml
- name: Merge coverage reports
  run: |
    # 确保至少有一个报告文件存在
    if [ ! -f cobertura.xml ]; then
      echo "::error::cobertura.xml not found"
      exit 1
    fi
    node scripts/audit/cobertura-merger.js \
      cobertura.xml \
      --output merged-cobertura.xml
```

---

#### 4. **缺少迁移验证步骤（流程完整性）**
**严重度：低**

PR 仅移除了 CI 配置，未验证企业版仓库是否已正确接收 `sz-rust-sz300` 的测试配置。若企业版 CI 配置有遗漏，将导致**静默失败**。

**建议**：在 PR 描述或 CI 中添加验证步骤：

```yaml
- name: Verify enterprise repo sync
  run: |
    # 检查企业版仓库是否包含 db_integration_test
    git clone --depth 1 https://github.com/your-org/sz-rust-enterprise.git ../enterprise
    if ! grep -q "db_integration_test" ../enterprise/.github/workflows/ci.yml; then
      echo "::error::Enterprise CI missing db_integration_test"
      exit 1
    fi
```

---

#### 5. **注释信息不完整（可维护性）**
**严重度：低**

注释提到"完整 job 定义见 git 历史"，但未提供具体 commit hash 或命令，后续维护者需要手动搜索。

**建议**：提供精确的检索方式：

```yaml
# 门禁 12：DB 集成测试（P1-TEST-02）—— 已移除
# sz-rust-sz300 业务包已迁移至企业版仓库（1614e84）
# 完整 job 定义（含 MySQL/Postgres services）:
#   git show 1614e84^:.github/workflows/ci.yml | grep -A 50 "db-integration"
```

---

### 整体评分：**6/10**

**理由**：
- ✅ 变更方向正确（移除已迁移包的 CI 配置）
- ✅ 注释说明了迁移原因
- ❌ 完全移除 job 定义而非保留占位符，降低可追溯性
- ❌ 未重新验证覆盖率阈值
- ❌ 缺少企业版同步验证机制
- ❌ 注释信息不够精确，不利于后续维护


## 结论
✅ 通过（无 ≥ medium 级别问题）
