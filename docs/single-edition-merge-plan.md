# sz-rust 单一版本（Single Edition）合并方案

> **版本**：v1.0
> **日期**：2026-10-25
> **性质**：产品战略决策 + 技术实施方案（权威方案 `docs/product-technical-plan.md` 的修订提案）
> **决策**：**AI 时代下「企业版」失去存在意义，所有版本合并为单一开源版本（One Version）**
> **关联文档**：`docs/product-technical-plan.md`、`docs/plugin-marketplace-design.md`、`docs/audit/README.md`、`docs/audit/2026-10-25-架构评测修复记录.md`

---

## 一、决策背景：为什么 AI 时代企业版没有意义

### 1.1 原「开源版 + 企业版」分离的假设

`docs/product-technical-plan.md` 第 2 章设计了两层商业模型：

- **开源版**：框架核心 + AI 基础设施 + 插件加载 + CLI + 示例（Apache-2.0）
- **企业版**：行业插件包（菜市场/餐饮/零售）+ 企业级功能插件（SSO/审计/HA）+ SDD Agent + AI 迁移工具（商业许可）

其隐含假设是：**行业 Know-How 稀缺，行业插件可以卖钱；企业级功能可以卖钱；开发者生态需要市场激励才能供给插件。**

### 1.2 现实代价（已验证）

| 代价 | 证据 |
|------|------|
| 双仓库维护成本高 | 企业版仓库在 `E:/www/rust/sz-rust-enterprise`，无 git remote；README 多处引用该本地绝对路径（`README.md:17,45`），外部用户**无法访问**，所谓「生产已接入」证据链不可验证 |
| 文档撕裂 | README 大量「🔒 企业版」「✅ 生产已接入（企业版）」标注；英文 README 版本停在 v0.7.0（`README.en.md:7`）；中文 README 版本声明 v1.2.2 而实际 tag 已到 v1.8.0 |
| 门禁覆盖不全 | 防幻影交付门禁（doc-code-consistency / adr-code / assertion-value / feature-consistency，`ci.yml:435-465`）只能管本仓库，企业版代码不在门禁范围内 |
| 生态冷启动死结 | GitHub **0 stars / 0 forks / 0 issues**（2026-10-25 API 实测），企业版从未产生任何外部收入或用户验证 |
| 自评与外评落差 | 内部报告自评 8.0-9.0 分（`docs/audit/2026-10-22-全面深入评估报告.md:12-20`）；外部架构师评测发现 cargo audit 35 条公告、/metrics 公开、CI audit non-blocking 等 P1 缺口（`docs/audit/2026-10-25-架构评测修复记录.md:13-15`） |

### 1.3 AI 时代的三重冲击

1. **AI 抹平「行业插件」的供给壁垒**
   原方案假设「第三方开发者手动发布行业插件」需要市场激励。但 AI 编程工具（Cursor / Windsurf / 本框架的 SDD Agent）已经能让任何开发者低成本生成行业插件——**私有插件包不再是稀缺资产**。行业 Know-How 的护城河从「代码」转移到「数据 / 服务 / 合规」，而不是「闭源代码」。

2. **「卖代码」的商业模式在 Rust 开源生态中不可持续**
   Rust 开发者默认要求开放源码。竞品（axum / actix-web / loco）全部开放，闭源企业版在开发者心智中是**负资产**——用户会直接选择生态更大的开放框架。企业版「未卖出过一分钱」就是最强证据。

3. **企业版的核心卖点与开源天然一致**
   原方案企业版卖点之一是「数据主权 + 自部署」（`product-technical-plan.md:47`）。但自部署和数据自控**本来就是开源就能提供的**。企业客户真正愿意付费的是「服务 / 保障 / 合规报告 / 迁移实施」，不是「代码本身」。

### 1.4 结论

> **企业版既没卖出过，又在 AI 时代失去了供给壁垒逻辑。合并为单一开源版本是成本更低、信任更高、生态更好的选择。**

---

## 二、合并目标与原则

1. **一个仓库**：本仓库（`github.com/ljclz/sz-rust`）作为唯一真源，企业版仓库转为只读归档。
2. **一个 workspace**：全部 crate 在同一 `Cargo.toml` members 中。
3. **一个版本号**：建议下一个版本 = **v2.0.0**（破坏性战略变更，符合 SemVer）。
4. **一套 CI/门禁**：27 个 job 扩展覆盖全部 crate；防幻影交付门禁覆盖所有代码。
5. **一种许可证**：全部 crate 统一 Apache-2.0（工具 crate 可 MIT）；删除商业许可。
6. **商业化转型**：从「卖代码」转向「卖服务与保障」（技术支持订阅 / 托管 / 合规报告 / 迁移服务）。

---

## 三、合并前现状盘点（2026-10-25 实测）

| 项 | 开源仓库（本仓库） | 企业版仓库（E:/www/rust/sz-rust-enterprise） |
|----|-------------------|---------------------------------------------|
| workspace members | 56 个（`Cargo.toml:3-76`） | 10 个（`Cargo.toml:3-14`） |
| 代表性 crate | sz-rust-core / ai-facade / capability / marketplace / addons-admin / addons-loader / 简化版 sz300 | 生产版 sz300 / addons-cms / crm / ecommerce / erp / forum / im / operate / sdd / migration |
| sz300 形态 | 简化版（CORS+CSRF+JWT，基础 CRUD；`packages/sz-rust-sz300/src/controllers/` 无 ai.rs/wasm_api.rs/graphql_api.rs/view.rs） | 生产版（含 `ai.rs`、`wasm_api.rs`、`graphql_api.rs`、`view.rs`，即 README 引用的「生产已接入」证据） |
| 依赖方式 | path / workspace 内部依赖 | 通过 crates.io `sz-rust-core = "1.2"` 拉取（`Cargo.toml:147`） |
| 许可证 | Apache-2.0 | LicenseRef-SZ-Commercial（`LICENSE-COMMERCIAL`） |
| CI | 27 jobs（`ci.yml`） | 独立 ci.yml / publish.yml |
| 隔离脚本 | scripts/audit/* | check_isolation.py / check_license_compliance.py / check_no_crate_level_allow_dead_code.py |

**关键冲突点：存在两个 `sz-rust-sz300`。** 合并时必须二选一（见 4.2）。

---

## 四、具体技术解决方案

### 4.1 仓库与目录合并

**目标结构**（在现有 `packages/` 基础上新增 10 个企业 crate，并保留唯一 sz300）：

```
packages/
├── sz-rust-core / sz-rust-ai-facade / ...   # 现有 56 个 member（含 marketplace/admin 等）
├── sz-rust-sz300                              # 以企业生产版为唯一 sz300
├── sz-rust-sdd                                 # SDD Agent 编排引擎（企业版移入）
├── sz-rust-migration                           # TP6 → sz-rust AI 迁移工具（企业版移入）
├── sz-rust-addons-cms / crm / ecommerce / erp / forum / im / operate
```

**实施步骤**：

1. 将企业版 `packages/` 下 10 个 crate 复制/迁移到本仓库 `packages/`。
   - 推荐方式：`git subtree add` 保留历史，或直接复制源码后以新提交入库（企业仓库保留为只读归档分支）。
   - 复制时排除 `uploads/`、`coverage-lib.*`、`uncovered-paths.json` 等运行时产物。
2. 将 10 个新 crate 加入根 `Cargo.toml` 的 `members`（合并后共 **65 个 member**：56 - 1 简化版 sz300 + 10 企业 crate）。
3. 清理全仓库所有 `E:/www/rust/sz-rust-enterprise` 引用（README.md / README.en.md / 审计报告 / 代码注释），改为仓库内相对路径：
   ```bash
   grep -rn "E:/www/rust" --include="*.md" --include="*.rs" --include="*.toml" .
   ```
4. 删除本仓库 git remote 中对企业版的引用（如有），GitHub 仓库成为唯一真源。

### 4.2 sz300 双版本合并（最关键决策）

| 方案 | 说明 | 推荐 |
|------|------|------|
| **A. 以生产版为唯一 sz300** | 保留企业生产版（含 ai/graphql/ws/wasm/view），删除 OSS 简化版；将简化版独有的安全资产（`security_gauntlet_tests.rs`、`schemas/openapi-sz300.json`、`scripts/audit/security-gauntlet.sh`、`examples/dump_openapi.rs`）**移植到生产版并重新验证** | ✅ 推荐 |
| B. 保留两个 sz300 | 命名冲突（同名 crate 不能共存），需改名，增加维护负担 | ❌ |

**实施要点**：
- 将 `packages/sz-rust-sz300/src/security_gauntlet_tests.rs`（26 项测试）移植到生产版，确认生产版路由/中间件/上传服务的鉴权语义下 26 项全部通过；不通过的用例按生产版实际行为修正断言（生产版 auth_middleware 与简化版不同）。
- 重新生成 `schemas/openapi-sz300.json`（`cargo run -p sz-rust-sz300 --example dump_openapi`），确认 paths/tags 与生产版一致。
- 简化版的基础 CRUD 示例能力由 `sz-rust-examples` 承担，无信息损失。

### 4.3 依赖解析统一（技术关键点）

现状：企业 crate 通过 `[workspace.dependencies]` 中的 `sz-rust-core = "1.2"`（`Cargo.toml:147`）从 crates.io 拉取开源框架。

合并后：**全部改为同一 workspace 的 path 依赖**，消除版本漂移：

```toml
# 根 Cargo.toml [workspace.dependencies]
sz-rust-core = { version = "2.0.0", path = "packages/sz-rust-core" }
sz-rust-ai-facade = { version = "2.0.0", path = "packages/sz-rust-ai-facade" }
sz-rust-capability = { version = "2.0.0", path = "packages/sz-rust-capability" }
# ... 其余 OSS crate 同理
```

企业 crate 的 `Cargo.toml` 保持 `.workspace = true` 写法不变，解析自动落到本地 path。这样：
- 合并后新增 crate 与既有 crate **共享同一份源码编译**；
- 不会再出现「开源框架发版了，企业版还在拉旧版本」的漂移；
- 防幻影交付门禁能覆盖到全部代码。

### 4.4 许可证统一

1. 删除企业版 `LICENSE-COMMERCIAL`，根 `LICENSE-APACHE` 作为唯一许可证。
2. 企业 crate 的 `license = "LicenseRef-SZ-Commercial"` → `Apache-2.0`（通过 `[workspace.package] license = "Apache-2.0"` 统一）。
3. `deny.toml` 许可证白名单已含 Apache-2.0 / MIT，无需改动（确认无 GPL 依赖混入）。
4. 全量检查无残留商业许可声明：
   ```bash
   grep -rni "LicenseRef-SZ-Commercial\|商业许可" --include="*.toml" --include="*.md" --include="*.rs" .
   ```

### 4.5 版本号统一

- 下一个发布版本 = **v2.0.0**。
- 所有 crate `version.workspace = true` 统一继承。
- `CHANGELOG.md` 顶部新增 v2.0.0 条目，记录本次合并与战略转向。
- git tag `v2.0.0`，同时清理/忽略旧 tag 的误导（README 版本声明以 v2.0.0 为准）。

### 4.6 CI/门禁统一

1. 企业版 `ci.yml` / `publish.yml` 的 job 并入本仓库 CI（现有 27 job 基础上扩展覆盖新 crate）。
2. 企业版 3 个隔离脚本并入 `scripts/audit/`：
   - `check_isolation.py`（企业版代码不得修改开源核心）
   - `check_license_compliance.py`（许可证头检查）
   - `check_no_crate_level_allow_dead_code.py`
3. 防幻影交付门禁（doc-code-consistency 等）现在**覆盖全部代码**——这是合并的最大收益：企业版代码从此受 27 道门禁 + 防幻影交付机制约束。
4. 修正 README 中 CI job 数字：`README.md:226` 的「23 个 job」改为实测值，并移除「无 continue-on-error」的错误声称（实际 3 处 `continue-on-error: true`，见 `ci.yml:199,278,377`）。
5. 将 `scripts/audit/security-gauntlet.sh` 设为 CI 阻塞步骤（先修复 4.8 的编译问题）。

### 4.7 功能默认值与生产模板

合并后的生产 sz300 保留 feature gate（`admin` / `otlp` / `qdrant` / `hot-reload` / `real-broker`），但补齐生产可用性：

1. 新增 `docker-compose.prod.yml` 生产部署模板：必选 `--features` 清单 + 环境变量（DB_POOL_* / SZ300_METRICS_TOKEN / SZ300_ENV=production 等）。
2. 默认安全基线：
   - `/metrics` 鉴权：已有 `metrics_token` WIP（`state.rs:17`），合并后完成并测试；
   - CORS 生产约束：支持 `SZ300_CORS_ORIGIN` 环境变量（修复架构评测 P2-8）；
   - 生产环境挂载 `panic_guard_middleware`（`2026-10-25-security-gauntlet.md` 建议项）。
3. v1.9.0 `all-v1-9` 的 10 大能力（`README.md:51-66`）评估后：有消费者的并入默认，无消费者的保留 feature gate 并在 README 标注「实验性」。

### 4.8 供应链与安全修复（合并后立即执行）

按 `docs/audit/2026-10-25-架构评测修复记录.md` 逐项闭环，其中 P1 按 P0 处理：

| # | 缺陷 | 修复 |
|---|------|------|
| 1 | cargo audit 35 条公告（zip critical / rsa / rustls / tokio） | `cargo update` 升级 rustls/tokio；升级 `utoipa-swagger-ui` / `rust_xlsxwriter` 消除 zip critical |
| 2 | `/metrics` 公开无鉴权 | 完成 `metrics_token` WIP（已开始） |
| 3 | CI audit non-blocking | 修复依赖后移除 `continue-on-error` |
| 4 | 安全 feature 默认全关 | 见 4.7 生产模板 |

**前置修复（阻塞项）**：当前工作区 `cargo test -p sz-rust-sz300 security_gauntlet` 编译失败（集成测试缺 `metrics_token` 字段，`tests/mqtt_dispatch_test.rs:246`、`tests/e2e_ota_test.rs:62`）。合并前必须先提交/修复该 WIP，否则安全 Gauntlet 不可复现。

### 4.9 文档统一

1. `README.md` / `README.en.md` 重写：
   - 删除「企业版 / 开源版」区分、🔒 标注、`E:/www` 引用；
   - 所有「✅ 生产已接入（企业版）」改为「✅ 生产已接入：`packages/sz-rust-sz300/src/...`」仓库内可验证引用；
   - 版本声明统一 v2.0.0；英文 README 与中文同步（解决 `README.en.md:7` v0.7.0 滞后）。
2. `docs/product-technical-plan.md`：新增「版本合并决策」章节，替代原「开源/企业分离」叙事；或在本方案评审通过后更新。
3. 新增 ADR-043「AI 时代单一版本合并」（草稿见附录 A）。
4. `docs/audit/README.md` 与审计报告索引同步更新。
5. 修正 `README.md:233` 关于 marketplace「已定性虚构交付」的过时声明（`packages/sz-rust-marketplace/` 已存在且有消费者 `sz-rust-cli/Cargo.toml:44`）。

---

## 五、商业化转型：不卖代码，卖什么

| 收入来源 | 说明 | 可行性 |
|----------|------|--------|
| **技术支持订阅** | 企业级 SLA、升级保障、安全补丁 | 高（企业客户习惯） |
| **迁移服务** | TP6→sz-rust 人工 + AI 辅助迁移项目（工具开源获客，实施服务变现） | 高（匹配最佳早期客户） |
| **安全与合规报告** | 定制渗透测试 / 审计报告 / 合规材料（依托防幻影交付 + 安全 Gauntlet 卖点） | 高（差异化强） |
| **托管服务** | 部署 / 运维 / 监控托管 | 中 |
| **插件市场服务** | 认证 / 托管 / 分发服务费（插件本身免费） | 低（需生态先起来） |
| **AI 调用分成** | 不推荐（利润薄、客户敏感） | 低 |

核心逻辑：**开源代码是获客漏斗，服务与保障是收入引擎。** 合并后「完全开源」本身就是信任资产，与「防幻影交付」卖点互相强化。

---

## 六、实施步骤与排期

| 阶段 | 内容 | 工期 |
|------|------|------|
| Phase 0 | 本方案评审 + ADR-043 + 更新 product-technical-plan | 0.5 周 |
| Phase 1 | 仓库合并：复制 10 crate + members 更新 + 依赖 path 化 + 许可证统一 | 1 周 |
| Phase 2 | sz300 合并：以生产版为唯一 sz300 + 移植安全测试/OpenAPI/脚本 | 1 周 |
| Phase 3 | CI 统一 + 隔离脚本并入 + 全量门禁修复（含 cargo audit、当前 WIP 编译修复） | 1 周 |
| Phase 4 | README / 英文 README / product-plan 重写 + 数字修正 + marketplace 声明更新 | 1 周 |
| Phase 5 | crates.io 全量发布 v2.0.0 + 公开文档站点 + docker-compose.prod 模板 | 2 周 |

---

## 七、验收标准（Checklist）

合并完成后必须满足：

- [ ] git 历史无企业版 remote；全仓库无 `E:/www/rust` 引用
- [ ] 根 `Cargo.toml` members = **65 个**，全部存在且可编译
- [ ] 全 workspace 无 `LicenseRef-SZ-Commercial` / 「商业许可」字样
- [ ] `cargo test --workspace` 全绿（含企业版迁移代码）
- [ ] `cargo clippy --all-targets` 0 warnings；`cargo fmt` 通过
- [ ] `cargo audit` 0 条公告（或已升级消除 critical）
- [ ] `bash scripts/audit/security-gauntlet.sh` 在干净 checkout 上 PASS（26 项）
- [ ] crates.io 全部 crate 发布为 v2.0.0
- [ ] README / README.en 版本一致，无「企业版」字样
- [ ] `/metrics` 鉴权、CORS 生产约束、panic_guard 默认挂载已落地并有测试

---

## 八、风险与应对

| 风险 | 应对 |
|------|------|
| 合并后全量门禁重跑工作量大（65 crate 测试/覆盖率/变异） | 分批合并：先 sz300 + sdd + migration，再行业插件包 |
| 生产版 sz300 的 26 项安全测试可能需要适配（鉴权语义不同） | 以生产版实际行为为准移植并重新验证，修正断言而非反向改代码 |
| 企业版代码质量未经过 OSS 门禁洗礼，可能有一批门禁失败 | 这正是合并的价值：让门禁把历史债务暴露并清零 |
| 「高级功能免费」可能让早期付费预期落空 | 定位转向「完全开源 + 企业服务」，服务即收入 |
| 行业插件被竞品复制（AI 时代不可避免） | 靠服务 / 品牌 / 数据资产 / 合规能力取胜，而非代码封闭 |

---

## 附录 A：ADR-043 草稿

```markdown
# ADR-043：AI 时代单一版本合并（废除企业版）

- 状态：已接受（2026-10-25）
- 背景：原「开源版 + 企业版」分离基于「行业插件稀缺可售卖」假设；AI 编程工具抹平插件供给壁垒，
  企业版从未产生外部收入，且双仓库导致文档撕裂、门禁覆盖不全、证据链不可达（E:/www 本地路径）。
- 决策：所有版本合并为单一开源版本（One Version，v2.0.0），全部 crate 统一 Apache-2.0；
  商业化转向服务与保障（技术支持订阅 / 迁移服务 / 安全合规报告 / 托管）。
- 影响：
  - 正面：门禁覆盖全部代码；文档单一可信源；开源获客 + 服务变现；
  - 负面：失去「企业版」收费叙事；合并初期全量门禁重跑工作量；
- 关联：docs/product-technical-plan.md、docs/single-edition-merge-plan.md
```

## 附录 B：仓库合并命令参考（Git Bash）

```bash
# 1. 复制企业版 crate 到本仓库（保留 git 历史用 subtree，或直接复制）
git subtree add --prefix=packages/sz-rust-sdd <enterprise-repo-path> main --squash
# 其余 crate 同理；或直接复制：
cp -r /e/www/rust/sz-rust-enterprise/packages/sz-rust-migration packages/

# 2. 将新 crate 加入根 Cargo.toml members（手工编辑）

# 3. 依赖 path 化：根 Cargo.toml [workspace.dependencies] 补 path 字段

# 4. 许可证统一
rm /e/www/rust/sz-rust-enterprise/LICENSE-COMMERCIAL   # 归档仓库保留，本仓库不复制
grep -rni "LicenseRef-SZ-Commercial" --include="*.toml" --include="*.md" packages/ | wc -l  # 应为 0

# 5. 清理本地路径引用
grep -rn "E:/www/rust" --include="*.md" --include="*.rs" --include="*.toml" . | wc -l  # 应为 0

# 6. 全量验证
cargo check --workspace
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all  # 注意：逐包 fmt，避免命令行超长（os error 206）
```