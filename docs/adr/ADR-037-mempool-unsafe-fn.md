# ADR-037: MemPool 区域分配器 API 收紧为 unsafe fn

- **日期**：2026-08-16
- **状态**：已接受
- **相关代码**：`packages/sz-rust-core/src/mem_pool.rs`（trait 定义 55-70 行，实现 124-160/221-236 行）

## 背景

外部代码审查（2026-08-16）指出 `BumpaloPool::alloc_str/alloc_bytes` 使用
`std::mem::transmute` 将池内引用延长到调用方输入生命周期 `'a`（mem_pool.rs:209,216），
`reset()` 后继续使用引用构成 use-after-free。

复核结论：
- 技术机制属实：arena allocator 标准模式，但**无编译期保证**
- 严重性修正：MemPool 生产 0 调用 + `mem-pool`/`bumpalo-pool` feature 默认关闭
  （Cargo.toml:119 `default = []`），当前无触发路径——定性为"未激活的 unsound API 设计缺陷"
- **真正的根因**：`alloc_str`/`alloc_bytes`/`reset` 均为 **safe fn**，却依赖
  "reset 前引用有效"这一 unsafe 不变量——Safe Rust 调用方可触发 UB 而无需写 unsafe 块
  （借用检查器无法表达"引用在 &self 的 reset 后失效"）

## 决策

将 `MemPool::alloc_str` / `MemPool::alloc_bytes` 从 safe fn 改为 **unsafe fn**：

```rust
unsafe fn alloc_str<'a>(&self, s: &'a str) -> &'a str;
unsafe fn alloc_bytes<'a>(&self, b: &'a [u8]) -> &'a [u8];
```

调用方必须写 `unsafe` 块，显式承担"使用返回引用期间不得调用 reset()"的契约。

## 备选方案

1. **保持 safe fn + 文档强化**：不改变 soundness 缺陷，Safe Rust 仍可触发 UB——拒绝
2. **`&mut self` + 借用生命周期**（返回 `&'self str`，reset 也 `&mut self`）：
   借用检查器可保证安全，但丧失并发共享分配能力（trait 文档承诺 `&self` 多线程并发）——
   拒绝，语义退化
3. **unsafe fn**（采纳）：契约显式化，调用方承担责任，保留并发分配能力；
   与 bumpalo 原版 `Bump::alloc_str`（unsafe 语义）对齐

## 影响

- **pub API 变更**：`MemPool` trait 方法签名变更（safe → unsafe）
- 实现同步：`StackPool`（mem_pool.rs:124,147）、`BumpaloPool`（:221,230）加 `unsafe` 修饰
- 测试同步：13 个 mem_pool 测试调用处包 `unsafe {}`（mem_pool.rs:320-440）
- 全 workspace 无其他调用方（grep 确认，MemPool 生产 0 使用）
- feature 门控不变：`mem-pool`/`bumpalo-pool` 默认关闭

## Bug 定位提示

若启用 `mem-pool` feature 后出现编译错误，检查调用处是否遗漏 `unsafe` 块；
若出现 use-after-free（reset 后引用内容被覆盖），检查调用方是否在持有返回引用期间
调用了 `reset()`（含其他线程的 `reset()`，trait 为 `Send + Sync`，并发 reset 与 alloc
需调用方自行同步）。
