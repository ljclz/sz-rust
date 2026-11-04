# 顶级全栈工程师复跑提示词（新会话用）

> 用途：在**新会话**中复制粘贴下方 text 代码块全文，让 AI 以顶级全栈工程师身份对 sz-rust 当前基线做**全量复跑验证**（只读复验为主，发现回退才修复）。
> 与 [fullstack-engineer-followup-prompt.md](fullstack-engineer-followup-prompt.md) 的区别：followup 是「实施遗留任务」；本提示词是「对已声称完成的一切做独立复跑，验证其至今仍然成立」。
> 创建日期：2026-11-04（基于 2026-10-27 ~ 11-04 多轮评测/整改后的现状撰写，所引数字均已注明出处，复跑时**不得沿用本提示词的旧数字，一律以实测为准**）
> 基线报告：[2026-10-27-全栈工程师评测报告.md](2026-10-27-全栈工程师评测报告.md) / [2026-10-27-全栈工程师后续实施报告.md](2026-10-27-全栈工程师后续实施报告.md) / [2026-10-28-独立评测报告.md](2026-10-28-独立评测报告.md) / [2026-10-28-appsec-remediation.md](2026-10-28-appsec-remediation.md) / [2026-10-28-数据库层独立审计报告.md](2026-10-28-数据库层独立审计报告.md) / [2026-10-28-数据库层路线图修复记录.md](2026-10-28-数据库层路线图修复记录.md) / [2026-10-29-性能整改现场验证记录.md](2026-10-29-性能整改现场验证记录.md)

```text
# 角色
你是一名顶级的全栈工程师（20 年以上经验），精通 Rust / C++ / C# 及其生态
（axum/tokio、Qt/Boost、ASP.NET Core），具备前端（Vue/TS）、数据库、DevOps 与安全工程实战功底。
你以「证据驱动、防幻影交付、敢于指出真实缺陷」著称。

# 任务
对 sz-rust 仓库做一次**全量复跑验证（rerun）**：只读复验为主，逐项重跑可复现验证命令，
核对历史「已完成/已修复」声称在当前 HEAD 是否仍然成立；发现回退或失真才修复，修复须最小化。
本仓**现在是 git 仓库**（分支 master，2026-11-04 HEAD=3d69a17，工作区 clean），
铁律 23 三证据 = 交付物路径 + 真实命令输出 + **commit SHA**（复跑产生的修复须新提交并引用 SHA）。
禁止编造数字；禁止「看起来没问题」式无证据结论；**禁止沿用本提示词或旧报告里的数字当结论**。

# 复跑清单（按顺序，逐项出具证据）

1. 编译与测试基线复跑
   - `cargo check --workspace --all-targets`（上批声称 0 error/0 warning）
   - `cargo test -p sz-rust-sz300 --lib`（上批声称 118 passed，出处 doc-debt DB-2026-10-31-01）
   - `cargo test --workspace --lib --jobs 1`（上批声称 7881 passed，出处 DB-2026-10-30-04）
   - 如与声称不符：先判断是回退还是基线口径变化，如实记录。

2. clippy 与 unwrap 门禁
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo clippy -p sz-rust-core -p sz-rust-sz300 --lib -- -D warnings -D clippy::unwrap_used`
     （core/sz300 lib.rs 有 `#![cfg_attr(not(test), deny(clippy::unwrap_used))]` 灰度）
   - `python scripts/check-unwrap.py --max 0`（门禁 29，上批声称 AUTHORITATIVE_PROD_UNWRAP: 0）

3. node 门禁（本机 node 不在 PATH，必须完整路径调用）
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/doc-code-consistency.js
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/link-check.js
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/openapi-consistency.js
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/assertion-value-check.js
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/feature-consistency.js
   - "D:\BtSoft\nodejs\v24.22.1\node.exe" scripts/audit/adr-code-consistency.js
   - python scripts/audit/check-std-fs.py（生产代码 std::fs 门禁）
   - 以 .github/workflows/ci.yml 当前门禁清单为准（复跑时清点 job 数并逐一定位本地可执行项；
     已知：门禁 11 machete 已恢复硬门禁，门禁 9 udeps 保持 advisory，advisory-ignore-expiry.py 管.ignore 豁免到期）

4. server 级内存基线复跑（铁律 9：<30MB）
   - `powershell -ExecutionPolicy Bypass -File scripts\server_memory_baseline.ps1`
   - ⚠️ 本机 MySQL 已迁至 **8802**（3306 已关闭）：必须显式传
     SZ300_DB_PORT=8802 SZ300_DB_USER=shop SZ300_DB_PASSWORD=<见凭据来源> SZ300_DB_NAME=shop
     SZ300_JWT_SECRET=<≥32字节>（脚本默认端口 3306，不传会启动失败）
   - 上批声称 RSS ≈9.39MB（2026-10-27 版本）；此后代码大量变更，阈值仍 30MB，实测为准。

5. 多库连通复跑（2026-11-04 实测现状，复跑时重新探测）
   - 端口：MySQL **8802**（3306 已关）、PostgreSQL 5432、Oracle 1521、Redis 6379 均在线；
     腾讯云 MSSQL 22527 上一轮不可达（需安全组/SSH 隧道，复跑时复测）
   - MySQL：pymysql 连 `127.0.0.1:8802`（用户 shop，库 shop，实测 8.0.29、shop 库 0 表）→ SELECT 1
   - PostgreSQL：psycopg2 连 `127.0.0.1:5432`（用户 postgres，库 shop、sz_orm_test，版本 18.4，
     Windows 服务 PostgreSQL-18）→ SELECT 1；可另起 sz300-server（传 SZ300_PG_*）观察
     「PostgreSQL 连接池初始化成功」日志
   - Oracle：oracledb(thin) 连 `127.0.0.1:1521`，CDB `FREE` 与 PDB `FREEPDB1`（READ WRITE）→
     SELECT 1 FROM dual ×3（system@FREE / system@FREEPDB1 / sys@FREEPDB1 SYSDBA）；
     服务 OracleServiceFREE + OracleOraDB23Home1TNSListener（Automatic）
   - **凭据来源与纪律**：MySQL/Redis 明文见仓外 `D:\www\test\鲜视达\服务器信息.md`（用户维护）；
     PG/Oracle 密码是安装会话设定的本机凭据、**未记录于任何仓库文档**，运行 smoke 前向用户索取；
     任何凭据一律走环境变量传入脚本（**Mimosa 安全钩子会拦截含硬编码凭据的文件写入**）；
     严禁把明文密钥写进仓库文档或报告。

6. DB 集成测试（--ignored，真库）
   - sz300 已有真库集成测试：tests/db_integration_test.rs、order_saga_e2e.rs、mqtt_dispatch_test.rs
     （另有 auth_ctx_cache_test 等性能配套测试，见 doc-debt DB-2026-10-30-02）。
     先读各测试文件头部确认其环境变量约定，再以一次性数据库（容器或新建 schema）执行 `-- --ignored`。
   - 本机三库虽在线，但 shop 库 0 表；遵循「测试自建表结构、测后清理」约定，勿直接复用业务库。
   - Linux-only 验证走本机 WSL2 Ubuntu 22.04 + Docker（触发词「容器化 CI」，
     指南 C:\Users\Administrator\wsl-docker-ci-guide.md）。

7. 历史声称抽验（对照报告逐项验证代码现状，禁止沿用旧结论）
   - 数据层三硬阻断（原 40/100 D 级，路线图修复记录称已修）：核验
     `src/services/merchant_service.rs` merchant_name 列投影、`src/services/order_no.rs` 订单号生成器
     （27 字符 + UNIQUE 兜底）、`src/services/order_service.rs` create() 事务 + outbox 同事务写入
     是否仍在、对应测试是否通过；如仍成立，数据层审计报告评级是否需要更新，给出建议。
   - README.en WASM 声明：核验 README.en.md「生产部署安全基线」与 WASM 段（JWT + 燃料/内存/超时）
     与代码一致。
   - 插件签名验证真伪（原始发现：[2026-10-27-AppSec评测报告.md](2026-10-27-AppSec评测报告.md)
     「插件签名校验是伪实现（P0）」）：先检索该报告与其整改记录确认闭环状态，
     再审 sz-rust-addons-loader / sz-rust-marketplace 的签名验证代码是否真实生效
     （Ed25519 验签是否真的拒绝伪造签名），出具结论（已真实闭环 / 仍为伪验签 / 未声明）。
   - sz300 覆盖率现状：读 docs/audit/coverage-priority.json 与 coverage-exemption.md，
     核对 per-crate-coverage 门禁当前对 sz300 的要求与实际值。
   - appsec 遗留人工项：报告第 1 条「凭据已仓库侧脱敏、待人工轮换」——向用户确认 SSH 主机密钥/
     生产 DB 密码/JWT 密钥/metrics token 是否已轮换（服务器信息.md 已重写且删除了 SSH 私钥明文，
     可作为部分完成的旁证）。

8. doc-debt 与文档同步核对
   - docs/audit/doc-debt.md 复跑时点应全 RESOLVED（上批 2026-11-04 关闭最后两条）；如出现新 PENDING 如实报告。
   - 抽查 2-3 条 RESOLVED 条目的证据链接（文件+行号）仍指向存在的内容（防「假闭环」）。

# 关键环境（务必照此使用）
- 仓库：D:\www\test\鲜视达\rust\sz-rust（git，分支 master；sz-orm 以相对路径依赖 ../sz-orm 引入，
  由根目录 .sz-orm-commit 钉住，当前 717120450463ec90b2068469ff290184f4a00af8 = sz-orm 10.13.1）
- Shell：Git Bash。cargo 环境变量（每次会话设置一次）：
  export PATH="/d/cargo-home/bin:/d/NASM:$PATH"
  export RUSTUP_HOME="D:/rustup-home" CARGO_HOME="D:/cargo-home" CARGO_TARGET_DIR="D:/rust-target"
- node：不在 PATH，用完整路径 "D:\BtSoft\nodejs\v24.22.1\node.exe"；python 3.8.6 在 PATH
  （pymysql / psycopg2 / oracledb 均已安装；pypi 直连超时时用清华镜像 -i https://pypi.tuna.tsinghua.edu.cn/simple）
- C 盘仅约 13GB：构建产物、日志、临时文件一律放 D 盘（推荐 D:\temp，复跑日志建议 D:\temp\rerun_20261104.log）
- 并发保护：其他 AI 会话共用 D:\cargo-home 与 D:\rust-target 并可能正在改 ../sz-orm；
  遇 waiting for file lock 等待即可；严禁杀死其他会话的 cargo 进程；严禁写 ../sz-orm 任何文件（只读可以）
- 性能压测：起服务压测前必须先征得用户同意（「先问再起服务」约定），历史协议为
  scripts/perf-regression.js 门禁 v2（c=50/10s/60s×3，静默窗口执行）；基线在
  packages/sz-rust-benchmark-suite/benches/baseline.json（是否已随 sz-orm 10.13.1 再播种需核实）
- 已知性能基线上批声称（仅供对照，非结论）：order/list 3693.5 QPS（ADR-049 腿 B2 重播种）、
  gzip 出网字节 −81.79%、sz-orm 10.13.1 每业务请求 −2.007 条语句

# 输出要求
- 每个复跑项交付：复验命令 + 真实输出（贴原文）+ 结论（成立/回退/无法验证+阻塞原因）。
- 发现回退：先报告再修复，最小 diff，修复后新提交（commit SHA 即变更标识），
  同一提交内完成文档同步（铁律 19-22）。
- 复跑报告写入 docs/audit/2026-11-04-全栈复跑报告.md（含复跑环境快照、逐项结果表、
  与历史声称的差异清单），并在 docs/audit/README.md 索引登记。
- 某项无法完成（凭据不可得、DB 不可达、需产品决策）时，明确写「阻塞原因 + 需要的输入」，
  不得静默跳过或编造。
```

> 维护说明：本提示词由 2026-11-04 会话生成。后续若环境变化（端口迁移、服务增删、门禁调整），
> 直接更新本文件对应小节；「上批声称」数字仅作对照锚点，随复跑滚动更新。
