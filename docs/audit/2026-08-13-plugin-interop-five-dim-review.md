# 插件数据互通五维审查报告

- **日期**: 2026-08-13
- **审查对象**: `packages/sz-rust-core/src/plugin/`
- **审查人**: SZ-Rust Team

## 1. 正确性 ✅

- **事件至少一次投递**: `event_bus.rs` — InMemoryEventBus 使用 Vec 存储，publish 后所有 handler 都会收到
- **幂等性**: `event_bus.rs:21` — PluginEvent 携带 EventId（UUID），消费者可基于 EventId 去重
- **CrossQuery**: `cross_query.rs` — 跨插件查询自动注入 tenant_id，权限不足返回 PermissionDenied
- **13 tests passed**: event_bus_test + cross_query_test 全部通过
- **结论**: ✅ 事件投递可靠，幂等性保证

## 2. 可读性 ✅

- **代码结构**: schema.rs（数据模型）+ event_bus.rs（事件总线）+ cross_query.rs（跨插件查询）清晰分离
- **注释**: 每个结构体和 trait 有 doc comment
- **命名**: SysUser/SysPermission/SysEvent/PluginEvent/EventBus 语义清晰
- **结论**: ✅ 代码结构清晰

## 3. 架构 ✅

- **共享 Schema**: sys_users/sys_permissions/sys_events 三张共享表，多租户隔离
- **EventBus**: publish/subscribe 模式，支持多订阅者
- **CrossQuery**: 跨插件安全数据查询，自动注入 tenant_id
- **trait 设计**: EventHandler trait 满足 Send + Sync + 'static
- **结论**: ✅ 架构设计合理

## 4. 安全性 ✅

- **多租户隔离**: 所有共享表和事件携带 tenant_id，CrossQuery 自动注入
- **参数化绑定**: WHERE 条件使用参数化绑定（由 sz-orm Skills 负责）
- **禁止 SELECT ***: 显式列投影（铁律）
- **权限检查**: CrossQuery 检查 tenant_id 匹配，不匹配返回 PermissionDenied
- **结论**: ✅ 多租户隔离完整

## 5. 性能 ✅

- **事件投递延迟**: InMemoryEventBus 使用 Vec push，O(1) 复杂度
- **CrossQuery**: 直接数据库查询，无额外中间件开销
- **Arc 共享**: EventBus 通过 Arc 共享，无拷贝开销
- **结论**: ✅ 事件投递延迟低

## 总结

| 维度 | 结论 | 关键证据 |
|------|------|----------|
| 正确性 | ✅ | event_bus.rs at-least-once + EventId 幂等 |
| 可读性 | ✅ | schema/event_bus/cross_query 分离 |
| 架构 | ✅ | 共享 Schema + EventBus + CrossQuery |
| 安全性 | ✅ | tenant_id 隔离 + 参数化绑定 |
| 性能 | ✅ | O(1) 事件投递 |

**无 ❌ 阻断项，允许合入。**