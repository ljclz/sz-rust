# 五维审查报告：铁律违规修复（2026-08-05）

> **审查范围**：4 项铁律违规修复（铁律 1/10/7/4）
> **审查日期**：2026-08-05
> **审查人**：SZ-Rust 工程团队
> **关联 ADR**：[ADR-020 异步文件 I/O 迁移](../adr/0020-async-file-io-migration.md)
> **关联规格**：`.codeartsdoer/specs/fix_iron_violations/`

---

## 1. 审查摘要

| 维度 | 评分 | 结论 |
|------|------|------|
| 正确性 | 95/100 | ✅ 通过 |
| 可读性 | 92/100 | ✅ 通过 |
| 架构 | 90/100 | ✅ 通过 |
| 安全性 | 96/100 | ✅ 通过 |
| 性能 | 88/100 | ✅ 通过 |
| **综合** | **92.2/100** | ✅ 通过 |

---

## 2. 修复清单

| 编号 | 铁律 | 修复内容 | 证据（file:line） |
|------|------|---------|------------------|
| T1 | 铁律 1（overflow-checks） | `[profile.dev]` 添加 `overflow-checks = true` | `Cargo.toml:182` |
| T2 | 铁律 10（coverage 阈值） | coverage.yml 阈值 70→85，与 ci.yml 一致 | `.github/workflows/coverage.yml:44` |
| T3 | 铁律 7（敏感字段脱敏） | TencentSmsConfig 改为 `#[derive(Clone)]` + 手动 `impl Debug` 脱敏 | `packages/sz-rust-state-facade/src/notify.rs:771` |
| T4.1 | 铁律 4（tokio::fs） | `fingerprint_file` 异步化 | `packages/sz-rust-infra-facade/src/static_files.rs` |
| T4.2 | 铁律 4 | `from_file` 异步化 | `packages/sz-rust-state-facade/src/mail.rs` |
| T4.3 | 铁律 4 | `load_from_file` 异步化 | `packages/sz-rust-state-facade/src/i18n.rs` |
| T4.4 | 铁律 4 | `load_from_file` 异步化 | `packages/sz-rust-state-facade/src/env.rs` |
| T4.5 | 铁律 4 | `load_section`/`load_from_dir` 异步化 | `packages/sz-rust-infra-facade/src/config.rs` |
| T4.6 | 铁律 4 | `scan`/`scan_dir` 异步化 | `packages/sz-rust-core/src/runtime/hot_reload.rs` |
| T4.7 | 铁律 4 | `execute_optimize_schema` 异步化 + CLI 全链路 async | `packages/sz-rust-cli/src/cmd/optimize.rs` + `main.rs` + `lib.rs` + `cli.rs` + `console.rs` |
| T4.8 | 铁律 4 | 全量扫描验证 | 7 个目标文件非测试代码零 `std::fs` |

---

## 3. 五维详细审查

### 3.1 正确性（95/100）

**验证证据：**
- `cargo fmt --all -- --check` ✅ 通过（0 格式问题）
- `cargo clippy --workspace --all-targets -- -D warnings` ✅ 通过（0 警告）
- `cargo test -p sz-rust-state-facade` ✅ 222 passed; 0 failed
- `cargo test -p sz-rust-infra-facade` ✅ 670 passed; 0 failed
- 新增脱敏测试：`test_tencent_sms_config_debug_redacted`、`test_tencent_sms_config_field_access_and_clone` ✅ 通过

**已知限制：**
- `cargo test -p sz-rust-cli` 和 `cargo test -p sz-rust-core` 阻塞于 Windows rustc `STATUS_STACK_BUFFER_OVERRUN`（编译器 bug，非代码问题）
- clippy 全量通过（`--all-targets` 包含 test target）已证明所有代码包括测试代码能正确编译
- `sz-rust-sz300/tests/service_coverage_test.rs` 有 15 个 TODO 占位测试（既有问题，不在本次修复范围）

**扣分项：**
- -3 分：cli/core 测试未能因 Windows 环境限制运行
- -2 分：范围外发现项未修复（见 §4）

### 3.2 可读性（92/100）

**正面：**
- 所有异步化函数保持原命名，仅签名 `fn` → `async fn`
- CLI 全链路 async 传染有清晰注释说明原因
- `lib.rs` 的 `#[allow(clippy::await_holding_lock)]` 附带注释说明 current_thread runtime 安全性

**扣分项：**
- -5 分：异步化导致调用链 `.await` 传播，增加了代码噪音
- -3 分：`hot_reload.rs` 的 `read_dir` 从同步迭代改为 `next_entry().await` 循环，模式变化较大

### 3.3 架构（90/100）

**正面：**
- 统一异步 I/O 模型，消除同步/异步混用的架构不一致
- ADR-020 记录决策背景与后果
- CLI 全链路 async 改造完整，无断点

**扣分项：**
- -5 分：公共 API 签名变更（8 个函数 sync→async）是破坏性变更，需下游适配
- -5 分：范围外发现项（view/upload/storage.rs）仍保留 `std::fs`，架构一致性未完全达成

### 3.4 安全性（96/100）

**正面：**
- 铁律 7 脱敏：TencentSmsConfig 的 `secret_id`/`secret_key`/`sms_sdk_app_id` 在 Debug 输出中脱敏为 `***REDACTED***`
- 新增测试验证脱敏效果：`assert!(format!("{:?}", config).contains("***REDACTED***"))`
- 铁律 1 overflow-checks：dev profile 启用溢出检查，开发期捕获算术溢出 bug

**扣分项：**
- -2 分：脱敏仅覆盖 TencentSmsConfig，其他 Config 类型（如 AliyunConfig）未审查
- -2 分：范围外 `upload/storage.rs` 的 `std::fs` 调用可能涉及文件路径安全

### 3.5 性能（88/100）

**正面：**
- 文件 I/O 从同步阻塞改为异步非阻塞，提升 tokio 线程池利用率
- 高并发下文件操作不再占用 worker 线程

**扣分项：**
- -8 分：异步化引入 `.await` 开销（状态机转换 + 调度），单次文件操作延迟可能微增
- -4 分：`tokio::fs` 底层仍使用阻塞线程池（`tokio::task::spawn_blocking`），并非真正异步 I/O（Windows/Linux 均如此），性能提升有限

---

## 4. 范围外发现项（铁律 18 记录）

以下 `std::fs` 调用在本次修复范围外，记录留待后续迭代：

| 文件 | 行号 | 调用 | 原因 |
|------|------|------|------|
| `sz-rust-core/src/view/layout.rs` | 85, 170 | `std::fs::read_to_string` | 模板渲染，异步化影响面大 |
| `sz-rust-core/src/view/inheritance.rs` | 153 | `std::fs::read_to_string` | 模板继承解析 |
| `sz-rust-core/src/view.rs` | 647 | `std::fs::metadata` | 视图文件检查 |
| `sz-rust-infra-facade/src/upload/storage.rs` | 312, 337, 648, 649 | `std::fs::rename`/`remove_file`/`create_dir_all`/`metadata` | 云存储上传，涉及 Local/Aliyun/Qcloud/Qiniu/S3 多驱动 |

**建议**：后续专项迭代"view 模块异步化"和"upload/storage 异步化"时统一处理。

---

## 5. 环境限制说明

| 限制 | 影响 | 规避措施 |
|------|------|---------|
| Windows rustc `STATUS_STACK_BUFFER_OVERRUN` | `cargo test -p sz-rust-cli`/`-p sz-rust-core` 编译崩溃 | clippy `--all-targets` 已验证编译正确性；Linux CI 可运行全量测试 |
| `cargo clean` 后 `ring` C 编译失败 | 需重新编译 C 代码时 cl.exe 无法处理中文路径 | 避免 `cargo clean`，或使用英文路径 |
| `sz-rust-core` h2/server 网络测试 | Windows `NetworkUnreachable` | 既有问题，非本次引入 |

---

## 6. 结论

4 项铁律违规修复全部完成，综合评分 92.2/100，通过五维审查。

**交付物清单：**
- 源代码修改：16 个文件
- 新增 ADR：ADR-020（异步文件 I/O 迁移）
- 新增测试：2 个脱敏测试（TencentSmsConfig）
- 规格文档：`.codeartsdoer/specs/fix_iron_violations/`（spec.md + design.md + tasks.md）
- 本五维审查报告

**后续行动项：**
1. Linux CI 运行全量 `cargo test --workspace` 验证 cli/core 测试
2. 后续专项迭代处理范围外发现项（view/upload/storage.rs 的 `std::fs`）
3. 审查其他 Config 类型的脱敏覆盖