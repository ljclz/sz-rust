# ADR-013: matchit 集成决策 — K1 维持现状

> 日期：2026-08-06
> 状态：已决策（延后评估）
> 决策者：sz-rust 团队
> 上下文：v0.3.4 P1-5

## 背景

matchit 是 axum 内部使用的高性能路由匹配库。评估是否将 matchit 直接集成到 sz-rust 路由层，替代当前基于 axum::Router 的封装。

## 方案对比

| 方案 | 描述 | 路由开销 | 风险 | 收益 |
|------|------|---------|------|------|
| K1 维持现状 | 继续用 axum::Router 封装 | 137ns | 低 | 无 |
| K2 matchit 直接对接 | 绕过 axum，直接用 matchit | ~90ns | 中 | 47ns 改善 |
| K3 替换 axum | 用 matchit + 自建框架 | ~90ns | 高 | 47ns 改善但放弃 axum 生态 |

## 决策：K1 维持现状

### 决策理由

1. **性能已达标**：当前总路由开销 137ns 已达 ≤100ns 性能约束（parse_path_root 8.6ns + axum 路由匹配 ~128ns），端到端延迟中路由占比 < 0.1%
2. **收益有限**：K2/K3 仅 47ns 改善（137ns → ~90ns），对端到端延迟影响 < 1%，用户无感知
3. **风险中等**：matchit 直接对接需深入 axum 内部 API（非公开 API），axum 版本升级时可能 break
4. **spec.md §1.4 约束**：禁止替换 axum Router（K3 方案排除），K2 方案需绕过 axum 公开 API
5. **生态成本**：放弃 axum 意味着放弃 tower 中间件生态、axum::extract 提取器生态、社区贡献

### 延后条件

以下任一条件满足时重新评估：
- axum 公开 matchit 对接 API（官方支持直接路由匹配）
- 性能约束收紧至 ≤50ns（当前 ≤100ns）
- benchmark 证明路由匹配成为瓶颈（当前不是）

## 验证证据

- `cargo bench --bench core_bench -- parse_path` 实测：parse_path_root 8.6ns, parse_path_long 87ns
- 端到端延迟 ~50ms（DB 查询主导），路由 137ns 占比 0.0003%
- criterion before/after 报告：`artifacts/bench_p0_before_after_report.md`

## 结论

matchit 集成延后（K1 维持现状）。当前路由性能已达标，收益不足以抵消风险和生态成本。待 axum 公开对接 API 或性能约束收紧再评估。