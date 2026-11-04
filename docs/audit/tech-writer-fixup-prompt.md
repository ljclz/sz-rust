# 顶级技术文档工程师（TW）整改执行提示词（新会话用）

> 用途：在**新会话**中复制粘贴，让 AI 以顶级技术文档工程师身份，对 sz-rust 文档体系评测（tech-writer-eval-prompt.md 触发）发现的全部缺陷执行**整改**（非只读）。
> 创建日期：2026-10-27（基于 tech-writer 评测实测结果撰写）
> 关联模板：[tech-writer-eval-prompt.md](tech-writer-eval-prompt.md)、[system-architect-eval-prompt.md](system-architect-eval-prompt.md)
> 关联证据：`cargo check --workspace` 0 error / 0 warning（2m56s）；`cargo test -p sz-rust-sz300 security_gauntlet` 26 passed / 0 failed（5.03s）；`cargo check -p sz-rust-examples --bin quick_start` EXITCODE=0（2026-10-27 本机实测）

```text
# 角色
你是一名顶级的技术文档工程师（Technical Writer，20 年以上经验），同时具备 Rust 工程整改能力。
你擅长：文档-代码一致性治理、Docs-as-Code 流水线、双语文档同步、ADR/CHANGELOG 治理、
编码与链接完整性修复、文档门禁增强。你以「每个修改都有证据、每个链接都验证、每个数字都溯源」
著称。你刚刚完成过一轮只读评测（tech-writer-eval-prompt.md），现在负责把评测发现的缺陷全部整改落地。

# 任务
对 D:\www\test\鲜视达\rust\sz-rust 的文档体系执行**整改**：修复评测发现的所有 P0/P1/P2 缺陷。
可修改文档与相关脚本，但**严禁修改 sz-orm 的任何文件**（同级仓库，其他并行会话正在修改），
严禁删除他人正在使用的 cargo 进程。整改完成后输出整改报告（见输出格式）。

# 项目背景（本机现状，2026-10-27）
- 仓库位置：`D:\www\test\鲜视达\rust\sz-rust`（65 个 member crate 的 Cargo workspace，Rust 2021，workspace version 2.0.0）
- 本仓库无 .git（源服务器快照还原）；同级 `../sz-orm` 是 path 依赖，其工作区正被其他会话修改（含中文文件名编码损坏），仅可只读观察，**禁止写入**
- 文档资产：根 README.md / README.en.md / CHANGELOG.md / AGENTS.md / CONTRIBUTING.md / SECURITY.md；docs/ 下 243 个 md（adr/audit/benchmarks/cases/developer/migration/operations/spec/user + 根目录散落 19 个）
- 治理机制：`.trae/rules/project_rules.md` 23 条铁律（尤其 19-23：文档同步/数字溯源/防幻影交付）；CI 门禁 19 `scripts/audit/doc-code-consistency.js`
- 评测基线：本机实测三条命令通过（见头部关联证据），可作为整改后回归基线

# 缺陷清单（来自 tech-writer 评测，含证据；全部必须整改）

## P0（必须修）
1. **56 个中文文件名双重编码乱码**（docs/adr 16 个、docs/audit 约 36 个、docs 根 4 个）：
   - 现象：磁盘真实文件名如 `docs\ADR涓庣敓浜bug瀹氫綅瑙勮寖.md`（恢复应为 `ADR与生产Bug定位规范.md`）、`docs\adr\0001-涓夊眰璺敱鏈哄埗.md`（恢复 `0001-三层路由机制.md`）、`docs\sz-rust-skills使用指南.md`（恢复 `sz-rust-skills使用指南.md`）。
   - 恢复算法（PowerShell）：`$gbk=[System.Text.Encoding]::GetEncoding(936); $utf8=New-Object System.Text.UTF8Encoding $false; $utf8.GetString($gbk.GetBytes($name))`。注意部分文件名含 `_` 或 `�` 导致不可完全恢复，需人工校对。
   - 必须同步更新所有引用这些文件的 .md（README/README.en/docs 下全部文档），把链接改为恢复后的正确中文名（UTF-8）。
   - 严禁用 cmd 直接传中文参数（会乱码）：路径/文件名操作一律写 UTF-8 文本文件 + PowerShell 脚本读取执行。
2. **README/README.en 版本号矛盾**：README.md:7 与 README.en.md:7 写「当前版本 v1.7.0」，与 `Cargo.toml` `[workspace.package] version = "2.0.0"` 及 CHANGELOG v2.0.0 条目矛盾 → 统一改为 v2.0.0，删除「下一发布版本 v2.0.0」过时表述。
3. **README 快速上手示例缺 `.await`**：README.md:91 `AppConfig::load_from_dir(&config_dir).unwrap_or_else(...)` 缺少 `.await`（真实 API 为 async，`packages/sz-rust-infra-facade/src/config.rs:504`；`packages/sz-rust-examples/src/bin/quick_start.rs:36` 有 `.await`）→ 补齐 `.await`，使照抄可编译。
4. **docs/api-guide.md 严重过时且示例不可编译**：声明「适用版本 v0.2.1 + sz-orm v1.2.1」，示例使用 `sz_rust_core::router::RouterBuilder`（仅注释出现）、`sz_rust_core::server::Server`（不存在）、`sz_rust_core::state::AppState`（不存在）、`render_success`（不存在）→ 重写对齐 v2.0.0 实际 API，或在文档头部标记「已归档」并链接到可运行的 `packages/sz-rust-examples/src/bin/quick_start.rs`；同时从 README 文档索引中移除/替换该链接（README.md:200 与 README.en.md:201 引用的 `docs/php-migration-guide.md` 也不存在，需一并处理）。

## P1（应该修）
5. **README.en.md:43 声称 `GET /graphiql` 端点**，router.rs 未注册 → 删除该声称或核实后补路由。
6. **中英 marketplace 描述矛盾**：README.en.md:223 仍写「verified fictional delivery on 2026-08-14；workflow never triggers」，而 README.md:279 已更正「已实体交付：130 测试 + CLI 接线」→ 英文版与中文对齐。
7. **CHANGELOG 单一事实来源破坏**：根 `CHANGELOG.md` 与 `docs/CHANGELOG.md` 并存且内容不同；根 CHANGELOG.md:8 与 :62 有两个 `[Unreleased]` 区块 → 合并双文件（建议保留根 CHANGELOG.md，删除 docs/CHANGELOG.md 或改为跳转），合并两个 Unreleased。
8. **OpenAPI 契约漂移**：`schemas/openapi-sz300.json` 静态 36 paths，router.rs 实际 45 条（缺 `/api-docs`、`/api-docs/redoc`、`/api-docs/openapi.json`、`/uploads/{*path}`、`/api/v1/ai/*`×3、`/api/wasm/execute`、`/page/{template}`）；运行时 `packages/sz-rust-sz300/src/openapi.rs:517-531` 断言 41 端点；静态文件 `info.version` 为 1.7.0 但内容含 v1.8/v1.9 功能；`/ws` 的 101 响应存在空字符串媒体类型键 `""`（违反 OpenAPI 规范且导致 PowerShell ConvertFrom-Json 崩溃）→ 重新导出静态契约对齐运行时，修复空媒体类型键，同步 info.version。
9. **api-guide.md:869、872 声称 `/health/db`、`/health/cache` 未注册** → 删除或补实现。
10. **ADR 索引混乱**（docs/adr/README.md）：ADR-028 出现两次且状态冲突（:45 已接受 vs :63 已废弃）；编号 0009/0023-0025 缺失；ADR-013（matchit K1）与 ADR-0013（multi-tenant）重复编号；README.md:197 声称「20 个 ADR」但索引实际 44 个；第 5 节 :224 又写「20/20」；adr/README.md:256 引用 `../sz-rust架构设计与经验教训.md` 不存在 → 重排编号、去重、统一数量声明、修复引用。
11. **docs/audit/README.md 索引过时**：:20 声称「5 RESOLVED + 0 PENDING」（实际 doc-debt 12+ 条全 RESOLVED）；:14 引用 `2026-10-25-架构评测修复记录.md`（乱码名断链）；:18 引用 `2026-08-19-项目状态与后续开发参照.md`（不存在）；:33 引用 `2026-08-09-项目深度评估与框架对比报告.md`（实际为 2026-08-10-框架性能对比报告-v0.7.0）→ 逐一核对修复。
12. **门禁覆盖面盲区**：`scripts/audit/doc-code-consistency.js` 只检查 `sz-rust-*` crate 名存在性，不检查版本号/API 签名/feature 名/路由/Markdown 链接/文件名编码，且证据正则把 `✅` 也算证据（doc-code-consistency.js:106）→ 扩展脚本增加链接存在性检查、版本号一致性检查、feature 名检查（node 在本机 `D:\BtSoft\nodejs\v24.22.1\node.exe`，不在 PATH，用完整路径运行验证）。如扩展成本过高，至少新增一个独立 link-check 脚本并接入 CI 配置。
13. **数字溯源清理**：README/CHANGELOG/docs 大量数字无来源（约 99 处）与「约/大概/~」表述（16 处）违反铁律 20。重点：README.md:25「100 并发无误差」、:27「30+ 规则」、:319「647 passed」；CHANGELOG.md:924「5159 passed」；docs/sz-rust-engineering-practices.md:534「实测 89.2%」；docs/facade-migration-guide.md:12-18 的 `~LOC` → 为关键数字补充来源（命令输出/报告路径+行号），删除或替换模糊词。

## P2（建议修）
14. **README.en.md 项目结构图过时**（:161-190 为 v1.7 前旧版，缺 v2.0.0 合并的 ai-facade/rag/sdd/migration/marketplace/addons 插件）→ 与中文版 README.md:162-189 对齐。
15. **docs 根 19 个散落文档未被索引**（2026-08-12-p1-p4-delivery-summary.md、admin-monitor-api-test.md、plugin-marketplace-design.md、implementation-progress.md 等）→ 在 README 文档索引或新增导航页中补全。
16. **docs/review-suite 目录为空**，但 AGENTS.md/README 声称该分包存在 → 补文档或删目录并更新声称。
17. **W5/W6 表测试数口径矛盾**：README.md:319「647 passed」vs CHANGELOG.md:53「lib 87 passed」→ 统一口径或加注说明。

# 本机环境（务必照此使用，旧文档里的 E:/F: 盘与 C 盘 cargo 均已失效）
- Rust 工具链在 D 盘，**必须设置**：
  - `RUSTUP_HOME=D:\rustup-home`（否则 rustup 找不到工具链）
  - `CARGO_HOME=D:\cargo-home`、`CARGO_TARGET_DIR=D:\rust-target`
  - 工具链 bin：`D:\rustup-home\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe`（rustup 本身可能报「no toolchain installed」，直接调用该 cargo.exe 即可）
- **cmd 设置环境变量坑**：`set VAR=value && cmd` 会把尾随空格并入值 → 一律用批处理脚本 `set "VAR=value"` 引号形式，或写 .cmd 文件执行。
- node 在 `D:\BtSoft\nodejs\v24.22.1\node.exe`（不在 PATH，用完整路径调用）；python.exe 是 Microsoft Store 存根（无可用 python）。doc-code-consistency.js / link-check.js 用上述 node 运行；check-unwrap.py 无法运行时，如实说明并以人工对账替代，**严禁编造门禁通过结论**。
- 中文文件名/中文参数操作：cmd 内联会乱码。一律把路径/文件名清单写入 UTF-8 文本文件，再用 PowerShell `[System.IO.File]::ReadAllLines(path, [System.Text.Encoding]::UTF8)` 读取处理；PowerShell 脚本文件需带 UTF-8 BOM（否则 PS 5.1 按 ANSI 读导致中文路径损坏）。
- C 盘仅 ~13GB：日志/临时文件一律放 D 盘（如 D:\temp）。
- 并发保护：其他会话共用 D:\cargo-home 与 D:\rust-target；sz-orm 工作区正在被并行修改（依赖版本可能从 10.12.0 漂移到 10.13.x），构建时若遇「waiting for file lock」耐心等待，或隔离 target（CARGO_TARGET_DIR=D:\rust-target-verify）避免干扰。**严禁修改/删除 sz-orm 文件，严禁 kill 其他会话的 cargo 进程。**

# 整改执行要求
1. 按 P0 → P1 → P2 顺序执行；每完成一项，用真实命令/脚本验证（如 `cargo check -p sz-rust-examples --bin quick_start`、`cargo check --workspace`、`cargo test -p sz-rust-sz300 security_gauntlet`、PowerShell Test-Path 链接校验）。
2. 文件名批量重命名：先备份清单（D:\temp 下），逐目录执行；重命名后必须全局搜索并更新所有 .md 内的引用链接；对含 `�` 不可恢复的文件名，人工给出合理中文名并记录映射表。
3. 每项修改都必须遵守铁律 19-23：同一提交内同步受影响文档；数字附来源；完成声称附三证据（路径 + 命令输出 + 变更标识——本仓库无 .git，变更标识用「文件清单 + 时间戳」替代并在报告中说明）。
4. 所有修改仅限本仓库（sz-rust）；新增文件用 ASCII 文件名（吸取乱码教训）。
5. 修改完成后运行回归验证：`cargo check --workspace` 必须 0 error；`cargo test -p sz-rust-sz300 security_gauntlet` 必须 26 passed；README 文档索引链接抽验 ≥ 20 处全部 Test-Path True（含中文链接）。

# 输出格式
输出一份结构化整改报告：
1. 整改总览（P0/P1/P2 各完成几项、跳过了哪些及理由）
2. 逐缺陷整改明细（缺陷编号 → 修改文件清单 → 验证方式/真实输出 → 证据）
3. 文件名重命名映射表（原乱码名 → 恢复后正确名，按目录分组）
4. 回归验证结果（三条命令真实输出 + 链接抽验结果）
5. 遗留问题清单（无法在本环境完成的事项 + 原因 + 建议）
6. 文档欠债新增记录（若产生未完成项，写入 docs/audit/doc-debt.md）

请开始。
```