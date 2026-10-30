# PR 审查报告（2026-09-04，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ a5d49ce`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 .cargo/mutants.toml           | 28 ++++++++++++++++++++++++++++
 .github/workflows/mutants.yml | 11 +++++++----
 .github/workflows/release.yml | 16 +++++++++++++---
 CHANGELOG.md                  |  9 +++++++++
 docs/audit/doc-debt.md        |  2 ++
 5 files changed, 59 insertions(+), 7 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

### 潜在问题与修改建议

#### 1. CI 流水线并发风险：`--in-place` 导致变异测试与测试任务冲突
**问题分析**：
`cargo mutants --in-place` 会直接在当前 workspace 目录下原地修改源码进行变异测试。如果 `mutants.yml` 中的 `mutants` job 与 `release.yml` 中的 `test` job（或同仓库的其他 CI 任务）在同一 Runner 并发执行，或者未做好缓存隔离，会导致严重的竞态条件和源码污染。此外，`--in-place` 在失败时可能残留被变异的源码，影响后续步骤。

**修改建议**：
移除 `--in-place`，让 `cargo-mutants` 使用默认的临时拷贝目录（`mutants.out` 下的临时工作目录），确保 workspace 源码不受污染。

```yaml
# .github/workflows/mutants.yml
      - name: Run cargo mutants
        run: >
          cargo mutants -p sz-rust-core --no-shuffle --all-features
          --timeout 120
          --output-dir mutants.out
```

#### 2. 安全/可维护性：`rustls-tls` 可能破坏企业内网 MITM 代理兼容性
**问题分析**：
CHANGELOG 中提到：“证书校验链由系统 OpenSSL 切至 rustls（webpki-roots 系统探测行为差异，企业 MITM 代理场景需验证）”。
`reqwest` 使用 `rustls-tls` 时，默认依赖 `webpki-roots`（Mozilla 的根证书集）。如果 sz-rust 部署在带有企业自签证书或 MITM 代理的内网环境中，由于无法读取系统证书库，所有的 HTTPS 出站请求（如支付回调、API 请求）都会因证书校验失败而中断。这对于企业级应用是致命的。

**修改建议**：
如果项目需要兼容企业内网环境，应使用 `rustls-tls-native-roots` 特性，使其读取操作系统的本地根证书。

```toml
# Cargo.toml
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "multipart", "rustls-tls-native-roots", "charset", "http2"] }
```

#### 3. 可维护性：`pay.rs` 资金逻辑变异测试长期豁免缺乏强约束
**问题分析**：
`.cargo/mutants.toml` 中对 `**/pay.rs` 进行了排除，虽然标注了 `FIXME(DB-2026-09-03-01)` 和退出条件，但这只是一个文档约定。在后续的 PR 中，开发者可能无意间修改了 `pay.rs` 的资金逻辑，由于被排除在变异测试之外，这些高风险逻辑的变异体将无法被检测，形成安全盲区。

**修改建议**：
在 CI 的变异测试步骤之前，增加一个轻量级的“门禁检查”脚本，解析 `.cargo/mutants.toml` 的修改记录，如果 `pay.rs` 的排除规则被修改或超期未清理，则发出警告或阻断流水线。或者，不全量排除 `pay.rs`，而是通过 `cargo-mutants` 的目录级限制，仅对 `pay.rs` 中的非核心工具函数运行变异测试。

```yaml
# .github/workflows/mutants.yml
      - name: Enforce pay.rs mutation debt
        run: |
          # 检查 doc-debt.md 中 DB-2026-09-03-01 的状态是否仍为 PENDING
          if grep -q "DB-2026-09-03-01.*PENDING" docs/audit/doc-debt.md; then
            echo "::warning::pay.rs is excluded from mutation testing. Ensure debt is cleared."
          fi
```

#### 4. CI 健壮性：Release 产物完整性断言存在路径硬编码脆弱性
**问题分析**：
在 `release.yml` 的 `Assert all release artifacts exist` 步骤中，硬编码了产物路径 `artifacts/sz300-server-${t}/sz300-server-${VERSION}-${t}.tar.gz`。如果上游构建任务修改了 artifact 的命名规则或上传路径，该断言会失败，且错误信息不够明确。另外，`test -s` 仅检查文件存在且非空，未校验 tar.gz 的完整性（如是否为损坏的 gzip 文件）。

**修改建议**：
使用通配符或 `find` 命令进行更鲁棒的检测，并增加 gzip 完整性校验。

```yaml
      - name: Assert all release artifacts exist and are valid
        run: |
          VERSION=${GITHUB_REF#refs/tags/}
          for t in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
            ARTIFACT="artifacts/sz300-server-${t}/sz300-server-${VERSION}-${t}.tar.gz"
            SHA256="artifacts/sz300-server-${t}/sz300-server-${VERSION}-${t}.tar.gz.sha256"

            test -s "$ARTIFACT" || { echo "::error::missing artifact for ${t}"; exit 1; }
            test -s "$SHA256" || { echo "::error::missing sha256 for ${t}"; exit 1; }

            # 校验 gzip 完整性
            gzip -t "$ARTIFACT" || { echo "::error::corrupted gzip for ${t}"; exit 1; }
            # 校验 sha256
            sha256sum -c < "$SHA256" || { echo "::error::sha256 mismatch for ${t}"; exit 1; }
          done
          echo "All platform artifacts present and valid"
```

### 整体评分
**8/10**

**评审总结**：
该 PR 质量较高，主要解决了 CI 流水线的超时截断隐患、静默失败通道（移除 `continue-on-error` 并增加产物断言），以及将变异测试排除清单版本化管理，这些都是极佳的工程实践。CHANGELOG 记录详尽，技术决策有理有据。
扣分点在于：`rustls-tls` 的切换可能引入内网部署的兼容性灾难，需要优先确认；`--in-place` 在 CI 环境中存在潜在的源码污染风险；产物校验逻辑可以更加鲁棒。建议针对上述 1、2 点进行微调后合入。


## 结论
✅ 通过（无 ≥ medium 级别问题）
