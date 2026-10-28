# ADR-043：AI 时代单一版本合并（废除企业版）

- **状态**：已接受（2026-10-25）
- **决策人**：产品负责人（用户）
- **关联**：`docs/single-edition-merge-plan.md`（完整方案）、`docs/product-technical-plan.md`（原权威方案，其「开源/企业分离」章节被本决策取代）、ADR-028（原开源/企业版物理分离决策，被本决策逆转）

---

## 背景

ADR-028（2026-09-05）将 workspace 拆分为开源仓库（29 crate）与企业版仓库（`E:/www/rust/sz-rust-enterprise`，10 crate，LicenseRef-SZ-Commercial），假设「行业插件稀缺可售卖、企业级功能可收费」。

该假设在 AI 时代失效，且分离模式产生了真实成本：

1. **AI 抹平插件供给壁垒**：AI 编程工具让任何开发者可低成本生成行业插件，私有插件包不再是稀缺资产。
2. **企业版零收入**：企业版从未产生外部收入；GitHub 0 stars / 0 forks / 0 issues（2026-10-25 API 实测），无市场验证。
3. **文档撕裂与证据链不可达**：README 大量引用 `E:/www/...` 本地路径，「生产已接入」证据外部用户无法核验；中英文 README 版本声明互相矛盾。
4. **门禁覆盖不全**：防幻影交付门禁（CI doc-code-consistency 等）管不到企业版仓库。
5. **双仓库维护成本**：依赖版本漂移（企业版锁 sz-rust 1.2 / sz-orm 5.0，开源已到 1.7 / 10.12）。

## 决策

1. **所有版本合并为单一开源版本（One Version）**：企业版 10 crate 能力并入本仓库（合并后 65 members），下一发布版本 **v2.0.0**。
2. **许可证统一 Apache-2.0**：删除 LicenseRef-SZ-Commercial；全仓库源码 SPDX 头统一。
3. **企业版仓库转为只读归档**：本仓库成为唯一真源。
4. **sz300 以本仓库版本为唯一实体**：生产级能力（限流/熔断中间件链、AI chat/embed/stream 端点、WASM 执行、视图渲染）移植进本仓库 sz300，使「生产已接入」声明在本仓库内可验证。
5. **商业化转向服务**：技术支持订阅 / TP6 迁移服务 / 安全合规报告 / 托管；不再卖代码。

## 影响

### 正面

- 防幻影交付门禁覆盖全部代码（企业版代码首次纳入 27 job CI）。
- 文档单一可信源，消除 `E:/www` 引用与「企业版」叙事。
- 「完全开源」成为信任资产，与安全 Gauntlet 卖点互相强化。
- 迁移工具（sz-rust-migration）开源后成为获客漏斗。

### 负面 / 风险

- 失去「企业版收费」叙事——但该叙事从未产生收入。
- 合并初期全量门禁重跑工作量（已在本仓库完成：65 members cargo check 全过、sz300 全量测试绿、变更 crate clippy 零警告）。
- 行业插件可能被竞品复制——以服务 / 品牌 / 数据资产应对。

## 实施记录（2026-10-25 本会话完成）

| 步骤 | 结果 |
|------|------|
| 复制 9 个企业 crate（7 addons + sdd + migration）入 packages/ | ✅ 完成 |
| 根 Cargo.toml members 56 → 65 + workspace.dependencies 补录 | ✅ 完成 |
| sz300 移植：ai/wasm/view 控制器 + 限流/熔断生产中间件链 + AI 初始化（SZ300_AI_API_KEY） | ✅ 完成 |
| 许可证统一：119 个文件 SPDX 头 LicenseRef-SZ-Commercial → Apache-2.0 | ✅ 完成 |
| 集成测试修复：AppState.metrics_token 12 处 + mock_app_state + hot_path bench | ✅ 完成 |
| 验证：`cargo check --workspace` 全过；sz300 lib 87 passed（含 26 项安全 Gauntlet）；变更 crate clippy -D warnings 零警告 | ✅ 完成 |
| 死代码处理：sz-rust-sdd visual_adapter（引用未实现 API 的 WIP）移出编译，文件保留 | ✅ 完成 |
| 文档：README / CHANGELOG / 本 ADR / docker-compose.prod.yml | ✅ 完成 |

## 未完成（需外部条件）

- **crates.io 全量发布 v2.0.0**：`node publish-crates.js`（dry-run，2026-10-25 实测）给出阻塞链：
  1. sz-rust 2.0.0 全部 65 crate 依赖 sz-orm **10.12.0**（本地 path `../sz-orm`），而 crates.io 线上最新为 **10.7.0**——须先在 sz-orm 仓库按其发布流程将 17 个包发布至 10.12.0（另一仓库的不可逆操作）；
  2. 发布前必须先 commit 本合并的约 100 文件（溯源铁律 23：crates.io 产物须对应 commit SHA；`cargo publish` 对 dirty 工作区需 `--allow-dirty`，不建议）；
  3. `publish-crates.js` 中硬编码的 sz-orm 期望版本（2.1.0）已过时，发布前需更新为 10.12.0。
  发布管线本身已验证可用：`cargo publish --dry-run -p sz-rust-macros` 成功打包（v2.0.0），凭据存在（`F:/cargo-home/credentials.toml`），cargo 可访问 crates.io。
- **公开文档站点（GitHub Pages）**：需要仓库远端操作权限。
- **cargo audit 公告的升级修复**：`.cargo/audit.toml` 已建立带理由的忽略清单（rustls-webpki 系列 / rustls 0.23.x / zip 2.4.2 写场景无解压攻击面），CI audit 已改为阻塞；待上游发布兼容版本后逐项消除忽略项（见 `docs/audit/2026-10-25-架构评测修复记录.md` 第 1 项）。
