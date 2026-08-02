# SZ-Rust — 对标 ThinkPHP 8 的 Rust Web 框架

基于 axum 0.8 + SZ-ORM 的 Rust Web 框架，API 设计对齐 ThinkPHP 8，便于 PHP 工程师迁移。

**当前版本：v0.2.1**（2026-07-30）— 新增可观测性、分布式追踪、模糊测试、覆盖率、soak test、CI 门禁增强

---

## 核心特性

以下特性均来自 `sz-rust-core` 实际源码，模块结构见 `packages/sz-rust-core/src/lib.rs`。

- **HTTP 服务器 + 路由**：基于 axum 0.8 + tower 0.5 + hyper 1.x，支持三层路由机制（属性宏 / 配置式 / 约定式）。
- **控制器层**：`SzController` → `BaseController` → `AddonsBaseController` 三层 trait 继承链，对齐 PHP `app\SzController` / `app\BaseController` / `addons\BaseController`。提供 `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` 等方法。
- **模型层**：`BaseModel` trait 组合 SZ-ORM 的 `Model` + `ModelExt` + `RelationLoader`，对齐 `think\Model`。支持 `$append` 虚拟字段、访问器（`Accessor`）、修改器（`Mutator`）、动态 Append（`Appendable`）。
- **中间件**：内置 CORS / Auth(JWT) / Log / RateLimit / Trace 五个中间件，外加链构建器（`MiddlewareChain`）、Handler=Middleware 双向转换器、tower-http 兼容层。
- **验证器**：对齐 `think\Validate`，内置 30+ 规则（require / integer / float / email / url / ip / regex / length / max / min / between / in / notIn / confirm / different / date / after / before / requireIf / requireWith 等），支持批量验证、场景、自定义消息。
- **缓存系统**：对齐 `think\facade\Cache`，复用 sz-orm-storage 驱动。
- **事件系统**：对齐 `think\Event`，支持 Listener / Subscriber / Observer 三种模式。
- **模型钩子**：`HookDispatcher` 16 事件（PHP 原生 12 + sz-orm-core 扩展 4：BeforeSave / AfterSave / BeforeValidate / AfterValidate）。
- **文件上传 + 图像处理**：对齐 `think\File` + `think\file\UploadedFile`，5 种存储引擎（Local / 阿里云 OSS / 腾讯云 COS / 七牛 Kodo / AWS S3 兼容）；图像处理对齐 PHP Grafika（缩放 / 裁剪 / 水印 / 文字）。
- **多应用分发**：对齐 ThinkPHP `auto_multi_app`，按 URI 前缀分发到子应用。
- **Guard 认证授权**：自研 Guard 模式（融合 NestJS Guard + Spring Security 思路）。
- **视图模板**：对齐 PHP 模板引擎，支持 layout 布局与模板渲染。
- **HTTP/2 + TLS**：基于 rustls + tokio-rustls，对齐 think-swoole SSL。
- **CLI 命令行工具**：`sz-rust-cli` 提供 make / migrate / route / cache / scheduler 等命令。
- **插件系统**：`sz-rust-addons-loader` 实现 `addons/` 插件加载与路由挂载。
- **基于 SZ-ORM**：L4 金融级 ORM（Data Mapper + Repository 模式），编译时 SQL 校验（`sql_string!` / `query!` 宏）。
- **可观测性（v0.2.0 新增）**：`sz-rust-observability` 包提供 `MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）。
- **分布式追踪（v0.2.0 新增）**：`sz-rust-tracing` 包实现 W3C TraceContext 标准（`traceparent: 00-<trace_id>-<span_id>-<flags>`），legacy header 兼容，OTLP exporter 占位。

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
    ├── sz-rust-addons-loader/    # 插件加载器
    ├── sz-rust-addons-operate/   # 插件操作库
    ├── sz-rust-pdf/              # PDF/Excel 导入导出
    ├── sz-rust-observability/    # 可观测性模块（MetricsRegistry + SLO 燃烧率，v0.2.0）
    ├── sz-rust-tracing/          # 分布式追踪模块（W3C TraceContext + OTLP，v0.2.0）
    └── sz-rust-sz300/            # SZ300 业务应用（端到端集成示例）
```

---

## 文档索引

详细文档位于 `docs/` 目录：

- [ADR 索引](docs/adr/README.md) — 12 个架构决策记录（ADR-001 ~ ADR-012），全部已接受
- [ADR-011 可观测性模块](docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) — MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（v0.2.0）
- [ADR-012 分布式追踪](docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) — W3C TraceContext 标准、OTLP exporter 路径（v0.2.0）
- [PHP 迁移指南](docs/php-migration-guide.md) — PHP → Rust 概念映射与行为对齐（R5 硬约束），含 15 章节
- [工程化实践规范](docs/sz-rust-engineering-practices.md) — 10 道门禁、CI/CD、代码风格
- [软件项目审计清单](docs/软件项目审计清单.md) — P0/P1/P2 审计维度
- [ADR 与生产 Bug 定位规范](docs/ADR与生产Bug定位规范.md) — 可复用规范（v1.0）
- [初始审计报告](docs/audit/2026-07-22-初始审计.md) — P0 全通过 / P1 需改进 2 项
- [性能基线 v0.1.0](docs/benchmarks/baseline-v0.1.0.md) — criterion 基线数据，回归参照

> 注：模块级文档注释（`cargo doc -p sz-rust-core --open`）包含完整的 PHP 源码行号对照与 bug 复刻说明。CI doc job 启用 `-D missing_docs` 严格检查，所有公开 API 必须有文档注释。

---

## CI 门禁与质量保障（v0.2.0 增强）

项目通过 GitHub Actions 实施 10+ 道门禁，所有门禁严格生效（无 `continue-on-error`）：

| Workflow | 触发条件 | 职责 |
|----------|---------|------|
| `ci.yml` | push/PR | fmt / check / clippy / test / doc(missing_docs) / audit / feature-matrix / unused-deps / **deny(cargo-deny)** |
| `soak.yml` | 每周日 00:00 UTC + workflow_dispatch | 6 小时 soak test，60 秒指标采样，420 分钟超时 |
| `coverage.yml` | push/PR | cargo-tarpaulin 覆盖率统计 + Codecov 上传 |
| `benchmark.yml` | push main / PR | criterion 性能基准测试 + gh-pages-bench 分支保存 |
| `fuzz.yml` | push/PR + 每周六 00:00 UTC + workflow_dispatch | 10 用例 × 1000 迭代模糊测试，支持 `FUZZ_ITERATIONS` 自定义 |

**cargo-deny 审计维度**（`deny.toml`）：
- 许可证白名单：MIT / Apache-2.0 / BSD / ISC / Zlib
- 许可证黑名单：GPL / AGPL / EUPL
- RUSTSEC 安全漏洞检查
- 重复依赖警告 + 通配符禁止
- 来源限制：仅允许 crates.io

---

## 许可证

MIT
