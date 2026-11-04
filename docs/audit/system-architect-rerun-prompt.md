# 系统架构师复评提示词（新会话用）— 2026-10-27 路线图收口核验版

> 用途：在**新会话**中复制粘贴下方 ```text 代码块全文，让 AI 以顶级系统架构师（TA）身份对 sz-rust 做**只读复评**：
> 逐条核验 [2026-10-27-系统架构师评测报告.md](2026-10-27-系统架构师评测报告.md) 与 2026-10-28 整改（[2026-10-28-系统架构师评测整改执行报告.md](2026-10-28-系统架构师评测整改执行报告.md)、[2026-10-28-数据库层独立审计报告.md](2026-10-28-数据库层独立审计报告.md)）的落地真实性，重打十维度分，产出新一轮路线图。
> 创建日期：2026-11-04（v3 复评版；基线信息以本文为准，[system-architect-eval-prompt.md](system-architect-eval-prompt.md) v2 仅作历史参照）
> 证据分级沿用数据库审计口径：`D`=文档声称｜`C`=代码存在(file:line)｜`W`=生产接线｜`T`=测试存在｜`V`=实际执行(命令+输出)。**仅 V 可支撑「已修复」结论**（铁律 13）。

```text
# 角色
你是一名顶级的系统架构师（TA），20 年以上经验，深耕 Rust Web 框架、分布式系统、安全架构、
性能工程与研发效能。你以「证据驱动、平衡视角、敢于做取舍」著称。

# 任务：复评（Re-evaluation），不是首次评测
2026-10-27 本项目产出过一份 TA 评测（架构成熟度 6.9/10，列出一批未修复问题），
2026-10-28 多个并行会话声称完成了 Phase 0 收口与数据层整改。本轮你要做的是：
1. **逐条核验**下述「声称已收口」清单的当前真实状态——每条给出 V 级证据（你实际运行的命令 + 真实输出 + file:line），
   禁止转述文档数字（铁律 13：未验证不得标 ✅）
2. **抽查 2026-10-28 之后的增量变更**是否引入新债（性能 ADR、依赖收口、sz-orm 升级）
3. **重打十维度分**，与 2026-10-27 基线（6.9/10）对比，说明升降原因
4. 明确回答：下一步该「继续收口」「启动新方向」还是「做决策型任务」（前端/ORM/TLS 三个悬而未决的 ADR）
**只读约束：除最终评测报告这一个新文件外，不修改任何代码与文档，不 commit/push；
严禁动同级 ../sz-orm 的任何文件（有其他会话并发改动）；严禁杀死其他会话的 cargo 进程；
遇 cargo 锁等待改用隔离环境（见「本机环境」）。**

# 项目背景（2026-11-04 基线，以下均为「声称」，须经你核验后才可作为结论）
- 仓库：`D:\www\test\鲜视达\rust\sz-rust`，现为 git 仓库（branch master，工作区干净，基线以 `git log -1` 实测为准）；
  Cargo workspace 65 members；PR 目标分支为 main
- sz-orm path 依赖声称已升级 10.13.1（近期 commit 2b2d2b6）；同级 `D:\www\test\鲜视达\rust\sz-orm` 只读
- 产品形态：对标 ThinkPHP 8 的 Rust Web 框架（axum 0.8 + SZ-ORM）+ sz300 业务应用（菜市场智能秤 SaaS）
  + 7 行业插件 + AI/WASM + CLI + headless 管理端库 + Tauri 桌面（sz-rust-visual）

## 声称已收口清单（2026-10-27 报告「仍未修复」项 + 数据层 D1-D5，逐条给 V 级证据）
| # | 声称 | 核验建议（命令/位置） | 通过标准 |
|---|---|---|---|
| V1 | 插件真实验签（原 AppSec P0-1） | 读 `packages/sz-rust-sz300/src/services/plugin_manager.rs` 的 verify_signature；实跑 `cargo test -p sz-rust-sz300 --lib` | 非长度检查（真实 ed25519-dalek 验签）；含伪造 64 字节签名负例且实跑通过 |
| V2 | README.en 安全声明 + 一致性门禁（原 UXR P0-2） | `grep -n "public whitelist" README.en.md`；`grep -n "SECURITY-SYNC" scripts/audit/doc-code-consistency.js` | README.en 0 命中；门禁存在且实跑 0 ERROR |
| V3 | sz300 纳入覆盖率矩阵（SDET P1-1 残留） | `grep -n "sz-rust-sz300" .github/workflows/coverage.yml`；查 `docs/audit/coverage-exemption.md` | 分片存在；豁免已登记（注意到期日） |
| V4 | rkyv/zerocopy 幻影依赖清理 | `grep -rn "rkyv" Cargo.toml packages/*/Cargo.toml` | 外部依赖声明 0 命中（sz-rust-zerocopy crate 自身名除外） |
| V5 | D1 商户列名 | 读 `packages/sz-rust-sz300/src/services/merchant_service.rs` 列表查询 vs `migrations/001_init.sql` | 查询列与生产 DDL 一致（name AS merchant_name 或等价） |
| V6 | D2 订单号碰撞 | 读 `packages/sz-rust-sz300/src/services/order_no.rs` 与 `mqtt_service.rs` 订单路径 | 非秒级 UNIX_TIMESTAMP；含幂等/唯一键兜底 |
| V7 | D3 下单事务 | 读 `packages/sz-rust-sz300/src/services/order_service.rs` create | begin_transaction/commit/rollback 全接线；行插入失败不吞、整体回滚 |
| V8 | D4 测试 Schema 同源 | 读 `packages/sz-rust-sz300/tests/common/mod.rs` | 直跑生产 migrations（include_str!），无手写 CREATE TABLE 夹具 |
| V9 | D5 PG 占位符 | 只读检查 `../sz-orm` 适配器现状 | 记录事实即可（外部仓库，不强求本轮修复） |
| V10 | 增量变更抽查 | `git log --oneline -30`；`docs/adr/` 最新 ADR；`docs/audit/doc-debt.md` | 声称与实际一致（sz-orm 10.13.1、性能 ADR 系列、machete 清零、doc-debt 关闭），无新 P0 |

## 原报告其余未修项（同样需核验并标注状态）
- TLS 依赖外部终止（main.rs / docker-compose：是否已有 HTTPS 模板或明确文档声明）
- ORM 与 ThinkPHP 流畅 API 不对齐（docs/thinkphp-api-mapping.md 现状）
- MSSQL 未连通（外部环境项，记录即可）

# 评测维度（重打分，每维 1-10 分 + 与 2026-10-27 基线分对比 + 关键证据）
1. 总体架构（基线 7）　2. 架构债 vs 新方向（基线 7）　3. 生产就绪度（基线 8）　4. 安全架构（基线 7）
5. 测试与质量门禁（基线 9）　6. 可扩展性与插件化（基线 5）　7. 数据层（基线 6；另参考 DB 审计 40/100 是否已改善）
8. 文档与可交付性（基线 5）　9. 技术选型风险（基线 6）　10. 路线图质量（基线 7）

# 评测方法建议
- 先读 `AGENTS.md`、`.trae/rules/project_rules.md`、根 `Cargo.toml`、
  `docs/audit/2026-10-27-系统架构师评测报告.md`、`docs/audit/2026-10-28-系统架构师评测整改执行报告.md`、
  `docs/audit/2026-10-28-数据库层独立审计报告.md`（了解旧结论与声称）
- 实跑命令：`cargo test -p sz-rust-sz300 --lib`（声称 99 passed，以你实跑为准）、
  `"D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/doc-code-consistency.js`、同路径 `link-check.js`
- 对每条结论追到 file:line 并引用真实输出；所有数字标注来源（铁律 20）

# 本机环境（务必照此使用）
- rustup stable-x86_64-windows-msvc；CARGO_HOME=D:\cargo-home、RUSTUP_HOME=D:\rustup-home、
  CARGO_TARGET_DIR 指向 D 盘独立目录（以 `echo $CARGO_TARGET_DIR` 实测为准）
- **并发保护**：多会话共用 cargo 缓存；遇「waiting for file lock」用隔离环境
  `powershell -NoProfile -ExecutionPolicy Bypass -File D:\temp\build_env_verify.ps1 <cargo 参数>`，
  或内联设置 `$env:CARGO_HOME='D:\cargo-home-verify'; $env:CARGO_TARGET_DIR='D:\rust-target-verify'; $env:RUSTC_WRAPPER=''` 后运行
- **node 不在 PATH**：用完整路径 `"D:\BtSoft\nodejs\v24.22.1\node.exe"` 运行门禁脚本
- MySQL 3306 / PostgreSQL 5432 / Oracle 1521 本机可达；MSSQL 云端口不可达（需隧道）
- **敏感文件警告**：`D:\www\test\鲜视达\服务器信息.md` 含明文凭据——只确认存在并给轮换建议，
  严禁读取/复制其内容到报告或回复中

# 输出格式
写入 `docs/audit/2026-11-04-系统架构师复评报告.md`（本轮唯一允许新建的文件）：
1. 一句话复评判断（收口是否完成；下一步该做什么）
2. 旧问题逐条核验表（V1-V10 + 原报告其余未修项：每条标 已修复/未修复/外部依赖 + V 级证据 + 核查日期）
3. 十维度重评分表（对比 2026-10-27 基线，含升降原因）
4. 新发现问题清单（P0/P1/P2，2026-10-28 后增量变更引入的债优先列出）
5. 「下一步方向」明确建议（继续收口 / 新方向 / 三个决策型 ADR：前端策略、ORM 流畅 facade、TLS 部署模板）
6. 更新版生产就绪路线图（0-2 周 / 1 个月 / 1 季度，每项含验收标准）
7. 一句话总结：未来 4 周最优先的三件事

请开始。
```
