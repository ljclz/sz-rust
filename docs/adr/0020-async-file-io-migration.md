# ADR-020：异步文件 I/O 迁移（std::fs → tokio::fs）

> **状态**：已接受
> **日期**：2026-08-05
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-010（配置加载方式）、ADR-016（Addon 热加载）

## 背景

SZ-Rust 铁律 4 明确规定"禁止在任何 crate 中使用 `std::fs`，统一 `tokio::fs`"。该铁律源于 ThinkPHP 8 对标需求：Web 框架的文件操作必须在异步运行时内执行，避免同步 I/O 阻塞 tokio 线程池。

2026-08-05 审计发现 7 个文件的非测试代码仍使用 `std::fs`，违反铁律 4：

| 文件 | 违规函数 | 违规调用 |
|------|---------|---------|
| `infra-facade/src/static_files.rs` | `fingerprint_file` | `std::fs::read` |
| `state-facade/src/mail.rs` | `from_file` | `std::fs::read` |
| `state-facade/src/i18n.rs` | `load_from_file` | `std::fs::read_to_string` |
| `state-facade/src/env.rs` | `load_from_file` | `std::fs::read_to_string` |
| `infra-facade/src/config.rs` | `load_section` / `load_from_dir` | `std::fs::read_to_string` / `std::fs::read_dir` |
| `core/src/runtime/hot_reload.rs` | `scan` / `scan_dir` | `std::fs::metadata` / `std::fs::read_dir` |
| `cli/src/cmd/optimize.rs` | `execute_optimize_schema` | `std::fs::create_dir_all` / `std::fs::write` |

## 决策

将上述 7 个文件中所有非测试代码的 `std::fs` 调用迁移为 `tokio::fs` 异步等价物：

- `std::fs::read(path)` → `tokio::fs::read(path).await`
- `std::fs::read_to_string(path)` → `tokio::fs::read_to_string(path).await`
- `std::fs::write(path, data)` → `tokio::fs::write(path, data).await`
- `std::fs::create_dir_all(path)` → `tokio::fs::create_dir_all(path).await`
- `std::fs::metadata(path)` → `tokio::fs::metadata(path).await`
- `std::fs::read_dir(path)` → `tokio::fs::read_dir(path).await`（返回 `ReadDir` 异步迭代器）

所有包含迁移函数的公共函数签名改为 `async fn`，调用点加 `.await`。CLI 入口 `main.rs` 改为 `#[tokio::main] async fn main`。

## 后果

### 正面后果
- 铁律 4 完全合规：7 个目标文件非测试代码零 `std::fs` 调用
- 文件 I/O 不再阻塞 tokio 线程池，提升高并发下的响应延迟尾部表现
- 统一异步 I/O 模型，消除同步/异步混用的认知负担
- `cargo clippy --workspace --all-targets -- -D warnings` 全量通过

### 负面后果
- 公共 API 签名变更（同步 → async）：`fingerprint_file`、`from_file`、`load_from_file`、`load_section`、`load_from_dir`、`scan`、`scan_dir`、`execute_optimize_schema` 均改为 `async fn`，所有调用方需加 `.await`
- CLI 入口从 `fn main` 改为 `#[tokio::main] async fn main`，`cli::execute`、`cli::run`、`console::run` 均改为 async
- 测试函数需改为 `#[tokio::test] async fn`，调用点加 `.await`
- 范围外发现项（铁律 18 记录但不修复）：`view/layout.rs:85,170`、`view/inheritance.rs:153`、`view.rs:647`、`upload/storage.rs:312,337,648,649` 仍有非测试代码的 `std::fs` 调用，留待后续迭代

## 注意事项

1. **`tokio::fs::read_dir` 返回异步迭代器**：不能直接 `for entry in read_dir().await?`，需 `let mut rd = read_dir().await?; while let Some(entry) = rd.next_entry().await? { ... }`
2. **CLI 全链路 async 传染**：`main.rs` → `lib.rs::run` → `cli.rs::execute` → `console.rs::run` → `optimize.rs::execute_optimize_schema` 必须全链路 async，否则中间断点无法 `.await`
3. **测试锁跨 await**：`lib.rs::test_run_cache_clear_command` 持有 `std::sync::MutexGuard` 跨 await，在 `#[tokio::test]`（current_thread runtime）下安全，但需 `#[allow(clippy::await_holding_lock)]` 抑制 clippy 警告
4. **`hot_reload.rs` 条件编译**：该文件有 `#![cfg(feature = "hot-reload")]`，测试需 `--features hot-reload`
5. **范围外发现项不修复**：`view/` 和 `upload/storage.rs` 的 `std::fs` 调用涉及模板渲染和云存储上传，异步化影响面大，留待专项迭代

## Bug 定位提示

- 若生产环境出现"同步文件 I/O 阻塞线程池"的性能问题（如 P99 延迟突增），首先检查 `tokio::fs` 是否被误替换为 `std::fs`
- 搜索命令：`rg "std::fs::" --type rust -g '!**/tests/**' -g '!**/*test*.rs'` 排除测试代码
- tracing span：文件 I/O 操作应出现在 `span!("file_io", op = "read", path = %path)` 中，若出现在同步上下文则违反本 ADR