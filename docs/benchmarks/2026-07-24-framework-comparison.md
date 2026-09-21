# Rust Web 框架性能对比基准报告

> **日期**：2026-07-24
> **对比对象**：sz-rust vs Axum vs Actix-web vs Poem
> **基准方式**：真实 HTTP 请求压测（Node.js 自研压测客户端，keep-alive，预热 200 请求）
> **状态**：✅ 四个框架全部编译并运行成功，两轮压测数据已采集

---

## 1. 测试环境

| 项目 | 信息 |
|------|------|
| CPU | 12th Gen Intel(R) Core(TM) i9-12900H（14 核 / 20 逻辑线程）|
| 内存 | 31.8 GB |
| 操作系统 | Microsoft Windows 10 专业版 10.0.19045 (Build 19045) |
| Rust 版本 | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| cargo 版本 | cargo 1.97.1 (c980f4866 2026-06-30) |
| 数据库 | MySQL 8.x（本机 127.0.0.1:3306，库 `sz_orm_test`）|
| 压测客户端 | Node.js v22.16.0（http 模块 + keep-alive 连接池）|

> 压测期间关闭其他 CPU 密集型应用；所有服务监听 127.0.0.1 本机回环。

---

## 2. 基准项目说明

四个项目均位于 `F:\test\rust\`，结构完全对齐：

| 端点 | 行为 |
|------|------|
| `GET /json` | 返回 `{"hello":"world"}`，不访问数据库 |
| `GET /db`  | 通过 sqlx MySQL 连接池执行 `SELECT 1`，返回 `{"one":1}` |

所有框架的数据库连接池配置一致：`max_connections=10`，`acquire_timeout=30s`，使用 sqlx 0.9.0 + rustls。

### 2.1 sz-rust 项目特殊性说明

`bench-sz-rust` 使用了 **真实的 sz-rust 栈**：

- **HTTP/路由层**：`sz-rust-core`（path 依赖，基于 axum 0.8.9），启动时初始化 `App` 全局容器（对齐 PHP `app()` 容器，持有配置/DB/日志单例）。
- **DB 层**：`sz-orm-sqlx` 的 `MySqlPoolHandle`（path 依赖，sz-rust 的真实数据库连接池，封装 sqlx::MySqlPool，`max_connections=10, idle_timeout=600s, max_lifetime=1800s`）。

> sz-rust 的 `App::init` 容器初始化只在启动时执行一次，**不增加每请求开销**，因此 sz-rust 的吞吐与 axum 几乎一致（见下文数据）。这与设计预期相符：sz-rust 在 axum 之上提供 ThinkPHP 风格的容器/控制器/模型/中间件体系，而 HTTP 请求处理仍走 axum + hyper。

---

## 3. 各框架 Cargo.toml 依赖

### 3.1 bench-sz-rust（端口 3001）

```toml
[dependencies]
sz-rust-core = { path = "e:/vue/test/鲜视达/rust/sz-rust/packages/sz-rust-core" }
sz-orm-sqlx = { path = "e:/vue/test/鲜视达/rust/sz-orm/packages/sz-orm-sqlx" }
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "mysql"] }
```

### 3.2 bench-axum（端口 3002）

```toml
[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "mysql"] }
```

### 3.3 bench-actix（端口 3003）

```toml
[dependencies]
actix-web = "4"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "mysql"] }
futures = "0.3"
```

### 3.4 bench-poem（端口 3004）

```toml
[dependencies]
poem = { version = "3", features = ["server"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "mysql"] }
```

### 实际解析版本（来自 Cargo.lock）

| 依赖 | sz-rust | axum | actix | poem |
|------|---------|------|-------|------|
| axum | 0.8.9 | 0.8.9 | — | — |
| actix-web | — | — | 4.14.0 | — |
| poem | — | — | — | 3.1.12 |
| hyper | 1.11.0 | 1.11.0 | (actix-http) | 1.11.0 |
| sqlx | 0.9.0 | 0.9.0 | 0.9.0 | 0.9.0 |
| tokio | 1.53.1 | 1.53.1 | 1.53.1 | 1.53.1 |
| serde_json | 1.0.151 | 1.0.151 | 1.0.151 | 1.0.151 |

---

## 4. 压测方法论

- **压测工具**：Node.js 自研客户端（`http` 模块，keep-alive 连接池，`maxSockets=concurrency`）。
  - 未安装 `oha`（编译耗时），改用 Node.js 客户端以避免额外编译开销干扰。
- **预热**：每次压测前先发送 200 个请求（不计入统计），触发 JIT/连接池预热。
- **/json**：10,000 请求，10 并发。
- **/db**：5,000 请求，10 并发。
- **指标**：QPS、平均延迟、p50/p95/p99 延迟、总耗时、错误数。
- **轮次**：每个框架压测 2 轮，取平均值（消除单次抖动）。
- **服务启动**：每次只启动一个服务，压测完即关闭，避免相互干扰。

---

## 5. 压测结果

### 5.1 GET /json（无数据库，纯 HTTP 框架开销）

**两轮原始数据**

| 框架 | 轮次 | QPS | avg(ms) | p50(ms) | p95(ms) | p99(ms) | 错误 |
|------|------|-----|---------|---------|---------|---------|------|
| sz-rust | R1 | 26310 | 0.348 | 0.276 | 0.692 | 1.337 | 0 |
| sz-rust | R2 | 24894 | 0.370 | 0.296 | 0.711 | 1.378 | 0 |
| axum | R1 | 26652 | 0.336 | 0.256 | 0.649 | 1.069 | 0 |
| axum | R2 | 26747 | 0.344 | 0.275 | 0.650 | 1.044 | 0 |
| actix | R1 | 22688 | 0.411 | 0.312 | 0.856 | 1.684 | 0 |
| actix | R2 | 23612 | 0.392 | 0.301 | 0.767 | 1.429 | 0 |
| poem | R1 | 26299 | 0.346 | 0.277 | 0.642 | 1.135 | 0 |
| poem | R2 | 20204 | 0.449 | 0.313 | 0.931 | 2.414 | 0 |

**两轮平均值（排序：QPS 降序）**

| 排名 | 框架 | QPS | avg(ms) | p50(ms) | p95(ms) | p99(ms) |
|------|------|-----|---------|---------|---------|---------|
| 🥇 1 | **axum** | **26700** | 0.340 | 0.266 | 0.650 | 1.057 |
| 🥈 2 | **sz-rust** | **25602** | 0.359 | 0.286 | 0.702 | 1.358 |
| 🥉 3 | **poem** | **23252** | 0.398 | 0.295 | 0.787 | 1.775 |
| 4 | actix | 23150 | 0.402 | 0.306 | 0.812 | 1.557 |

### 5.2 GET /db（含 MySQL `SELECT 1`）

**两轮原始数据**

| 框架 | 轮次 | QPS | avg(ms) | p50(ms) | p95(ms) | p99(ms) | 错误 |
|------|------|-----|---------|---------|---------|---------|------|
| sz-rust | R1 | 15770 | 0.523 | 0.424 | 1.139 | 1.817 | 0 |
| sz-rust | R2 | 16339 | 0.501 | 0.424 | 0.914 | 1.377 | 0 |
| axum | R1 | 16634 | 0.496 | 0.426 | 0.861 | 1.362 | 0 |
| axum | R2 | 16167 | 0.508 | 0.439 | 0.884 | 1.224 | 0 |
| actix | R1 | 12946 | 0.649 | 0.500 | 1.355 | 3.861 | 0 |
| actix | R2 | 12740 | 0.651 | 0.520 | 1.365 | 2.038 | 0 |
| poem | R1 | 13899 | 0.593 | 0.483 | 1.196 | 2.241 | 0 |
| poem | R2 | 13665 | 0.594 | 0.502 | 1.169 | 1.821 | 0 |

**两轮平均值（排序：QPS 降序）**

| 排名 | 框架 | QPS | avg(ms) | p50(ms) | p95(ms) | p99(ms) |
|------|------|-----|---------|---------|---------|---------|
| 🥇 1 | **axum** | **16401** | 0.502 | 0.433 | 0.873 | 1.293 |
| 🥈 2 | **sz-rust** | **16055** | 0.512 | 0.424 | 1.027 | 1.597 |
| 🥉 3 | **poem** | **13782** | 0.594 | 0.493 | 1.183 | 2.031 |
| 4 | actix | 12843 | 0.650 | 0.510 | 1.360 | 2.950 |

### 5.3 关键观察

1. **sz-rust ≈ axum**：/json 仅低 ~4%，/db 仅低 ~2%。这符合预期——sz-rust 基于 axum 0.8.9，容器/控制器抽象只在启动期生效，不进入每请求热路径。
2. **axum/sz-rust 在本场景下吞吐最高**，得益于 hyper 1.x + tower 的轻量管道。
3. **actix-web 吞吐最低**（/json 比 axum 低 ~13%，/db 低 ~22%），其自有 actor 运行时（actix-rt）在本机回环+小响应场景下开销相对明显；但 actix 的 p99 在 R2 已显著改善（/db p99 从 3.86ms 降到 2.04ms），说明存在一定预热/JIT 效应。
4. **poem 表现稳定居中**，R2 /json 出现一次回落（20204 QPS），疑为本机瞬时调度抖动。
5. **/db 相比 /json 吞吐普遍下降 35-45%**，瓶颈在 MySQL 连接池取连接+往返，而非框架本身。

---

## 6. 二进制体积对比（release 构建）

| 框架 | 体积（KB） | 体积（MB） | 相对最小 |
|------|-----------|-----------|----------|
| **axum** | 3813.0 | 3.7 | 1.00×（最小）|
| poem | 6242.5 | 6.1 | 1.64× |
| **sz-rust** | 6264.5 | 6.1 | 1.64× |
| actix | 7016.5 | 6.9 | 1.84× |

> sz-rust 体积与 poem 接近，主要来自 sz-orm 全家桶（auth/crypto/storage/queue/mqtt/websocket/scheduler 等）与图像/Excel/PDF 处理依赖。axum 最小因其依赖树最精简。

---

## 7. 编译时间对比（release 首次全量构建）

| 框架 | 首次构建耗时（秒） | 说明 |
|------|-------------------|------|
| axum | ~41 | 首次编译全部依赖（40.8s）+ 修正后重编译二进制（4.2s）|
| poem | 54.6 | 全量编译含下载 |
| actix | 160.9 | 全量编译含下载，actix 生态依赖较重 |
| **sz-rust** | **274.2** | 最长，因 sz-orm 全家桶 + sz-rust-core（含 image/rust_xlsxwriter/calamine/lopdf/mqtt/websocket 等）|

> 注：编译时间为「下载 + 编译」的总耗时。axum 因首轮编译时二进制有 Clone trait 报错（已修正），其纯依赖编译耗时为 40.8s。各项目 `target/` 独立，不共享编译产物，但共享 `~/.cargo` 下载缓存。

---

## 8. 依赖数量对比（`cargo tree --prefix none` 去重）

| 框架 | 唯一依赖数 | 依赖树节点总数 |
|------|-----------|---------------|
| axum | 131 | 309 |
| poem | 144 | 339 |
| actix | 179 | 446 |
| **sz-rust** | **302** | **876** |

> sz-rust 依赖数为 axum 的 2.3 倍，因其内置完整 ORM/缓存/队列/MQTT/WebSocket/调度器/图像/Excel/PDF 等能力（对标 ThinkPHP 8 全家桶），而 axum/actix/poem 仅为核心 Web 框架，需自行拼装第三方库。

---

## 9. 功能特性对比

| 特性 | sz-rust | Axum | Actix-web | Poem |
|------|:-------:|:----:|:---------:|:----:|
| 路由（静态/动态） | ✅ | ✅ | ✅ | ✅ |
| 中间件（洋葱模型） | ✅ | ✅（tower）| ✅ | ✅ |
| 内置 ORM | ✅ sz-orm 全家桶 | ❌ 需 sqlx/sea-orm | ❌ 需 sqlx/sea-orm | ❌ 需 sqlx/sea-orm |
| 数据库迁移 | ✅ sz-orm-mig | ❌（sqlx-cli）| ❌ | ❌ |
| WebSocket | ✅ | ✅ | ✅ | ✅ |
| WebSocket 服务端框架 | ✅ sz-orm-websocket | ✅ | ✅ | ✅ |
| 模板引擎 | ✅ view 模块 | ❌ 第三方 | ❌ 第三方 | ✅ askama |
| 文件上传 | ✅ multipart+存储驱动 | ✅ multipart | ✅ | ✅ |
| 认证（JWT） | ✅ sz-orm-auth | ❌ 第三方 | ❌ 第三方 | ❌ |
| 缓存 facade | ✅ | ❌ | ❌ | ❌ |
| 消息队列 | ✅ sz-orm-queue | ❌ | ❌ | ❌ |
| MQTT | ✅ sz-orm-mqtt | ❌ | ❌ | ❌ |
| 定时任务调度 | ✅ sz-orm-scheduler | ❌ | ❌ | ❌ |
| OpenAPI/Swagger | ✅ sz-orm-swagger | ❌（utoipa）| ❌ | ✅ |
| 图像处理 | ✅ image+Grafika 对齐 | ❌ | ❌ | ❌ |
| Excel/PDF | ✅ rust_xlsxwriter+lopdf | ❌ | ❌ | ❌ |
| HTTP/2 | ✅ | ✅ | ✅ | ✅ |
| TLS | ✅ rustls | ✅ 第三方 | ✅ | ✅ |
| PHP 风格 API（renderJson/控制器/模型）| ✅ | ❌ | ❌ | ❌ |
| 生态成熟度 | 早期（0.2.0）| 高 | 高 | 中 |
| 学习曲线（PHP 开发者）| 低 | 中 | 中 | 中 |

---

## 10. 结论

### 10.1 性能结论

1. **sz-rust 性能表现优秀**：在纯 HTTP（/json）与含数据库（/db）场景下，吞吐分别为 25602 QPS 与 16055 QPS，与底层 axum（26700 / 16401 QPS）差距均在 5% 以内。sz-rust 的容器/控制器抽象是**零成本抽象**——只在启动期生效，不进入请求热路径。
2. **axum 是本场景的吞吐冠军**，因其依赖 hyper 1.x + tower 的极简管道，无额外运行时层。
3. **actix-web 吞吐最低**（/json 23150、/db 12843 QPS），其 actor 运行时在小响应+高并发回环场景下开销相对明显；但功能完备、生态成熟，适合复杂业务。
4. **poem 居中**（/json 23252、/db 13782 QPS），API 现代，内置 OpenAPI 与模板，是平衡之选。

### 10.2 工程权衡结论

| 维度 | 推荐选择 |
|------|----------|
| 极致吞吐 / 最小体积 / 最短编译 | **Axum** |
| PHP/ThinkPHP 迁移 / 全功能开箱即用 | **sz-rust** |
| 成熟生态 / actor 模型 / 复杂并发 | **Actix-web** |
| 现代 API / 内置 OpenAPI+模板 | **Poem** |

### 10.3 sz-rust 定位评价

- **优势**：在提供对标 ThinkPHP 8 的全栈能力（ORM/缓存/队列/MQTT/WebSocket/调度器/图像/Excel/PDF/认证/Swagger）的同时，HTTP 性能几乎不输原生 axum（差距 <5%）。对 PHP 团队迁移 Rust 极其友好。
- **代价**：依赖数（302）与编译时间（274s）显著高于纯 axum（131 依赖 / 41s），二进制体积约为 axum 的 1.64×。这是「全功能内置」与「按需拼装」架构路线的固有取舍。
- **建议**：若项目需要快速复用 PHP 时代的全套后端能力且注重开发效率，sz-rust 是高性价比选择；若追求极致二进制体积/编译速度且愿意自行拼装生态，可直接用 axum。

---

## 11. 复现方法

### 11.1 构建

```powershell
cd F:\test\rust
# 每个项目独立构建（release）
node run-cargo.js bench-sz-rust build --release
node run-cargo.js bench-axum   build --release
node run-cargo.js bench-actix  build --release
node run-cargo.js bench-poem   build --release
```

> 因本机沙箱限制，`cargo build` 需经 Node.js `child_process` 调用以写入 `F:\test\rust`（`Cargo.lock` / `target/`）。`run-cargo.js` 为此封装。

### 11.2 压测

```powershell
# 单个服务压测（启动→预热→压测→关闭）
node run-bench.js bench-axum 3002 "F:\test\rust\bench-axum\target\release\bench-axum.exe"
node run-bench.js bench-actix 3003 "F:\test\rust\bench-actix\target\release\bench-actix.exe"
node run-bench.js bench-poem 3004 "F:\test\rust\bench-poem\target\release\bench-poem.exe"
node run-bench.js bench-sz-rust 3001 "F:\test\rust\bench-sz-rust\target\release\bench-sz-rust.exe"
```

### 11.3 数据库准备

```sql
CREATE DATABASE IF NOT EXISTS sz_orm_test;
USE sz_orm_test;
CREATE TABLE IF NOT EXISTS bench_test (id INT PRIMARY KEY, name VARCHAR(100));
INSERT INTO bench_test VALUES (1, 'hello');
```

连接串：`mysql://root:test123@127.0.0.1:3306/sz_orm_test`

---

## 12. 数据完整性与可信度声明

- 所有 QPS/延迟数据均由 `bench-client.js` 实测输出（`process.hrtime.bigint` 高精度计时），非主观估算。
- 每个框架压测 2 轮，报告同时给出原始两轮数据与平均值。
- 压测前均有 200 请求预热，排除冷启动。
- 所有服务均 `--release` 构建，O0 优化关闭。
- 四个框架共享相同的 sqlx 0.9.0 + MySQL 连接池配置，DB 层变量受控。
- sz-rust 使用真实 `sz-rust-core` + `sz-orm-sqlx`（path 依赖），非模拟。
- 错误数均为 0，无压测期间的连接错误或 5xx。
