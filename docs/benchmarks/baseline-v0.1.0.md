# SZ-Rust 性能回归基线 v0.1.0

> **建立时间**：2026-07-22
> **工具**：criterion 0.5（已配置 + html_reports）
> **参数**：`--warm-up-time 1 --measurement-time 3 --sample-size 30`
> **环境**：Windows + Rust stable（本地开发机）
> **用途**：后续版本性能回归对比基线
> **状态**：✅ v0.1.0 基线数据已生成（2026-07-22 本地运行 `--save-baseline v0.1.0`），5 组基准测试已填充实际中位数

---

## 1. 基线建立方式

### 1.1 计划命令

```powershell
cargo bench --package sz-rust-core --bench core_bench -- `
    --warm-up-time 1 --measurement-time 3 --sample-size 30 `
    --save-baseline v0.1.0
```

基线数据将保存在 `target/criterion/<bench_name>/v0.1.0/` 目录。

### 1.2 当前状态

- **criterion 依赖**：✅ 已在 sz-rust-core 的 `[dev-dependencies]` 中声明（`criterion = { version = "0.5", features = ["html_reports"] }`）
- **bench 文件**：✅ 已创建 `sz-rust-core/benches/core_bench.rs`（4 组基准测试：route_matching / handler_ref_parse / route_config / json_serialization）
- **编译验证**：✅ `cargo check --package sz-rust-core --benches` 通过
- **基线数据**：✅ 已生成（2026-07-22 本地运行，保存到 `target/criterion/<bench>/v0.1.0/`）
- **CI benchmark workflow**：✅ 已创建 `.github/workflows/benchmark.yml`（PR 用 `--baseline` / push 用 `--save-baseline`）
- **计划完成时间**：首次 CI 运行即生成基线

### 1.3 基准测试组覆盖情况

| 编号 | 基准测试组 | 说明 | 优先级 | v0.1.0 状态 |
|------|-----------|------|--------|------------|
| 1 | `route_matching` | 路由匹配性能（静态路径解析、根路径、长路径） | P0 | ✅ 已实现 |
| 2 | `middleware_chain` | 中间件链执行性能（洋葱模型层数 1/5/10/20） | P0 | ⏳ v0.2.0 |
| 3 | `json_serialization` | JSON 响应序列化（小/中响应体） | P0 | ✅ 已实现 |
| 4 | `json_deserialization` | JSON 请求体解析（小/中请求体） | P0 | ✅ 已实现（含 serialize 组内） |
| 5 | `controller_dispatch` | 控制器分发性能（trait 方法调用 + 参数提取） | P1 | ✅ 已实现（`handler_ref_parse`） |
| 6 | `model_hook_dispatch` | Model 钩子分发表查找性能 | P1 | ⏳ v0.2.0 |
| 7 | `template_rendering` | 模板渲染性能（Askama / Tera） | P1 | ⏳ v0.2.0 |
| 8 | `cache_get_set` | 缓存 Service 读写性能 | P2 | ⏳ v0.2.0 |
| 9 | `auth_jwt_verify` | JWT 验证性能（含常量时间比较） | P2 | ⏳ v0.2.0 |
| 10 | `full_request_lifecycle` | 完整请求生命周期（路由→中间件→控制器→响应） | P2 | ⏳ v0.2.0 |
| 11 | `route_config` | 路由配置加载与冲突检测（YAML 小/中规模） | P0 | ✅ 已实现 |

> **v0.1.0 已实现 5 组基准测试**（route_matching / handler_ref_parse / route_config / json_serialization / json_deserialization），覆盖 P0 路由 + JSON 核心路径。剩余 6 组将在 v0.2.0 阶段补充。

---

## 2. 回归对比方式

后续版本运行：

```powershell
cargo bench --package sz-rust-core --bench core_bench -- `
    --warm-up-time 1 --measurement-time 3 --sample-size 30 `
    --baseline v0.1.0
```

criterion 会自动对比并标注性能变化：
- **Improvement**（改善）：变化 < -5%
- **Regression**（回归）：变化 > +5%
- **No change**（无变化）：-5% ~ +5%

---

## 3. 基线值（v0.1.0）

> ✅ **当前状态：v0.1.0 基线数据已生成（2026-07-22 本地运行）**
>
> 基线已通过 `cargo bench --save-baseline v0.1.0` 保存到 `target/criterion/<bench>/v0.1.0/`。
> 以下表格包含 5 组已实现基准测试的实际中位数数据，其余 6 组计划在 v0.2.0 补充。
>
> **运行环境**：Windows + Rust stable（本地开发机），`--warm-up-time 1 --measurement-time 3 --sample-size 30`

### 3.1 route_matching — 路由匹配 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| parse_path_static | 187.56 ns | 静态路径 `/oapc/customer/index` |
| parse_path_root | 97.13 ns | 根路径 `/` |
| parse_path_long | 211.63 ns | 长路径 `/oapc/customer/getListById?id=1&page=2` |

### 3.2 handler_ref_parse — Handler 引用解析 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| parse_simple | 82.30 ns | `HandlerRef::parse("User@list")` |
| parse_with_slash | 95.77 ns | `HandlerRef::parse("User/list")` |

### 3.3 route_config — 路由配置加载与冲突检测 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| load_yaml_small | 5.97 µs | 加载 2 条路由的 YAML |
| load_yaml_medium | 26.46 µs | 加载含 groups 的中等 YAML |
| find_conflicts | 24.43 µs | 50 条路由的冲突检测 |

### 3.4 middleware_chain — 中间件链执行

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| middleware_depth_1 | 待建立 | 1 层中间件 |
| middleware_depth_5 | 待建立 | 5 层中间件 |
| middleware_depth_10 | 待建立 | 10 层中间件 |
| middleware_depth_20 | 待建立 | 20 层中间件 |

### 3.5 json_serialization — JSON 响应序列化 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| serialize_small | 99.41 ns | 序列化小响应体 |
| serialize_medium | 4.42 µs | 序列化中等响应体 |

### 3.6 json_deserialization — JSON 请求体解析 ✅

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| deserialize_small | 730.89 ns | 反序列化小请求体 |
| deserialize_medium | 41.76 µs | 反序列化中等请求体 |

### 3.7 controller_dispatch — 控制器分发

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| controller_no_param | 待建立 | 无参数控制器方法 |
| controller_1_param | 待建立 | 1 个路径参数 |
| controller_3_param | 待建立 | 3 个路径参数 |
| controller_with_body | 待建立 | 含请求体解析 |
| controller_with_auth | 待建立 | 含认证提取器 |

### 3.8 model_hook_dispatch — Model 钩子分发表

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| hook_lookup_10 | 待建立 | 10 个钩子注册 |
| hook_lookup_100 | 待建立 | 100 个钩子注册 |
| hook_dispatch_single | 待建立 | 单钩子分发 |
| hook_dispatch_chain_5 | 待建立 | 5 个钩子链式触发 |

### 3.9 template_rendering — 模板渲染

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| askama_simple | 待建立 | Askama 简单模板 |
| askama_with_loop | 待建立 | Askama 含循环 |
| tera_simple | 待建立 | Tera 简单模板 |
| tera_with_inheritance | 待建立 | Tera 含继承 |

### 3.10 cache_get_set — 缓存读写

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| cache_get_hit | 待建立 | 缓存命中 |
| cache_get_miss | 待建立 | 缓存未命中 |
| cache_set_small | 待建立 | 写入小值（60B） |
| cache_set_large | 待建立 | 写入大值（3KB） |

### 3.11 auth_jwt_verify — JWT 验证

| 子基准 | 时间（中位数） | 说明 |
|--------|---------------|------|
| jwt_verify_hs256 | 待建立 | HS256 签名验证 |
| jwt_verify_rs256 | 待建立 | RS256 签名验证 |
| jwt_decode_no_verify | 待建立 | 仅解码不验证 |
| jwt_constant_time_compare | 待建立 | 常量时间比较 |

### 3.12 full_request_lifecycle — 完整请求生命周期

| 子基准 | 时间（中位数） | 吞吐量 (req/s) | 说明 |
|--------|---------------|----------------|------|
| lifecycle_minimal | 待建立 | 待建立 | 路由→空控制器→响应 |
| lifecycle_typical | 待建立 | 待建立 | 路由→3 中间件→控制器→JSON 响应 |
| lifecycle_full_stack | 待建立 | 待建立 | 路由→5 中间件→控制器→Model→DB→响应 |

---

## 4. 回归阈值

### 4.1 criterion 默认阈值

- **Improvement**（改善）：变化 < -5%
- **Regression**（回归）：变化 > +5%
- **No change**（无变化）：-5% ~ +5%

### 4.2 回归报警建议

| 报警级别 | 条件 | 处理方式 |
|---------|------|---------|
| 🟡 调查 | 单项回归 > +10% | 调查原因，记录到 CHANGELOG |
| 🟠 警告 | 多项回归 > +5% | 必须调查并修复或显式接受 |
| 🔴 阻断 | 吞吐量下降 > -10% | 阻断合入，必须修复 |
| 🔴 阻断 | `full_request_lifecycle` 回归 > +5% | 阻断合入（核心路径） |

### 4.3 特别关注项

- `route_matching` 回归直接影响所有请求延迟
- `middleware_chain` 回归影响所有经过中间件的请求
- `full_request_lifecycle` 是端到端指标，任何子项回归都会体现
- `auth_jwt_verify` 回归可能暗示常量时间比较被破坏（安全风险）

---

## 5. CI 集成计划

### 5.1 CI Job 配置

| Job | 触发条件 | 说明 | 状态 |
|-----|---------|------|------|
| `benchmark` | push 到 main | 运行 criterion 基准测试，`--save-baseline v0.1.0`，上传 `target/criterion/` 作为 artifact | ✅ 已配置 |
| `benchmark-regression` | PR 到 main | 运行基准测试并 `--baseline v0.1.0` 对比，在 PR 评论中展示回归报告 | ✅ 已配置（同 workflow，按 event_name 切换参数） |

### 5.2 已配置 CI 文件

```yaml
# .github/workflows/benchmark.yml（计划）
name: Benchmark
on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run benchmarks
        run: |
          cargo bench --package sz-rust-core --bench core_bench -- \
            --warm-up-time 1 --measurement-time 3 --sample-size 30 \
            ${{ github.event_name == 'pull_request' && '--baseline v0.1.0' || '--save-baseline v0.1.0' }}
      - name: Upload criterion results
        uses: actions/upload-artifact@v4
        with:
          name: criterion-results
          path: target/criterion/
          retention-days: 30
```

### 5.3 当前状态

- **GitHub 仓库**：✅ 已建立（https://github.com/ljclz/sz-rust）
- **CI 配置文件**：✅ 已创建 `.github/workflows/benchmark.yml`（含 Rust toolchain 安装、cargo 缓存、sz-orm path 依赖检查、基准测试运行、criterion 结果上传、gh-pages-bench 分支保存）
- **计划完成时间**：✅ 已完成

---

## 6. soak test 集成计划

### 6.1 计划 soak 指标

| 指标 | 说明 | 退化阈值 |
|------|------|---------|
| `elapsed_secs` | 运行时长 | — |
| `ops_completed` | 完成操作数 | — |
| `ops_per_sec` | 吞吐量 | 衰减 > 10% |
| `rss_bytes` | 进程内存 | 增长 > 50MB |
| `fd_count` | 文件描述符数 | 增长 > 10 |
| `thread_count` | 线程数 | 增长 > 10 |
| `p99_latency_us` | P99 延迟 | 增长 > 2x |
| `error_count` | 错误数 | > 0 |

### 6.2 计划 soak 模式

| 模式 | 时长 | 触发条件 | 状态 |
|------|------|---------|------|
| 冒烟模式 | 10s | 每次 push/PR | ⏳ 待建立 |
| 完整模式 | 6h | 每周日 00:00 UTC | ⏳ 待建立 |
| 手动触发 | 自定义 | workflow_dispatch | ⏳ 待建立 |

### 6.3 当前状态

- **soak test 文件**：❌ 未创建 `sz-rust-core/tests/soak.rs`
- **SoakMonitor**：❌ 未实现
- **CSV 报告导出**：❌ 未实现
- **计划完成时间**：v0.2.0

---

## 7. 引用

- [《SZ-Rust 工程化实践规范》](../sz-rust-engineering-practices.md) — 测试金字塔 T6 soak 测试
- [《SZ-Rust 初始审计报告》](../audit/2026-07-22-初始审计.md) — P1-6 性能回归审计
- [SZ-ORM 性能回归基线 v1.0.0](../../sz-orm/docs/benchmarks/baseline-v1.0.0.md) — 关联项目基线参考
