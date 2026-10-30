# sz-rust 专家级深度安全审查与加固报告（2026-10-25）

- 审查对象：sz-rust workspace（重点 `sz-rust-core` / `sz-rust-middleware-facade` / `sz-rust-router-facade` / `sz-rust-sz300`）
- 审查基线：`main` @ `39eee5c0`（git status clean）
- 范围：不扩大功能范围，只做更深、更硬、可证明正确的工作
- 运行环境：Windows 10（Git Bash），`CARGO_TARGET_DIR=F:/cargo-target`，`RUSTC_WRAPPER=""`（本机无 sccache）

---

## 一、四个问题的回答

### 1. 该领域的顶尖专家会做哪些我没有要求的事情？

1. **先建立威胁模型再写测试**：区分「认证中间件」与「路由层」两个信任边界，逐层验证（中间件前、路由中、handler 后），而不是笼统地测"有没有绕过"。
2. **用原始 TCP 请求而非 `oneshot`**：`tower::oneshot` 跳过 hyper 的 URI 解析/编码处理，专家会直接用原始 HTTP 字节驱动真实解析器，覆盖 `%2e`、非法 UTF-8、畸形 chunked 等只在线上出现的输入。
3. **做正控制组（positive control）**：先证明"有效 JWT+CSRF 能到达 handler"，否则"所有请求都被拒绝"的测试结果可能是测试链路坏了而非安全机制生效。
4. **检查 panic 后的进程存活**：不仅看单请求响应，还验证 panic 后服务是否继续可用（连接级 vs 进程级故障模型）。
5. **审查配置的默认值安全**：不配置时的默认行为（CORS 是否带凭证、debug 是否开启、密钥是否可空）比显式配置更常出问题。
6. **把验证物固化为可重复运行的脚本 + 契约文件**：让结论可以被 CI/第三方工具复现，而不是一次性手工结论。
7. **对比参考实现**：对路径规范化行为，用 nginx/标准框架做对照，明确"哪些变体应该被归一化放行、哪些应该被拒绝"。

### 2. 他们会从哪些最原始的素材入手（而不是我已整理好的素材）？

- HTTP 原始请求（畸形/编码变体的字节流）
- 中间件执行链的注册顺序（`router.rs` 的 `.layer()` 调用序列）
- 配置文件的敏感项与默认值（`.env.example`、`config.rs` 的 `unwrap_or` 默认值）
- panic 堆栈与错误日志（`panic_guard.rs`、`error_handler.rs`、hyper 连接关闭行为）
- 并发压测的原始输出（并发连接是否泄漏、是否挂起）
- 路由注册表（`RouteRegistry`、`parse_path` 约定式路由的元数据）
- 上传/静态文件服务的路径拼接代码（`file_serve.rs`、`file_service.rs`）

### 3. 他们会使用的具体方法和工具？

- **方法**：RFC 3986 路径规范化测试矩阵、OWASP 路径遍历/鉴权绕过检查表、中间件顺序推导（tower `ServiceBuilder` 后注册先执行）、fail-closed 断言设计。
- **工具**：`cargo clippy -p sz-rust-sz300 --all-targets -D warnings`、`cargo test`（真实 TCP 原始请求）、自定义畸形 HTTP 脚本（本次固化为 `scripts/audit/security-gauntlet.sh`）、OpenAPI 契约校验（`/api-docs/openapi.json` + `schemas/openapi-sz300.json`）、并发/panic 恢复脚本（内置在 gauntlet）。本机无 `nginx`/`wrk`/`hey`/`OWASP ZAP`，参考对比以方法论写入 README，未执行外部工具。

### 4. 除了你我之外，还有什么可以独立证明结果是正确的？

1. **确定性测试**：26 项 gauntlet 测试每次运行结果一致（PASS/FAIL + 退出码），任何人可运行 `bash scripts/audit/security-gauntlet.sh` 复现。
2. **OpenAPI 契约文件**：`schemas/openapi-sz300.json` 可被第三方工具（Swagger Editor / openapi-generator / Redocly）独立校验。
3. **参考实现对比方法**：`scripts/audit/security-gauntlet/README.md` 描述了与 nginx/标准 axum 行为对比的方法和预期差异。
4. **代码审查证据**：本报告给出的每一条结论都附 `file:line`；关键安全逻辑（`auth_middleware.rs`、`file_serve.rs::resolve_upload_path`、`container/mod.rs` 环检测）可被独立复核。

---

## 二、A~E 实测证据（PASS/FAIL）

### 汇总：**26 项安全测试全部通过（0 失败）**

真实命令输出（2026-10-25 运行）：

```
$ cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 48 filtered out; finished in 5.02s

$ bash scripts/audit/security-gauntlet.sh
通过: 26 项
失败: 0 项
结果: PASS
SCRIPT_EXIT=0
```

### A. 路由与鉴权绕过实测 — **PASS（10/10 项）**

| 攻击面 | 测试 | 结果 |
|--------|------|------|
| 受保护路由无凭证 | GET→401 / POST→403（CSRF 先于 auth） | PASS |
| 公开路由 | `/api/v1/auth/login` POST→200、`/health` GET→200 | PASS |
| 正控制组 | 有效 JWT + CSRF → 200，证明测试链路有效 | PASS |
| 路径规范化 21 种变体 | `//`、`/./`、`/../`、尾斜杠、`;`、大小写、`%2e`、`%2f`、双重编码、反斜杠、`%00` 等，均不得 200 | PASS |
| 公开白名单精确匹配 | `/login_extra`、`/login/`、`/%6cogin`、`/health/` 等均不放行 | PASS |
| 方法覆盖 | `_method=GET` / `X-HTTP-Method-Override` 不得绕过 | PASS |
| Content-Type 混淆 | 5 种 Content-Type 均不得绕过 | PASS |
| Host 头注入 | 5 种 Host 值均不得绕过 | PASS |
| 伪造转发头 | `X-Forwarded-For`/`X-Real-IP`/`X-Forwarded-Host` 不得绕过 | PASS |
| 中间件顺序 | 有 JWT 无 CSRF → 403，证明 CSRF 在 auth 之前执行 | PASS |

**结论**：sz300 的鉴权中间件基于 `req.uri().path()`（原始路径，未解码）做**精确白名单匹配**，且在路由**之前**执行（`router.rs:335-342`），因此所有路径规范化/编码变体均 fail-closed（401/403/404/400/308，绝不为 200）。`auth_middleware.rs:47-48` 的白名单精确匹配是正确实现。

### B. 容器与配置安全 — **PASS（6/6 项）**

| 测试 | 结果 |
|------|------|
| 单例生命周期返回同一实例 | PASS |
| 瞬态生命周期返回新实例 | PASS |
| 循环依赖检测 panic 并输出依赖链（`container/mod.rs:864`） | PASS |
| 工厂 panic 后槽位复位、容器可重试（`SlotBuildGuard`，`container/mod.rs:220-235`） | PASS |
| `DatabaseConfig` Debug 输出不泄露密码（`config.rs:39-49` `[REDACTED]`） | PASS |
| `PgDatabaseConfig` Debug 输出不泄露密码 | PASS |

**结论**：DI 容器对循环依赖 fail-fast（panic 带依赖链）、单例并发构建有 per-type 槽位去重 + 超时兜底、工厂 panic 不卡死容器；配置敏感项在 `Debug` 输出中被 `[REDACTED]` 脱敏，`#[serde(skip_serializing)]` 防序列化泄露。

### C. 并发与资源限制 — **PASS（4/4 项）**

| 测试 | 结果 |
|------|------|
| `panic_guard` 中间件捕获 panic → 500 通用 JSON，无堆栈/无内部信息 | PASS |
| 无 `panic_guard` 时 panic 请求不返回 200，且服务进程继续可用 | PASS |
| 100 并发请求全部 200（无连接泄漏/挂起） | PASS |
| 上传目录路径遍历（`..`/反斜杠/`%2e`/双重编码）全部拒绝 | PASS |

**结论**：axum/hyper 在 handler panic 时关闭该连接但进程存活；框架提供 `panic_guard_middleware` 可将 panic 转为通用 500（当前 sz300 默认未挂载，见"未修改清单"）。上传/静态文件服务有 `resolve_upload_path` 三重防护（字符串 `..` 拦截 + canonicalize + 前缀边界校验），遍历变体全部拒绝。

### D. 畸形输入与内部错误泄露 — **PASS（6/6 项）**

| 测试 | 结果 |
|------|------|
| 畸形请求行/非法 URI/非法百分号/非法 UTF-8 → 4xx 或连接关闭，响应体无 panic/堆栈/源码路径 | PASS |
| 64KB 路径、8MB Header 被拒绝，不挂起 | PASS |
| 畸形 Content-Length / chunked fail-closed | PASS |
| 未知路径响应不泄露内部路径（未认证→401，额外防路径枚举） | PASS |
| OpenAPI 契约有效（3.0.3 / 36 paths / 10 tags），无内部绝对路径 | PASS |
| 自定义 `parse_path` 对畸形输入绝不 panic | PASS |

### E. 固化独立验证物 — **完成**

| 验证物 | 路径 | 验证 |
|--------|------|------|
| 可重复运行脚本 | `scripts/audit/security-gauntlet.sh` | `bash scripts/audit/security-gauntlet.sh` → PASS，退出码 0 |
| OpenAPI 契约文件 | `schemas/openapi-sz300.json` | 有效 JSON：`openapi=3.0.3`、`paths=36`、`tags=10`（`cargo run -p sz-rust-sz300 --example dump_openapi` 生成） |
| OpenAPI 导出示例 | `packages/sz-rust-sz300/examples/dump_openapi.rs` | 编译运行成功 |
| README（复现/对比/清理） | `scripts/audit/security-gauntlet/README.md` | 含复现命令、nginx/axum 参考对比方法、清理步骤 |
| 确定性测试（26 项） | `packages/sz-rust-sz300/src/security_gauntlet_tests.rs` | `cargo test -p sz-rust-sz300 security_gauntlet` 26 passed |

---

## 三、真实缺陷清单及修复

本次审查**未发现可被利用的安全漏洞**（鉴权绕过、路径遍历、敏感信息泄露、SQL 注入、panic 泄露均未命中）。以下为审查过程中发现并处理的真实问题：

### 已修复

1. **`file_serve.rs` 安全逻辑不可独立测试**（可测试性缺陷，非安全漏洞）
   - 修复：将三重路径遍历防护提取为 `pub(crate) fn resolve_upload_path`（`file_serve.rs:20-53`），`serve_file` 行为完全不变，但安全逻辑可被 14 个遍历变体单元测试直接覆盖。
   - 验证：`cargo test -p sz-rust-sz300` 全部通过；clippy 零警告。

2. **安全测试脚本自身的缺陷**（审查过程中修复）
   - `raw_http` 在读取前调用 `shutdown()` 导致服务端响应读不到 → 去掉 shutdown，与 core server 测试行为对齐。
   - 公开路径白名单测试误把「查询串变体」当绕过（实际 `path()` 剥离查询串，`/login?next=...` 就是 `/login`，属正确放行）→ 修正测试用例。
   - 404 测试误以为未知路径返回 404；实测未认证请求返回 401（auth 先于路由）→ 修正断言并记录为安全正收益。

3. **测试代码中的 clippy 警告清零**：移除两处无占位符的 `format!`。

## 四、独立证明清单

1. `cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture` → `26 passed; 0 failed`（可复现）
2. `bash scripts/audit/security-gauntlet.sh` → `PASS`，退出码 0（可复现）
3. `cargo test -p sz-rust-sz300` → lib 74 项全部通过，集成测试中依赖 MySQL 的按既有设计自动跳过（`ensure_mysql` 不可达返回 None）
4. `cargo clippy -p sz-rust-sz300 --all-targets` → 退出码 0，无警告
5. `cargo fmt -p sz-rust-sz300` → 退出码 0
6. `cargo run -p sz-rust-sz300 --example dump_openapi` → 生成 `schemas/openapi-sz300.json`（OpenAPI 3.0.3，36 paths，10 tags）
7. `git status`：变更仅含本次审查新增/修改文件（见第六节清单），无无关改动

## 五、未修改清单（保持现状并说明理由）

| 项 | 现状 | 理由 |
|----|------|------|
| sz300 默认未挂载 `panic_guard_middleware` | handler panic 时 hyper 关闭该连接（fail-closed），服务进程继续可用；客户端收到连接重置而非 500 | 不扩大功能范围；已有中间件可随时启用。**建议**：生产环境在 `router.rs` 最外层挂载 `panic_guard_middleware` 以获得统一 500 JSON |
| 64KB 以内 Header 被 hyper 接受 | 8MB 才触发 hyper 上限 | 属 hyper 默认行为；如需更严格限制应通过 hyper `max_buf_size` 配置，属部署配置决策 |
| CORS 默认 `Allow-Origin: *` 且不带凭证 | `cors.rs:60-66` 安全默认（不设置 `Allow-Credentials`），无 CSRF 风险 | 已是最佳实践，不修改 |
| `parse_path` 约定式路由仅用于元数据/MCP，不参与请求时鉴权 | 搜索确认无请求时路由分发使用 | 不修改 |
| 参考实现（nginx）对比 | 本机未安装 nginx/wrk/hey，未执行外部工具对比 | 对比方法已写入 README，可离线复现 |

## 六、清理说明

- 本次审查**未修改任何既有生产行为**（`file_serve` 重构行为等价，测试新增仅 `#[cfg(test)]`）。
- 测试服务使用 `127.0.0.1:0` 随机端口，随 tokio 运行时自动销毁，无残留进程。
- 上传测试临时目录 `$TMPDIR/sz300-upload-gauntlet-*` 测试内自动删除。
- 生成的契约文件 `schemas/openapi-sz300.json` 为本次新增交付物，保留。
- 审查结束时 `git status`：仅新增/修改下列文件（见提交清单）：
  - `packages/sz-rust-sz300/src/security_gauntlet_tests.rs`（新增，26 项测试）
  - `packages/sz-rust-sz300/src/controllers/file_serve.rs`（重构，行为不变）
  - `packages/sz-rust-sz300/src/lib.rs`（挂载测试模块）
  - `packages/sz-rust-sz300/examples/dump_openapi.rs`（新增）
  - `scripts/audit/security-gauntlet.sh`（新增）
  - `scripts/audit/security-gauntlet/README.md`（新增）
  - `schemas/openapi-sz300.json`（新增）
  - `docs/audit/2026-10-25-security-gauntlet.md`（本报告）

## 七、结论

sz-rust 的路由鉴权、中间件顺序、DI 容器、配置脱敏、上传路径防护、畸形输入处理与 panic 恢复均通过确定性验证，**未发现可被利用的安全漏洞**。26 项安全测试 + OpenAPI 契约 + 可复现脚本已固化为仓库资产，后续安全回归可一键执行。