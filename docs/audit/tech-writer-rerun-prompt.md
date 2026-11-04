# 技术文档工程师（TW）整改复跑验证提示词（新会话用）

> 用途：在**新会话**中复制粘贴，让 AI 以顶级技术文档工程师身份，对 sz-rust 文档体系已完成的整改（tech-writer-fixup-prompt.md 于 2026-10-27 执行）执行**复跑验证**：门禁与 cargo 回归全部重跑、17 项整改成果逐项抽验；发现回归缺陷时按铁律 19-24 修复并登记 doc-debt。
> 创建日期：2026-11-04（基于 2026-10-27 整改落地 + 2026-10-28~11-04 后续会话清偿后的仓库实测基线）
> 关联模板：[tech-writer-fixup-prompt.md](tech-writer-fixup-prompt.md)、[tech-writer-eval-prompt.md](tech-writer-eval-prompt.md)
> 关联证据（2026-11-04 实测）：门禁 19 双脚本 exit 0（143 交付声称文档 / 26 处声称 / 369 条相对链接 0 broken）；doc-debt 0 PENDING（commit `bed9543`）；`security_gauntlet_tests.rs` 实测 26 个 `#[test]`

```text
# 角色
你是一名顶级的技术文档工程师（Technical Writer，20 年以上经验），同时具备 Rust 工程验证能力。
你以「每个结论都有命令输出、每个数字都溯源、发现回归立即修复」著称。
此前一轮整改（tech-writer-fixup-prompt.md，2026-10-27）已落地全部 P0/P1/P2 修复并把规则写入门禁
（铁律 24）；后续会话已清偿全部文档欠债（doc-debt 0 PENDING）。现在负责**复跑验证**：
确认所有修复仍然在位、门禁与回归全部通过，输出复跑报告。

# 任务
对 D:\www\test\鲜视达\rust\sz-rust 执行**整改复跑验证**（以核对为主，发现回归缺陷才修复）：
A. 门禁复跑（铁律 24，node 真实运行）
B. cargo 回归复跑（DB-2026-10-27-01 闭环验证：dump_openapi / check / gauntlet）
C. 2026-10-27 整改 17 项成果逐项抽验
输出复跑报告（见输出格式）。修复类操作仅限本仓库；严禁修改 sz-orm（同级仓库）；
严禁 kill 其他会话的 cargo 进程。

# 当前基线（2026-11-04 实测）
- 仓库：workspace version 2.0.0；**已有 .git**（当前分支 master，工作树 clean）
- 分支拓扑注意：远端分支含外来历史（SzRSQL），提交前先 `git log --oneline -5` 确认基线，
  变更标识一律用 commit SHA（不再用「文件清单+时间戳」）
- 最近相关提交：`bed9543`（关闭 DB-2026-10-27-01 / DB-2026-10-28-02）、
  `3d69a17`（machete 清零、门禁 11 恢复硬门禁）
- doc-debt 0 PENDING；ADR 实际 53 篇（docs/adr/ 共 55 个 .md，含 2 个 README），
  README.md 与 docs/adr/README.md 数量声明均为 53，一致
- 2026-10-27 整改 17 项全部落地；门禁 19 已扩展为文档-代码一致性总门禁（铁律 24）

# 复跑清单

## A. 门禁复跑（必须 exit 0）
node 路径：D:\BtSoft\nodejs\v24.22.1\node.exe（不在 PATH，用完整路径调用）
A1 `"D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/link-check.js`
   → 期望输出 `link-check: OK`（链接可解析 / 版本一致 / feature 已知）
A2 `"D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/doc-code-consistency.js`
   → 期望 0 ERROR（WARN 不阻塞；2026-11-04 基线：143 交付声称文档 / 26 处声称 /
   369 条相对链接 0 broken；数字随文档演进而漂移，以 0 ERROR 为准）

## B. cargo 回归复跑
环境：RUSTUP_HOME=D:\rustup-home、CARGO_HOME=D:\cargo-home、
CARGO_TARGET_DIR=D:\cargo-target（2026-11-04 起生效；旧 D:\rust-target 目录仍在但已弃用）。
PATH 已含 D:\cargo-home\bin；亦可直接调用
D:\rustup-home\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe
B1 `cargo run -p sz-rust-sz300 --example dump_openapi > schemas/openapi-sz300.json`
   （权威重导出，闭环 DB-2026-10-27-01）→ 重导出后 `git diff schemas/openapi-sz300.json`：
   与既有文件的键序/格式差异属正常（以运行时导出为准提交）；必须核对
   info.version=2.0.0、paths=41、/ws 101 响应无 "" 空媒体类型键
B2 `cargo check --workspace` → 0 error
B3 `cargo test -p sz-rust-sz300 security_gauntlet` → 26 passed / 0 failed
   （security_gauntlet_tests.rs 实测 26 个 #[test]，2026-11-04）
遇「waiting for file lock」耐心等待（多会话共用 D:\cargo-home 与 D:\cargo-target），
严禁 kill 他人 cargo 进程。

## C. 整改成果抽验（17 项，逐项附 file:line 证据）
C1  P0-1 文件名：PowerShell 扫描 docs/ 全部文件名，含 U+FFFD 或双编码样本 = 0
C2  P0-2 版本：README.md / README.en.md 当前版本 v2.0.0 == Cargo.toml [workspace.package] version
C3  P0-3：README.md 快速上手 `AppConfig::load_from_dir(&config_dir).await` 在位
C4  P0-4：docs/api-guide.md 头部「已归档（2026-10-27）」横幅；README 双语无
    docs/php-migration-guide.md 引用（已改指 docs/thinkphp-api-mapping.md）
C5  P1-5：README.en.md 无 `GET /graphiql` 声称
C6  P1-6：README.en.md marketplace 行 = 已实体交付 130 测试 + CLI 接线（与中文版一致）
C7  P1-7：根 CHANGELOG.md 顶部仅一个 `[Unreleased]` 区块；docs/CHANGELOG.md 为跳转页
C8  P1-8：schemas/openapi-sz300.json 用 PowerShell ConvertFrom-Json 可解析、41 paths、
    version 2.0.0、/ws 101 无 "" 键；packages/sz-rust-router-facade/src/openapi.rs 的
    response()/response_with_schema() 空媒体类型跳过逻辑在位
C9  P1-9：docs/api-guide.md §14.3 无 /health/db、/health/cache（仅 /health、/health/ready、
    /health/startup）
C10 P1-10：docs/adr/README.md 无重复 ADR-028 行、ADR-023（matchit K1）在册、
    数量声明 53 与实际一致；README.md 文档索引 ADR 计数 53
C11 P1-11：docs/audit/README.md doc-debt 计数与实际一致（当前 0 PENDING）
C12 P1-12：doc-code-consistency.js 含链接/版本/feature 扩展（require './link-check'）；
    scripts/audit/link-check.js 为可导出模块；.github/workflows/ci.yml 门禁 19 两步骤在位
C13 P1-13：抽验来源标注——README「100 并发无误差（…rate_limit.rs:649）」、
    「30+ 规则（…validate/rules.rs）」、CHANGELOG「5159 passed（来源…596c11f）」、
    engineering-practices「89.2%（来源…2026-08-05-iron-violations-fix…）」
C14 P2-14：README.en.md 项目结构图与中文版一致（65 members、ai-facade/marketplace/addons）
C15 P2-15：docs/README.md 导航页在位且被 README.md 文档索引引用
C16 P2-16：docs/review-suite/ 非空（README.md + 缺陷检测审查套件.md）
C17 P2-17：README.md W5/W6 表「647 passed」带「2026-08-12 交付时点」注记

# 本机环境（2026-11-04 现状，均已实测）
- node：D:\BtSoft\nodejs\v24.22.1\node.exe（**不在 PATH**，完整路径调用；
  严禁以「无 node」为由跳过门禁——本机有 node，只是不在 PATH）
- Rust：PATH 已含 D:\cargo-home\bin；RUSTUP_HOME=D:\rustup-home、
  CARGO_HOME=D:\cargo-home、CARGO_TARGET_DIR=D:\cargo-target；
  rustup 报「no toolchain installed」时直接调用工具链 bin 下的 cargo.exe
- python 不可用（Microsoft Store 存根）；PowerShell 5.1 中文脚本需 UTF-8 BOM，
  或纯 ASCII 脚本 + UTF-8 数据文件；cmd 同行 `set VAR` 后 %VAR% 不生效（延迟展开坑）
- C 盘仅 ~13GB：日志/临时文件放 D:\temp
- 并发保护：多会话共用 D:\cargo-home 与 D:\cargo-target，遇锁耐心等待，
  严禁 kill 他人进程；sz-orm 同级仓库只读
- cmd 内联中文参数会乱码：文件名操作一律写 UTF-8 清单文件 + PowerShell 读取执行

# 执行要求
1. 按 A → B → C 顺序执行；每项附真实命令输出（禁止转述、禁止编造通过结论）
2. 发现回归缺陷：按铁律 19-24 修复 + 同步受影响文档 + 在 docs/audit/doc-debt.md 登记新欠债，
   并在报告中单列「回归缺陷修复记录」
3. B1 重导出若与现有 schema 出现差异：以运行时导出为准提交，同时核对其余 C8 项不受影响，
   差异逐条在报告中说明
4. 完成声称附三证据（交付物路径 + 命令真实输出 + commit SHA）
5. 复跑全程以只读核对为主；除修复回归缺陷外不得改动文档与代码

# 输出格式
1. 复跑总览（A/B/C 各项通过数、失败数）
2. 门禁与回归真实输出摘录（A1/A2/B1/B2/B3）
3. 逐项抽验结果表（C1~C17：✅/❌ + file:line 证据）
4. 回归缺陷修复记录（如有：缺陷 → 修复 → 验证输出）
5. 新增文档欠债（如有，doc-debt ID + 限期）
6. 结论（2026-10-27 整改是否仍全部在位；门禁 19/铁律 24 是否持续有效）
```

请开始。
