# ADR-AI-03: 模型路由 — ArcSwap 无锁热替换

- **状态**: Accepted
- **日期**: 2026-08-10
- **相关代码**: `packages/sz-rust-ai-facade/src/llm/router.rs`

## 背景

生产环境需要运行时切换模型路由（如灰度发布、故障切换、A/B 测试），不能重启服务。

## 决策

使用 `arc_swap::ArcSwap` 实现无锁热替换路由表：

```rust
pub struct ModelRouter {
    routes: ArcSwap<HashMap<String, ProviderRef>>,
    default_model: ArcSwap<String>,
}

pub fn apply_update(&self, routes: HashMap<String, ProviderRef>, default_model: String) {
    self.routes.store(Arc::new(routes));
    self.default_model.store(Arc::new(default_model));
}
```

## 理由

- **无锁读**：`route()` 调用 `ArcSwap::load()` 无竞争，O(1)
- **原子写**：`apply_update()` 原子替换整个路由表，无中间状态
- **无 unsafe**：`ArcSwap` 内部安全实现，符合 workspace `unsafe_code = "forbid"` 约束

## 代价

- 整体替换（非细粒度单条路由更新），但路由表通常 < 100 条，开销可忽略

## 影响

配合 `ProviderFailover` 实现运行时故障切换无停机。