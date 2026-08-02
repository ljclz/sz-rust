---
name: sz-rust-doc-check
description: 文档完整性检查 — 确保公共 API 有 rustdoc 文档。修改 pub 接口时触发。
tools: [cargo-doc]
agentMode: auto
---

# 文档完整性检查（sz-rust）

## 触发条件

- 新增 `pub` 函数、结构体、trait
- 修改现有公共 API 签名

## 检查步骤

1. 运行：`cargo doc --no-deps --workspace`
2. 检查是否有 `missing_docs` 警告
3. 确认所有 `pub` 项有文档注释

## 通过标准

- 无 `missing_docs` 警告
- 所有 `pub fn`、`pub struct`、`pub trait` 有 `///` 文档
- 复杂函数有 `## Examples` 代码示例

## 失败处理

补充 rustdoc 注释，格式：
```rust
/// 简短描述（一行）
///
/// 详细描述（可选）。
///
/// ## Examples
///
/// ```
/// // 代码示例
/// ```
```
