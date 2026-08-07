# SZ-Rust 变更日志

本文件记录 SZ-Rust 框架的所有显著变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

---

## [v0.3.1] — 2026-08-05 — 生产就绪修复

### 概要

从综合评分 55/100 提升到可生产状态。执行 8 阶段 / 24 主任务 / 62 子任务的修复计划，覆盖联编修复、依赖治理、失实声称撤回、限流熔断落地、性能基线建立、CI 门禁强化、文档更新。

### 新增

- **限流器**（`packages/sz-rust-middleware-facade/src/rate_limit.rs`）
  - `TokenBucket`：令牌桶算法，parking_lot::Mutex 保护，`with_max_keys` OOM 防护
  - `SlidingWindow`：滑动窗口算法，时间戳清理 + `with_max_keys` OOM 防护
  - 两者实现 `sz_rust_orm_facade::RateLimiter` trait（非 re-export sz-orm-limit）
  - 11 个单元测试（基础放行/拒绝、时间补充、独立 key、reset、OOM 防护、100 并发无误差）

- **熔断器**（`packages/sz-rust-middleware-facade/src/circuit_breaker.rs`）
  - `CircuitState` enum（Closed/Open/HalfOpen）
  - `CircuitBreakerConfig`（error_threshold / cooldown / probe_requests / stat_window）
  - `CircuitBreaker`（parking_lot::Mutex 保护状态机）
  - `circuit_breaker_middleware`（Open 返回 503，Closed/HalfOpen 放行）
  - 10 个单元测试（三态流转、错误率边界、并发安全、探测限制）

- **性能基线**（`packages/sz-rust-core/benches/`）
  - `collect_env.rs`：`BenchEnvironment` 结构体 + `collect()` + `write_to_path()` 元数据采集
  - `results/README.md`：结果目录说明 + 命名规范
  - `core_bench.rs` 新增 2 个基准组：`bench_rate_limiting` + `bench_circuit_breaker`
  - 9 类基准覆盖（超出 7 类最低要求）：route_matching / handler_ref_parse / route_config / json_serialization / middleware_chain / di_container / framework_vs_native / rate_limiting / circuit_breaker

- **CI 门禁强化**
  - `benchmark.yml`：新增 bench 基准覆盖检查（校验 9 类基准存在）
  - `security.yml`：新增 unmaintained 真实编译检查（paste / rustls-pemfile / rkyv 逐个 `cargo tree -i` 验证）
  - `ci.yml`：新增 Windows 兼容性 job（`CARGO_INCREMENTAL=0` 规避 STATUS_STACK_BUFFER_OVERRUN）

### 变更

- **依赖治理**
  - `paste` 消除：升级 imageproc 0.25→0.27（default-features=false）+ image 禁用 exr feature，切断 image→exr→pulp→paste 依赖链
  - `rustls-pemfile` 消除：迁移至 `rustls-pki-types::pem::PemObject`（`h2.rs` 中 PEM 解析代码）
  - `ttf-parser` 部分消除：lopdf 0.42→0.44 消除 lopdf 路径；ab_glyph→owned_ttf_parser 路径无替代品（已在 deny.toml 记录理由）
  - `rkyv` 确认为幻影依赖（`cargo tree -i rkyv` = nothing to print，含 --all-features）
  - `sz-rust-core` 清理：移除未使用的 imageproc / ab_glyph 依赖（实际使用在 infra-facade）
  - `deny.toml` + `audit.toml`：更新所有 ignore 条目（paste / rustls-pemfile / ttf-parser / rkyv）附理由

- **版本 bump**：workspace `Cargo.toml` version 0.3.0 → 0.3.1

- **文档更新**
  - `README.md`：版本号更新、限流熔断标注"已落地"、addons 标注脚手架状态、CI 门禁数 10→17
  - `docs/audit/2026-08-04-成熟度评估与生产差距.md`：修正 3 处失实声称（sz-protocol / sz-orm-dtx / 限流熔断）
  - `docs/audit/2026-08-04-代码实测深度评测.md`：限流熔断状态更新为"已落地"
  - `docs/audit/2026-08-05-项目状态评估报告.md`：限流熔断状态更新、版本号更新
  - `docs/sz-rust-engineering-practices.md`：版本号更新、测试数更新

### 撤回

- **失实声称撤回**（`docs/audit/2026-08-04-成熟度评估与生产差距.md`）
  - sz-protocol "护城河" → "已撤回：目录空，Cargo.toml 无引用"
  - sz-orm-dtx 分布式事务 → "Roadmap：无 src/ 实现"
  - 智能秤行业框架 → "Roadmap：sz-protocol 当前不存在"
  - 限流/熔断/降级 → "v0.3.1 已落地"

### 脚手架标注

- `packages/sz-rust-addons-crm/src/lib.rs`：添加脚手架标注 doc-comment
- `packages/sz-rust-addons-erp/src/lib.rs`：添加脚手架标注 doc-comment
- `packages/sz-rust-addons-ecommerce/src/lib.rs`：添加脚手架标注 doc-comment

### 测试

- workspace 联编：`cargo test --workspace --no-run` 退出码 0
- 逐包测试基线：core 424, infra 670, middleware 440(+21 新增), state 222, operate 466, loader 227, cli 184
- **总计 2,633 测试，0 失败**

### 约束遵守

- 未修改上游 `../sz-orm/` 仓库任何文件（sz-pay 依赖硬约束）
- 未删除任何有效测试
- Windows 环境全程使用 `CARGO_INCREMENTAL=0`
- 所有结论附 file:line 证据

---

## [v0.3.0] — 2026-08-02 — addons 生态 + RouterBuilder 泛型状态

（见历史提交记录）

---

## [v0.2.0] — 可观测性 + 分布式追踪

（见历史提交记录）