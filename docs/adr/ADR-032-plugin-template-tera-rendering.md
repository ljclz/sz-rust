# ADR-032: 插件模板库 Tera 渲染

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-cli/templates/`, `packages/sz-rust-cli/src/template_engine.rs`

## 背景

P1-3 缺口：开发者创建插件需要手动编写大量样板代码，缺乏模板化生成能力。

## 决策

1. **4 类模板**：plugin-workflow（6 模板）+ plugin-report（6 模板）+ plugin-api + plugin-model
2. **Tera 引擎**：支持变量替换、条件渲染、循环、自定义过滤器
3. **铁律检查集成**：SafetyValidator 在模板生成后检查 unsafe/std::fs/unwrap
4. **自定义过滤器**：pascal_case/snake_case/tojson

## 替代方案

- **Askama**：编译期模板，灵活性不足
- **Handlebars**：功能足够但 Rust 生态中 Tera 更成熟

## Bug 定位提示

- `templates/plugin-workflow/` — 6 个工作流模板
- `templates/plugin-report/` — 6 个报告模板
- `src/template_engine.rs` — pascal_case/snake_case 过滤器注册
- `src/safety_validator.rs` — SafetyValidator 铁律检查

## 影响

- 289 tests passed
- `make.rs` 支持 4 类模板一键生成
- 生成代码自动通过安全检查