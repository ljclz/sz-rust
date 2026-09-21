# SZ-Rust 性能回归基线 v0.2.1

> **建立时间**：2026-08-01
> **工具**：criterion 0.5
> **参数**：`--warm-up-time 1 --measurement-time 3 --sample-size 30`
> **环境**：Windows + Rust stable（本地开发机，与 v0.1.0 同机）
> **命令**：`cargo bench --package sz-rust-core --bench core_bench -- --warm-up-time 1 --measurement-time 3 --sample-size 30 --save-baseline v0.2.1`
> **状态**：✅ v0.2.1 基线已生成（6 组 16 项基准全部填充实际中位数）
> **数据来源**：`target/criterion/<group>/<bench>/v0.2.1/estimates.json`（median.point_estimate，单位 ns）

---

## 1. 本次新增内容（相对 v0.1.0）

| 项 | v0.1.0 | v0.2.1 |
|----|--------|--------|
| 基准测试组数 | 4 组 | 6 组 |
| 子基准数 | 12 项 | 16 项 |
| middleware_chain 组 | ❌ 未实现 | ✅ 6 项（default_chain / push_5 / service_builder_order / remove_from_auth / has_duplicates / contains_auth） |
| di_container 组 | ❌ 未实现 | ✅ 4 项（bind_and_make_transient / singleton_reuse / scoped_make / make_missing） |

> 对应代码：`sz-rust-core/benches/core_bench.rs`（criterion_group 注册 `bench_middleware_chain` + `bench_di_container`）

---

## 2. 基线值（v0.2.1）

### 2.1 route_matching — 路由匹配 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| parse_path_static | 175.59 ns | 静态路径 `/oapc/customer/index` |
| parse_path_root | 92.33 ns | 根路径 `/` |
| parse_path_long | 1009.15 ns | 长路径 `/oapc/customer/getListById?id=1&page=2` |

### 2.2 handler_ref_parse — Handler 引用解析 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| parse_simple | 86.71 ns | `HandlerRef::parse("User@list")` |
| parse_with_slash | 90.30 ns | `HandlerRef::parse("User/list")` |

### 2.3 route_config — 路由配置加载与冲突检测 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| load_yaml_small | 8348.65 ns | 加载 2 条路由的 YAML |
| load_yaml_medium | 43629.29 ns | 加载含 groups 的中等 YAML |
| find_conflicts | 19788.25 ns | 50 条路由的冲突检测 |

### 2.4 json_serialization — JSON 序列化/反序列化 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| serialize_small | 89.14 ns | 序列化小响应体 |
| serialize_medium | 4250.62 ns | 序列化中等响应体 |
| deserialize_small | 598.88 ns | 反序列化小请求体 |
| deserialize_medium | 42832.98 ns | 反序列化中等请求体 |

### 2.5 middleware_chain — 中间件链构建与操作 ✅（v0.2.1 新增）

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| default_chain | 29.46 ns | `MiddlewareChain::default_chain()`（5 个中间件） |
| push_5 | 46.88 ns | 空链 push 5 个中间件 |
| service_builder_order | 31.98 ns | 默认链 → ServiceBuilder 注册逆序 |
| remove_from_auth | 30.24 ns | 默认链 remove_from(Auth) |
| has_duplicates | 83.28 ns | 默认链重复检测 |
| contains_auth | 1.67 ns | 默认链包含 Auth 查询 |

### 2.6 di_container — DI 容器注册与解析 ✅（v0.2.1 新增）

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| bind_and_make_transient | 74.25 ns | bind 后 make（每次新建实例） |
| singleton_reuse | 24.92 ns | singleton 后 make（缓存命中） |
| scoped_make | 36.75 ns | scoped + make_with_scope(1) |
| make_missing | 15.75 ns | make 未注册类型（返回 None） |

---

## 3. 与 v0.1.0 对比（2026-07-22 → 2026-08-01）

> ⚠️ 两轮基线间隔 10 天，期间存在路由/配置模块代码变更，以下差异为**实测数据**，仅供参考，具体原因需结合代码 diff 调查。

| 子基准 | v0.1.0 | v0.2.1 | 变化 | 判定 |
|--------|--------|--------|------|------|
| parse_path_static | 187.56 ns | 175.59 ns | -6.4% | ✅ 改善 |
| parse_path_root | 97.13 ns | 92.33 ns | -4.9% | ➖ 无变化 |
| parse_path_long | 211.63 ns | 1009.15 ns | +376.8% | 🔴 显著回归，需调查 |
| parse_simple | 82.30 ns | 86.71 ns | +5.4% | 🟡 需关注 |
| parse_with_slash | 95.77 ns | 90.30 ns | -5.7% | ✅ 改善 |
| load_yaml_small | 5.97 µs | 8.35 µs | +39.9% | 🟠 显著变慢，需调查 |
| load_yaml_medium | 26.46 µs | 43.63 µs | +64.9% | 🟠 显著变慢，需调查 |
| find_conflicts | 24.43 µs | 19.79 µs | -19.0% | ✅ 改善 |
| serialize_small | 99.41 ns | 89.14 ns | -10.3% | ✅ 改善 |
| serialize_medium | 4.42 µs | 4.25 µs | -3.8% | ➖ 无变化 |
| deserialize_small | 730.89 ns | 598.88 ns | -18.1% | ✅ 改善 |
| deserialize_medium | 41.76 µs | 42.83 µs | +2.6% | ➖ 无变化 |

### 需调查项（不主观定因，仅记录事实）

1. **parse_path_long（+376.8%）**：v0.1.0 记录值 211.63 ns 与本次 1009.15 ns 差异巨大，且 `parse_path_long`（含 query string 解析）与 `parse_path_static`（-6.4%）走势相反。需复查 v0.1.0 数据采集是否含 query 分支，或 `parse_path` 实现自 07-22 后有变更。
2. **load_yaml_small / load_yaml_medium（+40% / +65%）**：YAML 加载显著变慢，与 `find_conflicts`（-19%）反向。可能涉及 `routing.rs` 的 YAML 解析实现或 serde_yaml 版本变更，需结合 `git log` 调查。

> 处理建议：下轮基准（v0.3.0 或 CI 首跑）与 v0.2.1 对比，若上述两项仍显著偏差，再定位代码变更点；同时建议为 `parse_path` / `load_routes_from_yaml_str` 添加针对性单元测试锁定行为。

---

## 4. 回归阈值（沿用 v0.1.0 约定）

- **Improvement**（改善）：变化 < -5%
- **Regression**（回归）：变化 > +5%
- **No change**（无变化）：-5% ~ +5%

报警级别：

| 报警级别 | 条件 | 处理方式 |
|---------|------|---------|
| 🟡 调查 | 单项回归 > +10% | 调查原因，记录到 CHANGELOG |
| 🟠 警告 | 多项回归 > +5% | 必须调查并修复或显式接受 |
| 🔴 阻断 | 吞吐量下降 > -10% | 阻断合入，必须修复 |

---

## 5. 后续待补基准组（v0.1.0 计划遗留）

| 基准组 | 优先级 | 状态 |
|--------|--------|------|
| controller_dispatch（controller_no_param / 1_param / 3_param / with_body / with_auth） | P1 | ⏳ 待建立 |
| model_hook_dispatch（hook_lookup_10/100 / dispatch_single / chain_5） | P1 | ⏳ 待建立 |
| template_rendering（askama / tera） | P1 | ⏳ 待建立 |
| cache_get_set（get_hit / get_miss / set_small / set_large） | P2 | ⏳ 待建立 |
| auth_jwt_verify（hs256 / rs256 / decode / constant_time） | P2 | ⏳ 待建立 |
| full_request_lifecycle（minimal / typical / full_stack） | P2 | ⏳ 待建立 |

---

## 6. 引用

- [v0.1.0 基线](../benchmarks/baseline-v0.1.0.md) — 首版基线
- [v0.2.0 回归报告](../benchmarks/regression-report-v0.2.0.md)
- [《SZ-Rust 工程化实践规范》](../sz-rust-engineering-practices.md)
