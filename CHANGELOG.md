# 变更日志

本项目所有重要变更均会记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.3.2] - 2026-08-05

### 修复
- **§5.3 SQL 注入根治**：
  - `cli/src/cmd/migrate.rs:444` — `delete_migration_record` 从 `format!` 拼接改为 `execute_with_params` 参数化绑定
  - `orm-ext-facade/src/hooks.rs:777` — `soft_delete_update_sql` / `soft_delete_restore_sql` 加 `debug_assert!(is_valid_identifier(...))` 校验
- **§5.2 路由纳秒级优化**：
  - `router-facade/src/router.rs:35` — `APP_MAP` 从 `LazyLock<HashSet>` 改为 `const APP_LIST: &[&str]` 线性查找
  - `router-facade/src/router.rs:130` — `parse_path` 从 `Vec::collect` 改为迭代器直接消费
  - 新增 3 个性能测试（release-only，p99 < 300/500/800ns）
- **clippy 修复**：`middleware-facade/src/rate_limit.rs:501` — `or_insert_with(Vec::new)` → `or_default()`

### 变更
- **§5.1 addons 文档更新**：crm/erp/ecommerce 三包 lib.rs doc-comment 从"0 测试脚手架"改为"已填实（v0.3.2）：N 测试"
- **§5.7 评估报告重写**：综合评分 55→78，7 维度评分更新，12 处过时表述修正
- `Cargo.toml` workspace version 0.3.1 → 0.3.2

### 已知问题
- **crates.io 0.3.2 发布阻塞**：sz-orm-* 包在 crates.io 上版本不一致（core 1.5.0 / auth 1.2.2 / graphql 1.2.1），需上游统一发布 1.5.0 后方可发布

## [0.6.0] - 2026-08-07

### P3 性能优化（6 大方向）

#### 新增
- **方向 3：SIMD 字符串加速** — `router-facade/src/simd_str.rs`
  - SSE2 并行 ASCII 检测 + 字节操作（`capitalize_first_simd`）
  - SSE2 memchr 风格分隔符查找（`find_separator_simd`），一次扫描 16 字节
  - x86_64 运行时检测 + 非 x86_64 标量回退，18 个单元测试
- **方向 4：内存池** — `sz-rust-core/src/mem_pool.rs`
  - `MemPool` trait + `StackPool<const CAP: usize>`（区域分配器，零堆分配）
  - `BumpaloPool` 实现（`bumpalo-pool` feature gate），13 个单元测试
  - `AllocCounter` GlobalAlloc wrapper（`alloc-count` feature gate），4 个单元测试
- **方向 2：连接池 L3 调优** — `orm-facade/src/{pool_warmer,query_cache,pool_scaler}.rs`
  - `PoolWarmer`：并发连接预热，支持超时和降级，7 个单元测试
  - `QueryCache`：L2 查询缓存（TTL + jitter + LRU 淘汰 + invalidate pattern），10 个单元测试
  - `PoolScaler`：动态扩容/缩容（基于 timeout_rate / idle_rate），10 个单元测试
- **方向 5：零拷贝优化**
  - `routing.rs` 新增 `HandlerRefRef<'a>` 借用版本（零堆分配），13 个单元测试
  - `response.rs` 新增 `to_json_bytes() -> bytes::Bytes`（避免 String UTF-8 验证开销），4 个单元测试
- **方向 6：异步优化** — `sz-rust-core/src/runtime.rs`
  - `SzRuntime` 新增 `blocking_threads` 字段 + `with_blocking_threads()` 链式配置
  - 3 种预设：`for_io_intensive()` / `for_cpu_intensive()` / `for_balanced()`
  - 11 个单元测试
- **P3 bench 框架** — `sz-rust-core/benches/p3_bench.rs`
  - 5 类 benchmark（22 个）：端到端 p99 / SIMD 字符串 / alloc 计数 / 拷贝计数 / 异步调度
- **P3 soak 测试** — `sz-rust-core/tests/soak_p3.rs`
  - 3 个 soak 测试：优化点全覆盖 / SIMD 稳定性 / 异步调度稳定性
- **spawn_blocking 审计脚本** — `scripts/audit_blocking.sh`
  - 静态扫描 async fn 内阻塞调用，审计报告 `docs/audit/blocking_audit_20260807.md`
- **火焰图脚本** — `scripts/flamegraph.sh`

#### 变更
- **方向 1：热路径内联优化**
  - `router.rs`：`parse_path` / `split_first_segment` / `is_app_in_map` / `capitalize_first` / `ParsedPath::new` 添加 `#[inline]`
  - `chain.rs`：`has_duplicates` 添加 `#[inline]`
  - `container/mod.rs`：`make` / `make_or_panic` / `make_with_scope` 添加 `#[inline]`
  - `simd_str.rs`：`capitalize_first_simd` / `find_separator_simd` / `is_ascii_simd` 添加 `#[inline]`
- `router.rs`：`parse_path` 使用 SIMD 分隔符查找替代 `split` 迭代器
- `router-facade/Cargo.toml`：移除 `[lints] workspace = true`，添加 `[lints.rust] unsafe_code = "allow"`
- `sz-rust-core/Cargo.toml`：添加 `alloc-count` / `mem-pool` / `bumpalo-pool` feature、`p3_bench` bench target
- `sz-rust-orm-facade/Cargo.toml`：添加 `async-trait` / `tokio` / `parking_lot` / `rand` / `thiserror` 依赖
- `sz-rust-http-facade/Cargo.toml`：添加 `bytes` 依赖
- workspace 版本 0.5.0 → 0.6.0

#### 验证
- workspace 全量测试：5174 passed, 0 failed
- sz-pay 兼容性：cargo check + 全量测试通过
- sz-orm 上游：无变更（git status 空）
- clippy：无新警告（3 个预存警告）
- fmt：`cargo fmt --all -- --check` 通过
- bench：22 个 benchmark 全部编译通过，`capitalize_first` ~38ns

## [Unreleased]

### 新增
- **sz-rust-addons-crm**：CRM 模板插件（联系人/线索/商机管理，15 个 REST 端点）
- **sz-rust-addons-erp**：ERP 模板插件（商品/供应商/采购单管理，16 个 REST 端点）
- **sz-rust-addons-ecommerce**：电商模板插件（订单/订单项/购物车管理，13 个 REST 端点）
- **RouterBuilder 泛型状态支持**：`RouterBuilder<S>` 支持 `axum::extract::State<S>`，addon 可通过闭包捕获状态注册路由
- **CLI `make:middleware` 命令**：`sz-rust-cli make middleware <name>` 生成中间件骨架
- **10 个新增 .trae/skills/**：test-coverage、performance-check、doc-check、migration、deploy、orm-query、n-plus-one、auth-guard、error-handling、ci-cd

### 变更
- sz300 集成可观测性模块（sz-rust-observability）：Prometheus /metrics 端点 + MetricsRegistry 注入 AppState
- sz300 readiness 探针（/health/ready 端点 + DB 健康检查 + 503 状态码）
- sz300 优雅关闭（with_graceful_shutdown 支持 Ctrl+C + SIGTERM）
- sz300 MQTT 消费者优雅退出（CancellationToken 协调器）
- sz300 tracing 初始化改为 EnvFilter + JSON 格式
- sz300 集成框架统一 AppConfig（sz_rust_core::config::AppConfig）
- 全代码库关键路径添加 #[tracing::instrument] 自动 span 注入
- sz-rust-addons-operate 和 sz-rust-sz300 加入 workspace.members（CI 覆盖 10/10 包）
- 生产就绪度审计报告（docs/audit/2026-07-24-生产就绪度审计报告.md）

### 变更
- 所有 10 个包添加 rust-version.workspace = true
- sz-rust-tracing 依赖改为 workspace 继承
- CI 缓存策略统一为 Swatinem/rust-cache@v2
- CI audit job 从 rustsec/audit-check@v2.0.0 替换为 taiki-e/install-action + cargo audit
- CI mcdc continue-on-error 改为 false（分支覆盖率硬门禁）
- CI outdated continue-on-error 改为 false
- CI 添加 paths-ignore（文档变更不触发 CI）
- CI fmt/no-placeholder job 移除不必要的 sz-orm clone
- docs/audit/ 历史文档归档至 archive/ 目录

### 修复
- P0: middleware/auth.rs JWT 密钥从硬编码改为环境变量 SZ_JWT_SECRET
- P0: sz300/main.rs JWT 密钥改为环境变量 SZ300_JWT_SECRET
- P0: sz300/config.rs 数据库密码改为环境变量 SZ300_DB_PASSWORD
- P0: deny.toml allow-build 非法字段修复为 reason
- P1: upload/storage.rs 路径遍历漏洞修复（.. 检查 + canonicalize 验证）
- P1: sz300 + addons-operate 补齐 #![forbid(unsafe_code)] + #![warn(missing_docs)]
- P1: sz300 missing_docs 226 个警告清零
- P1: addons-operate missing_docs 48 个警告清零
- P1: sz300 unused imports 清理

## [0.2.0] - 2026-07-23

### 新增
- **可观测性模块**（`sz-rust-observability` 包）：`MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）
- **分布式追踪模块**（`sz-rust-tracing` 包）：`Span` / `Tracer` / `SzTracer`，W3C TraceContext 格式（`traceparent: 00-<trace_id>-<span_id>-<flags>`），legacy header 兼容，OTLP exporter 占位
- **ADR-011 可观测性架构决策**：MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（L1 决策层 / L2 运行时层 / L3 指标层 / L4 代码层）
- **ADR-012 分布式追踪架构决策**：W3C TraceContext 标准、OTLP exporter 路径、legacy header 兼容策略
- **missing_docs 严格检查**：CI doc job 启用 `RUSTDOCFLAGS: "-D warnings -D missing_docs"`，所有公开 API 必须有文档注释
- **首次性能基线数据**：`docs/benchmarks/baseline-v0.1.0.md` 记录 criterion 基线，后续版本以此为回归参照
- **6 小时 soak test**：`soak.yml` workflow，每周日 00:00 UTC 自动执行，60 秒指标采样，420 分钟超时
- **cargo-tarpaulin 覆盖率**：`coverage.yml` workflow，统计代码覆盖率并上传 Codecov
- **模糊测试套件**：`sz-rust-core/tests/fuzz.rs`，7 个 fuzz 用例 × 1000 次迭代（parse_path / HandlerRef / route_config / ApiResponse / ErrorCode / AppConfig / Validate），使用自定义 xorshift64 PRNG，不依赖 cargo-fuzz
- **fuzz CI workflow**：`fuzz.yml`，push/PR + 每周六 00:00 UTC + workflow_dispatch 触发，支持 `FUZZ_ITERATIONS` 环境变量自定义迭代次数
- **cargo-deny 依赖审计**：`deny.toml` 配置（许可证白名单 MIT/Apache-2.0/BSD/ISC/Zlib，黑名单 GPL/AGPL/EUPL；RUSTSEC 漏洞检查；重复依赖警告；来源限制仅 crates.io）
- **PHP 迁移指南补充 5 章节**：第 11 章缓存系统迁移 / 第 12 章文件上传迁移 / 第 13 章视图模板迁移 / 第 14 章可观测性迁移（v0.2.0 新增）/ 第 15 章分布式追踪迁移（v0.2.0 新增）

### 变更
- **workspace.package.version**：`0.1.0` → `0.2.0`
- **CI 门禁增强**：移除 test/doc/audit/feature-matrix/unused-deps 5 个 job 的 `continue-on-error: true`，门禁严格生效
- **CI doc job**：添加 sz-orm path 依赖检查 + missing_docs 检查
- **CI test job**：添加 sz-orm path 依赖检查
- **CI 新增 deny job**：cargo-deny 检查 advisories（RUSTSEC）/ licenses / bans（重复依赖）/ sources
- **ADR README 索引**：将"待编写 ADR 清单"改为"ADR 完成状态"，12 个 ADR 全部标记为 ✅ 已接受；关键路径覆盖表 13 项全部标记为已覆盖

### 修复
- 无

## [0.1.0] - 2026-07-22

### 新增
- **框架核心**：sz-rust-core 28 模块就绪（controller/model/relation/middleware/guard/hooks/multi_app/health/h2/routing/addons/cache/event/validate/upload/view 等）
- **路由系统**：三层路由机制（属性宏 / 配置式 / 约定式），对齐 PHP `auto_multi_app` + `config/route.php`
- **中间件**：Tower Service + 洋葱模型，5 个内置中间件（Trace/Cors/Log/RateLimit/Auth）
- **控制器**：SzController trait + BaseController，对齐 PHP `app\SzController`
- **Model 钩子**：16 事件 HookDispatcher（PHP 原生 12 + sz-orm-core 扩展 4）
- **Guard 守卫**：鉴权决策层（AuthGuard/PermissionGuard/GuardChain），借鉴 NestJS
- **响应格式**：`{code, msg, data}` 标准响应，对齐 PHP `renderJson/renderSuccess/renderError`
- **错误体系**：BaseException + 9 个错误码（对齐 PHP + Rust 扩展）
- **缓存系统**：Cache facade + 多驱动（Memory/Redis），对齐 PHP `think\facade\Cache`
- **配置系统**：YAML 加载 + 环境变量覆盖 + 默认值，对齐 PHP `config/*.php`
- **验证器**：规则/场景/消息三件套，对齐 PHP `think\Validate`
- **事件系统**：事件监听器 + 订阅者，对齐 PHP `think\Event`
- **上传**：文件上传 + 图像处理（对齐 PHP `think\Filesystem` + `Grafika`）
- **视图**：模板渲染 + 布局继承，对齐 PHP `think\View`
- **多应用**：`auto_multi_app` 路径解析（oapc/admin/api/farm/oapi/cashier/scene）
- **HTTP/2**：完整 HTTP/2 支持（含 h2c upgrade）
- **插件系统**：addon 插件化机制
- **PDF 处理**：sz-rust-pdf 独立包
- **CLI 工具**：sz-rust-cli 命令行工具
- **业务示例**：sz-rust-addons-operate（375 测试，控制器+服务层迁移完成）
- **示例应用**：sz-rust-examples/crud_demo 完整 CRUD 示例
- **工程化**：10 道门禁（fmt/check/clippy/test/doc/audit/integration + 占位检查/安全扫描/feature 全组合）
- **CI**：GitHub Actions 7 道门禁
- **测试**：2938+ 测试通过（sz-rust-core 2563 + sz-rust-addons-operate 375）
- **文档**：README.md、LICENSE(MIT)、ADR 规范、审计清单、工程化实践规范

### 测试
- 2938+ 测试全部通过
- clippy 0 警告
- fmt 0 差异

## 版本对比链接

[Unreleased]: https://github.com/ljclz/sz-rust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/ljclz/sz-rust/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ljclz/sz-rust/releases/tag/v0.1.0
