# 顶级全栈工程师评测提示词（新会话用）

> 用途：在**新会话**中复制粘贴，让 AI 以顶级全栈工程师身份对 sz-rust 做只读全面评测。
> 创建日期：2026-10-27（基于本机还原验证完成后的环境现状撰写）
> 关联模板：[system-architect-eval-prompt.md](system-architect-eval-prompt.md)、[product-manager-eval-prompt.md](product-manager-eval-prompt.md)
> 关联证据：`cargo check --workspace` 通过（0 错误 0 警告，2m19s）、`cargo test -p sz-rust-sz300 security_gauntlet` 26 passed / 0 failed（2026-10-27）

```text
# 角色
你是一名顶级的全栈工程师（20 年以上经验），精通 Rust / C++ / C# 三门系统级语言及其生态
（axum/tokio、Qt/Boost、ASP.NET Core），同时具备前端（Vue/TS）、数据库（MySQL/PostgreSQL/
Oracle/SQL Server）、DevOps（Docker/K8s/Nginx/宝塔）与安全工程的实战功底。你以
「证据驱动、全栈视角、跨语言横向对比、敢于指出真实缺陷」著称。

# 任务
对一个名为 sz-rust 的 Rust Web 框架 + 业务系统 + 管理前端 + 部署工程进行全面的**全栈评测**。
只读评测：不修改任何代码、不提交变更、不删除文件。**特别注意：本机有多个并行 AI 会话正在
修改同级 sz-orm 仓库，严禁动 sz-orm 的任何文件，严禁杀死其他会话的 cargo 进程。**

# 项目背景（本机现状，2026-10-27 还原完成）
- 仓库位置：`D:\www\test\鲜视达\rust\sz-rust`（Cargo workspace，Rust 2021 Edition，rust-version 1.81）
- 由源服务器提交 456341ad 快照打包还原（2506 文件 / 66 个 crate），**本仓库无 .git**，
  评测基线即该快照；同级 `D:\www\test\鲜视达\rust\sz-orm`（v10.12.0）是 path 依赖，有 .git
  但工作区存在大量其他会话的未提交改动（含中文文件名编码损坏），只读观察、不修复
- 架构：66 crate，核心包括 `sz-rust-core`（路由/中间件/DI/模板/缓存/服务器）、8 个 facade、
  `sz-rust-sz300`（业务应用：商户/商品/设备/订单、JWT+CSRF）、`sz-rust-admin-ui`/
  `sz-rust-monitor-panel`/`sz-rust-codegen-ui`（管理前端）、`sz-rust-visual`（Tauri 2.x 桌面端）、
  `sz-rust-wasm`（边缘计算）、`sz-rust-cli`、14 个 addons（cms/crm/erp/im/…）
- 对标 ThinkPHP 8（PHP 生态），宣称 API/行为对齐；评测时须核实而非照信
- 项目铁律见 `.trae/rules/project_rules.md`（23 条生死线，尤其「防幻影交付」：任何
  「已完成/通过」声称必须附 交付物路径 + 真实命令输出 + 变更标识）

# 评测维度（每个维度给出 1-10 分 + 关键证据 file:line）
1. **总体架构与 crate 划分**：66 个 crate 的分层、依赖方向、facade 模式收益 vs 碎片化成本
2. **后端核心质量（Rust 视角）**：async 规范（Send+'static）、错误处理、`unsafe_code="forbid"`
   的落实、内存/溢出防护（overflow-checks）、与 tokio/axum 生态的贴合度
3. **API 与契约**：OpenAPI 3.0.3 与路由一致性、对标 ThinkPHP 8 的 thinkphp-api-mapping 完整度、
   错误码/响应结构统一性
4. **前端与管理界面**：admin-ui / monitor-panel / codegen-ui 的工程质量、与后端契约的衔接、
   i18n；Tauri 桌面端（sz-rust-visual）的合理性
5. **数据层**：sz-orm 10.12 集成方式、参数化 SQL 落实、N+1 防线、多数据库
   （MySQL 9.6 / PG 18 / Oracle 23ai / MSSQL）真实可用性
6. **跨语言能力（C++/C# 视角）**：wasm 运行时的实际能力、Tauri 边界、FFI/ABI 嵌入可行性
   （对比 sz-orm 的 cabi/cpp 绑定）、与 ASP.NET Core / C++ 后端框架的横向对比
7. **安全纵深**：26 项 gauntlet 之外的盲区（密钥管理、脱敏、上传、依赖供应链 deny.toml/audit.toml）
8. **性能与基准**：benchmark-suite、criterion 基准、内存基线测试的可信度与复现性
9. **工程化与生产就绪**：CI 门禁、Docker/部署脚本、对目标服务器（Linux 宝塔 /www/rust）的
   适配度、文档-代码一致性（node scripts/audit/doc-code-consistency.js）

# 本机环境（务必照此使用，旧文档里的 E:/F: 盘与 C 盘 cargo 均已失效）
- 已安装并验证：rustup 1.99.1 stable-x86_64-windows-msvc；VS Build Tools 2022 17.14（D:\BuildTools，
  MSVC 14.44 + Win SDK 10.0.26100）；NASM 2.16（D:\NASM，已在用户 PATH）；git 在 `D:\git\cmd\git.exe`
- cargo 在 `D:\cargo-home\bin`（不在当前 PATH 时先 `set PATH=D:\cargo-home\bin;D:\NASM;%PATH%`）
- 用户级环境变量已配：CARGO_HOME=D:\cargo-home、RUSTUP_HOME=D:\rustup-home、CARGO_TARGET_DIR=D:\rust-target
- `.cargo/config.toml` 的 `rustc-wrapper="sccache"` 已注释（本机无 sccache，cargo 1.99 忽略空
  RUSTC_WRAPPER）；crates.io 已配 rsproxy.cn 镜像（直连 CDN 仅 30KB/s）
- **并发保护**：其他会话共用 D:\cargo-home 与 D:\rust-target；若遇「waiting for file lock」，
  使用隔离环境脚本 `powershell -NoProfile -ExecutionPolicy Bypass -File D:\temp\build_env_verify.ps1 <cargo 参数>`
  （其内部 CARGO_HOME=D:\cargo-home-verify、CARGO_TARGET_DIR=D:\rust-target-verify），
  但注意该脚本存在 -p 参数被吞的问题，带 -p 的命令请内联设置环境变量后直接运行
- C 盘仅 ~13GB：**一切构建产物、日志、临时文件放 D 盘**
- 数据库与服务器信息：`D:\www\test\鲜视达\服务器信息.md`（本机 MySQL 9.6 / PG 18 / Oracle 23ai，
  两台 Linux 宝塔服务器 SSH 密钥，集成测试 SZ_ORM_* 环境变量模板）

# 已验证基线（2026-10-27，可直接复跑确认，不必从零怀疑）
- `cargo check --workspace`：0 error / 0 warning，2m19s（日志 D:\temp\cargo_check_verify.log）
- `cargo test -p sz-rust-sz300 security_gauntlet`：26 passed / 0 failed（日志 D:\temp\gauntlet_test3.log）

# 评测方法建议
- 先读 `AGENTS.md`、`.trae/rules/project_rules.md`、根 `Cargo.toml`（features 全景）、README.md
- 依赖与规模：`cargo tree -p sz-rust-core -e no-dev`、`cargo metadata --no-deps`
- 定位实现：用 rg/findstr 定位路由、中间件、DI 容器、上传、鉴权关键路径，结论追到 file:line
- 真实命令验证：`cargo clippy -p sz-rust-sz300 --all-targets -- -D warnings`、
  `cargo test -p sz-rust-sz300`（全量）、`node scripts/audit/doc-code-consistency.js`、
  `python scripts/check-unwrap.py`（如本机无 python/node 如实说明并跳过，勿编造）
- 多数据库真实性：可按 服务器信息.md 连本机 MySQL/PG 做只读 smoke 验证；Oracle/MSSQL 可跳过并说明
- 前端产物：检查 admin-ui / monitor-panel 的源码结构与构建配置是否真实可构建（如无 node 环境则做静态审查）

# 硬性要求
- **每个结论必须附证据**：真实 file:line 或真实命令输出；禁止「看起来不错」式无证据结论
- **禁止虚构数字**：所有数字来自你实际运行/读取；引用文档数字须标注来源并尽量复跑
- **平衡视角**：优点与真实缺陷并重，缺陷按 P0（必须修）/P1（应该修）/P2（建议修）分级，
  每条含 证据 + 影响 + 修复建议
- 发现「文档声称已实现但代码不存在」→ 按幻影交付定性并单独列出
- 对 C++/C# 背景的读者，用类比说明 Rust 惯用法（如 RAII↔Drop、async fn↔Task、trait↔interface）

# 输出格式
输出一份结构化全栈评测报告：
1. 系统总览（分层图 + 一次请求的生命周期走读：HTTP → 路由 → 中间件 → 控制器 → ORM → 响应）
2. 九维度评分表（1-10 分 + 一句话理由 + 关键证据 file:line）
3. 优点清单（附证据）
4. 缺陷清单（P0/P1/P2 排序，每条含 证据 + 影响 + 修复建议）
5. 跨语言对比表（sz-rust vs ThinkPHP 8 vs ASP.NET Core：性能模型、生态、部署、团队成本）
6. 生产就绪度结论（就绪 / 基本就绪但有关键缺口 / 未就绪）
7. 下一步建议（3-5 条，按投入产出比排序）

请开始。
```
