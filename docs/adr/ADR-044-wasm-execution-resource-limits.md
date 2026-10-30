# ADR-044：WASM 执行资源限制

- **状态**：已接受
- **日期**：2026-10-27
- **决策者**：SZ-Rust Team（2026-10-27 全栈工程师评测 P0-1 修复）
- **决策**：`POST /api/wasm/execute` 从公开白名单移除，改为 JWT 保护；`sz-rust-wasm` 运行时启用 wasmi 燃料计量（`Config::consume_fuel(true)` + `Store::add_fuel`），并施加单实例线性内存上限（256 MiB）；HTTP 控制器将编译+执行移入 `spawn_blocking` 并包裹 5 秒超时。

## 背景

2026-10-27 全栈评测发现：`/api/wasm/execute` 位于公开白名单（免鉴权），且运行时使用 `Engine::default()`（无燃料计量）、`func.call` 同步直调（无超时、无 `spawn_blocking`）。恶意请求提交一个 `loop {}` 模块即可**永久占死一个 tokio worker 线程**；多个并发请求可瘫痪整个服务。虽然 body 读取已有 1MB 上限，但执行面无任何资源约束，构成远程 DoS。同时 wasm 规范允许模块自声明最高 4 GiB 线性内存，可触发宿主 OOM。

## 相关代码

- `packages/sz-rust-sz300/src/router.rs`：PUBLIC_PATHS 移除 `/api/wasm/execute`（约 39-44 行）；路由注册处注释更新（约 104 行）
- `packages/sz-rust-wasm/src/lib.rs`：`Config::consume_fuel(true)`（WasmRuntime::new）；`Store::add_fuel(DEFAULT_FUEL_LIMIT)` + `StoreLimitsBuilder::memory_size(DEFAULT_MEMORY_LIMIT_BYTES)` + `store.limiter`（WasmModule::execute）
- `packages/sz-rust-sz300/src/controllers/wasm_api.rs`：编译+执行移入 `tokio::task::spawn_blocking`，`tokio::time::timeout(5s)` 兜底
- `packages/sz-rust-sz300/Cargo.toml`：`[dev-dependencies] sysinfo`（内存基线测试用）

## 后果

- **收益**：WASM 执行面从「单请求占死 worker」变为「燃料耗尽/内存受限/5 秒超时快速失败」，公开端点需 JWT。
- **成本**：合法 WASM 计算量受 1000 万燃料上限约束（正常 add/mul 等演示级函数无感）；超长耗时任务会被 5 秒超时截断，需在业务侧改异步/流式方案。
- **行为变化**：`/api/wasm/execute` 不再免鉴权；调用方需携带 JWT（`Authorization: Bearer`）。

## 注意事项

- 燃料上限 `DEFAULT_FUEL_LIMIT = 10_000_000`、内存上限 `DEFAULT_MEMORY_LIMIT_BYTES = 256 MiB` 为常量，如需放宽需走配置化评审。
- wasmi 的 `Store::add_fuel` 在 `Config::consume_fuel(true)` 下才生效；若未来切换 wasmi 大版本，需核对 fuel API 是否变化（0.31 为 `add_fuel`，非旧版 `set_fuel`）。
- `spawn_blocking` 会消耗阻塞线程池配额，需监控 `tokio::task::blocking` 相关指标（并发 WASM 请求量较大时）。
- 服务器信息文件 `D:\www\test\鲜视达\服务器信息.md` 含明文凭据，建议轮换（与本次修复无关，但属评测发现的安全项）。

## Bug 定位提示

- 若 WASM 端点返回「WASM 执行失败」且错误含 `OutOfFuel` → 燃料耗尽，请检查模块是否含无限循环/超高计算量。
- 若返回「WASM 执行超时」→ 5 秒 `tokio::time::timeout` 触发，模块计算量超过预算。
- 若实例化报错含 `Memory`/`memory size` → 命中 256 MiB 内存上限。
- 若调用方收到 401 → 未携带 JWT（端点已不在公开白名单）。