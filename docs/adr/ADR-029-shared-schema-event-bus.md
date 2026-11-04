# ADR-029: 共享 Schema + 插件事件总线

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-core/src/plugin/schema.rs`, `packages/sz-rust-core/src/plugin/event_bus.rs`

## 背景

P0-4 缺口：插件之间无法数据互通，缺乏统一的事件总线和安全的数据共享机制。

## 决策

1. **共享 Schema**：`sys_users`/`sys_permissions`/`sys_events` 三张共享表，多租户隔离（tenant_id）
2. **EventBus**：`async fn publish(event)` + `async fn subscribe(handler)`，at-least-once 投递
3. **幂等性**：事件携带 `EventId`（UUID），消费者去重
4. **多租户隔离**：所有事件携带 `tenant_id`，CrossQuery 自动注入

## 替代方案

- **Kafka/Redis Streams**：外部中间件依赖重，开发环境不友好
- **Channel-based**：tokio::mpsc 无多订阅者支持

## Bug 定位提示

- `plugin/schema.rs:14` — `SysUser` 结构，注意 tenant_id 字段
- `plugin/event_bus.rs:21` — `PluginEvent` 结构，EventId 去重
- `plugin/event_bus.rs:76` — `InMemoryEventBus::new()` 构造函数
- `plugin/cross_query.rs:12` — `CrossQueryError` 多租户权限拒绝

## 影响

- 插件可通过 EventBus 发布/订阅事件
- CrossQuery 支持跨插件安全数据查询
- 13 tests passed（event_bus_test + cross_query_test）