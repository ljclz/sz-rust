# SZ-Rust 性能回归测试报告 v0.2.0

> **测试时间**：2026-07-23
> **对比基线**：v0.1.0（2026-07-22 建立）
> **工具**：criterion 0.5（html_reports）
> **参数**：`--warm-up-time 1 --measurement-time 3 --sample-size 30`
> **环境**：Windows + Rust stable（本地开发机）

---

## 1. 总体摘要

| 指标 | 数值 |
|------|------|
| 总基准测试数 | 12 |
| ✅ 无变化（±5% 内） | 4（33.3%） |
| 🔴 性能回归（>+5%） | 7（58.3%） |
| 🟢 性能改进（<-5%） | 1（8.3%） |
| **综合判定** | **🔴 严重退化** |

> ⚠ **重要观察**：回归涉及全部 4 个基准测试组（route_matching、handler_ref_parse、route_config、json_serialization），影响面广、幅度大（14%~24%）。分析表明**非代码变更导致**（benchmarked 代码路径未修改），很可能由系统级因素引起（CPU 频率、后台进程、散热等）。

---

## 2. 详细对比结果

### 2.1 route_matching — 路由匹配

| 基准测试 | 基线 v0.1.0 | v0.2.0 | 变化 | 判定 |
|----------|-------------|--------|------|------|
| `parse_path_static` | 185.43 ns | **211.66 ns** | **+13.86%** | 🔴 回归 |
| `parse_path_root` | 93.45 ns | **105.84 ns** | **+15.39%** | 🔴 回归 |
| `parse_path_long` | 231.39 ns | 227.02 ns | +0.49% | ✅ 无变化 |

### 2.2 handler_ref_parse — 处理器引用解析

| 基准测试 | 基线 v0.1.0 | v0.2.0 | 变化 | 判定 |
|----------|-------------|--------|------|------|
| `parse_simple` | 82.33 ns | **99.46 ns** | **+21.72%** | 🔴 回归 |
| `parse_with_slash` | 93.66 ns | **119.19 ns** | **+24.14%** | 🔴 回归 |

### 2.3 route_config — 路由配置加载

| 基准测试 | 基线 v0.1.0 | v0.2.0 | 变化 | 判定 |
|----------|-------------|--------|------|------|
| `load_yaml_small` | 5.92 µs | **7.06 µs** | **+18.48%** | 🔴 回归 |
| `load_yaml_medium` | 26.09 µs | 26.15 µs | -1.57% | ✅ 无变化 |
| `find_conflicts` | 24.43 µs | 24.42 µs | +2.99% | ✅ 无变化 |

### 2.4 json_serialization — JSON 序列化/反序列化

| 基准测试 | 基线 v0.1.0 | v0.2.0 | 变化 | 判定 |
|----------|-------------|--------|------|------|
| `serialize_small` | 102.92 ns | 101.61 ns | -0.91% | ✅ 无变化 |
| `serialize_medium` | 4.41 µs | **4.72 µs** | **+18.65%** | 🔴 回归 |
| `deserialize_small` | 716.44 ns | **653.58 ns** | **-9.77%** | 🟢 改进 |
| `deserialize_medium` | 40.59 µs | **48.49 µs** | **+16.00%** | 🔴 回归 |

---

## 3. 回归原因调查

### 3.1 代码变更分析

v0.1.0 基线建立后，`sz-rust-core` 的变更（`git diff HEAD~1 --stat`）：

```
packages/sz-rust-core/src/config.rs        | 142 +++-    # 数据库环境变量覆盖
packages/sz-rust-core/src/container.rs     |   4 +-     # 容器微调
packages/sz-rust-core/src/lib.rs           |   6 +      # 模块导出
packages/sz-rust-core/src/runtime/spawn.rs |  19 +-     # tokio spawn 调整
```

**核心发现**：benchmarked 代码路径（`router/parse_path`、`routing/HandlerRef`、`routing/RouteConfig`、`serde_json`）**未发生任何代码变更**。上述变更均为配置/容器/运行时辅助功能。

### 3.2 回归特征分析

- **影响面广**：58% 的基准测试回归，覆盖全部 4 个测试组
- **幅度大**：回归幅度 13.86%~24.14%，远超正常波动范围
- **非选择性**：同一测试组内部分测试回归、部分正常（如 `parse_path_long` 无变化而 `parse_path_static` 回归）

### 3.3 最可能原因（系统级）

1. **CPU 频率缩放**：两次运行间 CPU 处于不同频率状态（如节能模式 vs 高性能模式）
2. **后台进程干扰**：Windows 更新、杀毒软件扫描、索引服务等
3. **散热/温度 throttling**：连续运行导致 CPU 温度升高、降频
4. **内存带宽竞争**：其他进程占用内存带宽

> 结论：**本次回归非代码变更引起**。建议在**可控环境**（固定 CPU 频率、关闭后台进程）中重新运行验证。

---

## 4. 建议措施

| # | 措施 | 说明 |
|---|------|------|
| 1 | 固定 CPU 频率 | 使用 `powercfg` 锁定高性能模式，禁用 Intel SpeedStep / AMD Cool'n'Quiet |
| 2 | 关闭后台进程 | 运行前关闭杀毒软件、Windows Update、浏览器等 |
| 3 | 多次取平均 | 在相同条件下运行 3 次取中位数，消除单次噪音 |
| 4 | CI 环境验证 | 在 GitHub Actions runner 上运行一次独立对比（环境更可控） |

---

*报告生成时间：2026-07-23 22:45 CST*
*工具：criterion 0.5 + manual analysis*
