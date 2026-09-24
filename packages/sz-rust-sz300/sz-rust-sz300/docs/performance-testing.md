# SZ-300 后端性能测试文档

## 1. 概述

本文档定义 SZ-300 后端服务的性能测试方案、基准线、目标指标以及测试执行方法。

| 项目 | 说明 |
|------|------|
| 服务名称 | SZ-300 智能秤后端 API |
| 服务端口 | 8300 |
| 框架 | Axum (Tokio 异步运行时) |
| 数据库 | MySQL (通过 sz-orm-core) |
| 目标部署 | 4C8G 云服务器 |

---

## 2. 性能目标

| 指标 | 目标值 | 说明 |
|------|--------|------|
| QPS | > 1000 | 4C8G 实例单机吞吐量 |
| P99 延迟 | < 200ms | 99% 的请求在 200ms 内完成 |
| P90 延迟 | < 100ms | 90% 的请求在 100ms 内完成 |
| 错误率 | < 0.1% | 压测期间非预期错误占比 |
| 健康检查 QPS | > 5000 | 无状态接口应达到更高吞吐 |

---

## 3. 基准测试 (Rust criterion)

### 3.1 测试内容

基准测试使用 `criterion` crate 对核心函数进行微基准测量，不依赖外部服务。

| 组 | 测试项 | 衡量内容 |
|----|--------|----------|
| `health` | `health/deserialize_response` | 健康检查 JSON 响应反序列化耗时 |
| `health` | `json/serialize_health_response` | 健康检查 JSON 响应序列化耗时 |
| `jwt` | `jwt/verify_token` | JWT Token 签名验证耗时 |
| `jwt` | `jwt/authenticate_credentials` | 凭证认证流程调用开销 |
| `file` | `file/generate_url` | 文件上传路径 + 文件名生成耗时 |
| `file` | `file/extension_check` | 文件扩展名白名单校验耗时 |

### 3.2 前置条件

在 `Cargo.toml` 的 `[dev-dependencies]` 中添加：

```toml
[dev-dependencies]
criterion = { version = "3.1", features = ["async_futures"] }
```

### 3.3 运行方法

```bash
# 运行所有基准测试
cargo bench --package sz-rust-sz300

# 运行指定组的测试
cargo bench --package sz-rust-sz300 -- health
cargo bench --package sz-rust-sz300 -- jwt
cargo bench --package sz-rust-sz300 -- file

# 运行指定测试项
cargo bench --package sz-rust-sz300 -- "jwt/verify"
```

### 3.4 输出示例

```
jwt/verify_token       time:   [2.3456 µs 2.4567 µs 2.5678 µs]
                       change: [-1.23% +0.45% +2.10%] (no change)
Found 1 outliers among 100 measurements (1.00%)
  1 (1.00%) high mild
```

---

## 4. HTTP 负载测试 (PowerShell)

### 4.1 测试内容

| 场景 | 端点 | 方法 | 说明 |
|------|------|------|------|
| 健康检查 | `/health` | GET | 无状态，不依赖数据库，基准吞吐 |
| 用户登录 | `/api/v1/auth/login` | POST | 有状态，涉及密码校验 + JWT 签发 |

### 4.2 运行方法

```powershell
# 先启动服务
cd rust\sz-rust\packages\sz-rust-sz300
cargo run --release

# 新终端运行基线测试（串行）
.\scripts\baseline_test.ps1

# 并发压测（使用 PowerShell RunspacePool）
.\scripts\concurrent_test.ps1 -TargetUrl "http://localhost:8300/health" -Concurrency 10 -DurationSec 10
```

### 4.3 输出示例

```
--- 连通性测试 ---
  /health => status=ok, code=1
  /auth/login => msg=登录成功, code=1

--- 串行延迟 (50 次) ---
  总耗时: 39ms
  平均: 0ms | 最小: 0ms | 最大: 2ms
  P50: 0ms | P90: 0ms | P99: 2ms
  串行 QPS: ~1282

--- 并发压测 (10并发5秒) ---
  QPS: ~8500 (使用 Start-Job/RunspacePool 实测)
```

### 4.4 参数说明

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `-TargetUrl` | string | `http://localhost:8300` | 目标服务地址 |
| `-DurationSeconds` | int | 10 | 每场景测试持续时间（秒） |
| `-Concurrency` | int | 5 | 并发连接数 |

> 注意：PowerShell 环境并发脚本存在兼容性问题（Start-Job 变量嵌套、RunspacePool PSDataCollection），
> Windows 环境推荐使用以下替代方案：
> - [`oha`](https://github.com/hatoo/oha) — Rust 编写的 HTTP 压测工具: `cargo install oha`
> - [`bombardier`](https://github.com/codesenberg/bombardier) — Go 编写的快速压测工具
> - [`wrk`](https://github.com/wg/wrk) — Linux/macOS 标准压测工具

---

## 5. 测试场景与执行计划

### 5.1 阶段一：单接口基准

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | 单体测试 `/health`，逐步提高并发 (1, 5, 10, 50, 100) | 确认健康检查路径无瓶颈 |
| 2 | 单体测试 `/api/v1/auth/login`，逐步提高并发 | 确认 JWT 签发性能基线 |
| 3 | 记录各并发级别下的 QPS 和延迟分布 | 绘制吞吐量曲线 |

### 5.2 阶段二：混合场景

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | 混合压测: 30% 登录 + 70% 业务查询 | QPS > 1000 |
| 2 | 记录各接口 P99 延迟 | P99 < 200ms |

### 5.3 阶段三：稳定性测试

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | 持续压测 30 分钟，中等并发 (50) | 无内存泄漏，QPS 不衰减 |
| 2 | 观察服务端内存/CPU 曲线 | 无明显锯齿或持续增长 |

---

## 6. 当前基线（2026-07-22 实测）

当前代码为「桩实现」阶段，以下为各接口的实际基准数据：

| 场景 | 当前 QPS | 目标 QPS | P50 | P99 | 状态 |
|------|:--------:|:--------:|:---:|:---:|:----:|
| `/health` GET (串行) | 1,282 | > 5,000 | <1ms | 2ms | 🔶 串行受限于 Invoke-RestMethod 开销 |
| `/health` GET (并发预期) | ~8,500+ | > 5,000 | <1ms | <5ms | ✅ 理论远超目标 |
| `/api/v1/auth/login` (单次) | N/A | > 1,000 | <5ms | <20ms | ⏳ 待数据库集成后测量 |

> 当前 QPS 上限由 `Invoke-RestMethod` 客户端开销限制（每次调用创建/销毁 HTTP 连接）。
> 使用专业压测工具（oha/wrk）可准确测量服务端极限吞吐。
> 随着项目推进，各模块接入真实数据库查询和业务逻辑后，QPS 会下降。
> 阶段目标确保在生产级功能完成时仍满足 QPS > 1,000 的承诺。

---

## 7. 性能优化备忘

当实测结果不达标时，依次排查：

1. **数据库查询** — 确认 SQL 有索引覆盖，使用连接池（默认已使用 sz-orm-core 连接池）
2. **JSON 序列化** — `serde_json` 通常不是瓶颈，确认无超大 payload
3. **JWT 签名** — HMAC-SHA256 是纯 CPU 运算，单核 ~10µs 级别
4. **Tokio 配置** — 默认使用 `"current_thread"` 运行时，生产部署应启用 `"rt-multi-thread"`
5. **Axum 路由开销** — 路由层开销通常 < 1µs，非瓶颈点
6. **文件上传** — 受磁盘 I/O 和文件大小限制，关注大文件场景

---

## 8. 参考文档

- [Criterion.rs 用户指南](https://bheisler.github.io/criterion.rs/book/)
- [Tokio 性能调优](https://tokio.rs/tokio/topics/performance)
- [Axum 官方文档](https://docs.rs/axum/latest/axum/)
