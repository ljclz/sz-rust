> **中文** | [English](README.en.md)

# SZ-Rust — 对标 ThinkPHP 8 的 Rust Web 框架

基于 axum 0.8 + SZ-ORM 的 Rust Web 框架，API 设计对齐 ThinkPHP 8，便于 PHP 工程师迁移。

**当前版本：v0.7.0**（2026-08-10）— crates.io 全量发布 + 4 框架 × 3 路由 × 4 并发压测 + 资源监控集成 + 深度评估更新

> **v0.6.7 → v0.7.0 变更摘要**：见 [docs/CHANGELOG.md](docs/CHANGELOG.md)

---

## 核心特性

以下特性均来自 `sz-rust-core` 实际源码，模块结构见 `packages/sz-rust-core/src/lib.rs`。

- **HTTP 服务器 + 路由**：基于 axum 0.8 + tower 0.5 + hyper 1.x，支持三层路由机制（属性宏 / 配置式 / 约定式）。
- **控制器层**：`SzController` → `BaseController` → `AddonsBaseController` 三层 trait 继承链，对齐 PHP `app\SzController` / `app\BaseController` / `addons\BaseController`。提供 `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` 等方法。
- **模型层**：`BaseModel` trait 组合 SZ-ORM 的 `Model` + `ModelExt` + `RelationLoader`，对齐 `think\Model`。支持 `$append` 虚拟字段、访问器（`Accessor`）、修改器（`Mutator`）、动态 Append（`Appendable`）。
- **中间件**：内置 CORS / Auth(JWT) / Log / RateLimit / Trace 五个中间件，外加链构建器（`MiddlewareChain`）、Handler=Middleware 双向转换器、tower-http 兼容层。（✅ 生产接入状态：sz300 挂载 CORS + CSRF + 限流(令牌桶) + 熔断器 + 自研 auth_middleware（router.rs:148-167）；middleware-facade 的 Auth/Log/Trace 未挂载，auth 用自研版因签名不兼容）
- **限流器（v0.3.1 已落地）**：`sz-rust-middleware-facade::rate_limit` 提供令牌桶（`TokenBucket`）+ 滑动窗口（`SlidingWindow`）两种算法，实现 `sz_rust_orm_facade::RateLimiter` trait，含 OOM 防护与 100 并发无误差测试。（✅ 生产已接入：sz300 `router.rs:158-160` 挂载令牌桶中间件，默认生效，配置由 `RateLimitProductionConfig::from_env()` 读取）
- **熔断器（v0.3.1 已落地）**：`sz-rust-middleware-facade::circuit_breaker` 提供 Closed/Open/HalfOpen 三态状态机 + `circuit_breaker_middleware`（Open 返回 503），parking_lot::Mutex 保护并发安全。（✅ 生产已接入：sz300 `router.rs:153-155` 挂载熔断中间件，默认生效，配置由 `CircuitBreakerProductionConfig::from_env()` 读取）
- **验证器**：对齐 `think\Validate`，内置 30+ 规则（require / integer / float / email / url / ip / regex / length / max / min / between / in / notIn / confirm / different / date / after / before / requireIf / requireWith 等），支持批量验证、场景、自定义消息。（✅ 生产已接入：`merchant.rs:103` 调用 `Validate::new()`）
- **缓存系统**：对齐 `think\facade\Cache`，复用 sz-orm-storage 驱动。（⚠️ 生产未接入：sz300 未调用 core::cache，仅 Cargo.toml 有 cache-facade 依赖声明）
- **事件系统**：对齐 `think\Event`，支持 Listener / Subscriber / Observer 三种模式。（⚠️ 生产未接入：sz300 零调用，仅 CLI stubs 模板引用）
- **模型钩子**：`HookDispatcher` 16 事件（PHP 原生 12 + sz-orm-core 扩展 4：BeforeSave / AfterSave / BeforeValidate / AfterValidate）。（✅ 生产已接入：`order.rs:26` 引入 HookContext/HookEvent，`main.rs:174` 初始化 HookRegistry）
- **文件上传 + 图像处理**：对齐 `think\File` + `think\file\UploadedFile`，5 种存储引擎（Local / 阿里云 OSS / 腾讯云 COS / 七牛 Kodo / AWS S3 兼容）；图像处理对齐 PHP Grafika（缩放 / 裁剪 / 水印 / 文字）。（⚠️ 生产接入状态：sz300 用自研 `FileService`（`file_service.rs`，tokio::fs 直写 + 扩展名白名单），core 5 存储引擎零生产调用）
- **多应用分发**：对齐 ThinkPHP `auto_multi_app`，按 URI 前缀分发到子应用。（⚠️ 生产未接入：sz300 单应用部署，multi_app 零调用）
- **Guard 认证授权**：自研 Guard 模式（融合 NestJS Guard + Spring Security 思路）。（⚠️ 生产未接入：sz300 用自研 `auth_middleware` + `role_guard`，未走 core::guard）
- **视图模板**：对齐 PHP 模板引擎，支持 layout 布局与模板渲染。（✅ 生产已接入：`view.rs:18` 调用 `View::with_default_engine()`）
- **HTTP/2 + TLS**：基于 rustls + tokio-rustls，对齐 think-swoole SSL。（⚠️ 生产未接入：sz300 用裸 `axum::serve`，无 TLS）
- **CLI 命令行工具**：`sz-rust-cli` 提供 make / migrate / route / cache / scheduler 等命令。
- **插件系统**：`sz-rust-addons-loader` 实现 `addons/` 插件加载与路由挂载。（⚠️ 生产接入状态：core 编译期依赖并 re-export；运行时由 hot-reload feature 门控，sz300 默认 `default = []` 关闭，`main.rs:104-106` 仅打日志）
- **基于 SZ-ORM**：L4 金融级 ORM（Data Mapper + Repository 模式），编译时 SQL 校验（`sql_string!` / `query!` 宏）。
- **可观测性（v0.2.0 新增）**：`sz-rust-observability` 包提供 `MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）。（✅ 生产已接入：`main.rs:125` MetricsRegistry + `main.rs:168` SLO 监控器，`health.rs:37/39` 记录燃烧率）
- **metrics 端点访问控制（T7）**：`MetricsAuthConfig` 提供 Bearer token + IP 白名单（支持 CIDR，v4/v6）双机制；`/metrics` 路由独立挂载 `metrics_auth_middleware`（`router.rs:54` metrics_router()，独立子路由不污染业务 API），未授权返回 403；`SZ300_ENV=production` 时启动强制校验必须配置鉴权否则拒绝启动（`main.rs:243`），客户端真实 IP 通过 `into_make_service_with_connect_info` 注入（`main.rs:262`）。（✅ 生产已接入）
- **分布式追踪**：sz300 用原生 `tracing` + `sz_rust_observability::otlp`（OTLP gRPC exporter，`main.rs:169`）。（✅ 生产已接入：`otlp` feature 门控）
- **Admin Monitor API（v1.1.0 新增）**：`admin` feature（默认关闭）提供 3 个管理端点：`GET /api/admin/server/info`（CPU/内存/磁盘/负载/Rust版本/主机名）、`GET /api/admin/db/pool`（连接池 active/idle/max/usage）、`GET /api/admin/redis/info`（Redis 版本/模式/连接数/内存/角色）。路由级 `RoleGuard` 中间件校验 `admin` 角色，无 Redis 连接时自动降级返回 `connected: false`。

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
    let config = AppConfig::load_from_dir(&config_dir).unwrap_or_else(|_| AppConfig::default());

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

```
sz-rust/                          # workspace 根目录
├── Cargo.toml                    # workspace 配置（axum 0.8 / SZ-ORM 全家桶）
├── deny.toml                     # cargo-deny 配置（许可证/RUSTSEC/重复依赖/来源审计）
├── config/                       # 默认配置（app/database/cache/log/addons YAML）
└── packages/
    ├── sz-rust-core/             # 核心框架包（controller/model/middleware/validate/...）
    ├── sz-rust-macros/           # 过程宏包（compact 等）
    ├── sz-rust-examples/         # 示例包（quick_start / crud_demo）
    ├── sz-rust-cli/              # CLI 命令行工具（make/migrate/route/cache/scheduler）
    ├── sz-rust-http-facade/      # HTTP 基础层（response/error/request）
    ├── sz-rust-orm-facade/       # ORM 全家桶统一入口
    ├── sz-rust-cache-facade/     # 缓存抽象层（Memory/Redis/Memcached/MultiLevel）
    ├── sz-rust-state-facade/     # 应用状态（session/cookie/env/event/i18n/mail/notify）
    ├── sz-rust-infra-facade/     # 基础设施（config/validate/static_files/upload/debug_page）
    ├── sz-rust-auth-facade/      # 认证层（wechat/oauth/gateway/sso/redis_store）
    ├── sz-rust-pay-facade/       # 支付聚合层（Alipay/WeChat Pay）
    ├── sz-rust-orm-ext-facade/   # ORM 扩展
    ├── sz-rust-router-facade/    # 路由 facade
    ├── sz-rust-middleware-facade/ # 中间件 facade（rate_limit/circuit_breaker/csrf）
    ├── sz-rust-mvc-facade/       # MVC facade
    ├── sz-rust-ai-facade/        # AI facade（LLM/Embedding/RAG/Agent/MCP Bridge）
    ├── sz-rust-mcp/              # MCP 协议（stdio JSON-RPC）
    ├── sz-rust-addons-loader/    # 插件加载器
    ├── sz-rust-addons-ecommerce/ # 电商插件
    ├── sz-rust-addons-cms/       # CMS 插件（文章/分类/标签）
    ├── sz-rust-addons-crm/       # CRM 插件（客户/线索/商机）
    ├── sz-rust-observability/    # 可观测性模块（MetricsRegistry + SLO 燃烧率）

    └── sz-rust-sz300/            # SZ300 业务应用（端到端集成示例）
```

---

## 文档索引

详细文档位于 `docs/` 目录：

- [ADR 索引](docs/adr/README.md) — 20 个架构决策记录（ADR-001 ~ ADR-020），全部已接受
- [ADR-011 可观测性模块](docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) — MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（v0.2.0）
- [ADR-012 分布式追踪](docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) — W3C TraceContext 标准、OTLP exporter 路径（v0.2.0）
- [PHP 迁移指南](docs/php-migration-guide.md) — PHP → Rust 概念映射与行为对齐（R5 硬约束），含 15 章节
- [工程化实践规范](docs/sz-rust-engineering-practices.md) — 10 道门禁、CI/CD、代码风格
- [软件项目审计清单](docs/软件项目审计清单.md) — P0/P1/P2 审计维度
- [ADR 与生产 Bug 定位规范](docs/ADR与生产Bug定位规范.md) — 可复用规范（v1.0）
- [初始审计报告](docs/audit/2026-07-22-初始审计.md) — P0 全通过 / P1 需改进 2 项
- [性能基线 v0.1.0](docs/benchmarks/baseline-v0.1.0.md) — criterion 基线数据，回归参照
- [项目深度评估与框架对比报告](docs/audit/archive/2026-08/2026-08-10-项目深度评估与框架对比报告.md) — v0.7.0 综合评估 + 4 框架 × 3 路由 × 4 并发压测（48 数据点）
- [审计与评估报告索引](docs/audit/README.md) — 所有审计/评估/对比报告的统一导航

> 注：模块级文档注释（`cargo doc -p sz-rust-core --open`）包含完整的 PHP 源码行号对照与 bug 复刻说明。CI doc job 启用 `-D missing_docs` 严格检查，所有公开 API 必须有文档注释。

---

## CI 门禁与质量保障（v0.7.0 增强）

项目通过 GitHub Actions 实施 23 道门禁，所有门禁严格生效（无 `continue-on-error`）：

| Workflow | 触发条件 | 职责 |
|----------|---------|------|
| `ci.yml` | push/PR | 23 个 job：fmt / check / clippy / test / doc(missing_docs) / audit / deny(cargo-deny) / no-placeholder / feature-matrix / unused-deps / outdated / machete / adr-coverage / db-integration / coverage(≥85%) / compile-time / miri / windows / ai-facade / **doc-code-consistency（门禁 19，防幻影交付）** / **adr-code（门禁 20，ADR 引用代码对账）** / **assertion-value（门禁 21，拒绝无断言空洞测试）** / **feature-consistency（门禁 22，feature 声明对账）** |
| `coverage.yml` | push/PR | cargo-tarpaulin 覆盖率统计 + Codecov 上传 |
| `benchmark.yml` | push main / PR | criterion 性能基准测试 + **9 类基准覆盖门禁** + gh-pages-bench 分支保存 |
| `security.yml` | push/PR + 每周日 00:00 UTC | cargo-audit + **unmaintained 真实编译检查（paste/rustls-pemfile/rkyv）** + cargo-geiger |
| `fuzz.yml` | push/PR + 每周六 00:00 UTC + workflow_dispatch | 10 用例 × 1000 迭代模糊测试，支持 `FUZZ_ITERATIONS` 自定义 |
| `mcdc.yml` / `mutants.yml` | push/PR | 分支覆盖率门禁 / 变异测试 |
| `publish-oss.yml` / `release.yml` | 发布流程 | crates.io 发布 / release 构建 |
| `marketplace-ci.yml` | 插件市场路径变更 | ⚠️ 引用 `sz-rust-marketplace`（已定性虚构交付，见 2026-08-13 审计报告，workflow 永不触发） |

> 注：`soak.yml` / `soak-nightly.yml` 已停用（重命名为 `.disabled`，2026-08-13 前），6h soak 由本地自托管工具 `scripts/soak-self-hosted/` 承担（见 `docs/soak-toolkit-guide.md`）。

**cargo-deny 审计维度**（`deny.toml`）：
- 许可证白名单：MIT / Apache-2.0 / BSD / ISC / Zlib
- 许可证黑名单：GPL / AGPL / EUPL
- RUSTSEC 安全漏洞检查
- 重复依赖警告 + 通配符禁止
- 来源限制：仅允许 crates.io

---

## 许可证

MIT

---

## W5/W6 质量交付（2026-08-12）

> 覆盖弱点：W1（测试隔离）/ W3（插件测试覆盖）/ W5（性能基准）/ W6（安全审计）/ W7（E2E 验证）

### 交付内容摘要

| 任务域 | 结论 | 关键数字 | 来源 |
|--------|------|---------|------|
| TF（测试修复） | ✅ | 171 passed, 0 failed | `cargo test -p sz-rust-sz300` |
| PB（性能基准） | ✅ | 45 bench case, RSS 7 MB | `docs/benchmarks/2026-08-12-w5-w6-baseline.md` |
| SA（安全审计） | ✅ | 22 条铁律全通过 | `docs/audit/2026-08-12-w5-w6-security-audit.md` |
| E2E（端到端） | ✅ | 8 阶段全执行 | `docs/audit/2026-08-12-w5-w6-e2e-report.md` |

### 新增文件

- `scripts/check_iron_laws.py` — 22 条铁律自动化检查
- `scripts/run_security_audit.py` — 漏洞与许可证扫描
- `scripts/measure_startup_rss.ps1` — 启动内存 RSS 测量
- `scripts/e2e_deploy.js` — ssh2 部署脚本（禁止 sshpass/killall）
- `scripts/e2e_orchestrate.js` — E2E 8 阶段编排
- `docs/benchmarks/2026-08-12-w5-w6-baseline.md` — 性能基线报告
- `docs/audit/2026-08-12-w5-w6-security-audit.md` — 安全审计报告
- `docs/audit/2026-08-12-w5-w6-e2e-report.md` — E2E 验证报告
