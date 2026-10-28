# ADR-026: Capability Registry 统一能力抽象

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-capability/src/registry.rs`, `packages/sz-rust-capability/src/permission.rs`

## 背景

P0-1 缺口：框架缺乏统一的能力注册与调用抽象，插件/工具/服务各自实现调用逻辑，无法统一鉴权、限流、审计。

## 决策

采用 `Capability` trait + `CapabilityRegistry` 统一抽象：

1. **Capability trait**：所有能力（工具/服务/插件）实现 `async fn call(&self, args) -> Result<Value>`
2. **RwLock 并发模型**：注册用写锁，调用用读锁，读锁不跨 await（parking_lot::RwLockReadGuard 是 !Send）
3. **OnceLock facade**：`Cap` 静态 facade 通过 OnceLock 保证全局单例
4. **PermissionChecker 集成**：调用前检查权限，支持 AllowAll/TenantScopeChecker

## 替代方案

- **trait object + Box<dyn Fn>**：缺乏 async 支持，无法表达 Send + 'static 约束
- **Erlang-style 消息传递**：过度工程化，Rust 生态不成熟

## Bug 定位提示

- `registry.rs:45` — `register()` 写锁获取，注意不持有锁跨 await
- `registry.rs:78` — `call()` 读锁获取后克隆 Arc 释放锁再 await
- `permission.rs:22` — `TenantScopeChecker` 多租户隔离逻辑
- `facade.rs:15` — `Cap::init()` OnceLock 初始化，重复调用 panic

## 影响

- 所有 MCP 工具通过 `ExtendedMcpAdapter` 适配为 Capability
- 权限检查在调用前执行，未通过返回 `CapError::PermissionDenied`
- p99 调用延迟 ≤ 0.5ms（来自 criterion bench 输出）