# Capability Registry 五维审查报告

- **日期**: 2026-08-13
- **审查对象**: `packages/sz-rust-capability/`
- **审查人**: SZ-Rust Team

## 1. 正确性 ✅

- **参数校验**: `registry.rs:78` — `call()` 方法对 args 进行 JSON Schema 校验，未通过返回 `CapError::ValidationFailed`
- **权限检查逻辑**: `permission.rs:22` — `TenantScopeChecker` 正确检查 tenant_id 匹配
- **json_type_of 修复**: `registry.rs:45` — 正确区分 integer/number 类型（integer 是 number 子类型）
- **结论**: ✅ 参数校验完整，权限检查逻辑正确

## 2. 可读性 ✅

- **代码结构**: trait → registry → facade 三层清晰分离
- **注释**: 每个公开 API 有 doc comment，模块有 `//!` 说明
- **命名**: `Capability`/`CapabilityRegistry`/`PermissionChecker` 语义清晰
- **结论**: ✅ 代码结构清晰，命名规范

## 3. 架构 ✅

- **trait 设计**: `Capability` trait 满足 `Send + Sync + 'static`（铁律 1）
- **并发模型**: RwLock 读锁不跨 await（parking_lot::RwLockReadGuard 是 !Send），在 await 前克隆 Arc 释放锁
- **OnceLock facade**: `Cap::init()` 全局单例，重复调用 panic
- **ExtendedMcpAdapter**: 适配器模式将 McpTool 适配为 Capability，避免循环依赖
- **结论**: ✅ 架构设计合理，并发安全

## 4. 安全性 ✅

- **权限检查**: 调用前执行 `PermissionChecker::check()`，未通过返回 `PermissionDenied`
- **敏感字段脱敏**: `PaymentGatewayConfig.api_key/api_secret` 标注 `#[serde(skip_serializing)]`（铁律 7）
- **unsafe_code**: `#![forbid(unsafe_code)]` 强制禁止
- **结论**: ✅ 权限检查完整，敏感字段脱敏

## 5. 性能 ✅

- **p99 调用延迟**: ≤ 0.5ms（来自 `benches/cap_bench.rs` criterion 输出，4 benchmarks）
- **RwLock 并发**: 读多写少场景使用 RwLock，读锁不阻塞
- **Arc 克隆**: 读锁释放后克隆 Arc 开销极小（引用计数原子操作）
- **结论**: ✅ p99 ≤ 0.5ms，满足性能基线

## 总结

| 维度 | 结论 | 关键证据 |
|------|------|----------|
| 正确性 | ✅ | registry.rs:78 参数校验 + permission.rs:22 权限检查 |
| 可读性 | ✅ | 三层分离 + doc comment |
| 架构 | ✅ | Send+Sync+'static + RwLock 不跨 await |
| 安全性 | ✅ | 权限检查 + skip_serializing |
| 性能 | ✅ | p99 ≤ 0.5ms（criterion bench） |

**无 ❌ 阻断项，允许合入。**