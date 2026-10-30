> **中文** | [English](README.en.md)

# SZ-Rust — 对标 ThinkPHP 8 的 Rust Web 框架

基于 axum 0.8 + SZ-ORM 的 Rust Web 框架，API 设计对齐 ThinkPHP 8，便于 PHP 工程师迁移。

**当前版本：v2.0.0（workspace）**；自 2026-10-25 起原企业版仓库已合并入本仓库，全部 crate 统一 Apache-2.0，详见 [单一版本合并方案](docs/single-edition-merge-plan.md) 与 ADR-043（`docs/adr/`）。

> **v2.0.0 单一版本合并摘要**：企业版 10 crate 能力已并入本 workspace（现 65 members），详见 [单一版本合并方案](docs/single-edition-merge-plan.md) 与 [CHANGELOG.md](CHANGELOG.md)。

---

## 快速上手

最小 Hello World 示例（完整代码见 `packages/sz-rust-examples/src/bin/quick_start.rs`）：

```rust
use sz_rust_core::config::AppConfig;
use sz_rust_core::container::App;
use sz_rust_core::log::LogFacade;
use sz_rust_examples::build_router;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 加载配置（失败时回退默认配置）
    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    let config = AppConfig::load_from_dir(&config_dir).await.unwrap_or_else(|_| AppConfig::default());

    // 初始化 App 容器
    let app = App::init(config);
    let log_facade = LogFacade::init(&app.config().log);
    log_facade.info("SZ-Rust Hello World 端点启动中...");

    // 构建路由（GET / 返回 {"code":1,"msg":"hello","data":{}}）
    let router = build_router();

    // 启动 HTTP 服务
    let addr = "127.0.0.1:9527";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
```

运行：

```bash
cargo run -p sz-rust-examples --bin quick_start
```

访问 http://127.0.0.1:9527/ 返回：

```json
{"code":1,"msg":"hello","data":{}}
```

完整 CRUD 示例见 `packages/sz-rust-examples/src/bin/crud_demo.rs`：

```bash
cargo run -p sz-rust-examples --bin crud_demo
```

> 更多示例的运行方式见 [docs/examples.md](docs/examples.md)。

---

## 核心特性

以下特性均来自 `sz-rust-core` 实际源码，模块结构见 `packages/sz-rust-core/src/lib.rs`。

> ✅ **单一版本说明（v2.0.0 合并，2026-10-25）**：原企业版仓库已合并入本仓库，下方「生产已接入」引用均指向**本仓库内** `packages/sz-rust-sz300` 的实际代码（路径可直接验证，不再引用外部企业版仓库路径）。无法在本仓库 sz300 中对应的能力，一律改标「框架能力」，不再声称生产接入。

- **HTTP 服务器 + 路由**：基于 axum 0.8 + tower 0.5 + hyper 1.x，支持三层路由机制（属性宏 / 配置式 / 约定式）。
- **控制器层**：`SzController` → `BaseController` → `AddonsBaseController` 三层 trait 继承链，对齐 PHP `app\SzController` / `app\BaseController` / `addons\BaseController`。提供 `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` 等方法。
- **模型层**：`BaseModel` trait 组合 SZ-ORM 的 `Model` + `ModelExt` + `RelationLoader`，对齐 `think\Model`。支持 `$append` 虚拟字段、访问器（`Accessor`）、修改器（`Mutator`）、动态 Append（`Appendable`）。
- **中间件**：内置 CORS / Auth(JWT) / Log / RateLimit / Trace 五个中间件，外加链构建器（`MiddlewareChain`）、Handler=Middleware 双向转换器、tower-http 兼容层。（✅ 生产接入：本仓库 sz300 `router.rs` 挂载 CORS + CSRF + 限流(令牌桶) + 熔断器 + 自研 auth_middleware，生产链路与原企业版一致）
- **限流器（v0.3.1 已落地）**：`sz-rust-middleware-facade::rate_limit` 提供令牌桶（`TokenBucket`）+ 滑动窗口（`SlidingWindow`）两种算法，实现 `sz_rust_orm_facade::RateLimiter` trait，含 OOM 防护与 100 并发无误差测试（测试 `test_token_bucket_concurrent_100_requests`，`packages/sz-rust-middleware-facade/src/rate_limit.rs:649`）。（✅ 生产已接入：sz300 `router.rs` 挂载令牌桶中间件，默认生效，配置由 `RateLimitProductionConfig::from_env()` 读取，`config.rs`）
- **熔断器（v0.3.1 已落地）**：`sz-rust-middleware-facade::circuit_breaker` 提供 Closed/Open/HalfOpen 三态状态机 + `circuit_breaker_middleware`（Open 返回 503），parking_lot::Mutex 保护并发安全。（✅ 生产已接入：sz300 `router.rs` 挂载熔断中间件，默认生效，配置由 `CircuitBreakerProductionConfig::from_env()` 读取，`config.rs`）
- **验证器**：对齐 `think\Validate`，内置 30+ 规则（规则实现见 `packages/sz-rust-infra-facade/src/validate/rules.rs`：26 个 `pub fn` 规则函数 + require/integer/float/email/url/regex 等基础规则，共 30+），支持批量验证、场景、自定义消息。（✅ 生产已接入示范：sz300 商户创建接口 `controllers/merchant.rs:127` 使用 `Validate` 校验名称必填/长度限制（来源: 2026-10-28 UXR P1-5 接入）；框架能力由 sz-rust-facade-tests 集成测试覆盖）
- **缓存系统**：对齐 `think\facade\Cache`，复用 sz-orm-storage 驱动；`sz_rust_core::cache` 聚合 Memory / Redis / Memcached / 多级驱动（`sz-rust-cache-facade`）。（框架能力，4 驱动均有单测；sz300 业务侧暂未调用）
- **事件系统**：对齐 `think\Event`，支持 Listener / Subscriber / Observer 三种模式；`sz_rust_core::event`（`sz-rust-state-facade`）+ `Event` 静态门面，`dispatch_facade` 支持 Sync / Async 分发。（⚠️ core::event 生产未接入；跨插件事件总线 `core::plugin::InMemoryEventBus` 可用，7 个行业插件基于它实现数据互通）
- **模型钩子**：`HookDispatcher` 16 事件（PHP 原生 12 + sz-orm-core 扩展 4：BeforeSave / AfterSave / BeforeValidate / AfterValidate）。（框架能力，由 sz-rust-orm-ext-facade 测试覆盖）
- **文件上传 + 图像处理**：对齐 `think\File` + `think\file\UploadedFile`，5 种存储引擎（Local / 阿里云 OSS / 腾讯云 COS / 七牛 Kodo / AWS S3 兼容）；图像处理对齐 PHP Grafika（缩放 / 裁剪 / 水印 / 文字）。（ℹ️ 有意设计：sz300 用自研 `FileService`（`file_service.rs`，本地存储 + 扩展名白名单）；core 5 引擎为框架能力，激活条件=云存储/多租户文件服务需求）
- **二维码生成 + 验证闭环（v1.2.2+ 扩展）**：对齐 PHP `endroid/qr-code`，`sz_rust_core::qr_code` 提供 PNG/SVG/Unicode/矩阵输出；新增 `generate_and_verify`（生成后立即内存解码回读，数据不一致报 `DataMismatch`）与 `verify_png_bytes`，解码后端 `rqrr`（MSRV 1.64，纯内存、无 `std::fs`）。适用于产品溯源、支付码、门票码的 CI 可扫描性自检。（见 ADR-042）
- **多应用分发**：对齐 ThinkPHP `auto_multi_app`，按 URI 前缀分发到子应用。（ℹ️ 有意设计：sz300 单应用部署；激活条件=多应用部署需求）
- **Guard 认证授权**：自研 Guard 模式（融合 NestJS Guard + Spring Security 思路）。（ℹ️ 有意设计：sz300 用自研 `auth_middleware`；core::guard 为框架能力，激活条件=插件/复杂应用复用）
- **视图模板**：对齐 PHP 模板引擎，支持 layout 布局与模板渲染。（✅ 生产已接入：sz300 `controllers/view.rs` 提供 `/page/{template}` 端点，调用 `View::with_default_engine()`）
- **HTTP/2 + TLS**：基于 rustls + tokio-rustls，对齐 think-swoole SSL。（ℹ️ 有意设计：sz300 用裸 `axum::serve`，TLS 由 nginx/k8s ingress 终止；激活条件=需进程内 TLS/HTTP2）
- **CLI 命令行工具**：`sz-rust-cli` 提供 make / migrate / route / cache / scheduler 等命令。
- **serve 生产化（v1.2.0 新增）**：`sz-rust serve` 支持 9 个生产级参数：`--with-admin`（加载 admin 插件）、`--addr`（监听地址）、`--workers`（worker 线程数）、`--grace-timeout`（优雅关闭超时）、`--health`/`--no-health`（健康检查端点 `/health/` + `/health/ready`）、`--access-log`（访问日志中间件）、`--tls-cert`/`--tls-key`（TLS/HTTPS）、`--watch-config`（配置热重载）。Unix 信号 SIGUSR1/SIGHUP 触发配置重载、SIGUSR2 切换日志级别；Windows 用 `serve:reload` / `serve:log-level` 子命令替代。
- **插件系统**：`sz-rust-addons-loader` 实现 `addons/` 插件加载与路由挂载。（ℹ️ 有意设计：core 编译期依赖并 re-export；hot-reload feature 门控，sz300 默认 `default = []` 关闭；激活条件=开发环境热加载需求，生产应启动时加载）
- **基于 SZ-ORM**：L4 金融级 ORM（Data Mapper + Repository 模式），编译时 SQL 校验（`sql_string!` / `query!` 宏）。
- **可观测性（v0.2.0 新增）**：`sz-rust-observability` 包提供 `MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）。（✅ MetricsRegistry 已接入：sz300 `main.rs` 初始化 + `state.rs` 共享；SLO 燃烧率为框架能力）
- **metrics 端点访问控制（T7）**：设置 `SZ300_METRICS_TOKEN` 后 `/metrics` 要求 `Authorization: Bearer <token>`，未授权返回 401；未设置时保持公开并输出启动 WARN（`main.rs`，2026-10-25 架构评测修复）。原企业版另有 `MetricsAuthConfig`（Bearer + IP 白名单 CIDR 双机制）实现保留在 git 历史可参考。（✅ 生产已接入：sz300 `main.rs`）
- **分布式追踪**：`sz_rust_observability::otlp`（OTLP gRPC/HTTP exporter）+ `sz-rust-tracing`（W3C TraceContext 传播）。（框架能力，`otlp` feature 门控）
- **Admin Monitor API（v1.1.0 新增）**：`admin` feature（默认关闭）提供 3 个管理端点：`GET /api/admin/server/info`（CPU/内存/磁盘/负载/Rust版本/主机名）、`GET /api/admin/db/pool`（连接池 active/idle/max/usage）、`GET /api/admin/redis/info`（Redis 版本/模式/连接数/内存/角色）。路由级 `RoleGuard` 中间件校验 `admin` 角色，无 Redis 连接时自动降级返回 `connected: false`。
- **GraphQL API**：基于 async-graphql 7.x，提供 `POST /graphql` 查询端点（v1.9.0 支持 DB-backed 持久化 Schema）。（✅ 已挂载：sz300 `router.rs`，`v18-graphql` feature 门控）
- **WebSocket**：基于 sz-orm-websocket（tokio-tungstenite），支持 HTTP 端口复用 + 独立端口两种模式，`WsHandler` trait 对齐 PHP Workerman onConnect/onMessage/onClose。（✅ 已挂载：sz300 `router.rs` `/ws` 端点，`v18-websocket` feature 门控）
- **WASM 边缘计算**：`sz-rust-wasm` crate 基于 wasmi（纯 Rust WASM 解释器），提供 `WasmRuntime` 加载/执行 WASM 模块，含燃料计量（防无限循环）、单实例 256 MiB 内存上限。（✅ 生产已接入：sz300 `POST /api/wasm/execute` 端点，**受 JWT 保护**；2026-10-27 P0-1 安全修复：原公开白名单 + 无资源限制 = 远程 DoS，已移出白名单并加燃料/内存/超时三重限制，`controllers/wasm_api.rs` + `router.rs`）
- **K8s Operator**：（⚠️ 已移除：`sz-rust-k8s-operator` 孤儿 crate，无消费者，22 测试保留在 git 历史，详见 ADR-038）
- **AI 能力栈生产接线（v2.0.0 合并自企业版）**：sz300 提供 `/api/v1/ai/chat`（prompt 长度上限 + 模型白名单，安全修复 M-1）、`/api/v1/ai/embed`、`/api/v1/ai/stream`（SSE）三个端点（`controllers/ai.rs`，受 JWT 保护）；`main.rs` 由 `SZ300_AI_API_KEY`（可选 `SZ300_AI_BASE_URL`）驱动 `Ai::init_default`（OpenAI 兼容 Provider），未配置时端点返回明确错误提示。RAG 检索增强在 chat 链路中自动尝试（`sz-rust-rag` IndustryRag，失败不阻塞）。（✅ 生产已接线）
- **Admin 后台管理插件（v1.2.0 新增）**：`sz-rust-addons-admin` 提供 7 大模块（用户/角色/权限/菜单/配置/操作日志/仪表盘）21 个 REST API 端点 + 17 个 `admin.*` Capability。`AdminAddonPlugin` 通过 `plugin.router()` 合并路由 + `plugin.capability_hook()` 注册 Capability 实现生产接线。内置 `permission_guard_middleware`（路由→权限项映射，超级管理员 bypass）、bcrypt 密码脱敏、配置租户继承（tenant_id=0 全局 + 租户覆盖）、操作日志异步写入。（✅ 生产已接入：sz-rust-examples 演示接线 + 9 端到端验证测试）

> ⚠️ **headless 说明（2026-10-27 UXR P0-1）**：`sz-rust-addons-admin` / `sz-rust-admin-ui` / `sz-rust-monitor-panel` / `sz-rust-codegen-ui` 均为**纯后端库，无内置浏览器界面**——当前交付的是 REST API 与 Capability，**不包含可点击的 Web 管理后台**。需要图形后台时请对接 `sz-rust-visual`（Tauri 2 + Vue3 桌面工作台，走 IPC）或自行实现前端。

> **v1.9.0 生产深化 + 闭环验证 + 债务清偿（2026-10-07）**：10 大核心能力（GraphQL 持久化 / JWT Audience / Saga 分布式事务 / 可观测性闭环 / 性能深化 / AI 智能分类 / 国际化 / 配置中心 / 插件生态 / 生产配置）均由 `all-v1-9` feature gate 控制，默认不启用；验证：`cargo test --lib --features all-v1-9` = 106 passed。详见 [CHANGELOG v1.9.0](CHANGELOG.md)。

---

## 与 ThinkPHP 8 对标表

| ThinkPHP 8 能力 | SZ-Rust 对应实现 | 说明 |
|----------------|------------------|------|
| `app\SzController`（abstract） | `sz_rust_core::controller::SzController` trait | `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` |
| `app\BaseController` | `sz_rust_core::controller::BaseController` trait | `batchValidate` / `$middleware` / `initialize()` / `validate()` |
| `addons\BaseController` | `sz_rust_core::controller::AddonsBaseController` trait | `allowAllAction` / `getRouteinfo()` / `getToken()` / `checkLogin()` |
| `think\Model`（Active Record） | `sz_rust_core::model::BaseModel` trait + SZ-ORM Repository | Data Mapper 模式；`$name`/`$pk`/`$append`/`$hidden`/`$fillable` 全对齐 |
| `getXxxAttr` / `setXxxAttr` | `Accessor` / `Mutator` trait | 访问器缓存、修改器 merged_data、PHP bug 严格复刻 |
| `think\Validate` | `sz_rust_core::validate::Validate` | 30+ 规则、场景、批量验证、自定义消息 |
| `think\facade\Cache` | `sz_rust_core::cache` | 复用 sz-orm-storage 驱动 |
| `think\Event` | `sz_rust_core::event` | Listener / Subscriber / Observer |
| think-orm Model 钩子 | `sz_rust_core::hooks::HookDispatcher` | 16 事件（PHP 12 + 扩展 4） |
| `think\File` / `UploadedFile` | `sz_rust_core::upload::File` / `UploadedFile` | hash / move / hashName / isValid |
| storage engine（Local/Aliyun/Qcloud/Qiniu） | `sz_rust_core::upload::storage` | 5 引擎（+ S3 兼容） |
| `app/middleware.php` | `sz_rust_core::middleware` | CORS / Auth / Log / RateLimit / Trace + 链构建器 |
| `auto_multi_app` | `sz_rust_core::multi_app` | 按 URI 前缀分发子应用 |
| `think-swoole` SSL | `sz_rust_core::h2` | HTTP/2 + TLS（rustls） |
| `think-logger` | `sz_rust_core::log::LogFacade` | tracing 集成 |
| `compact()` | `sz-rust-macros` | 过程宏（独立包，非 `sz_rust_core::macros` 占位模块） |
| `config/app.php` / `database.php` | `sz_rust_core::config::AppConfig` | YAML 配置加载 |
| `app()` 容器 | `sz_rust_core::container::App` | 应用容器 |
| `BaseException` | `sz_rust_core::error::ErrorCode` | 标准错误码 |
| `addons/` 插件 | `sz-rust-addons-loader` | 插件加载 + 路由挂载 |
| think-swoole / think-worker | `sz_rust_core::server` | tokio 多线程运行时 |
| 模板引擎 | `sz_rust_core::view` | layout + template |
| —（自研） | `sz_rust_core::guard` | Guard 认证授权 |

---

## 项目结构

> v2.0.0 单一版本：全部 65 个 crate 在同一 workspace（完整 members 列表见根 `Cargo.toml`），以下为分层概览

```
sz-rust/                          # workspace 根目录（唯一真源）
├── Cargo.toml                    # workspace 配置（axum 0.8 / SZ-ORM 全家桶 / 65 members）
├── deny.toml                     # cargo-deny 配置（许可证/RUSTSEC/重复依赖/来源审计）
├── config/                       # 默认配置（app/database/cache/log/addons YAML）
├── docker-compose.prod.yml       # 生产部署模板（安全 feature + 令牌）
├── scripts/audit/                # 门禁脚本（安全 Gauntlet / 许可证合规 / doc-code 一致性…）
├── schemas/openapi-sz300.json    # OpenAPI 3.0.3 契约（41 paths，与运行时 /api-docs/openapi.json 一致）
└── packages/
    ├── sz-rust-core/             # 核心框架包（controller/model/middleware/validate/...）
    ├── sz-rust-macros/           # 过程宏包（compact 等）
    ├── sz-rust-examples/         # 示例包（quick_start / crud_demo）
    ├── sz-rust-cli/              # CLI 命令行工具（make/migrate/route/cache/scheduler/plugin/admin）
    ├── sz-rust-{http,orm,cache,state,infra,auth,pay,orm-ext,router,middleware,mvc}-facade/
    │                             # 11 个统一门面
    ├── sz-rust-ai-facade/        # AI facade（LLM/Embedding/RAG/Agent/MCP Bridge）
    ├── sz-rust-rag/              # RAG 检索管道
    ├── sz-rust-{sdd,migration}/  # SDD Agent 编排引擎 + TP6 AI 迁移工具（v2.0.0 合并）
    ├── sz-rust-marketplace/      # 插件市场（Ed25519 签名 + Web 平台 + CLI）
    ├── sz-rust-addons-loader/    # 插件加载器
    ├── sz-rust-addons-{admin,cms,crm,ecommerce,erp,forum,im,operate}/
    │                             # 8 个插件（v2.0.0 合并后全部开源）
    ├── sz-rust-{observability,tracing,pdf,workflow,wasm,websocket,i18n,...}/
    │                             # 可观测性 / 协议 / 工具 crate
    ├── sz-rust-{admin-ui,monitor-panel,codegen-ui}/
    │                             # headless 后端库（服务端 UI 数据装配/API 编排，无浏览器资产；
    │                             #   真实前端为 sz-rust-visual：Tauri 2 + Vue3，走 IPC）
    └── sz-rust-sz300/            # SZ300 业务应用（生产参考实现：含 AI/WASM/GraphQL/WS/限流/熔断）
```

---

## 文档索引

详细文档位于 `docs/` 目录：

- [docs 根目录文档导航](docs/README.md) — docs/ 根目录全部文档索引（ADR/审计/基准/规范/指南等）
- [ADR 索引](docs/adr/README.md) — 51 个架构决策记录（`docs/adr/*.md`，2026-10-30 实测计数）
- [ADR-011 可观测性模块](docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) — MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（v0.2.0）
- [ADR-012 分布式追踪](docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) — W3C TraceContext 标准、OTLP exporter 路径（v0.2.0）
- [ThinkPHP API 对照表](docs/thinkphp-api-mapping.md) — SZ-Rust 门面与 ThinkPHP API 逐项对照
- [工程化实践规范](docs/sz-rust-engineering-practices.md) — 10 道门禁、CI/CD、代码风格
- [数据库支持矩阵](docs/database-support-matrix.md) — 各方言支持状态与运维基线（MySQL 单主）
- [软件项目审计清单](docs/软件项目审计清单.md) — P0/P1/P2 审计维度
- [ADR 与生产 Bug 定位规范](docs/ADR与生产Bug定位规范.md) — 可复用规范（v1.0）
- [初始审计报告](docs/audit/archive/2026-07/2026-07-22-初始审计.md) — P0 全通过 / P1 需改进 2 项
- [性能基线 v0.1.0](docs/benchmarks/baseline-v0.1.0.md) — criterion 基线数据，回归参照
- [项目深度评估与框架对比报告](docs/audit/archive/2026-08/2026-08-10-项目深度评估与框架对比报告.md) — v0.7.0 综合评估 + 4 框架 × 3 路由 × 4 并发压测（48 数据点）
- [审计与评估报告索引](docs/audit/README.md) — 所有审计/评估/对比报告的统一导航
- [专家级深度安全审查（2026-10-25）](docs/audit/2026-10-25-security-gauntlet.md) — 26 项安全 Gauntlet 实测全过 + 可复现脚本 + OpenAPI 契约
- [单一版本合并方案（2026-10-25）](docs/single-edition-merge-plan.md) — AI 时代废除企业版、全版本合并的战略决策与技术实施
- [ADR-043 AI 时代单一版本合并](docs/adr/ADR-043-ai时代单一版本合并.md) — 废除开源/企业分离的架构决策记录

> 注：模块级文档注释（`cargo doc -p sz-rust-core --open`）包含完整的 PHP 源码行号对照与 bug 复刻说明。CI doc job 启用 `-D missing_docs` 严格检查，所有公开 API 必须有文档注释。

---

## 生产部署安全基线（2026-10-25 新增，2026-10-28 更新）

> 2026-10-28 起，`sz-rust-sz300` 的 `default` features 已默认启用 `v18-security-headers` / `v18-data-mask` / `v18-audit-chain` / `v18-key-rotation` / `v19-jwt-audience`。生产构建**无需再手动启用这五项**；`v18-rbac` 与 `v18-upload` 仍为可选，按需开启。

### 1. 默认已启用的安全 feature

默认构建即包含：
- `v18-security-headers`：CSP / HSTS / X-Frame-Options / X-Content-Type-Options / Referrer-Policy
- `v18-data-mask`：响应敏感字段脱敏
- `v18-audit-chain`：写操作审计链式哈希
- `v18-key-rotation`：JWT 密钥轮换（30 天，密码学安全 RNG）
- `v19-jwt-audience`：JWT audience 校验（登录签发带 `aud` 令牌）

如需额外启用 RBAC 与增强上传，可显式追加：

```bash
# 本地生产构建（默认安全 feature 已含，追加 RBAC/upload）
cargo build --release -p sz-rust-sz300 --features \
  v18-rbac,v18-upload

# Docker 生产构建（通过构建参数传入）
docker build --build-arg SZ300_FEATURES="v18-rbac,v18-upload" .
```

Docker 生产部署模板见 `docker-compose.prod.yml`：

```bash
export SZ300_JWT_SECRET="$(openssl rand -hex 32)"
export SZ300_DB_PASSWORD="$(openssl rand -hex 16)"
export SZ300_METRICS_TOKEN="$(openssl rand -hex 32)"
docker compose -f docker-compose.prod.yml up -d --build
```

**开发环境一键启动**（UXR 2026-10-27 P2-3 整改）：自动加载 `.env.example` 占位值，无需手动 export：

```bash
docker compose --env-file .env.example up -d --build
```

### 2. 保护 `/metrics` 端点

设置环境变量 `SZ300_METRICS_TOKEN` 后，`/metrics` 要求 `Authorization: Bearer <token>`，未授权返回 401。**生产 compose 模板已强制要求该变量**；未设置时端点保持公开并输出启动 WARN（仅建议内网环境）。

### 3. 配置可信 CORS 来源

默认 CORS 为 `Allow-Origin: *`（不带凭据，适合开发）。生产环境应设置 `SZ300_CORS_ORIGIN` 指定唯一可信前端来源（精确匹配，拒绝其他跨域来源）：

```bash
export SZ300_CORS_ORIGIN="https://admin.example.com"
```

### 4. 启用后自检

启动日志中出现以下信息即表示安全基线生效：

```
可观测性 /metrics 端点已启用 Bearer 令牌鉴权
```

若使用 `--no-default-features` 构建，启动时会输出 `未启用任何 v1.8.0 安全/协议 feature` 的 WARN，此时请显式追加安全 feature。

---

## CI 门禁与质量保障

项目通过 GitHub Actions 实施门禁体系（`ci.yml` 实际 **37 个 job**，2026-10-28 复核修正——数字以 `grep -c '^  [a-z-]*:$'` 解析 `jobs:` 直接子键为准；`machete` 为 `continue-on-error: true` 非阻塞；`coverage` 与 `miri` 为阻塞门禁，miri 检出 UB/内存问题即 fail-closed）：

| Workflow | 触发条件 | 职责 |
|----------|---------|------|
| `ci.yml` | push/PR | 37 个 job：fmt / check / **repro-check（依赖可复现：sz-orm 提交固定 + 声明版本一致）** / clippy / test / doc(missing_docs) / audit / deny(cargo-deny) / no-placeholder / feature-matrix / unused-deps / outdated / machete* / adr-coverage / coverage / compile-time / miri / windows / ai-facade / **doc-code-consistency（防幻影交付）** / **adr-code（ADR 引用代码对账）** / **assertion-value（拒绝无断言空洞测试）** / **feature-consistency（feature 声明对账）** / license-compliance（Apache-2.0 头+字段） / no-crate-dead-code / i18n-completeness / boundary-check / openapi-consistency / unwrap-gate（生产 unwrap=0） / ignored-test-routing / coverage-matrix-completeness / ci-config-audit / test-hygiene-check / orphan-module-check / **perf-static（N+1/锁跨 await/阻塞调用/串行 await，warn 级）** / **advisory-ignore-expiry（RUSTSEC 忽略项到期日）** / **unwired-crates（零入度 crate 登记）**（* 标记为 continue-on-error 非阻塞；coverage 与 miri 为阻塞门禁；原 isolation 隔离检查已随 ADR-043 单一版本合并废止） |
| `coverage.yml` | push/PR | cargo-tarpaulin 覆盖率统计 + Codecov 上传 |
| `benchmark.yml` | push main / PR | criterion 性能基准测试 + **11 类基准覆盖门禁**（`benchmark.yml` 第 30-62 行校验 `>= 11 categories`）+ gh-pages-bench 分支保存 |
| `perf-gate.yml` | PR（仅基线结构检查）/ 每周日 04:00 UTC / workflow_dispatch | **端到端 HTTP 性能门禁**（`scripts/perf-regression.js` v2）：MySQL 9.6 + migrations 001–007 + 确定性 2 万行数据集 + release 构建 + 真实负载（预热 10 s / 稳态 60 s × 3）；无指纹基线即**判定失败**并播种 artifact，不做"无基线即通过"。口径与阈值见 ADR-046 |
| `security.yml` | push/PR + 每周日 00:00 UTC | cargo-audit（**阻塞**，2026-10-25 起移除 continue-on-error；忽略清单及理由见 `.cargo/audit.toml`）+ **unmaintained 真实编译检查（paste/rustls-pemfile）** + cargo-geiger（非阻塞，仅汇总） |
| `fuzz.yml` | push/PR + 每周六 00:00 UTC + workflow_dispatch | 10 用例 × 1000 迭代模糊测试，支持 `FUZZ_ITERATIONS` 自定义 |
| `mcdc.yml` / `mutants.yml` | 每周 schedule + workflow_dispatch | 分支覆盖率门禁（阈值 75%，fail-closed） / 变异测试（杀死率 ≥90%，fail-closed） |
| `publish-oss.yml` / `release.yml` | 发布流程 | crates.io 发布 / release 构建 |
| `marketplace-ci.yml` | 插件市场路径变更 | 插件市场 crate CI（`sz-rust-marketplace` 已实体交付：130 测试 + CLI 接线，旧「虚构交付」定性已过时） |

> 本地安全回归：`bash scripts/audit/security-gauntlet.sh`（26 项安全 Gauntlet，真实 TCP 原始 HTTP，全部通过退出码 0）。

> 注：`soak.yml` / `soak-nightly.yml` 已停用（重命名为 `.disabled`，2026-08-13 前），6h soak 由本地自托管工具 `scripts/soak-self-hosted/` 承担（见 `docs/soak-toolkit-guide.md`）。

**cargo-deny 审计维度**（`deny.toml`）：
- 许可证白名单：MIT / Apache-2.0 / BSD / ISC / Zlib
- 许可证黑名单：GPL / AGPL / EUPL
- RUSTSEC 安全漏洞检查
- 重复依赖警告 + 通配符禁止
- 来源限制：仅允许 crates.io

---

## 许可证

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the LICENSE-APACHE file for the full license text.

SPDX-License-Identifier: Apache-2.0

---

## W5/W6 质量交付（2026-08-12）

> 覆盖弱点：W1（测试隔离）/ W3（插件测试覆盖）/ W5（性能基准）/ W6（安全审计）/ W7（E2E 验证）

### 交付内容摘要

| 任务域 | 结论 | 关键数字 | 来源 |
|--------|------|---------|------|
| TF（测试修复） | ✅ | 647 passed, 0 failed（2026-08-12 W5/W6 交付时点实测） | `cargo test -p sz-rust-sz300`；当前 sz300 测试总数随版本增长，以 CHANGELOG/CI 实测为准 |
| PB（性能基准） | ✅ | 45 bench case, RSS 7 MB | `docs/benchmarks/2026-08-12-w5-w6-baseline.md` |
| SA（安全审计） | ✅ | 23 条铁律全通过 | `docs/audit/2026-08-12-w5-w6-security-audit.md` |
| E2E（端到端） | ✅ | 8 阶段全执行 | `docs/audit/2026-08-12-w5-w6-e2e-report.md` |

### 新增文件

- `scripts/check_iron_laws.py` — 23 条铁律自动化检查 <!-- 来源: P0 对账 -->
- `scripts/run_security_audit.py` — 漏洞与许可证扫描
- `scripts/measure_startup_rss.ps1` — 启动内存 RSS 测量
- `scripts/e2e_deploy.js` — ssh2 部署脚本（禁止 sshpass/killall）
- `scripts/e2e_orchestrate.js` — E2E 8 阶段编排
- `docs/benchmarks/2026-08-12-w5-w6-baseline.md` — 性能基线报告
- `scripts/perf-baseline/` — 生产实例压测脚手架（ab + ssh2 + 资源采样 + 健康探测）
- `docs/audit/2026-08-20-perf-baseline.md` — sz300 生产性能基线报告（3 端点 × 5 并发）
- `docs/audit/2026-08-12-w5-w6-security-audit.md` — 安全审计报告
- `docs/audit/2026-08-12-w5-w6-e2e-report.md` — E2E 验证报告
