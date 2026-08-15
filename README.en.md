# SZ-Rust — A Rust Web Framework Aligned with ThinkPHP 8

> [中文](README.md) | **English**

A Rust Web framework built on axum 0.8 + SZ-ORM, with API design aligned to ThinkPHP 8 for easy migration by PHP engineers.

**Current Version: v0.7.0** (2026-08-09) — P0-P4 all completed: production validation + Redis store backends + pentest + performance benchmarks + addon templates + doc i18n

> **v0.6.6 → v0.7.0 changelog**: see [docs/CHANGELOG.md](docs/CHANGELOG.md)

---

## Core Features

All features below are from actual `sz-rust-core` source code. Module structure: `packages/sz-rust-core/src/lib.rs`.

- **HTTP Server + Routing**: Built on axum 0.8 + tower 0.5 + hyper 1.x, supporting three-layer routing (attribute macro / config-based / convention-based).
- **Controller Layer**: `SzController` → `BaseController` → `AddonsBaseController` three-layer trait inheritance chain, aligned with PHP `app\SzController` / `app\BaseController` / `addons\BaseController`. Provides `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` methods.
- **Model Layer**: `BaseModel` trait composing SZ-ORM's `Model` + `ModelExt` + `RelationLoader`, aligned with `think\Model`. Supports `$append` virtual fields, accessors (`Accessor`), mutators (`Mutator`), dynamic append (`Appendable`).
- **Middleware**: Built-in CORS / Auth(JWT) / Log / RateLimit / Trace, plus chain builder (`MiddlewareChain`), Handler=Middleware bidirectional converter, tower-http compatibility layer. (✅ Production: sz300 mounts CORS + CSRF + rate-limit + circuit-breaker + custom auth_middleware (router.rs:148-167); facade Auth/Log/Trace not mounted, auth uses custom version due to signature incompatibility)
- **Rate Limiter (v0.3.1 landed)**: `sz-rust-middleware-facade::rate_limit` provides Token Bucket (`TokenBucket`) + Sliding Window (`SlidingWindow`) algorithms, implementing `sz_rust_orm_facade::RateLimiter` trait, with OOM protection and 100-concurrent zero-error test. (✅ Production mounted: sz300 `router.rs:158-160`, default-enabled, config via `RateLimitProductionConfig::from_env()`)
- **Circuit Breaker (v0.3.1 landed)**: `sz-rust-middleware-facade::circuit_breaker` provides Closed/Open/HalfOpen three-state state machine + `circuit_breaker_middleware` (Open returns 503), parking_lot::Mutex for concurrency safety. (✅ Production mounted: sz300 `router.rs:153-155`, default-enabled, config via `CircuitBreakerProductionConfig::from_env()`)
- **Validator**: Aligned with `think\Validate`, 30+ built-in rules (require / integer / float / email / url / ip / regex / length / max / min / between / in / notIn / confirm / different / date / after / before / requireIf / requireWith, etc.), supporting batch validation, scenarios, custom messages. (✅ Production: `merchant.rs:103` calls `Validate::new()`)
- **Cache System**: Aligned with `think\facade\Cache`, reusing sz-orm-storage drivers. (⚠️ Production not mounted: sz300 doesn't call core::cache, only Cargo.toml has cache-facade dep)
- **Event System**: Aligned with `think\Event`, supporting Listener / Subscriber / Observer patterns. (⚠️ Production not mounted: zero calls in sz300, only CLI stubs template)
- **Model Hooks**: `HookDispatcher` with 16 events (PHP native 12 + sz-orm-core extension 4: BeforeSave / AfterSave / BeforeValidate / AfterValidate). (✅ Production: `order.rs:26` imports HookContext/HookEvent, `main.rs:174` inits HookRegistry)
- **File Upload + Image Processing**: Aligned with `think\File` + `think\file\UploadedFile`, 5 storage engines (Local / Aliyun OSS / Tencent COS / Qiniu Kodo / AWS S3 compatible); image processing aligned with PHP Grafika (resize / crop / watermark / text). (⚠️ Production: sz300 uses custom `FileService` (`file_service.rs`, tokio::fs direct write), core 5 storage engines zero production calls)
- **Multi-App Dispatch**: Aligned with ThinkPHP `auto_multi_app`, dispatching to sub-apps by URI prefix. (⚠️ Production not mounted: sz300 single-app deployment, multi_app zero calls)
- **Guard Authentication & Authorization**: Self-developed Guard pattern (combining NestJS Guard + Spring Security concepts). (⚠️ Production not mounted: sz300 uses custom `auth_middleware` + `role_guard`, not core::guard)
- **View Template**: Aligned with PHP template engine, supporting layout and template rendering. (✅ Production: `view.rs:18` calls `View::with_default_engine()`)
- **HTTP/2 + TLS**: Based on rustls + tokio-rustls, aligned with think-swoole SSL. (⚠️ Production not mounted: sz300 uses bare `axum::serve`, no TLS)
- **CLI Tool**: `sz-rust-cli` provides make / migrate / route / cache / scheduler commands.
- **Plugin System**: `sz-rust-addons-loader` implements `addons/` plugin loading and route mounting. (⚠️ Production: core compile-time dep + re-export; runtime gated by hot-reload feature, sz300 default `default = []` off, `main.rs:104-106` logs only)
- **Based on SZ-ORM**: L4 financial-grade ORM (Data Mapper + Repository pattern), compile-time SQL validation (`sql_string!` / `query!` macros).
- **Observability (v0.2.0)**: `sz-rust-observability` package provides `MetricsRegistry` + Counter/Gauge/Histogram metric types, SLO multi-window burn-rate alerting (1h/5m + 6h/30m dual-window pairs, aligned with Google SRE Workbook Chapter 5). (✅ Production: `main.rs:125` MetricsRegistry + `main.rs:168` SLO monitor, `health.rs:37/39` records burn rate)
- **Metrics Endpoint Access Control (T7)**: `MetricsAuthConfig` provides Bearer token + IP allowlist (CIDR, v4/v6) dual mechanisms; the `/metrics` route mounts `metrics_auth_middleware` as an isolated sub-router (`router.rs:54` metrics_router(), no pollution to business APIs), returning 403 when unauthorized; with `SZ300_ENV=production` startup fails unless auth is configured (`main.rs:243`); real client IP injected via `into_make_service_with_connect_info` (`main.rs:262`). (✅ Production: wired)
- **Distributed Tracing**: sz300 uses native `tracing` + `sz_rust_observability::otlp` (OTLP gRPC exporter, `main.rs:169`). (✅ Production: wired, `otlp` feature gated)

---

## Quick Start

Minimal Hello World example (full code: `packages/sz-rust-examples/src/bin/quick_start.rs`):

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

    // Load config (fall back to default on failure)
    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    let config = AppConfig::load_from_dir(&config_dir).unwrap_or_else(|_| AppConfig::default());

    // Initialize App container
    let app = App::init(config);
    let log_facade = LogFacade::init(&app.config().log);
    log_facade.info("SZ-Rust Hello World endpoint starting...");

    // Build router (GET / returns {"code":1,"msg":"hello","data":{}})
    let router = build_router();

    // Start HTTP service
    let addr = "127.0.0.1:9527";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
```

Run:

```bash
cargo run -p sz-rust-examples --bin quick_start
```

Visit http://127.0.0.1:9527/ returns:

```json
{"code":1,"msg":"hello","data":{}}
```

Full CRUD example: `packages/sz-rust-examples/src/bin/crud_demo.rs`:

```bash
cargo run -p sz-rust-examples --bin crud_demo
```

---

## ThinkPHP 8 Alignment Table

| ThinkPHP 8 Feature | SZ-Rust Implementation | Notes |
|---------------------|------------------------|-------|
| `app\SzController` (abstract) | `sz_rust_core::controller::SzController` trait | `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` |
| `app\BaseController` | `sz_rust_core::controller::BaseController` trait | `batchValidate` / `$middleware` / `initialize()` / `validate()` |
| `addons\BaseController` | `sz_rust_core::controller::AddonsBaseController` trait | `allowAllAction` / `getRouteinfo()` / `getToken()` / `checkLogin()` |
| `think\Model` (Active Record) | `sz_rust_core::model::BaseModel` trait + SZ-ORM Repository | Data Mapper pattern; `$name`/`$pk`/`$append`/`$hidden`/`$fillable` all aligned |
| `getXxxAttr` / `setXxxAttr` | `Accessor` / `Mutator` trait | Accessor caching, mutator merged_data, PHP bug strict replication |
| `think\Validate` | `sz_rust_core::validate::Validate` | 30+ rules, scenarios, batch validation, custom messages |
| `think\facade\Cache` | `sz_rust_core::cache` | Reuses sz-orm-storage drivers |
| `think\Event` | `sz_rust_core::event` | Listener / Subscriber / Observer |
| think-orm Model hooks | `sz_rust_core::hooks::HookDispatcher` | 16 events (PHP 12 + extension 4) |
| `think\File` / `UploadedFile` | `sz_rust_core::upload::File` / `UploadedFile` | hash / move / hashName / isValid |
| storage engine (Local/Aliyun/Qcloud/Qiniu) | `sz_rust_core::upload::storage` | 5 engines (+ S3 compatible) |
| `app/middleware.php` | `sz_rust_core::middleware` | CORS / Auth / Log / RateLimit / Trace + chain builder |
| `auto_multi_app` | `sz_rust_core::multi_app` | Dispatches sub-apps by URI prefix |
| `think-swoole` SSL | `sz_rust_core::h2` | HTTP/2 + TLS (rustls) |
| `think-logger` | `sz_rust_core::log::LogFacade` | tracing integration |
| `compact()` | `sz-rust-macros` | Proc macros (standalone crate, not `sz_rust_core::macros` placeholder module) |
| `config/app.php` / `database.php` | `sz_rust_core::config::AppConfig` | YAML config loading |
| `app()` container | `sz_rust_core::container::App` | Application container |
| `BaseException` | `sz_rust_core::error::ErrorCode` | Standard error codes |
| `addons/` plugins | `sz-rust-addons-loader` | Plugin loading + route mounting |
| think-swoole / think-worker | `sz_rust_core::server` | tokio multi-threaded runtime |
| Template engine | `sz_rust_core::view` | layout + template |
| — (self-developed) | `sz_rust_core::guard` | Guard authentication & authorization |

---

## Project Structure

```
sz-rust/                          # workspace root
├── Cargo.toml                    # workspace config (axum 0.8 / SZ-ORM全家桶)
├── deny.toml                     # cargo-deny config (license/RUSTSEC/duplicate/source audit)
├── config/                       # default config (app/database/cache/log/addons YAML)
└── packages/
    ├── sz-rust-core/             # core framework (controller/model/middleware/validate/...)
    ├── sz-rust-macros/           # proc macros (compact, etc.)
    ├── sz-rust-examples/         # examples (quick_start / crud_demo)
    ├── sz-rust-cli/              # CLI tool (make/migrate/route/cache/scheduler)
    ├── sz-rust-http-facade/      # HTTP foundation (response/error/request)
    ├── sz-rust-orm-facade/       # ORM unified entry
    ├── sz-rust-cache-facade/     # cache abstraction (Memory/Redis/Memcached/MultiLevel)
    ├── sz-rust-state-facade/     # app state (session/cookie/env/event/i18n/mail/notify)
    ├── sz-rust-infra-facade/     # infrastructure (config/validate/static_files/upload/debug_page)
    ├── sz-rust-auth-facade/      # auth (wechat/oauth/gateway/sso/redis_store)
    ├── sz-rust-pay-facade/       # payment aggregation (Alipay/WeChat Pay)
    ├── sz-rust-orm-ext-facade/   # ORM extensions
    ├── sz-rust-router-facade/    # router facade
    ├── sz-rust-middleware-facade/ # middleware facade (rate_limit/circuit_breaker/csrf)
    ├── sz-rust-mvc-facade/       # MVC facade
    ├── sz-rust-mcp/              # MCP protocol (stdio JSON-RPC)
    ├── sz-rust-addons-loader/    # plugin loader
    ├── sz-rust-addons-ecommerce/ # e-commerce plugin
    ├── sz-rust-addons-cms/       # CMS plugin (articles/categories/tags)
    ├── sz-rust-addons-crm/       # CRM plugin (contacts/leads/deals)
    ├── sz-rust-observability/    # observability (MetricsRegistry + SLO burn rate)

    └── sz-rust-sz300/            # SZ300 business app (end-to-end integration example)
```

---

## Documentation Index

Detailed docs in `docs/` directory:

- [ADR Index](docs/adr/README.en.md) — 20 Architecture Decision Records (ADR-001 ~ ADR-020), all accepted
- [ADR-011 Observability](docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) — MetricsRegistry design, SLO multi-window burn rate, four-layer observability model (v0.2.0)
- [ADR-012 Distributed Tracing](docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) — W3C TraceContext standard, OTLP exporter path (v0.2.0)
- [PHP Migration Guide](docs/php-migration-guide.md) — PHP → Rust concept mapping and behavior alignment (R5 hard constraint), 15 chapters
- [Engineering Practices](docs/sz-rust-engineering-practices.md) — 10 gates, CI/CD, code style
- [Project Assessment & Framework Comparison](docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md) — Comprehensive assessment (91/100, production-ready Beta+) + 5 frameworks × 5 dimensions comparison
- [Audit Reports Index](docs/audit/README.md) — Unified index for all audit/assessment reports

> Note: Module-level doc comments (`cargo doc -p sz-rust-core --open`) contain full PHP source line references and bug replication details. CI doc job enables `-D missing_docs` strict check; all public APIs must have doc comments.

---

## CI Gates & Quality Assurance (v0.7.0 enhanced)

Project implements 23 gates via GitHub Actions, all strictly enforced (no `continue-on-error`):

| Workflow | Trigger | Responsibility |
|----------|---------|----------------|
| `ci.yml` | push/PR | 23 jobs: fmt / check / clippy / test / doc(missing_docs) / audit / deny(cargo-deny) / no-placeholder / feature-matrix / unused-deps / outdated / machete / adr-coverage / db-integration / coverage(≥85%) / compile-time / miri / windows / ai-facade / **doc-code-consistency (gate 19, no phantom delivery)** / **adr-code (gate 20, ADR code reference check)** / **assertion-value (gate 21, no empty tests)** / **feature-consistency (gate 22, feature declaration check)** |
| `coverage.yml` | push/PR | cargo-tarpaulin coverage + Codecov upload |
| `benchmark.yml` | push main / PR | criterion benchmarks + **9-category baseline gate** + gh-pages-bench branch |
| `security.yml` | push/PR + Every Sunday 00:00 UTC | cargo-audit + **unmaintained compile check (paste/rustls-pemfile/rkyv)** + cargo-geiger |
| `fuzz.yml` | push/PR + Every Saturday 00:00 UTC + workflow_dispatch | 10 cases × 1000 iterations fuzzing, supports `FUZZ_ITERATIONS` |
| `mcdc.yml` / `mutants.yml` | push/PR | Branch coverage gate / mutation testing |
| `publish-oss.yml` / `release.yml` | Release flow | crates.io publishing / release build |
| `marketplace-ci.yml` | Marketplace path changes | ⚠️ references `sz-rust-marketplace` (verified fictional delivery on 2026-08-14, see audit report; workflow never triggers) |

> Note: `soak.yml` / `soak-nightly.yml` are disabled (renamed to `.disabled`), 6h soak is handled by the self-hosted toolkit `scripts/soak-self-hosted/` (see `docs/soak-toolkit-guide.md`).

**cargo-deny audit dimensions** (`deny.toml`):
- License whitelist: MIT / Apache-2.0 / BSD / ISC / Zlib
- License blacklist: GPL / AGPL / EUPL
- RUSTSEC security vulnerability check
- Duplicate dependency warning + wildcard prohibition
- Source restriction: crates.io only

---

## License

MIT