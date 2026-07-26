---
alwaysApply: true
---

# sz-rust 项目铁律 —— 全栈 Rust 生态统一红线

> 适用范围：sz-rust 全 workspace（packages/sz-rust-core / sz-rust-sz300 / sz-rust-addons-* / sz-rust-cli / sz-rust-examples）。
> 本铁律仅覆盖 Web 框架层（framework）。database / orm 模块由各自项目的 Skills 负责。

## 🧠 内存与溢出（全局硬约束）

1. **整数溢出即 Panic**：所有算术运算默认 `overflow-checks = true`（已在 Cargo.toml 强制）。AI 若使用未检查的 `+` 或 `-` 处理外部输入，必须补充 `checked_*` 或 `saturating_*`。
2. **严禁裸 unwrap**：任何 `Result` 或 `Option` 解包必须使用 `?` + `anyhow::Context` 携带调用栈。仅允许在测试代码或启动阶段（如 `env::var` 必须存在）使用 `.expect("明确原因")`。
3. **unsafe 围栏**：`unsafe` 仅限 FFI 或极致性能热点（须有 `# Safety` 文档），应用层（路由、执行器、映射）零容忍。

## 🔥 异步运行时（全模块通用）

4. **禁止阻塞运行时**：全项目禁止 `std::thread::sleep` 和同步文件 IO，统一使用 `tokio::time::sleep` 和 `tokio::fs`。
5. **超时兜底强制**：任何外部 IO（HTTP 调用、DB 查询、连接池获取）必须包裹 `tokio::time::timeout`（默认 5s）。
6. **禁止持锁跨 .await**：任何 `MutexGuard` 不得在持有状态下穿越 `.await` 点。

## 🔐 安全与脱敏（统一规范）

7. **敏感字段编译期脱敏**：所有包含 `password`、`secret`、`token`、`api_key` 的结构体，必须使用 `#[serde(skip_serializing)]` 或自定义 `Debug`。日志中严禁明文暴露。
8. **路径归一化**：任何静态文件服务或路径解析，必须拦截 `..` 防止目录遍历。

## 📊 性能基线（跨模块统一）

9. **启动内存 < 30MB**：空载 RSS 不得超过 30MB。
10. **测试覆盖率 ≥ 85%**：核心模块行覆盖率必须达标，由 `cargo tarpaulin` 统一检查。

## 🚨 提交流程硬约束

11. **PR 必须附带 framework 模块的 Skill 检查记录**：修改 framework 触发对应 Skills。AI 不得跳过。
12. **人类审查保留地**：中间件核心（Framework）的代码必须标记 `@REVIEW_REQUIRED`。
