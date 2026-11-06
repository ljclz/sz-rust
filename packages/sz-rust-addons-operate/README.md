# sz-rust-addons-operate

> CRUD 参考实现 · 起步模板 · 非核心依赖

## 定位

`sz-rust-addons-operate` 是 SZ-Rust 对齐 ThinkPHP `addons/operate` 的 **CRUD 参考实现**，定位为**起步模板**而非核心依赖。提供最完整的增删改查范式，供新建业务项目时复制改写，不应作为生产依赖直接引入。

## 规模

- 源码行数：**17316 行**（`src/**/*.rs`，2026-11-06 实测）
- 溯源命令：`Get-ChildItem -Path packages/sz-rust-addons-operate/src -Recurse -Filter *.rs | Get-Content | Measure-Object -Line`
- 无独立 `tests/` 目录（测试内联于 `#[cfg(test)]` 模块）

## 用途

1. **CRUD 范式参考**：展示 SZ-Rust 控制器 / 模型 / 验证器 / 中间件的完整增删改查写法，对齐 ThinkPHP `addons/operate` 的接口风格。
2. **项目起步模板**：新建业务项目时可复制本包结构改写，快速搭建 CRUD 骨架。
3. **非核心依赖**：本包不进入 `sz-rust-core` / `sz-rust-sz300` 的依赖链，仅作参考。`Cargo.toml` description 已标注 `[template crate — 起步模板，非核心依赖]`。

## 相关

- 同类模板包：`sz-rust-addons-{cms,crm,ecommerce,erp,forum,im}`（骨架规模较小，各自面向一个业务领域）
- 机制层：`sz-rust-addons-loader`（插件加载器，非模板）
- 功能插件：`sz-rust-addons-admin`（后台管理插件，非模板）
- 包定位决策：ADR-052（headless API）、ADR-053（包用途标注）