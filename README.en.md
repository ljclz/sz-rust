# SZ-Rust — A Rust Web Framework Aligned with ThinkPHP 8

> [中文](README.md) | **English**

A Rust Web framework built on axum 0.8 + SZ-ORM, with API design aligned to ThinkPHP 8 for easy migration by PHP engineers.

**Current version v2.0.0 (workspace)**; since 2026-10-25 the former enterprise repo has been merged into this repository, all crates unified under Apache-2.0. See [docs/single-edition-merge-plan.md](docs/single-edition-merge-plan.md) and ADR-043 (`docs/adr/`).

> **v2.0.0 Single Edition merge summary**: the former enterprise capabilities have been merged into this workspace (now 65 members). See [docs/single-edition-merge-plan.md](docs/single-edition-merge-plan.md) and [CHANGELOG.md](CHANGELOG.md).

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
    let config = AppConfig::load_from_dir(&config_dir).await.unwrap_or_else(|_| AppConfig::default());

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

> More examples: see [docs/examples.md](docs/examples.md).

---

## Core Features

All features below are from actual `sz-rust-core` source code. Module structure: `packages/sz-rust-core/src/lib.rs`.

> ✅ **Single Edition note (v2.0.0 merge, 2026-10-25)**: The former enterprise repo has been merged into this repository. All "✅ Production:" annotations below point to **actual code in this repo's** `packages/sz-rust-sz300` (paths directly verifiable; no external enterprise-repo paths are cited anymore). Capabilities that cannot be mapped to the in-repo sz300 are marked "framework capability" instead of claiming production wiring.

- **HTTP Server + Routing**: Built on axum 0.8 + tower 0.5 + hyper 1.x, supporting three-layer routing (attribute macro / config-based / convention-based).
- **Controller Layer**: `SzController` → `BaseController` → `AddonsBaseController` three-layer trait inheritance chain, aligned with PHP `app\SzController` / `app\BaseController` / `addons\BaseController`. Provides `renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` methods.
- **Model Layer**: `BaseModel` trait composing SZ-ORM's `Model` + `ModelExt` + `RelationLoader`, aligned with `think\Model`. Supports `$append` virtual fields, accessors (`Accessor`), mutators (`Mutator`), dynamic append (`Appendable`).
- **Middleware**: Built-in CORS / Auth(JWT) / Log / RateLimit / Trace, plus chain builder (`MiddlewareChain`), Handler=Middleware bidirectional converter, tower-http compatibility layer. (✅ Production wired: this repo's sz300 `router.rs` mounts CORS + CSRF + rate limiting (token bucket) + circuit breaker + custom auth_middleware; the production chain matches the former enterprise version)
- **Rate Limiter (v0.3.1 landed)**: `sz-rust-middleware-facade::rate_limit` provides Token Bucket (`TokenBucket`) + Sliding Window (`SlidingWindow`) algorithms, implementing `sz_rust_orm_facade::RateLimiter` trait, with OOM protection and 100-concurrent zero-error test. (✅ Production wired: sz300 `router.rs` mounts the token-bucket middleware, enabled by default; config read via `RateLimitProductionConfig::from_env()` in `config.rs`)
- **Circuit Breaker (v0.3.1 landed)**: `sz-rust-middleware-facade::circuit_breaker` provides Closed/Open/HalfOpen three-state state machine + `circuit_breaker_middleware` (Open returns 503), parking_lot::Mutex for concurrency safety. (✅ Production wired: sz300 `router.rs` mounts the circuit-breaker middleware, enabled by default; config read via `CircuitBreakerProductionConfig::from_env()` in `config.rs`)
- **Validator**: Aligned with `think\Validate`, 30+ built-in rules (require / integer / float / email / url / ip / regex / length / max / min / between / in / notIn / confirm / different / date / after / before / requireIf / requireWith, etc.), supporting batch validation, scenarios, custom messages. (✅ Production wiring sample: the sz300 merchant creation endpoint `controllers/merchant.rs:127` uses `Validate` for required/max-length checks (source: 2026-10-28 UXR P1-5); framework capability covered by sz-rust-facade-tests integration tests)
- **Cache System**: Aligned with `think\facade\Cache`, reusing sz-orm-storage drivers; `sz_rust_core::cache` aggregates Memory / Redis / Memcached / multi-level drivers (from `sz-rust-cache-facade`). (Framework capability, all 4 drivers unit-tested; not yet called by sz300 business code)
- **Event System**: Aligned with `think\Event`, supporting Listener / Subscriber / Observer patterns; `sz_rust_core::event` (from `sz-rust-state-facade`) + `Event` static facade, `dispatch_facade` supports Sync / Async dispatch. (⚠️ core::event not wired in production; the cross-plugin event bus `core::plugin::InMemoryEventBus` is available, and the 7 industry plugins use it for data interoperability)
- **Model Hooks**: `HookDispatcher` with 16 events (PHP native 12 + sz-orm-core extension 4: BeforeSave / AfterSave / BeforeValidate / AfterValidate). (Framework capability, covered by sz-rust-orm-ext-facade tests)
- **File Upload + Image Processing**: Aligned with `think\File` + `think\file\UploadedFile`, 5 storage engines (Local / Aliyun OSS / Tencent COS / Qiniu Kodo / AWS S3 compatible); image processing aligned with PHP Grafika (resize / crop / watermark / text). (ℹ️ Intentional: sz300 uses custom `FileService` for local storage; core 5 engines are framework capability, activate when cloud storage needed)
- **QR Code Generation + Verification Closed Loop (v1.2.2+ extension)**: Aligned with PHP `endroid/qr-code`, `sz_rust_core::qr_code` provides PNG / SVG / Unicode / matrix output; new `generate_and_verify` (generates then decodes in-memory, returning `DataMismatch` on inconsistency) and `verify_png_bytes`; decode backend is `rqrr` (MSRV 1.64, in-memory only, no `std::fs`). Suited for CI scannability checks of traceability / payment / ticket codes. (see ADR-042)
- **Multi-App Dispatch**: Aligned with ThinkPHP `auto_multi_app`, dispatching to sub-apps by URI prefix. (ℹ️ Intentional: sz300 is single-app deployment; activate when multi-app needed)
- **Guard Authentication & Authorization**: Self-developed Guard pattern (combining NestJS Guard + Spring Security concepts). (ℹ️ Intentional: sz300 uses custom `auth_middleware`; core::guard is framework capability, activate for plugin/complex app reuse)
- **View Template**: Aligned with PHP template engine, supporting layout and template rendering. (✅ Production wired: sz300 `controllers/view.rs` provides the `/page/{template}` endpoint, calling `View::with_default_engine()`)
- **HTTP/2 + TLS**: Based on rustls + tokio-rustls, aligned with think-swoole SSL. (ℹ️ Intentional: sz300 uses bare `axum::serve`, TLS terminated by nginx/k8s ingress; activate when process-level TLS needed)
- **CLI Tool**: `sz-rust-cli` provides make / migrate / route / cache / scheduler commands.
- **Plugin System**: `sz-rust-addons-loader` implements `addons/` plugin loading and route mounting. (ℹ️ Intentional: core compile-time dep + re-export; hot-reload feature-gated, sz300 default off; activate for dev hot-reload, production should load at startup)
- **Based on SZ-ORM**: L4 financial-grade ORM (Data Mapper + Repository pattern), compile-time SQL validation (`sql_string!` / `query!` macros).
- **Observability (v0.2.0)**: `sz-rust-observability` package provides `MetricsRegistry` + Counter/Gauge/Histogram metric types, SLO multi-window burn-rate alerting (1h/5m + 6h/30m dual-window pairs, aligned with Google SRE Workbook Chapter 5). (✅ MetricsRegistry wired: sz300 `main.rs` init + shared via `state.rs`; SLO burn rate is framework capability)
- **Metrics Endpoint Access Control (T7)**: Once `SZ300_METRICS_TOKEN` is set, `/metrics` requires `Authorization: Bearer <token>` and returns 401 when unauthorized; when unset the endpoint stays public with a startup WARN (`main.rs`, 2026-10-25 architecture-review fix). The former enterprise version also had a `MetricsAuthConfig` implementation (Bearer + IP allowlist CIDR dual mechanism), preserved in git history for reference. (✅ Production wired: sz300 `main.rs`)
- **Distributed Tracing**: `sz_rust_observability::otlp` (OTLP gRPC/HTTP exporter) + `sz-rust-tracing` (W3C TraceContext propagation). (Framework capability, `otlp` feature gated)
- **GraphQL API**: Based on async-graphql 7.x, provides `POST /graphql` query endpoint (v1.9.0 supports DB-backed persistent schemas). (✅ Mounted: sz300 `router.rs`, gated by the `v18-graphql` feature)
- **WebSocket**: Based on sz-orm-websocket (tokio-tungstenite), supports HTTP port multiplexing + standalone port modes, `WsHandler` trait aligned with PHP Workerman onConnect/onMessage/onClose. (✅ Mounted: sz300 `router.rs` `/ws` endpoint, gated by the `v18-websocket` feature)
- **WASM Edge Compute**: `sz-rust-wasm` crate based on wasmi (pure Rust WASM interpreter), provides `WasmRuntime` for loading/executing WASM modules, with fuel metering (prevents infinite loops) and a 256 MiB per-instance memory cap. (✅ Production wired: sz300 `POST /api/wasm/execute` endpoint, **JWT protected**; 2026-10-27 P0-1 security fix: the former public whitelist + unbounded resource usage was a remote DoS vector, so the endpoint was removed from the whitelist and hardened with fuel/memory/timeout triple limits — see `controllers/wasm_api.rs` + `router.rs`)
- **AI Capability Stack Production Wiring (v2.0.0 merged from enterprise)**: sz300 provides `/api/v1/ai/chat` (prompt length cap + model whitelist, security fix M-1), `/api/v1/ai/embed`, `/api/v1/ai/stream` (SSE) — three endpoints (`controllers/ai.rs`, JWT protected); `main.rs` drives `Ai::init_default` (OpenAI-compatible Provider) via `SZ300_AI_API_KEY` (optional `SZ300_AI_BASE_URL`); when unconfigured, the endpoints return an explicit error message. RAG retrieval augmentation is attempted automatically in the chat chain (`sz-rust-rag` IndustryRag, failure is non-blocking). (✅ Production wired: this repo's sz300)
- **K8s Operator**: (⚠️ Removed: `sz-rust-k8s-operator` was an orphan crate with no consumers; the 22 tests remain in git history, see ADR-038)
- **Admin Backend Plugin (v1.2.0)**: `sz-rust-addons-admin` provides 7 modules (users/roles/permissions/menus/config/operation logs/dashboard) with 21 REST API endpoints + 17 `admin.*` Capabilities. `AdminAddonPlugin` merges routes via `plugin.router()` and registers capabilities via `plugin.capability_hook()` for production wiring. It ships `permission_guard_middleware` (route→permission mapping, super-admin bypass), bcrypt password masking, tenant-inherited config (tenant_id=0 global + tenant override), and async operation-log writes. (✅ Production wired: sz-rust-examples demonstrates wiring + 9 E2E verification tests)

> ⚠️ **Headless notice (2026-10-27 UXR P0-1)**: `sz-rust-addons-admin` / `sz-rust-admin-ui` / `sz-rust-monitor-panel` / `sz-rust-codegen-ui` are **backend-only libraries with no built-in browser UI** — what is delivered is REST APIs and Capabilities, **not a clickable web admin console**. For a graphical backend, integrate `sz-rust-visual` (Tauri 2 + Vue3 desktop workbench, via IPC) or build your own frontend.

> **v1.9.0 Production Deepening + Closed-Loop Validation + Debt Clearance (2026-10-07)**: 10 core capabilities (GraphQL persistence / JWT Audience / Saga distributed transactions / observability closed loop / performance deepening / AI smart classification / internationalization / dynamic config center / plugin ecosystem / production config) are all gated by the `all-v1-9` feature flag, disabled by default. Verification: `cargo test --lib --features all-v1-9` = 106 passed. See [CHANGELOG v1.9.0](CHANGELOG.md).

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

> v2.0.0 Single Edition: all 65 crates live in this single workspace (full members list in the root `Cargo.toml`); the following is a layered overview.

```
sz-rust/                          # workspace root (single source of truth)
├── Cargo.toml                    # workspace config (axum 0.8 / SZ-ORM family / 65 members)
├── deny.toml                     # cargo-deny config (license/RUSTSEC/duplicate/source audit)
├── config/                       # default config (app/database/cache/log/addons YAML)
├── docker-compose.prod.yml       # production deployment template (security features + tokens)
├── scripts/audit/                # gate scripts (security Gauntlet / license compliance / doc-code consistency...)
├── schemas/openapi-sz300.json    # OpenAPI 3.0.3 contract (41 paths, consistent with runtime /api-docs/openapi.json)
└── packages/
    ├── sz-rust-core/             # core framework (controller/model/middleware/validate/...)
    ├── sz-rust-macros/           # proc macros (compact, etc.)
    ├── sz-rust-examples/         # examples (quick_start / crud_demo)
    ├── sz-rust-cli/              # CLI tool (make/migrate/route/cache/scheduler/plugin/admin)
    ├── sz-rust-{http,orm,cache,state,infra,auth,pay,orm-ext,router,middleware,mvc}-facade/
    │                             # 11 unified facades
    ├── sz-rust-ai-facade/        # AI facade (LLM/Embedding/RAG/Agent/MCP Bridge)
    ├── sz-rust-rag/              # RAG retrieval pipeline
    ├── sz-rust-{sdd,migration}/  # SDD Agent orchestration + TP6 AI migration tool (v2.0.0 merge)
    ├── sz-rust-marketplace/      # plugin marketplace (Ed25519 signature + web platform + CLI)
    ├── sz-rust-addons-loader/    # plugin loader
    ├── sz-rust-addons-{admin,cms,crm,ecommerce,erp,forum,im,operate}/
    │                             # 8 plugins (all open-sourced after v2.0.0 merge)
    ├── sz-rust-{observability,tracing,pdf,workflow,wasm,websocket,i18n,...}/
    │                             # observability / protocol / utility crates
    ├── sz-rust-{admin-ui,monitor-panel,codegen-ui}/
    │                             # headless backend libraries (server-side UI data assembly/API orchestration, no browser assets;
    │                             #   the real frontend is sz-rust-visual: Tauri 2 + Vue3, via IPC)
    └── sz-rust-sz300/            # SZ300 business app (production reference implementation: AI/WASM/GraphQL/WS/rate-limit/circuit-breaker)
```

---

## Documentation Index

Detailed docs in `docs/` directory:

- [ADR Index](docs/adr/README.en.md) — 49 Architecture Decision Records (counted from `docs/adr/*.md` on 2026-10-29)
- [ADR-011 Observability](docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) — MetricsRegistry design, SLO multi-window burn rate, four-layer observability model (v0.2.0)
- [ADR-012 Distributed Tracing](docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) — W3C TraceContext standard, OTLP exporter path (v0.2.0)
- [ThinkPHP API Mapping](docs/thinkphp-api-mapping.md) — SZ-Rust facade vs ThinkPHP API reference
- [Engineering Practices](docs/sz-rust-engineering-practices.md) — 10 gates, CI/CD, code style
- [Project Assessment & Framework Comparison](docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md) — Comprehensive assessment (91/100, production-ready Beta+) + 5 frameworks × 5 dimensions comparison
- [Audit Reports Index](docs/audit/README.md) — Unified index for all audit/assessment reports

> Note: Module-level doc comments (`cargo doc -p sz-rust-core --open`) contain full PHP source line references and bug replication details. CI doc job enables `-D missing_docs` strict check; all public APIs must have doc comments.

---

## CI Gates & Quality Assurance

Project implements its gate system via GitHub Actions (`ci.yml` actually has 37 jobs, re-verified on 2026-10-28 by parsing the direct children of `jobs:` — the old README claimed 23 jobs with "no continue-on-error", a stale description; `machete` runs with `continue-on-error: true` and is non-blocking; `coverage` and `miri` are blocking gates — `miri` fails closed on UB/memory issues):

| Workflow | Trigger | Responsibility |
|----------|---------|----------------|
| `ci.yml` | push/PR | 37 jobs: fmt / check / clippy / test / doc(missing_docs) / audit / deny(cargo-deny) / no-placeholder / feature-matrix / unused-deps / outdated / machete* / adr-coverage / coverage / compile-time / miri / windows / ai-facade / **doc-code-consistency (gate 19, no phantom delivery)** / **adr-code (gate 20, ADR code reference check)** / **assertion-value (gate 21, no empty tests)** / **feature-consistency (gate 22, feature declaration check)** / license-compliance (Apache-2.0 header + field) / no-crate-dead-code / i18n-completeness / boundary-check (* marks `continue-on-error` non-blocking; `coverage` and `miri` are blocking gates; the former isolation job was removed along with the ADR-043 Single Edition merge) |
| `coverage.yml` | push/PR | cargo-tarpaulin coverage + Codecov upload |
| `benchmark.yml` | push main / PR | criterion benchmarks + **9-category baseline gate** + gh-pages-bench branch |
| `security.yml` | push/PR + Every Sunday 00:00 UTC | cargo-audit (**blocking**; ignore list and reasons in `.cargo/audit.toml`) + **unmaintained compile check (paste/rustls-pemfile verified as real deps)** + cargo-geiger (non-blocking summary) |
| `fuzz.yml` | push/PR + Every Saturday 00:00 UTC + workflow_dispatch | 10 cases × 1000 iterations fuzzing, supports `FUZZ_ITERATIONS` |
| `mcdc.yml` / `mutants.yml` | push/PR | Branch coverage gate / mutation testing |
| `perf-gate.yml` | PR (baseline shape only) / Every Sunday 04:00 UTC / workflow_dispatch | **End-to-end HTTP performance gate** (`scripts/perf-regression.js` v2): MySQL 9.6 + migrations 001–007 + deterministic 20k-row dataset + release build + real load (10 s warmup / 60 s steady × 3). A missing fingerprint baseline **fails** the gate and seeds the artifact — never "no baseline ⇒ pass". Methodology and thresholds: ADR-046 |
| `publish-oss.yml` / `release.yml` | Release flow | crates.io publishing / release build |
| `marketplace-ci.yml` | Marketplace path changes | ✅ references `sz-rust-marketplace` (concretely delivered: 130 tests + CLI wiring; the former "fictional delivery" characterization is outdated) |

> Note: `soak.yml` / `soak-nightly.yml` are disabled (renamed to `.disabled`), 6h soak is handled by the self-hosted toolkit `scripts/soak-self-hosted/` (see `docs/soak-toolkit-guide.md`).

**cargo-deny audit dimensions** (`deny.toml`):
- License whitelist: MIT / Apache-2.0 / BSD / ISC / Zlib
- License blacklist: GPL / AGPL / EUPL
- RUSTSEC security vulnerability check
- Duplicate dependency warning + wildcard prohibition
- Source restriction: crates.io only

---

## License

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the LICENSE-APACHE file for the full license text.

SPDX-License-Identifier: Apache-2.0