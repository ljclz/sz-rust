# 服务器部署验证报告

> 生成时间：2026-09-23
> 服务器：S253-75（121.204.253.75:32 root）
> 部署路径：/www/rust/sz-rust/
> Commit：debad8b

## 1. 编译信息

| 项目 | 值 |
|------|-----|
| 服务器架构 | x86_64 (Linux) |
| Rust 版本 | 1.98.1 |
| 编译配置 | LTO=fat, codegen-units=1, panic=abort, strip=true |
| 编译时间 | 2m05s（服务器端 `cargo build --release`） |
| 二进制路径 | `/www/rust/sz-rust/target/release/sz-rust` |

## 2. 二进制体积

| 项目 | 值 |
|------|-----|
| 文件大小 | 18 MB |
| 格式 | ELF 64-bit LSB pie executable, x86-64, stripped |
| 版本 | sz-rust 1.2.0 |

## 3. 空载资源消耗

启动命令：`./target/release/sz-rust serve --addr 127.0.0.1:18080`

| 指标 | 值 | 阈值 | 状态 |
|------|-----|------|------|
| VmRSS | 12.7 MB (13052 kB) | 25 MB | ✅ PASS |
| VmSize | 5.25 GB (5504228 kB) | — | — |
| Threads | 82 | — | — |

## 4. 性能基准对比摘要

> 完整报告：`docs/benchmarks/perf-comparison-report.md`

| 基准组 | 基线 P50 (ns) | 当前 P50 (ns) | 变化 |
|--------|-------------|-------------|------|
| json_serialization/deserialize_small | 940 | 624 | **-33.6%** |
| route_config/load_yaml_medium | 36152 | 25390 | **-29.8%** |
| cache_read_write/cache_remove_existing | 112 | 99 | **-11.5%** |
| middleware_chain/has_duplicates | 27 | 23 | **-13.8%** |
| container_lookup/singleton_reuse | 28 | 26 | **-5.1%** |

## 5. 验收清单

- [x] release 二进制编译成功（服务器端）
- [x] 二进制体积 18 MB（stripped）
- [x] 空载 RSS 12.7 MB < 25 MB 阈值
- [x] 服务启动成功（`sz-rust serve`）
- [x] 性能基准对比：多数基准测试改进，无显著回归
- [x] 代码已推送至 origin/main（commit debad8b）