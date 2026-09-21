# PR 审查报告（2026-09-03，branch: main，range: 2fdc456..HEAD）

> 审查时点: `HEAD @ d79ffc5`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 .github/workflows/mutants.yml                      |   6 +-
 .github/workflows/release.yml                      |   8 +
 .gitignore                                         |   6 +
 .trae/settings.json                                |   3 +-
 CHANGELOG.md                                       |  20 +-
 Cargo.lock                                         | 162 ++----------
 Cargo.toml                                         |   2 +-
 Dockerfile                                         |  12 +-
 deny.toml                                          |  36 +--
 deploy/alertmanager.yml                            |  10 +-
 deploy/docker-compose.yml                          |  10 +-
 deploy/loadtest/loadtest.js                        |   2 +-
 deploy/monitoring/alertmanager/alertmanager.yml    |  22 +-
 deploy/monitoring/prometheus.yml                   |   5 +-
 deploy/setup.sh                                    |   2 +-
 ...257\271\346\257\224\346\212\245\345\221\212.md" |  48 ++--
 docs/audit/2026-08-22-pr-review-main.md            | 118 +++++++++
 ...224\271\350\277\233\346\212\245\345\221\212.md" | 159 ++++++++++++
 docs/audit/events.jsonl                            |   5 +
 packages/sz-rust-addons-crm/tests/coverage_test.rs |   5 +-
 packages/sz-rust-addons-operate/Cargo.toml         |   2 +
 .../sz-rust-addons-operate/src/capability/mod.rs   | 200 +++++++++++++++
 packages/sz-rust-addons-operate/src/lib.rs         |  78 +++++-
 packages/sz-rust-core/src/alloc_counter.rs         | 220 +++++++++++-----
 packages/sz-rust-core/src/container/tests.rs       |  76 ++++++
 packages/sz-rust-core/src/error_handler.rs         |  10 +
 packages/sz-rust-core/src/h2.rs                    |  85 +++++++
 packages/sz-rust-core/src/mem_pool.rs              | 156 +++++++++++-
 packages/sz-rust-core/src/plugin/event_bus.rs      |  36 +++
 packages/sz-rust-core/src/plugin/schema.rs         |  15 ++
 packages/sz-rust-core/src/runtime.rs               |   9 +
 packages/sz-rust-core/src/runtime/hot_reload.rs    | 184 +++++++++++++-
 packages/sz-rust-core/src/runtime/scheduler.rs     |  15 ++
 packages/sz-rust-core/src/runtime/websocket.rs     |  42 +++
 packages/sz-rust-core/src/runtime/worker.rs        |  55 ++++
 packages/sz-rust-core/src/seed.rs                  |  75 ++++++
 packages/sz-rust-pdf/Cargo.toml                    |   7 +
 packages/sz-rust-pdf/src/capability/mod.rs         | 278 ++++++++++++++++++++
 packages/sz-rust-pdf/src/lib.rs                    | 123 ++++++++-
 packages/sz-rust-sz300/src/controllers/ai.rs       |   4 +-
 packages/sz-rust-sz300/src/controllers/mod.rs      |  11 +-
 .../sz-rust-sz300/src/controllers/operate_api.rs   | 124 ---------
 packages/sz-rust-sz300/src/controllers/pdf_api.rs  | 177 -------------
 .../sz-rust-sz300/src/controllers/tracing_api.rs   | 181 -------------
 .../sz-rust-sz300/src/controllers/workflow_api.rs  | 121 ---------
 packages/sz-rust-sz300/src/main.rs                 |  28 ++
 packages/sz-rust-sz300/src/router.rs               |  37 +--
 packages/sz-rust-sz300/src/state.rs                |  12 +
 .../tests/addon_deep_wiring_v1_test.rs             | 196 ++++++++++++++
 .../tests/audit_remediation_v3_high_risk_test.rs   |   9 +-
 .../sz-rust-sz300/tests/db_integration_test.rs     |   4 +
 packages/sz-rust-tracing/Cargo.toml                |   8 +
 packages/sz-rust-tracing/src/capability/mod.rs     | 282 +++++++++++++++++++++
 packages/sz-rust-tracing/src/lib.rs                | 123 ++++++++-
 packages/sz-rust-workflow/Cargo.toml               |   2 +
 packages/sz-rust-workflow/src/capability/mod.rs    | 256 +++++++++++++++++++
 packages/sz-rust-workflow/src/lib.rs               |  91 +++++++
 scripts/audit/check-std-fs.py                      |   4 +
 scripts/dev/purge-target-cache.ps1                 |  60 +++++
 scripts/reverify_monitoring.js                     |  30 +++
 scripts/verify_docker_build.js                     | 156 ++++++++++++
 scripts/verify_docker_pull.js                      |  94 +++++++
 scripts/verify_monitoring_deploy.js                | 155 +++++++++++
 63 files changed, 3524 insertions(+), 948 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

# PR 评审：变异测试质量改进 + CI/发布流程调整

## 总体印象

本 PR 主体是 CI 配置、测试补强与文档更新，可见 diff 中无生产 Rust 代码变更。方向正确（missed 119→10 是实质改进），aarch64 交叉链接器修复也是必要的。但存在**质量门禁被削弱**、**CI 配置失效**、**机器相关配置入库**、**依赖变更未记录**四类需要处理的问题。

---

## 问题 1：变异测试排除清单削弱质量门禁，存在"指标美化"风险 ⚠️ 高

`pay.rs`（支付）、`qr_code.rs`、`multi_tenant.rs` 被排除出变异测试范围。CHANGELOG 宣称 "missed 119→10"，但其中一部分是靠**把难测的模块移出范围**实现的，而非真正提升了测试质量。支付逻辑恰恰是变异体存活代价最高的代码。此外 `**/pay.rs` 按文件名匹配，可能误伤 addons 中同名文件。

**建议**：将排除清单移入版本化的 `mutants.toml`，逐条写明理由和退出条件，并为被排除模块保留覆盖率兜底：

```toml
# mutants.toml（仓库根目录，变更需走 code review）
exclude_globs = [
    # 纯 schema 同步逻辑，已有集成测试覆盖，变异测试性价比低
    "**/schema_cache.rs",
    # FIXME(#<issue>): pay 涉及资金逻辑，禁止长期排除；
    # 当前因运行时长临时排除，补充单测后必须移回
    "**/pay.rs",
    "**/qr_code.rs",
]
```

```yaml
# 兜底：被排除模块仍强制行覆盖率门禁
- name: Coverage gate for excluded modules
  run: |
    cargo llvm-cov -p sz-rust-core --all-features --fail-under-lines 70 \
      -- 'src/pay.rs' 'src/qr_code.rs'
```

---

## 问题 2：`timeout-minutes: 480` 超出 GitHub 托管 Runner 上限，且掩盖了根因 ⚠️ 中高

GitHub 托管 Ubuntu Runner 单 job 上限为 **6 小时（360 分钟）**，480 分钟会被静默截断，配置具有误导性。同时结果中已有 1 个 timeout 变异体——单个挂死变异体可能吃掉整个预算。拉长 timeout 是治标，应改为**分片并行** + **单变异体超时**：

```yaml
strategy:
  fail-fast: false
  matrix:
    shard:
      - name: core-runtime
        files: "--file 'src/container/**' --file 'src/runtime/**' --file 'src/json/**'"
      - name: orm-mem
        files: "--file 'src/orm/**' --file 'src/mem_pool/**'"
      - name: server-misc
        files: "--file 'src/server/**' --file 'src/error_handler/**' --file 'src/health/**'"
steps:
  - run: >
      cargo mutants -p sz-rust-core --in-place --no-shuffle --all-features
      --timeout 120
      ${{ matrix.shard.files }}
```

`--timeout 120`（秒）约束单个变异体的测试时长，避免一个挂起变异体拖垮整个 job。若分片后仍超 2h，考虑将 mutants 移到 nightly 定时任务而非阻塞 PR。

---

## 问题 3：机器相关的 Windows 绝对路径提交进共享配置 ⚠️ 中

```json
"rust-analyzer.cargo.targetDir": "C:\\sz-rust-target-ra"
```

`.trae/settings.json` 是仓库级共享配置，Linux/macOS 开发者会拿到一个无效的 Windows 路径；且独立 targetDir 会使 rust-analyzer 与 `cargo build` 缓存分离，首次检查全量重编译、磁盘占用翻倍。

**建议**：从共享配置中删除，移入本地覆盖文件：

```diff
--- a/.trae/settings.json
+++ b/.trae/settings.json
@@ -8,6 +8,5 @@
   "trae.rules.rustStrictMode": true,
   "trae.skills.autoDiscovery": true,
   "trae.agent.defaultModel": "claude-3.7-sonnet",
   "trae.agent.temperature": 0.1,
-  "rust-analyzer.cargo.targetDir": "C:\\sz-rust-target-ra"
 }
```

```gitignore
# .gitignore
.trae/settings.local.json
```

若目的是隔离 RA 与构建缓存，用相对路径 `"target/ra"` 亦可，但需团队共识。

---

## 问题 4：Cargo.lock 中 TLS 栈变更未记录、意图不明 ⚠️ 中高（安全/供应链）

Lockfile 删除了 `native-tls`、`hyper-tls`、`foreign-types`、`core-foundation 0.9.4`，表明 HTTP 客户端 TLS 后端发生了迁移（大概率是 reqwest 切到 rustls）。注意：Cargo.lock 包含全部 feature 的可选依赖，**这不是 `--all-features` 的副作用，而是真实的依赖图变更**。这属于安全相关变更：证书校验行为、系统根证书来源（企业 MITM 代理场景）都会受影响，但 CHANGELOG 和可见 diff 中均无说明。

**建议**：在 Cargo.toml 中显式固化意图并补 CHANGELOG：

```toml
# 明确 TLS 后端，避免 default-features 隐式漂移
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls-native-roots"] }
```

```markdown
### Changed
- HTTP 客户端 TLS 后端由 native-tls 迁移至 rustls（影响证书校验链与根证书来源）
```

验证：`cargo tree -i native-tls` 应报 "did not match any packages"；同时确认 lockfile 中没有夹带无关 crate 的版本升级（`cargo update` 副作用需回滚）。

---

## 问题 5：`continue-on-error` 为发布流水线引入静默失败通道 ⚠️ 中

```yaml
continue-on-error: ${{ matrix.allow_failure || false }}
```

当前两个 target 均为 `false`，机制处于休眠状态——但一旦有人把 aarch64 改成 `allow_failure: true`，发布 job 会"绿灯"通过却**缺失该平台产物**，下游 publish 照常执行。发布流水线不应存在静默降级路径。

**建议**：删除该机制，改为在发布 job 中做产物完整性断言：

```yaml
# publish job 中
- name: Assert all release artifacts exist
  run: |
    for t in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
      test -s "dist/sz300-${t}.tar.gz" \
        || { echo "::error::missing artifact for ${t}"; exit 1; }
    done
```

另外，对 10 个存活变异体的"语义等价"声明（如 `Default::default()` 等价于 `for_balanced`）是**脆弱的隐式耦合**——将来有人独立修改 `Default` impl，等价性即静默破裂。建议用测试固化不变量：

```rust
#[test]
fn default_worker_config_matches_new() {
    let a = WorkerConfig::default();
    let b = WorkerConfig::new();
    assert_eq!(a.worker_count, b.worker_count);
    // 或为 WorkerConfig 实现 PartialEq 后全字段比较
}
```

---

## 评分：**6 / 10**

| 维度 | 评价 |
|---|---|
| 测试质量改进 | 实质有效（caught 233→325），但排除 pay.rs 等核心模块扣分 |
| CI 工程 | aarch64 linker 修复正确；timeout 配置失效、缺分片设计 |
| 安全 | TLS 栈变更未记录，需澄清意图 |
| 可维护性 | 机器相关配置入库、排除清单内联在 workflow 中缺乏治理 |

**合并前必须澄清**：pay.rs 排除的退出计划（问题 1）、TLS 变更意图（问题 4）。其余可在后续 PR 跟进。


## 结论
✅ 通过（无 ≥ medium 级别问题）
