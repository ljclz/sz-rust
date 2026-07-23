# 变更日志

本项目所有重要变更均会记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增
- 无

### 变更
- 无

### 修复
- 无

## [0.2.0] - 2026-07-23

### 新增
- **可观测性模块**（`sz-rust-observability` 包）：`MetricsRegistry` + Counter/Gauge/Histogram 三种指标类型，SLO 多窗口燃烧率告警（1h/5m + 6h/30m 双窗口对，对齐 Google SRE Workbook 第 5 章）
- **分布式追踪模块**（`sz-rust-tracing` 包）：`Span` / `Tracer` / `SzTracer`，W3C TraceContext 格式（`traceparent: 00-<trace_id>-<span_id>-<flags>`），legacy header 兼容，OTLP exporter 占位
- **ADR-011 可观测性架构决策**：MetricsRegistry 设计、SLO 多窗口燃烧率、四层可观测性模型（L1 决策层 / L2 运行时层 / L3 指标层 / L4 代码层）
- **ADR-012 分布式追踪架构决策**：W3C TraceContext 标准、OTLP exporter 路径、legacy header 兼容策略
- **missing_docs 严格检查**：CI doc job 启用 `RUSTDOCFLAGS: "-D warnings -D missing_docs"`，所有公开 API 必须有文档注释
- **首次性能基线数据**：`docs/benchmarks/baseline-v0.1.0.md` 记录 criterion 基线，后续版本以此为回归参照
- **24 小时 soak test**：`soak.yml` workflow，每周日 00:00 UTC 自动执行，60 秒指标采样，1500 分钟超时
- **cargo-tarpaulin 覆盖率**：`coverage.yml` workflow，统计代码覆盖率并上传 Codecov
- **模糊测试套件**：`sz-rust-core/tests/fuzz.rs`，7 个 fuzz 用例 × 1000 次迭代（parse_path / HandlerRef / route_config / ApiResponse / ErrorCode / AppConfig / Validate），使用自定义 xorshift64 PRNG，不依赖 cargo-fuzz
- **fuzz CI workflow**：`fuzz.yml`，push/PR + 每周六 00:00 UTC + workflow_dispatch 触发，支持 `FUZZ_ITERATIONS` 环境变量自定义迭代次数
- **cargo-deny 依赖审计**：`deny.toml` 配置（许可证白名单 MIT/Apache-2.0/BSD/ISC/Zlib，黑名单 GPL/AGPL/EUPL；RUSTSEC 漏洞检查；重复依赖警告；来源限制仅 crates.io）
- **PHP 迁移指南补充 5 章节**：第 11 章缓存系统迁移（Phase 6）/ 第 12 章文件上传迁移（Phase 5）/ 第 13 章视图模板迁移（Phase 7）/ 第 14 章可观测性迁移（v0.2.0 新增）/ 第 15 章分布式追踪迁移（v0.2.0 新增）

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
