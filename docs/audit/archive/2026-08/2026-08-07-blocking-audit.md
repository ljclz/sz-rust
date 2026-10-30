# spawn_blocking 审计报告

- **审计时间**: 2026-08-07
- **审计任务**: P3 任务 6.2
- **扫描目录**: `packages/`
- **审计工具**: `scripts/audit_blocking.sh` + 人工确认

## 审计模式

| 模式 | 类型 | 建议 |
|------|------|------|
| `std::fs::*` | 阻塞文件 IO | 改用 `tokio::fs` |
| `std::thread::sleep` | 阻塞睡眠 | 改用 `tokio::time::sleep` |
| `std::process::Command` | 阻塞进程 | 改用 `tokio::process::Command` |
| `std::net::Tcp*` / `std::net::Udp*` | 阻塞网络 IO | 改用 `tokio::net` |
| `blocking_lock` | 阻塞锁 | 改用 async lock 或 `spawn_blocking` |

## 扫描结果汇总

| 模式 | 总匹配数 | async fn 内违规 | 同步函数内 | 测试/bench 代码 |
|------|---------|----------------|-----------|----------------|
| `std::fs::*` | 216 | 0 | 6 | 210 |
| `std::thread::sleep` | 54 | 0 | 2 | 52 |
| `std::process::Command` | 0 | 0 | 0 | 0 |
| `std::net::Tcp/Udp` | 0 | 0 | 0 | 0 |
| `blocking_lock` | 0 | 0 | 0 | 0 |

## 同步函数内的 std::fs 调用（合法，非违规）

以下 `std::fs` 调用位于同步函数（`fn`，非 `async fn`）内，应在调用方用 `spawn_blocking` 包裹：

| 文件:行号 | 函数 | 调用 |
|-----------|------|------|
| `sz-rust-addons-loader/src/hot_reload.rs:81` | `fn graceful_shutdown()` | `std::fs::write` |
| `sz-rust-addons-loader/src/hot_reload.rs:96` | `fn restore_state()` | `std::fs::read_to_string` |
| `sz-rust-mvc-facade/src/view/layout.rs:85` | `fn apply_layout()` | `std::fs::read_to_string` |
| `sz-rust-mvc-facade/src/view/layout.rs:170` | `fn parse_layout_tag()` | `std::fs::read_to_string` |
| `sz-rust-mvc-facade/src/view/inheritance.rs:153` | `fn find_extend_recursive()` | `std::fs::read_to_string` |
| `sz-rust-infra-facade/src/static_files.rs:749` | `fn compute_etag()` | `std::fs::Metadata` 类型引用 |

## 同步函数内的 std::thread::sleep 调用（合法，非违规）

| 文件:行号 | 函数 | 调用 |
|-----------|------|------|
| `sz-rust-cli/src/cmd/scheduler.rs:321` | `fn run_scheduler()` CLI 入口 | `std::thread::sleep` 主线程阻塞循环 |
| `sz-rust-core/src/health.rs:401` | `fn SlowCheck::check()` 测试辅助 | `std::thread::sleep` 模拟慢检查 |

## 结论

**✅ 审计通过，未发现 async fn 内的阻塞调用违规项。**

workspace 异步代码符合 P3 规范：
- 所有 `std::fs` 调用位于同步函数或测试代码中
- 所有 `std::thread::sleep` 调用位于同步函数或测试代码中
- 未使用 `std::net` 阻塞网络 API
- 未使用 `blocking_lock`

### 建议

1. **同步函数调用方**：`graceful_shutdown`、`restore_state`、`apply_layout` 等同步函数若在 async 上下文中调用，应使用 `tokio::task::spawn_blocking` 包裹
2. **CLI scheduler.rs:321**：主线程阻塞循环可改为 `tokio::signal::ctrl_c().await` 优雅等待（代码注释已标注此建议）