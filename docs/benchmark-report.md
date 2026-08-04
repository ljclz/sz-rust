# SZ-Rust 性能基准报告

> **测量日期**：2026-08-03
> **环境**：Windows 10 x64 / i5-12400 / stable-x86_64-pc-windows-msvc rustc 1.97.1（debug 未优化？否——criterion 默认 release 编译）
> **工具**：criterion 0.5（warm-up 1s / measurement 3s / sample 30）
> **命令**：`cargo bench --package sz-rust-core --bench core_bench`
> **数据**：中位数（[min, max] 95% 置信区间省略，取中间值）

## 1. 路由匹配（route_matching）

| 基准 | 耗时 | 换算吞吐 |
|------|------|---------|
| `parse_path_static`（/oapc/customer/index） | 197.08 ns | ≈ 507 万 ops/s |
| `parse_path_root`（/） | 97.61 ns | ≈ 1024 万 ops/s |
| `parse_path_long`（含查询串） | 209.03 ns | ≈ 478 万 ops/s |

路由解析亚微秒级（< 210 ns），三层路由机制（属性宏/配置式/约定式）的核心路径无性能瓶颈。

## 2. HandlerRef 解析（handler_ref_parse）

| 基准 | 耗时 |
|------|------|
| `parse_simple`（User@list） | 99.09 ns |
| `parse_with_slash` | 103.09 ns |

## 3. 路由配置加载（route_config）

| 基准 | 耗时 |
|------|------|
| `load_yaml_small`（小配置） | 7.81 µs |
| `load_yaml_medium`（中配置） | 66.94 µs |
| `find_conflicts`（冲突检测） | 25.88 µs |

配置加载毫秒以下，适合启动期一次性解析。

## 4. JSON 序列化（json_serialization）

| 基准 | 耗时 | 说明 |
|------|------|------|
| `serialize_small` | 93.26 ns | ≈ 1072 万 ops/s |
| `serialize_medium` | 4.72 µs | ≈ 21 万 ops/s |
| `deserialize_small` | 675.34 ns | |
| `deserialize_medium` | 45.29 µs | |

## 5. 中间件链（middleware_chain）

| 基准 | 耗时 |
|------|------|
| `default_chain`（默认链构建） | 31.07 ns |
| `push_5`（追加 5 个中间件） | 51.94 ns |
| `service_builder_order` | 33.02 ns |
| `remove_from_auth` | 32.49 ns |
| `has_duplicates` | 80.43 ns |
| `contains_auth` | 4.55 ns |

中间件链操作纳秒级，多中间件组合无性能顾虑。

## 6. DI 容器（di_container）

| 基准 | 耗时 | 说明 |
|------|------|------|
| `bind_and_make_transient` | 242.79 ns | ≈ 412 万次/秒 |
| `singleton_reuse` | 42.83 ns | 单例命中 |
| `scoped_make` | 75.12 ns | 请求作用域 |
| `make_missing` | 29.33 ns | 未注册快速失败 |

## 7. 结论

- **路由/中间件/DI 热路径全部亚微秒级**（< 250 ns），请求处理开销主要由 axum/hyper 网络栈承担
- 单请求框架层附加成本估算：parse_path（197ns）+ 中间件链（~80ns）+ DI（~243ns）≈ **0.5 µs 以内**
- 换算单核 RPS 上限：路由层 ≈ 500 万 ops/s（受网络栈限制实际远低于此，框架层不构成瓶颈）
- 与 P2 审计基线（2026-07-25，89.4/100）相比无性能回归

## 8. 框架 vs 原生对照（D1，2026-08-03 补充）

| 基准 | 耗时 | 说明 |
|------|------|------|
| `native_match_static` | 56.75 ns | 手写静态路由匹配（近似 matchit 下限） |
| `parse_path_static_framework` | 210.23 ns | 框架三层路由解析（约 3.7x 原生，含 capitalize/app_map 查询） |
| `native_match_with_query` | — | 含查询串剥离的静态匹配 |
| `parse_path_long_framework` | 209.03 ns | 框架长路径解析 |

**结论**：框架路由层相对原生静态匹配约 **3.7x 开销**（+153ns），但绝对值 < 220 ns，在真实 HTTP 栈（百 µs 级）中占比 < 1%。这是"约定式路由解析 + 应用映射"能力付出的可忽略成本。

## 9. 复现

```bash
cargo bench --package sz-rust-core --bench core_bench -- \
    --warm-up-time 1 --measurement-time 3 --sample-size 30
```
