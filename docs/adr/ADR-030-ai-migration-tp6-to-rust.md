# ADR-030: AI 辅助迁移工具 TP6→Rust

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-migration/src/`, `packages/sz-rust-migration/tests/real_cases/`

## 背景

P1-1 缺口：需要将 ThinkPHP 6 项目自动迁移到 sz-rust，减少人工迁移工作量。

## 决策

1. **分析→生成→验证→报告流水线**：四阶段流水线，每阶段可独立测试
2. **增量验证策略**：先迁移路由→模型→控制器→迁移文件，每步验证编译
3. **3 个真实项目验证案例**：菜市场/餐饮/零售 TP6 项目片段
4. **MigrationReport**：`from_results`/`meets_threshold`/`summary` 方法定量评估迁移质量

## 替代方案

- **全量一次性迁移**：风险高，难以定位问题
- **手工迁移**：效率低，无法规模化

## Bug 定位提示

- `tests/real_cases/market/` — 菜市场 TP6 项目片段（route.php/Stall.php/StallController.php）
- `tests/real_cases/restaurant/` — 餐饮 TP6 项目片段
- `tests/real_cases/retail/` — 零售 TP6 项目片段
- `src/validator/response_comparator.rs` — MigrationReport 结构

## 影响

- 22 tests passed（7+7+8 三个真实案例）
- 迁移质量可量化评估（meets_threshold）
- 支持增量迁移和断点续跑