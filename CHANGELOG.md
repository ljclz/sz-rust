# 变更日志

本项目所有重要变更均会记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased] - 2026-09-02

### Added

- **sz-rust-core 变异测试质量改进**（mutants_quality_improvement）：
  - **最终结果**：missed 从 119 降至 **10**（目标 ≤ 30 达成），caught 从 233 升至 325
  - **两轮测试补强**：
    - 第一轮：29 个新增测试函数（13 文件，480 行），missed 119→95
    - 第二轮：16 新增 + 2 修改测试函数（5 文件，+316/-112 行），missed 95→10
  - **mutants.yml 优化**：添加 `--all-features` 启用 feature-gated 模块变异测试（解决 32 个 feature gate missed）
  - **变异测试 CI 验证**：Run ID 33605977190，434 mutants tested in 2h: **10 missed**, 325 caught, 98 unviable, 1 timeout
  - **10 个存活变异体**（均为语义等价或需真实环境）：
    - `runtime.rs:185` for_balanced → Default::default()（Default 调用 new()，语义等价）
    - `runtime/websocket.rs:156,161` connection_count/broadcast_to_all（需真实 WS 连接）
    - `runtime/hot_reload.rs:209,214,367` loaded_addons/get_manifest/registry（空状态语义等价）
    - `runtime/hot_reload.rs:253,320` delete !（需 .so 文件/加载插件）
    - `mem_pool.rs:297` delete match arm 4096（create_pool 默认容量语义等价）
  - **验证结果**：`cargo test -p sz-rust-core --lib` 476 passed / `--all-features --lib` 522 passed，0 failed
  - **侵入式扩展**（仅 3 处，均为 `#[cfg(test)]` 或 trait bound）：`WorkerConfig::new_unchecked`、`MockConnection`、`MemPool: Debug`

- **addon_deploy_ci_v3：CRM/CMS addon 接线到 sz300 生产环境**（第四轮审计修复）：
  - **CRM 路由 panic 修复**：`packages/sz-rust-addons-crm/src/lib.rs` 10 处旧式 `:id` 路径参数改为 axum 0.8 新式 `{{id}}` 格式，消除路由注册 panic
  - **sz300 Cargo.toml 添加 CRM/CMS 依赖**：`sz-rust-addons-crm` + `sz-rust-addons-cms` + `sz-rust-addons-loader`
  - **AppState 新增 CRM/CMS state 字段**：`crm_state: sz_rust_addons_crm::CrmState` + `cms_state: sz_rust_addons_cms::CmsState`
  - **router.rs 挂载 CRM/CMS 路由**：`register_routes(builder, state.crm_state.clone())` + `register_routes(builder, state.cms_state.clone())`，路由前缀 /api/crm/* + /api/cms/*
  - **CRM/CMS Capability 注册**：`CrmPlugin::new(CrmState::default()).register_capabilities(&capability_registry)`（7 项 CRM Capability）+ `CmsPlugin::new(CmsState::default()).register_capabilities(&capability_registry)`（5 项 CMS Capability）
  - **8 端到端测试通过**：CRM 4 测试（contacts/leads/deals list + contact detail）+ CMS 4 测试（articles/category/tags list + article detail），验证路由从 sz300 入口可达（非 404）
  - **README 版本号同步**：CRM/CMS README v1.1.0 → v1.2.0，与 workspace 一致
  - **T2 服务器代码同步**：服务器 `/www/rust/sz-rust-new` git pull `4167f10` → `210157b`（87 files changed），Rust 升级 1.75.0 → 1.97.1，`cargo build --release` Finished（7m46s），sz300-server 重启（PID 3914540，端口 8300），日志确认 `CRM Capability 已注册（7 项）` + `CMS Capability 已注册（5 项）`，端点验证 `/health → 200` + `/api/crm/contacts → 200` + `/api/cms/articles → 200`
  - **T3 workflow 触发验证**：
    - `ai-facade-perf-gate` ✅ conclusion=success（run 32574835756）
    - `mcdc` ✅ conclusion=success（run 32575534407，修复 nightly 工具链 `-Z instrument-coverage` 兼容性，commit `30fa598`）
    - `mutants` ⏹️ cancelled（run 32574837713，运行 2 小时后被取消，变异测试 timeout 120min）
    - `publish-oss` / `release` 跳过（需 tag 触发，有不可逆发布副作用）
    - `marketplace-ci` 跳过（已定性虚构，`sz-rust-marketplace` crate 不存在）

- **新增 sz-rust-vector-db crate**（P1 任务组4，AIH-1）：
  - Qdrant HTTP API 适配器，实现 `sz-rust-ai-facade::embedding::VectorStore` trait
  - 支持 upsert / query / delete / ensure_collection，多租户 payload filter 隔离
  - UUID v5 确定性 ID 映射，保证 upsert 幂等
  - feature gate `qdrant`（默认不启用），`qdrant-integration`（testcontainers 集成测试，需 Docker）
  - 14 单元测试通过，异常映射覆盖 401/403/404/429/5xx + 网络错误
  - sz300 接线：`qdrant` feature + `SZ300_QDRANT_URL` 环境变量，未配置时降级 FileVectorStore
  - workspace 成员 36 → 37

- **多模态 vision 支持**（P1 任务组6，AIH-2）：
  - 新增 `ContentPart` 枚举（Text / Image / ImageBase64）+ `ImageDetail`（Low / High / Auto）
  - `ChatMessage.content` 从 `String` 升级为 `ContentPart`，`#[serde(untagged)]` 保证 JSON 向后兼容
  - `From<String>` / `From<&str>` / `Display` / `Default` / `Hash` impl 保持源码兼容
  - `as_text()` / `text_or_empty()` / `is_image()` 方法提供文本访问
  - OpenAI Provider：`build_request_body` 适配 vision API 格式（image_url + detail）
  - Claude Provider：适配 vision API 格式（image + source.url / source.base64）
  - Gemini Provider：适配 vision API 格式（file_data / inline_data）
  - 18 序列化兼容性测试 + 11 Provider vision 格式单元测试，全部通过
  - `real-api` feature gate + 3 个 OpenAI vision 真实测试（ignored，需 API Key + 网络）

- **tiktoken 精确 token_count**（P1 任务组7，AIM-2）：
  - 新增 `tiktoken` feature gate（`tiktoken-rs = "0.5"` optional dep），默认不启用
  - OpenAI Provider `token_count` 使用 `cl100k_base` BPE 编码器，`OnceLock` 全局单例缓存
  - feature 未启用时回退到 `chars * 0.25` 估算，保持零依赖开销
  - 10 个精确性测试通过（英文/中文/代码/JSON/特殊字符/长文本）

- **LocalEmbedding 模型加载验证**（P1 任务组8，AIM-3）：
  - 移除 `#[allow(dead_code)]`，新增 `model_path()` / `is_model_loaded()` / `load_model()` API
  - `local-model` feature gate，`load_model` 验证 ONNX 模型文件非空
  - 11 测试通过（pseudo/loaded/error 路径覆盖）

- **LongTermMemory 持久化实现**（P1 任务组9，AIM-4）：
  - `LongTermMemoryStore` trait（store/retrieve/decay/by_agent 异步方法）
  - `FileLongTermMemoryStore` 实现：JSONL 持久化 + `tokio::fs` + `parking_lot::RwLock` 缓存
  - 衰减策略：`importance * exp(-λ * age)`，age 以天为单位，低于 threshold 删除
  - 10 测试通过（store/retrieve/decay/by_agent/persistence/tenant_filter）

- **Agent citations 端到端接入**（P1 任务组10，AIM-5）：
  - `Agent` 新增 `rag_pipeline: Option<Arc<RagPipeline>>` + `with_rag_pipeline()` 方法
  - `run` 方法开始时若 RAG pipeline 存在则检索 top 5 + 转换为 `Citation` + 注入 context
  - 5 端到端测试通过（非空 citations / doc_id+score 保持 / 无 RAG 时空 / context 注入 / 空向量存储）

- **RAG 重排序（Reranker）**（P1 任务组11，AIM-6）：
  - `Reranker` trait（`rerank(query, candidates, topk)` 异步方法）
  - `NoopReranker` 兜底实现（直接透传）+ `WeightedReranker`（向量分数+文本长度加权）
  - `CrossEncoderReranker`（Cohere Rerank API），feature gate `reranker` 默认不启用
  - `RagPipeline` 集成 `with_reranker()`，retrieve 后调用 reranker，失败时回退原序
  - 8 重排序测试 + 6 单元测试通过

- **RAG 混合检索（Hybrid Search）**（P1 任务组12，AIM-7）：
  - `Bm25Index`：内存 BM25 全文索引，支持增量添加/删除/搜索，case-insensitive 分词
  - `HybridRetriever`：向量检索 + BM25 关键词检索 + RRF（Reciprocal Rank Fusion）融合
  - `RagPipeline` 新增 `with_hybrid_retriever()` 运行时切换，默认纯向量检索
  - feature gate `hybrid`，7 混合检索测试 + 14 BM25 单元测试通过

- **RouterBuilder patch/head/options 方法**（P2 任务组13，FM-1）：
  - `RouterBuilder<S>` 新增 `patch()` / `head()` / `options()` 三个便捷方法
  - 与 get/post/put/delete 签名对齐，options 使用 `MethodFilter::OPTIONS`
  - 5 路由分发测试通过（PATCH/HEAD/OPTIONS 分发 + 错误方法 405 + 全方法共存）

- **模板 {include} 实现**（P2 任务组14，FM-2）：
  - `render_includes` 方法：解析 `{include file="header" /}` → 读取模板文件 → 递归渲染
  - 循环包含检测：`Rc<RefCell<HashSet<PathBuf>>>` 包含栈，递归前检查路径是否在栈中
  - 路径越狱防护：规范化路径后检查是否在 `view_path` 子树内，禁止 `../` 逃逸
  - 8 综合测试通过（单层/嵌套/循环检测/路径越狱/变量插值/自引用/不存在/多include）

- **audit_remediation_v2：6 项幻影交付生产接线**（第二轮审计修复）：
  - **sz300 Cargo.toml 启用 4 个 feature**：`tiktoken`/`reranker`/`hybrid`/`local-model`，使 ai-facade 内 `#[cfg(feature)]` 分支参与 sz300 编译
  - **RagPipeline 生产接线**：sz300 main.rs 构造 `RagPipeline::new` + `with_reranker` + `with_hybrid_retriever`，通过 `Ai::init_default` 第 4 参数注入；`Ai::agent` 内部接入 `with_rag_pipeline`（facade.rs:93）
  - **Agent + LongTermMemory 生产接线**：sz300 main.rs 构造 `ToolRegistry` + `FileLongTermMemoryStore`（`SZ300_AGENT_ENABLED=1` 时），`AppState` 新增 `long_term_memory` 字段
  - **LocalEmbedding 真实加载**：`build_local_embedding` 辅助函数，`SZ300_LOCAL_EMBEDDING_MODEL` 环境变量驱动，未设置时降级 `new_pseudo`
  - **环境变量驱动**：`SZ300_RERANKER_API_KEY`/`SZ300_HYBRID_ENABLED`/`SZ300_AGENT_ENABLED`/`SZ300_LOCAL_EMBEDDING_MODEL`，默认行为不破坏（无环境变量时降级 NoopReranker/纯向量/无 Agent/pseudo）
  - **7 端到端测试通过**：rag_pipeline（reranker+hybrid）、agent（citations+LongTermMemory roundtrip）、local_embedding（real_load+degrade）、tiktoken（BPE 精确值≠估算值）
  - **IndustryRag 保留**：与 RagPipeline 并行共存（ADR-001），非替换

- **audit_remediation_v3：5 项高风险幻影交付生产接线**（第三轮审计修复）：
  - **LlmChatCapability 注册到 CapabilityRegistry**：sz300 main.rs 在 `Cap::init_with` 后调用 `Cap::register(Arc::new(LlmChatCapability))`，能力名 `ai.llm_chat` 可被 CapabilityRegistry 发现
  - **Ai::embed 端点接线**：sz300 新增 `POST /api/v1/ai/embed` 端点（controllers/ai.rs:embed + router.rs），调用 `Ai::embed(texts, model)` 返回嵌入向量
  - **Ai::stream_chat 端点接线**：sz300 新增 `POST /api/v1/ai/stream` 端点（controllers/ai.rs:stream + router.rs），SSE 格式输出 `StreamDelta`，`StreamDelta` 未实现 Serialize 故用 `serde_json::json!` 手动构造
  - **McpToolBridge 工具注入**：sz300 main.rs `build_tool_registry` 改为通过 `McpToolBridge::new(&["parse_path","build_select_query","openapi_spec","redaction_check","url_decode","sql_validate"]).adapters()` 注入 6 个 MCP 工具到 ToolRegistry，Agent 获得真实可用工具
  - **AiMetrics 接入 Prometheus**：sz300 main.rs 在 MetricsRegistry 初始化后调用 `AiMetrics::global().register(&metrics_registry)`，12 项 AI 指标（request_count/latency/token_usage/error_rate 等）导出到 Prometheus
  - **5 端到端测试通过**：llm_chat_capability（trait 实现）、ai_embed（可调用返回向量）、ai_stream_chat（可调用返回流）、mcp_tool_bridge（3 工具注入 registry）、ai_metrics（global 实例可获取）
  - **futures 依赖提升**：sz300 Cargo.toml 将 `futures` 从 dev-dependency 提升到 dependencies（stream handler 需 `futures::StreamExt`）

### Removed

- **移除 sz-rust-k8s-operator 孤儿 crate**（`ADR-038`）：
  - workspace 成员 37 → 36，移除 k8s-openapi/kube/kube-derive workspace 依赖
  - 理由：无消费者、无生产入口、CI 无 K8s 集群、reconcile.rs:33 TODO 自承认测试覆盖缺口
  - 22 个测试保留在 git 历史，未来需要时可恢复

### Changed

- **HTTP 客户端 TLS 后端迁移：native-tls → rustls**（`90a0779`，AI 评审 P4 补记录）：
  - reqwest 配置 `default-features = false, features = ["json", "multipart", "rustls-tls", "charset", "http2"]`
  - 根因：reqwest 默认 features 含 default-tls(native-tls) → 引入 openssl-sys → aarch64 交叉编译找不到 OpenSSL
  - 影响：证书校验链由系统 OpenSSL 切至 rustls（webpki-roots 系统探测行为差异，企业 MITM 代理场景需验证）
  - 验证：`cargo tree -i native-tls` → "did not match any packages"（2026-09-03）
- **HTTP 客户端证书源修正：webpki-roots → 系统根证书库**（`rustls-tls` → `rustls-tls-native-roots`，2026-09-04 复审修正）：
  - 原风险：rustls-tls 默认用 webpki-roots 内置根证书且不读系统证书库，内网 MITM 代理/企业自签 CA 部署场景 HTTPS 握手直接失败（兼容性灾难）
  - 修正后 reqwest 依赖块含 `rustls-native-certs`（Linux 读 /etc/ssl/certs，Windows 读系统证书库），Docker 运行时镜像已有 ca-certificates
  - 验证：Cargo.lock 中 reqwest/hyper-rustls 依赖均含 rustls-native-certs；native-tls / openssl-sys 保持 ABSENT（aarch64 修复不回归）
- **变异测试排除清单版本化**（AI 评审 P1/P3 采纳 + 2026-09-04 复审增强）：
  - 排除清单从 mutants.yml 内联 `--exclude` 迁移至 `.cargo/mutants.toml`（每条含理由+退出条件，pay.rs FIXME(DB-2026-09-03-01)）
  - mutants job `timeout-minutes` 480 → 360（GitHub 托管 runner 上限，480 会被静默截断）
  - 新增单变异体 `--timeout 120`（秒），防止单个挂死变异体吃掉整个 job 预算
  - 新增 `if: always()` 源码洁净度断言：`--in-place` 改写的变异源码在 job 结束（含失败/超时）后必须恢复洁净，`git status --porcelain` 非空即报错并 checkout 还原（防 job 被 kill 后变异源码残留污染缓存与后续步骤）
- **README.md 版本号对账**：v0.7.0 → v1.2.0，sz300 测试数 171 → 642，铁律数 22 → 23
- **sz-rust 全部 18 个 crate 发布 1.2.0 到 crates.io**（`596c11f`）：
  - workspace 版本 1.1.0 → 1.2.0，33 个内部依赖版本同步
  - 18 crate 按依赖顺序发布：macros → facades → mcp → capability → addons-loader → core
  - sz-rust-core 1.2.0 依赖 sz-orm-core 5.0.0，外部消费者可直接 `sz-rust-core = "1.2.0"`
  - sz-pay 已改回 crates.io 依赖，5159 passed, 0 failed

### Verified

- `cargo search sz-rust-core` → `sz-rust-core = "1.2.0"`
- 18/18 crate 在 crates.io 验证通过
- sz-rust-core 450 + mcp 52 + sz300 477 = 979 passed, 0 failed

## [Unreleased] - 2026-08-21（sz-orm 5.0.0 统一升级）

### Changed

- **sz-orm 全家桶 17 个子 crate 从 4.9.0/4.9.1 统一升级到 5.0.0**：
  - workspace 根 `Cargo.toml:184-201` 版本号更新
  - 8 个未发布 crate（auth/mqtt/websocket/scheduler/logger/config/query-builder/graphql）已发布 5.0.0 到 crates.io
  - `cargo tree -d` 无 sz-orm-* 重复版本（全部统一 5.0.0）

### Verified

- 编译验证：sz-rust-orm-facade / sz-rust-core / sz-rust-mcp / sz-rust-sz300 / sz-rust-cli 全部 `cargo check` 通过
- 测试验证（零回归）：orm-facade 111 + core 835 + mcp 101 + sz300 477 + facade-tests 16 + cli 364 = 1904 passed, 0 failed

## [Unreleased] - 2026-08-20（边界测试补齐 + 性能压测基线）

### Added

- **WASM/K8s/GraphQL 边界测试补齐**（`d08bbfe`）：
  - sz-rust-wasm: 新增 12 边界测试（i64 溢出、空 wasm、非法 opcode），20 passed + 1 ignored
  - sz-rust-k8s-operator: 新增 11 边界测试（空 spec、无效 image、label 冲突），22 passed
  - sz-rust-sz300 graphql_api: 新增 11 边界测试（空 query、嵌套深度、product 溢出），17 passed
- **性能压测基线脚手架**（`scripts/perf-baseline/`）：
  - 5 脚本：install-hey.js / run-hey.js / sample-resource.js / health-probe.js / generate-report.js
  - 使用 ssh2 远程执行，禁用 sshpass/powershell
  - 压测工具：ab (Apache Benchmark 2.3)，oha 编译过慢改用服务器已有 ab
- **生产实例压测报告**（`docs/audit/2026-08-20-perf-baseline.md`）：
  - 3 端点 × 5 并发梯度 × 30s，错误率 0.00%，健康率 100%
  - health: QPS 2090-5562, P95 0-122ms
  - graphql: QPS 5916-13465, P95 0-17ms
  - wasm: QPS 6019-13531, P95 0-17ms
  - 峰值 RSS 28.8 MB，平均 CPU 109.4%

### Fixed

- **GraphQL product 查询溢出**（`d08bbfe`）：`100 * good_id` → `100i64.checked_mul(good_id)?`（`graphql_api.rs:65`）
- **WASM product 溢出**（`d08bbfe`）：同上 checked_mul 改造（`sz-rust-wasm/src/lib.rs`）

### Changed

- doc-debt 新增 reconcile 测试缺口记录（DB-2026-08-20-05）

## [Unreleased] - 2026-08-19（7 P1 死 crate 复活 + 生产接线 + R001-R007 安全复核）

### Added

- **7 个 P1 死 crate 全部复活到 workspace**（`6225cb5`）：
  - sz-rust-tracing / sz-rust-pdf / sz-rust-workflow / sz-rust-addons-operate / sz-rust-addons-erp / sz-rust-addons-forum / sz-rust-addons-im
  - 全部编译通过 + clippy 0 warning
- **3 crate 补测试**（`5d5147c`）：
  - sz-rust-tracing: 55 测试，100% 行覆盖率（新增 20）
  - sz-rust-pdf: 164 测试，95.86% 行覆盖率（新增 29）
  - sz-rust-addons-operate: 495 测试，87.84% 行覆盖率（新增 29）
- **7 crate 生产接线到 sz300**（`c371148`）：
  - erp/forum/im: 路由注册（`/api/erp/*`、`/api/forum/*`、`/api/im/*`），路径参数 `:id` → `{{id}}` 适配 axum 0.8
  - operate/workflow/tracing/pdf: sz300 依赖链接 + `/api/addons/status` 端点暴露生产入口
  - 验证: sz300 447 测试 + erp 23 + forum 23 + im 23 全部通过

### Fixed

- **R001-R007 安全项全部人工复核完成**（doc-debt DB-2026-08-16-05 RESOLVED）：
  - R001 mem_pool transmute: arena 标准模式，unsafe fn 契约 + ADR-037（`mem_pool.rs:227`）
  - R002 MCP SQL 注入: `validate_identifier` 白名单校验阻止注入（`mcp/lib.rs:74-98`）
  - R003 from_utf8_unchecked: 输入为合法 &str 副本（`mem_pool.rs:144`）
  - R004 std::fs: 已修，`PROD_STD_FS=0`
  - R005 admin 信息暴露: `admin_role_guard` 保护（`router.rs:173`）
  - R006 hot_reload unsafe: feature-gated 默认关闭（`hot_reload.rs:82`）
  - R007 SIMD unsafe: 不暴露 unsafe 到公开 API（`simd_safe.rs:8`）

### Changed

- workspace crate 数 28 → 35（新增 7 个 member）
- 全部提交已 push 到 origin/main（`c371148`）

## [Unreleased] - 2026-08-19（CI-001~007 覆盖率 CI 集成增强）

### Added

- **CI-001~007 覆盖率 CI 集成增强**（`d6379c5`）：
  - ci.yml 门禁 14: 拆分 llvm-cov 为常规+DB 两步 + per-crate-coverage.js 分 crate 门槛校验 + needs:[assertion-value] 联动 + COVERAGE_THRESHOLD 环境变量
  - coverage.yml: 4 并行分片 job (p0/p1/p2/p3) + coverage-merge job + sz300 --fail-under 85 + COVERAGE_THRESHOLD

## [Unreleased] - 2026-08-16（PR 审查编排 Skill：状态机 + 5 环节 + 严重度模型）

### Added

- **sz-rust-pr-review Skill**（`.trae/skills/sz-rust-pr-review/` + `scripts/audit/pr-review.sh`，对齐微信文章《代码审查 Skill 实战》架构模式，复用项目既有检查资产，不引入新依赖）：
  - 状态机：scanning → static → security → done / failed（任一步失败即 fail-closed，退出非零）
  - 环节：`git diff` 扫描（--check）→ `cargo fmt` → `clippy --workspace --all-targets -D warnings` → 5 个门禁脚本（sensitive-field / doc-code / feature / assertion / adr）
  - 严重度模型：critical（clippy 编译错误 / EXPOSED）/ high（feature 门禁）/ medium（fmt / lint / whitespace）/ low（doc/adr/assertion WARN），`--severity-threshold` 控制阻塞阈值
  - 报告：`docs/audit/<date>-pr-review-<branch>.md`（状态流转 + 问题清单 + 结论）
  - 验证（真实输出）：当前工作区审查正确识别 3 个问题（fmt medium + clippy critical/lint）→ EXIT=1 阻塞；非法 range → fail-closed
  - **AI 评审环节（2026-08-16 接入）**：`--ai` 参数启用，OpenAI 兼容端点通用配置（`AI_API_KEY`/`AI_BASE_URL`/`AI_MODEL`，默认 CSDN `https://ai.csdn.net/api/model/v1` + `model=glm_for_coding` 套餐，旧变量名 `CSDN_API_KEY` 兼容）；prompt 设计对齐文章 ai_reviewer（diff 截断 8000 字符 + 问题清单 → 3-5 个最重要问题 + 建议 + 评分）；无 key/请求失败/解析失败 → medium 问题如实记录；AI 结论只进报告不影响阻塞判定
  - **快手 Provider 实测通过（2026-08-16）**：`https://wanqing.streamlakeapi.com/api/gateway/coding/v1` + `KAT-Coder-Pro-V2.5`，全量审查 EXIT=0，AI 评审给出 3 条有效建议（错误脱敏/临时文件 mktemp/python 版本保护，采纳 2 条）；编码 bug 修复（Windows python 管道 GBK → 强制 UTF-8）；错误信息增强（输出 Provider 原始 error 详情）
  - 说明：`sz-rust-ai-facade::llm::OpenAiProvider`（llm/openai.rs）为同协议库实现（早已存在，非本次新增），可配置 CSDN base_url/model 供 Rust 应用接线（如 sz300 /api/v1/ai/chat 真实化）

## [Unreleased] - 2026-08-16（AI 交叉审查修复：stdout 版本保护 + 死变量清理 + 报告标注）

### Fixed

- **外部 AI 审查发现并核实 3 处真实问题**（交叉验证 `docs/audit/2026-08-16-pr-review-main.md`）：
  - `pr-review.sh` stdout reconfigure 无 hasattr 保护（:160，与 stdin :174 不一致）——上轮"采纳 AI 建议"的批量替换因模式不匹配（`glm_for_coding` vs `os.environ["AI_MODEL"]`）静默失败，只生效了 mktemp 与 stdin 保护；已补 stdout 保护
  - 死变量 `AI_BODY_TMP`（定义未使用）已清理
  - 报告格式误导：AI 评审意见不进入 ISSUES 计数（设计如此）但未标注——现标注"仅供参考：不进入问题计数，不参与阻塞判定"
- 正则加 `^` 行首锚定（外部 AI 建议 #4，低风险加固）

> 教训：批量代码替换必须逐项验证落地（本次静默失败源于 python str.replace 无匹配返回原串且未 assert）

## [Unreleased] - 2026-08-16（pr-review 升级为全量 15 项门禁）

### Added

- **pr-review.sh 全量门禁升级**（补齐此前缺失维度，对标 sz-orm-review 23 关）：
  - 状态机扩展：scanning → compile → static → security → test → integration → deep(可选) → ai(可选) → done
  - 新增门禁：编译检查（cargo check，critical）、裸 unwrap 检查（check-unwrap.py，铁律 2，high）、单元测试（cargo test，critical）、真实集成（jobs_integration --ignored 需 MySQL，high，`--skip-integration` 可跳过）、深验证（`--deep`：变异杀率 + jobs.rs ≥75% 行覆盖）
  - 门禁表 9 → 15 项；SKILL.md（Trae + ZCode /sz-rust-review）与执行指南同步
- **首次全量运行暴露既有债务**：生产代码 51 处裸 unwrap（铁律 2）——pre-commit 钩子此前仅警告，现以 high 阻塞；已登记 doc-debt（DB-2026-08-16-04，限期 08-19）
- 验证（真实输出）：全量 15 项门禁 EXIT=1（unwrap 51 处正确阻塞；此前 EXIT=0 为管道干扰误读）；check/clippy/test/integration/AI 各环节通过

## [Unreleased] - 2026-08-16（unwrap 专项清偿：51 处生产裸 unwrap → 0）

### Fixed

- **生产代码裸 unwrap 专项清偿**（铁律 2，doc-debt DB-2026-08-16-04 RESOLVED）：
  - `lock().unwrap()` 13 处 → `unwrap_or_else(|e| e.into_inner())`（锁中毒恢复，无 panic；mem_pool 3 + examples bin 10）
  - 启动阶段 `bind().await` / `serve().await` unwrap → `expect("绑定监听地址失败"/"HTTP 服务启动失败")`（铁律 2 允许启动阶段 expect；examples bin 10 处 + perf-compare 4 处）
  - 测试辅助函数 unwrap → `expect("测试请求构造失败"等)`（api_version 6 / handler_as_middleware 3 / role_guard 3 / mcp 3）
  - 生产必有值 unwrap → `expect("明确原因")`（capability 3 / workflow 2 / ip_access_control 2 / examples 6）
  - perf-compare 基准 11 处（Redis/启动）→ expect
- **附带修复**：core container 测试预存漂移（hostport 断言 8802→3306，与 config/database.yml 一致）
- 验证（真实输出）：AUTHORITATIVE_PROD_UNWRAP=0；cargo check 0 errors；受影响 crate 测试 1597 passed / 0 failed；clippy --workspace --all-targets -D warnings 0 警告；fmt 干净

## [Unreleased] - 2026-08-16（AI 评审意见 P2-P4 采纳：可观测性改进）

### Changed

- `/sz-rust-review --ai`（bff78e4 审查）AI 意见处理：
  - P2 采纳：builtin.rs 冗余 `assert!(is_ok())` + expect 双检查 → 单 expect
  - P3 采纳：bind/serve expect → `unwrap_or_else(panic! 带地址与底层 IO 错误)`（10 处，排查可区分端口占用/权限/地址格式）
  - P4 采纳：registry 并发 JoinHandle → `unwrap_or_else(panic! 带 JoinError)`（保留子任务 panic 信息）
  - P1/P5 不采纳（记录理由）：`into_inner()` 为标准库推荐的锁中毒恢复；mem_pool 分配器无中间状态数据；mem_pool 生产 0 调用 + feature 默认关闭（ADR-037）；examples 为演示代码
- 验证（真实输出）：check 0 errors；capability/sz300 测试通过；clippy -D warnings 0 警告；unwrap=0；全量审查 EXIT=0

## [Unreleased] - 2026-08-16（铁律 4 门禁 + 生产 std::fs 修复 + 安全审计清单）

### Fixed

- **铁律 4 门禁上线**：新增 `scripts/audit/check-std-fs.py`（复用 check-unwrap 上下文逻辑，排除测试块/目录），接入 pr-review security 环节（high 阻塞）
- **生产 std::fs 修复**：addons-loader 生产路径完整 async 化——`load_from_directory`/`parse_manifest` → tokio::fs（read_dir/read_to_string），tokio 转主依赖；连锁 async 化测试 30+ 个（registry/loader/route/manifest 的 #[tokio::test]）；addons-loader 231 测试通过
- **安全审计清单登记**（doc-debt DB-2026-08-16-05/06）：
  - R001-R007 逐项核实（外部 AI 9/12 真实；R004 已修；其余为论证/保护项）
  - 剩余 std::fs 30 处：infra-facade upload（同步公共 API，待专项 async 化）、mvc-facade view（同步渲染链）、pdf（第三方库接口要求，建议豁免）、cli（同步工具无 runtime，建议铁律 4 增豁免条款）
- 验证（真实输出）：addons-loader + 消费者 0 FAILED（231 passed）；workspace check 0 errors；clippy 0 警告；fmt 干净

## [Unreleased] - 2026-08-16（铁律 4 专项：infra upload 全链 async 化 + 门禁豁免裁定）

### Fixed

- **std::fs 债务专项（doc-debt DB-2026-08-16-06 RESOLVED）**：
  - infra-facade upload 全链 async 化：StorageEngine trait 5 引擎 set_upload_file/set_upload_file_by_real、from_uploaded_file/from_real_path、move_to/get_target_file/hash 链（compute_file_hash/hash/hash_name/md5/sha1）、image save/open/text/wrap_text/measure_text/load_font、debug_page load_source_snippet/with_source_snippet、JpegEncoder 改 Vec 缓冲 + tokio::fs::write
  - addons-loader 生产路径（上一轮）保持；core upload_parity 测试适配（await + tokio::test，移除嵌套 runtime）
  - 门禁 check-std-fs.py：**用户裁定豁免** pdf（umya 第三方库）/ cli（同步工具无 runtime）/ mvc view（引擎级 async 化单独排期，债务跟踪）
- 验证（真实输出）：std-fs 门禁 EXIT=0；clippy workspace all-targets 0 警告；infra 670+8+1、core 835、addons-loader 231 全部通过

## [Unreleased] - 2026-08-16（unsafe API 收紧）

### Fixed

- **MemPool unsafe fn 收紧（外部审查 HIGH 项，ADR-037）**：`MemPool::alloc_str`/`alloc_bytes` 从 safe fn 改为 **unsafe fn**（mem_pool.rs:61,70）——原 API 为 safe fn 却依赖"reset 前引用有效"的 unsafe 不变量，Safe Rust 调用方可触发 use-after-free 而无需 unsafe 块（unsound API）。trait Safety 文档强化（并发 reset 需调用方同步）。13 个 mem_pool 测试 + bumpalo-pool feature 462 测试通过。注：MemPool 生产 0 调用 + feature 默认关闭，无实际暴露

## [Unreleased] - 2026-08-15（幻影交付审计修复）

> 审计报告：`docs/audit/2026-08-15-幻影交付审计报告.md`

### Fixed

- **P1 死 crate 移除**：从 workspace members 移除 7 个零依赖方 crate（sz-rust-tracing/pdf/workflow/addons-operate/addons-erp/addons-forum/addons-im），`Cargo.toml` workspace members + dependencies 同步清理
- **P3 Cap facade 接线**：`main.rs:193` 调用 `Cap::init()` 替换直接构造，生产日志确认"Capability Registry 初始化完成（Cap facade 已接线）"
- **Cap 双实例缺陷修复（复核发现）**：`Cap::init()` 内部新建 registry 与 AppState 局部实例不互通（Cap::register 的能力业务 handler 不可见）。新增 `Cap::init_with(Arc<CapabilityRegistry>)`（capability/facade.rs），`main.rs` 改用 `Cap::init_with(capability_registry.clone())` 与 AppState 共享同一实例。capability crate 42 测试通过
- **P5 幻影测试删除**：删除 `tests/service_coverage_test.rs`（17 个 `#[ignore]` 占位测试，0 断言）+ `tests/mqtt_dispatch_test.rs` 中 2 个占位测试
- **P6 死代码删除**：移除 MockMqttPlugin::start/publish、check_permission、get_rbac、FileService::delete、create_multi_app_dispatcher 共 7 个零调用 pub 函数 + 2 个 unused import 警告
- **P7 文档过时修正**：更新审计报告中 rag/ai-facade 状态为"已接线"，更新 README.md 目录结构移除已删除 crate

### Added

- **AI facade 生产接线**：`main.rs:196-232` 构建 OpenAiProvider + ModelRouter + `Ai::init_default()`，`ai.rs:33` 用 `Ai::is_initialized()` 替换 `state.ai.is_none()`，启用 ai-facade `all-providers` feature
- **AI 集成测试**：`tests/ai_integration_test.rs`（6 个测试，验证初始化/降级/路由/错误处理）
- **幻影交付审计报告**：`docs/audit/2026-08-15-幻影交付审计报告.md`（8 维度 36 项审计）
- **服务层真实集成测试**：`tests/db_integration_test.rs` 补齐 16 个真实断言测试（替代已删除的 19 个占位）：ping_db/unbind/get_ota_version/update_status、MQTT handle_device_status|order|log（含负金额拒绝 A16）、dispatch 路由+安全、start_consumer 优雅退出、auth::me|logout、device::trigger_ota|status_report。真实 MySQL+PG 运行 `--ignored` → 26 passed; 0 failed

### Verified

- `cargo check` → 0 warning
- `cargo clippy` → 0 warning
- `cargo test -p sz-rust-sz300` → 72 passed, 0 failed, 0 ignored（43 lib + 6 ai + 6 ecommerce + 6 endpoint + 6 mqtt + 5 rag）
- 生产部署：健康端点 200，AI/Cap 初始化日志确认输出

### Kept（经决策保留）

- **P4 功能零调用**（6 项）：core::guard/event/cache/h2/multi_app + 5 存储引擎 — 全部保留，标注为「基础设施待接线」；其中 #23 存储引擎经复核**去 deprecated 并论证设计保留**（upload 引擎为文件路径模型 vs sz300 内存 bytes 流 API 不兼容；M-4 magic bytes 校验为 sz300 特有；URL 契约 `/uploads/YYYY/MM/DD` 需保持），FileService 设计定位已写入 file_service.rs:23-37
- **P3 #16 LogFacade / #17 DI 容器**：设计保留（sz300 采用 tracing 生态；DI 容器为插件体系前置能力）
- **P8 生产 MQTT Mock**：保持现状，标注为「待真实 broker 接入」（fail-safe 设计，无 broker 时应用可启动）

### Verified

- `cargo test -p sz-rust-sz300 --test db_integration_test -- --ignored --test-threads=1` → 26 passed, 0 failed（真实 MySQL 9.6 + PostgreSQL 18，含 16 个新增服务层/MQTT/控制器级测试）

## [Unreleased] - 2026-08-15（任务队列 gauntlet 验证：并发缺陷修复 + 并发/自愈测试）

### Fixed

- **任务队列并发缺陷（old-coder 验证发现）**：多 worker 并发领取时同一任务被执行 2 次（集成测试实测 calls=20≠10）。根因：v1 的"单条 `UPDATE ... WHERE id IN (SELECT ...)` 抢占"在 MySQL 默认 REPEATABLE READ 下，子查询为快照读且 InnoDB UPDATE 锁等待后不重新评估 WHERE（semi-consistent read 仅 READ COMMITTED 启用），乐观锁条件失效。修复：事务内 `SELECT ... FOR UPDATE SKIP LOCKED`（MySQL 8.0.1+，同 PostgreSQL 语义）——锁定阶段即跳过他人已锁行（`jobs.rs:claim_batch`）

### Added

- 集成测试 +2：`test_concurrent_workers_no_duplicate_execution`（双 worker 10 任务恰好执行 10 次）+ `test_stale_lease_reclaimed_and_rerun`（租约过期回收重跑）→ 集成测试共 7 例
- gauntlet 验证体系：`scripts/audit/jobs-gauntlet.sh`（8 层可复现入口）+ 验证报告 `docs/audit/2026-08-15-任务队列gauntlet验证报告.md`
- 验证结果（真实输出）：手动变异 4/4 杀死；工具变异 249（139 caught / 96 missed / 14 unviable）；jobs.rs 覆盖 line 75% / branch 80%；集成测试 7/7 ×2 次确定性通过

## [Unreleased] - 2026-08-15（可靠任务队列：持久化 Job 表 + 状态机 + 退避重试 + 死信）

### Added

- **可靠任务队列 `JobQueue`**（`sz-rust-orm-facade/src/jobs.rs`，ADR-036，来源: `cargo test -p sz-rust-orm-facade --lib jobs::` → 6 passed；`cargo test -p sz-rust-sz300 --test jobs_integration_test -- --ignored` → 5 passed，真实 MySQL 9.6）：
  - 任务数据化：`sz_jobs` 表（kind/payload/status/attempts/run_after/locked_until/last_error/dedupe_key），时间用 BIGINT 毫秒时间戳（UTC）
  - 状态机：pending（run_after 表达延迟与退避）/ running（locked_until 租约）/ succeeded / dead（可重放）
  - 原子领取：单条 `UPDATE ... WHERE id IN (SELECT ... LIMIT n)` 抢占，多实例 worker 安全，不依赖 SKIP LOCKED 方言
  - 退避重试：指数退避 + 随机抖动，Temporary/Permanent 错误分类，max_attempts 上限进死信
  - 幂等：`UNIQUE(kind, dedupe_key)`，重复入队返回已有任务 ID
  - 崩溃自愈：租约超时 running 任务自动回收重跑；死信 `retry_dead()` 人工重放
  - 观测：`queue_snapshot()`（pending/running/dead/最老 pending 等待秒数）
  - SQL 全参数化绑定 + 显式列投影（铁律合规）
- **sz300 接线**（`main.rs`）：JobQueue 初始化 + 幂等建表 + 示例 handler `order.expire_check`（订单超时未支付检查，演示延迟任务）+ worker 启动（复用优雅关闭信号）
- 文档：ADR-036 + 五维审查报告 `docs/audit/2026-08-15-可靠任务队列五维审查报告.md` + 本 CHANGELOG

## [Unreleased] - 2026-08-15（连接池调优接线：L2 配置 + 预热 + 动态扩缩容 + 池指标）

### Added

- **连接池 L2/L3 调优接线**（来源: `cargo test -p sz-rust-sz300 -j 2` → lib 43 passed / 0 failed，含新增 4 个 `db::tests`；clippy 零警告）：
  - `SqlxPoolConfig` 接线：`DB_POOL_MAX / DB_POOL_MIN / DB_POOL_ACQUIRE_TIMEOUT / DB_POOL_IDLE_TIMEOUT / DB_POOL_MAX_LIFETIME` 环境变量调优，未设置保持既有默认（20/10/30s/600s/1800s），非法值回退默认（`db.rs:26-47`）
  - SQLx 池补齐 `min_connections` / `idle_timeout` / `max_lifetime`，与 sz-orm 层参数同源对齐，两层池参数不再各设各的（`db.rs:56-65`、`db.rs:129-138`）
  - 连接池预热：sz-orm `prewarm(true)` + 启动时 `pool.prewarm()`，消除首次请求冷启动延迟（`db.rs:74`、`main.rs:200-207`）
  - PoolScaler 动态扩缩容：30s 周期后台任务，`acquire_failed_count` 作为超时率代理驱动决策，扩容上限对齐 SQLx 池容量（防两层池失配缺陷复发），仅实际调整时 resize（`main.rs:211-254`）
  - /metrics 实时输出池指标：`sz300_db_pool_active/idle/waiters/max/acquire_total/acquire_failed_total`（PG 池为 `sz300_pg_pool_*`），O(1) 原子读取；移除注册后无人更新的恒 0 gauge `sz300_active_connections`（`controllers/health.rs:112-146`）
- 五维审查报告：`docs/audit/2026-08-15-连接池调优接线五维审查报告.md`（5 维度无阻断项）

## [Unreleased] - 2026-08-15（sz300 首次真实运行 + metrics 鉴权接线）

### Fixed

- **sz300 首次真实运行冒烟测试修复**（commit `70ea91d`，来源: `cargo test -p sz-rust-sz300` → 134 passed / 0 failed）：
  - `merchant_service.rs`：商户列表投影引用不存在的 `merchant_name` 列 → 商户列表必崩缺陷，改为真实列并补全显式投影
  - `order_service.rs`：order_item 查询复用 2 参数数组但 SQL 仅 1 占位符 → order/info 必失败缺陷，改为独立单参数数组
  - device/merchant/order/product 四个 service 共 10 处 `SELECT *` → 显式列投影（铁律：禁 SELECT *）

### Security

- **/metrics 鉴权接线（T7）**：`MetricsAuthConfig`（此前仅定义未接线）现通过 `metrics_auth_middleware` 挂载到独立 `/metrics` 子路由（`router.rs:54` metrics_router()，route_layer 不污染业务 API）：
  - Bearer token（`SZ300_METRICS_BEARER_TOKEN`）+ IP 白名单（`SZ300_METRICS_ALLOWED_IPS`，支持 CIDR v4/v6）双机制，未授权返回 403
  - `SZ300_ENV=production` 启动强制校验：未配置任何鉴权 → 拒绝启动（fail-closed，`main.rs:243`）
  - 客户端真实 IP 注入：`into_make_service_with_connect_info`（`main.rs:262`）
  - 新增集成测试 `tests/metrics_auth_router_test.rs`（7 用例）+ CIDR 单测 3 用例（来源: `cargo test -p sz-rust-sz300` 真实输出）

## [Unreleased] - 2026-08-13（P0/P1/P2 能力完善）

> **文档降级声明**（2026-08-13 审计核实）：以下部分声称经独立验证存在跨仓库混淆或测试编译失败，已标注真实状态。
> 审计报告：`docs/audit/2026-08-13-文档已实现但生产零调用审计报告.md`

### Added

- **10 个能力缺口完成情况**（P0×4 + P1×4 + P2×2）：
  - P0-1 Capability Registry：PermissionChecker + tenant 隔离（38 tests ✅，来源: `cargo test -p sz-rust-capability`，开源仓库）
  - P0-2 SDD Agent：四阶段编排 + RAG 集成 + 铁律检查 + 16 Skill 映射（❌ **未完成**：`sz-rust-sdd-agent` 在开源版与企业版仓库 git 历史中均不存在，声称的 128 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P0-3 开源/企业分离：4 个企业 crate 分离 + 许可证合规 CI（✅ 来源: `check_license_compliance.py`）
  - P0-4 插件数据互通：EventBus + CrossQuery + 共享 Schema（13 tests ✅，来源: `cargo test -p sz-rust-core --test event_bus_test`，但 sz300 生产零调用）
  - P1-1 AI 迁移：3 个真实 TP6 项目案例 + MigrationReport（❌ **未完成**：`sz-rust-migration` 在开源版与企业版仓库 git 历史中均不存在，声称的 22 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P1-2 行业 RAG：25 术语 + 10 规则 + 7 模型模板（52 tests ✅，来源: `cargo test -p sz-rust-rag`，但生产零调用）
  - P1-3 插件模板库：12 个 Tera 模板 + SafetyValidator（289 tests ✅，来源: `cargo test -p sz-rust-cli`）
  - P1-4 MCP 扩展：8+ 新工具 + 白名单鉴权 + Capability 适配（70 tests ✅，来源: `cargo test -p sz-rust-mcp -p sz-rust-capability`）
  - P2-1 可视化画布：preview_app + 事件过滤 + HITL Abort + 冷启动脚本（❌ **未完成**：`sz-rust-visual` 在开源版与企业版仓库 git 历史中均不存在，声称的 56 tests 无法复现——2026-08-14 核验定性为虚构交付）
  - P2-2 插件市场：支付集成 + 订阅服务 + 5 项审核检查 + CLI/Web 同步（❌ **未完成**：`sz-rust-marketplace` 在开源版与企业版仓库 git 历史中均不存在，声称的 73 tests 无法复现——2026-08-14 核验定性为虚构交付）
- **6 个企业版业务插件**（119 tests ✅ 全部通过，来源: `cargo test -p sz-addons-market -p sz-addons-restaurant -p sz-addons-retail -p sz-plugin-sso-enterprise -p sz-plugin-audit -p sz-plugin-ha`，企业版仓库 `E:\www\rust\sz-rust-enterprise\`）
- **10 个 ADR**（ADR-026~035）+ **4 个五维审查报告**
- 关键数字（经审计核实）：开源仓库 462 tests ✅ + 企业版旧 4 crate 172 tests ✅ + 企业版新 6 插件 119 tests ✅ = 753 tests passed；`cargo test --workspace` 有 workspace 级依赖冲突待修复（来源: 各 crate `cargo test -p <crate>` 输出）

## [Unreleased] - 2026-08-12（W5/W6 质量交付）

### Added

- **W5/W6 质量交付**（覆盖 W1/W3/W5/W6/W7 弱点）：
  - TF 任务域：EnvGuard Drop guard 测试隔离工具（`packages/sz-rust-sz300/tests/common/mod.rs`），addons-forum/im 补充 Controller+Model 单元测试（23+23=46 个新测试）
  - PB 任务域：ORM 查询构建 + 缓存读写 2 组 10 个 criterion bench case，启动内存 RSS 测量脚本，性能基线 `w5_w6` 已保存
  - SA 任务域：22 条铁律自动化检查脚本（`scripts/check_iron_laws.py`），漏洞与许可证扫描脚本（`scripts/run_security_audit.py`），安全审计报告
  - E2E 任务域：ssh2 部署脚本（`scripts/e2e_deploy.js`，禁止 sshpass/killall），E2E 8 阶段编排脚本（`scripts/e2e_orchestrate.js`），E2E 验证报告
  - 新增文件 16 个，修改文件 10 个（来源: design.md §2.7 文件清单）
  - 关键数字：171 passed 0 failed / 45 bench case / 22 铁律全通过 / 8 阶段全执行（来源: 各任务域验证命令输出）

## [Unreleased] - 2026-08-12（P4-T2 工作流引擎）

### Added

- **工作流引擎**（`sz-rust-workflow`）：
  - 28 个错误码 `WF_001`～`WF_051`，按功能分区（定义加载/状态机/审批流/插件节点/实例管理/设计器 API）
  - 流程定义模型：`FlowDefinition`/`Node`/`NodeConfig`（6 种节点类型）/`Transition`/`CandidateStrategy`/`ApprovalStrategyType`/`FaultStrategy`
  - `DefinitionParser`：YAML/JSON 格式识别与解析，`flow_key` 命名规范校验
  - `DefinitionValidator`：结构完整性校验（必需字段/节点唯一性）、可达性校验（BFS）、终止性校验、插件节点引用校验
  - `DefaultGuardEvaluator`：纯函数表达式子集（字段访问/比较/逻辑运算），副作用检测，表达式长度限制
  - `StateMachineEngine`：迁移查找与触发，守卫条件求值，乐观锁原子迁移（持久化→内存→事件）
  - `ApprovalFlowEngine`：任务办理入口，审批策略检查（会签/或签），节点推进调度，死循环防护
  - `AndSignStrategy`/`OrSignStrategy`：会签（全同意完成）/或签（任一同意完成）
  - `DefaultCandidateResolver`：静态用户/动态表达式/能力调用三种候选人解析
  - `PluginNodeExecutor`：能力调用与超时，版本协商，容错策略（fail/skip/retry 指数退避）
  - `DefaultFaultStrategyHandler`：Fail 终止/Skip 跳过/Retry 指数退避重试
  - `SensitiveFieldRegistry`：动态敏感字段注册，递归脱敏（string/number/boolean/object/array），系统字段保护
  - `PluginUnloadWatcher`：插件卸载联动，在途实例标记不可用节点
  - `InstanceManager`：启动/挂起/恢复/终止/查询/历史轨迹，UUID v4 实例 ID
  - `HistoryRecorder`：迁移/节点/任务历史记录，敏感字段脱敏后持久化
  - `TaskManager`：任务生命周期管理（创建/失效/分页查询）
  - Repository trait 抽象：`DefinitionRepository`/`InstanceRepository`（乐观锁）/`TaskRepository`/`HistoryRepository`
  - `InMemoryRepository`：测试用默认实现（parking_lot::RwLock<HashMap>）
  - `WorkflowEventBus`：事件总线 trait + `InMemoryEventBus`（broadcast channel）+ `NoopEventBus`
  - `WorkflowMetrics`：4 个 Prometheus 指标（active instance/pending task/transition duration/plugin error）
  - `AuditLogger`：结构化审计日志（6 类操作），span 含 instance_id/node_id/transition
  - `VersionManager`：版本管理（列出/生效/弃用）
  - `DesignerApi`：定义校验（不持久化）/导入/导出，YAML/JSON 等价
  - `WorkflowDeps`/`WorkflowDepsBuilder`：依赖注入容器 + builder 模式
  - `WorkflowEngine`：统一门面，委托所有子模块
  - 131 个测试全部通过（121 单元 + 10 集成），clippy 零警告

## [Unreleased] - 2026-08-12（P4-T1 前端代码生成）

### Added

- **前端代码生成器**（`sz-rust-frontend-codegen`）：
  - 17 变体 `FrontendCodegenError` 错误类型 + `FE_CODEGEN_*` 错误码
  - 模型元信息结构（`FieldMetadata`/`RelationMetadata`/`ValidationRule`/`ModelMetadata`）
  - `ModelParser`：syn AST 解析 `#[derive(Model)]` 结构体，提取字段/关系/验证规则
  - Rust→TypeScript 类型映射（String→string, i32→number, Option<T>→T | null, Vec<T>→T[]）
  - `CodegenTemplateEngine`：Tera 1.20 封装 + 8 个自定义过滤器
  - `PathGuard`：路径穿越防护（拒绝 `..`/绝对路径/控制字符）
  - `UiAdapter`：Element Plus（el-*）/ Ant Design Vue（a-*）标签映射
  - `FileWriter`：原子文件写入（临时文件 + rename）+ 三种覆盖策略（Skip/Overwrite/Merge）
  - 21 个内置 Tera 模板（Vue 4 页面 + 测试骨架 + React 4 页面 + 路由 + 权限 + API + 类型）
  - `VueComponentGenerator`/`ReactComponentGenerator`：组件生成
  - `RouteGenerator`：Vue Router / React Router v6 路由生成
  - `PermissionGenerator`：路由守卫 + v-permission 指令 + usePermission 组合式函数
  - `ApiClientGenerator` + `OpenApiSchemaExtractor`：OpenAPI spec → API 客户端 + TS 类型
  - `CodegenService`：核心流水线编排（解析→适配→生成→写入→报告）
  - `GenerationReport`：结构化报告 + CLI 表格输出
  - 71 个测试全部通过（52 单元 + 19 集成）

- **CLI 集成**（`sz-rust-cli`）：
  - `sz-rust make:frontend` 子命令（--model/--framework/--ui/--output/--override/--with-tests 等 11 参数）
  - 275 个 CLI 测试全部通过

## [Previous] - 2026-08-12（P3-T2 插件市场 MVP）

### Added

- **插件市场**（`sz-rust-marketplace`）：
  - 28 变体 `MarketplaceError` 错误类型 + HTTP 状态码映射
  - 7 个领域模型（Plugin/PluginVersion/ReviewRecord/InstallRecord/Developer/MarketplaceManifest/PluginsLock）
  - `ManifestService`（JSON/TOML 清单解析）
  - `SignatureService`（Ed25519 签名 + SHA256 完;整性 + 公钥指纹）
  - `ObjectStore` trait + `LocalObjectStore` 实现
  - 5 个 Repository trait（Plugin/Version/Review/InstallRecord/Developer）
  - `LockFileManager`（plugins.lock 原子读写）
  - `PublishService` / `SearchService` / `InstallService` / `VersionService` / `ReviewService`
  - `MarketplaceService` 核心入口（依赖注入 + ArcSwap 配置热更新）
  - `Notifier` trait + `MemoryNotifier` 实现
  - Web 平台：13 个 RESTful API 路由 + 处理器 + 中间件（JWT/Trace/RateLimit）
  - 6 个 PostgreSQL 迁移脚本（含 append-only REVOKE UPDATE/DELETE）
  - 25 个单元测试全部通过

- **CLI 集成**（`sz-rust-cli`）：
  - `sz-rust plugin search/install/publish/uninstall/update/list/login` 七个子命令
  - 275 个 CLI 测试全部通过（含新增 plugin 命令测试）

- **CI/CD**：`.github/workflows/marketplace-ci.yml`（跨平台矩阵构建）

- **开发者文档**：`docs/developer/guide.md`（插件开发指南）

## [Unreleased] - 2026-08-11（P1-T4 完善现有业务插件）

### Added

- **CMS 插件**（`sz-rust-addons-cms`）：
  - 5 个 Capability 实现（`cms.search_article` / `cms.create_article` / `cms.publish_article` / `cms.manage_category` / `cms.manage_tag`）
  - `CmsPlugin` CapabilityHook 实现（register_capabilities + capability_names）
  - `manifest.json`（13 字段，5 能力，12 路由，2 事件）
  - `README.md`（7 章节，中文正文 + 英文 API）
  - 21 个测试（12 集成 + 9 capability lib 测试）

- **CRM 插件**（`sz-rust-addons-crm`）：
  - 7 个 Capability 实现（4 查询 + 3 写入，含 `crm.convert_lead` / `crm.update_deal_stage`）
  - `LeadController::convert` 改造为三步原子操作（lead→contact→deal + 手动回滚）
  - `DealController::update_stage` 新增 + pipeline 阶段名对齐（initial → requirement_confirmed → quoted → negotiating → won/lost）
  - `CrmPlugin` CapabilityHook 实现
  - `manifest.json`（13 字段，7 能力，17 路由，2 事件）
  - `README.md`（7 章节，含线索转化原子流程 + 商机阶段流转图）
  - 14 个新增测试（现有 21 个全部保留继续通过，总计 35 个）

- **电商插件**（`sz-rust-addons-ecommerce`）：
  - 6 个 Capability 实现（3 订单 + 3 购物车，含 `ecommerce.cancel_order` / `ecommerce.clear_cart`）
  - `CartController::add` 改造为同用户同商品数量累加
  - `OrderController` 正向流转方法（pay / ship / complete）+ 状态机常量
  - `EcommercePlugin` CapabilityHook 实现
  - `manifest.json`（13 字段，6 能力，17 路由，2 事件）
  - `README.md`（7 章节，含订单状态流转图 + 购物车累加说明）
  - 23 个新增测试（现有 21 个全部保留继续通过，总计 44 个）

- **跨插件集成测试**（`sz-rust-examples/tests/business_addons_integration.rs`）：
  - 12 个集成测试（6 跨插件能力注册 + 6 铁律合规 Send+Sync 断言）
  - 验证 18 个能力无命名冲突，各插件能力可独立调用

- **JSON Schema**（`schemas/plugin-manifest.schema.json`）：
  - 插件 manifest.json 校验 Schema（13 个必需字段）

### Changed

- CRM `DealController::pipeline` 阶段名对齐 spec 6.4（prospect → initial 等 6 阶段）
- CRM `LeadController::list` 和 `DealController::list` 增加 keyword 参数
- 3 个插件 `Cargo.toml` 新增 `sz-rust-capability` + `sz-rust-addons-loader` workspace 依赖
- workspace 根 `Cargo.toml` 新增 `sz-rust-addons-cms` / `sz-rust-addons-crm` / `sz-rust-addons-ecommerce` workspace 依赖声明
- `sz-rust-examples/Cargo.toml` dev-dependencies 新增 3 个插件 + capability + loader 依赖

### Verified

- 3 个插件 `cargo check` 通过（`cargo test -p sz-rust-addons-{cms,crm,ecommerce}` 编译成功）
- CMS 21 个测试全部通过（`cargo test -p sz-rust-addons-cms` 输出：`test result: ok. 21 passed`）
- CRM 35 个测试全部通过（`cargo test -p sz-rust-addons-crm` 输出：`test result: ok. 35 passed`）
- 电商 44 个测试全部通过（`cargo test -p sz-rust-addons-ecommerce` 输出：`test result: ok. 44 passed`）
- 跨插件集成测试 12 个全部通过（`cargo test -p sz-rust-examples --test business_addons_integration` 输出：`test result: ok. 12 passed`）
- 现有 42 个测试全部继续通过（CRM 21 + 电商 21 回归无破坏）
- 3 个插件源码无 `std::fs::` 调用（grep 验证）
- 3 个 Plugin struct + 3 个 State struct 均满足 `Send + Sync`（编译期断言通过）
- 18 个能力名全部以各自前缀开头（cms. / crm. / ecommerce.），无重名

## [1.1.0] - 2026-08-10（Admin Monitor API + AI 原生集成）

### 新增 — AI 原生集成 facade（`sz-rust-ai-facade`，workspace 第 34 成员）

- **LLM 统一抽象**：`LlmProvider` trait + OpenAI / Claude / Gemini 三家 Provider 完整 HTTP 调用实现
- **模型路由**：`ModelRouter` 基于 `ArcSwap` 无锁热替换路由表
- **故障切换**：`ProviderFailover` 状态机（Available → Degraded → Cooldown）+ `call_with_failover`
- **上下文裁剪**：`ContextTruncator` 自动裁剪超长上下文（保留 System 消息）
- **Token 计数**：`TokenCounter` 带缓存的无推理计数
- **Embedding**：`EmbeddingProvider` trait + OpenAI Embedding + `BatchChunker` 批量分片
- **向量存储**：`VectorStore` trait + `SimilarityMetric`（Cosine / Dot / L2）
- **RAG 管道**：`RagPipeline` 三段式（retrieve → assemble → generate）+ 引用溯源
- **Agent 引擎**：`Agent` + `AgentExecutor` 工具选择循环 + `TerminationPolicy` + 短期/长期记忆
- **MCP 桥接**：`McpToolBridge` 将 sz-rust-mcp 7 工具暴露为 Agent 可用工具（进程内调用，ADR-AI-01）
- **Facade 静态 API**：`Ai::chat / stream_chat / embed / rag / agent` + `OnceLock` 全局实例（对齐 PHP `think\facade\Ai`）
- **可观测性**：7 个 Prometheus 指标（ai_chat_total / ai_chat_duration / ai_stream_total / ai_embed_total / ai_rag_total / ai_agent_total / ai_tool_total）+ `tracing` 结构化日志
- **审计 HTTP 客户端**：`AuditHttpClient` + 令牌桶限流
- **配置扩展**：`infra-facade` 新增 `AiSection` + 8 个子配置结构体 + `load_optional_section()`
- **统一错误**：`AiError` 17 变体 + `error_code()` + `is_retryable()`
- **流式透传**：经 `http-facade::sse::SseEvent` 透传，不绕行新建 SSE 通道

### 新增 — Admin Monitor API（`admin` feature，默认关闭）

- **3 个管理端监控端点**（`/api/admin/*`，需 `admin` 角色）：
  - `GET /api/admin/server/info`：服务器信息（CPU/内存/磁盘/负载/进程启动时间/Rust版本/主机名）
  - `GET /api/admin/db/pool`：数据库连接池状态（active/idle/max/usage_percent）
  - `GET /api/admin/redis/info`：Redis 实例信息（版本/模式/连接数/内存/运行时间/角色）

- **`sz-rust-observability` 新增 `admin` 模块**：
  - `sysinfo_collector`：基于 `sysinfo` crate 0.32，跨平台（Windows `COMPUTERNAME` / Unix `hostname`）
  - `db_pool_collector`：`DbPoolStats` trait（trait object 适配，避免 observability crate 直连 sz-orm-core）
  - `redis_collector`：`RedisStats` trait + INFO 解析（无 Redis 连接时降级返回 `connected: false`）
  - `once_cell` 懒加载 rustc 版本检测

- **`sz-rust-sz300` 集成**：
  - `AppState` 新增 `db_pool_stats`（`Arc<dyn DbPoolStats>`）+ `redis_stats`（`Option<Arc<dyn RedisStats>>`）
  - `DbPoolStatsAdapter`：通过 `tokio::runtime::Handle::block_on` 桥接 async `Pool::status()`
  - `RedisStatsAdapter`：sync `redis::Client::get_connection()` + INFO 解析
  - `ADMIN_REDIS_URL` 环境变量可选：未配置时 `/api/admin/redis/info` 返回降级响应（200 + `connected: false`）

- **`RoleGuard` 中间件**：
  - `admin_role_guard`：路由级角色校验，叠加在全局 JWT 中间件之上
  - 401：未提供令牌 / 令牌无效；403：令牌有效但缺少 `admin` 角色
  - 通用 `role_guard(req, next, role)` 支持任意角色扩展

### 变更
- `sz-rust-core/src/orm.rs`：新增 `PoolStatus` re-export
- `sz-rust-sz300/Cargo.toml`：新增 `redis`（optional）+ `tower`/`http-body-util`（dev-dependencies）
- `sz-rust-sz300/src/main.rs`：`pool` 改为 `Arc::new(...)` 以同时注入 `db_pool` 和 `db_pool_stats`
- `sz-rust-sz300/src/services/auth_service.rs`：新增 `init_auth_test_only()` 测试辅助

### 测试
- 新增 20 个单元测试（observability 16 + role_guard 4），全部通过
- `cargo check -p sz-rust-sz300 --features admin` 与默认配置均编译通过

### 向后兼容
- `admin` feature 默认关闭，不影响现有部署
- 新增字段均为可选/有安全默认值

---

## [1.0.0] - 2026-08-10（GA 生产部署加固）

### 新增 — 10 项生产部署加固

- **敏感字段脱敏审计与补全**：
  - 审计脚本 `scripts/audit/sensitive-field-audit.js`：389 文件扫描，15 字段全部脱敏，0 EXPOSED
  - TokenPair/RenewedToken 自定义 Debug 脱敏（`auth-facade/src/refresh.rs`）
  - 回归测试：sz-rust-sz300 + sz-rust-cache-facade 共 6 个脱敏测试

- **Redis TLS 加密配置**：
  - `TlsConfig` + `TlsConfigError`（`auth-facade/src/redis_store.rs`）
  - `is_tls_enabled()` + `validate_production_tls()`：生产环境强制 TLS
  - cache-facade RedisConfig 新增 `enable_tls`/`tls_ca_cert_path` 字段
  - 11 个 TLS 测试

- **JWT 签名密钥轮换机制**：
  - `KeyRotation` 多密钥并存验证（`mvc-facade/src/controller.rs`）
  - 当前密钥签发 + 旧密钥 grace period 内验证
  - `spawn_rotation_task()` 定时轮换 + `fingerprint()` SHA256 指纹审计
  - Debug 脱敏：current/previous 输出 `[REDACTED]`
  - 12 个密钥轮换测试

- **限流中间件生产配置**：
  - `RateLimitProductionConfig`（capacity=2000, refill=1000/s，基于 v0.7.0 压测基线）
  - 排除路径含 /health、/metrics

- **熔断中间件生产配置**：
  - `CircuitBreakerProductionConfig`（error_threshold=0.5, cooldown=10s）
  - `validate()` 校验参数有效性

- **日志级别生产配置**：
  - `default_log_level()` 从 `info` 改为 `warn`
  - `LogConfig` + `validate_production()`：生产环境禁止 debug/trace
  - `ConfigError::LogLevelForbiddenInProduction`

- **metrics 端点访问控制**：
  - `MetricsAuthConfig`：Bearer token + IP 白名单双重校验
  - `validate_production()`：生产环境无鉴权 → 拒绝暴露
  - Debug 脱敏：bearer_token 输出 `[REDACTED]`

- **健康检查端点配置化**：
  - `HealthCheckConfig`：readiness 检查项可配置（db/redis/mqtt）
  - 未知检查项过滤 + 超时控制

- **优雅关闭超时配置化**：
  - `ShutdownConfig`：`shutdown_timeout`（默认 30s）+ `mqtt_timeout()`
  - MQTT 超时从硬编码 5s 改为可配置
  - 超时强制中止 + `MQTT_CONSUMER_FORCE_QUIT` 警告日志

- **K8s 探针模板化**：
  - Helm chart（`deploy/k8s/helm/sz300/`）：Chart.yaml + values.yaml + values-prod.yaml + templates/
  - RUST_LOG 从 `info,sz_rust_sz300=debug` 修正为 `warn,sz_rust_sz300=info`
  - 探针参数全量模板化（liveness/readiness/startup）
  - 部署指南 `docs/operations/k8s-deploy.md`

### 变更
- workspace.package.version 0.7.0 → 1.0.0
- 19 个内部依赖版本 0.7.0 → 1.0.0
- `packages/sz-rust-sz300/src/main.rs`：默认日志级别 + 优雅关闭超时配置化
- `deploy/k8s/sz300-deployment.yaml`：RUST_LOG 修正

### 测试
- 新增 73 个测试（10 个测试文件），全部通过
- GA 检查清单报告：`docs/audit/2026-08-10-v1.0-ga-checklist-report.md`

### 向后兼容
- v0.7.0 已发布 API 保持 semver 兼容
- 新增配置字段使用安全默认值，未配置时不破坏现有部署

---

## [0.7.0] - 2026-08-10

### 新增
- **crates.io 全量发布（29/29 crate）**：
  - workspace.package.version 0.6.7 → 0.7.0
  - 19 个 sz-rust-* 内部依赖 0.6.1 → 0.7.0
  - 拓扑排序发布：6 层依赖图（L0-L5），29 crate 全部发布成功
  - 审计日志 29 条，全部 verified

- **多并发压测（4 框架 × 3 路由 × 3 并发 = 36 组合）**：
  - 并发级别：32/128/256（C=64 复用 v0.6.7 历史基线）
  - 合计 48 数据点（36 新 + 12 历史基线）
  - 资源监控集成：sar（CPU/内存）+ dstat（网络），采集窗口 20s
  - 报告归档：docs/audit/2026-08-10-框架性能对比报告-v0.7.0.md

- **深度评估文档更新**：
  - 基线 v0.6.7 → v0.7.0
  - 代码行数 121,212 行（实测精确值，排除空行/注释）
  - 测试函数 4,610 个
  - ADR 21 个

### 变更
- sz-rust-sz300/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承
- sz-rust-examples/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承

### 向后兼容
- 无破坏性变更
- sz-orm-* 依赖保持 3.5.0 未修改
- sz-pay 兼容性：待验证

### 验证
- cargo check 通过（19.56s）
- 全量测试 0 failed
- crates.io 29/29 发布成功
- 性能回退校验：✅ 无回退

## [0.6.9] - 2026-08-10

### 变更
- **sz-orm 依赖升级 2.1.0 → 3.5.0**：
  - 17 个 sz-orm-* 子 crate 从 crates.io 2.1.0 升级到 3.5.0
  - `Cargo.toml` workspace 依赖版本号修改（行 179-195）
  - `Cargo.lock` 16 个 sz-orm 包全部更新到 3.5.0
  - 3 个 deprecation 警告：`sz_orm_query_builder::Query` 已废弃，推荐迁移到 `sz_orm_core::QueryBuilder<M>`

### 验证
- 本地 debug 编译通过（1m05s）
- 本地全量测试通过：5332 passed, 0 failed, 305 ignored
- sz-pay 兼容性验证通过（cargo check 21.11s）
- 服务器 release 编译通过（2m33s，Rust 1.97.1）
- 服务器 sz300-server 启动成功（数据库连接需额外配置）

## [0.6.8] - 2026-08-09

### 新增
- **Soak 自托管工具通用化（参数化 v2.0）**：
  - `config-defaults.sh` — 默认值定义模块（唯一允许 sz-rust 硬编码的位置）
  - `soak-runner.sh` 全参数化：`--project`/`--protected-port`/`--protected-process`/`--report-dir`/`--soak-ports`/`--restart-script`/`--cron-marker`
  - `soak-trigger.js` 全参数化：9 个参数分支，透传到 `soak-runner.sh`
  - `verify-6h-soak.sh` — 6h soak 验证脚本（4 项检查）
  - 参数校验函数 `validate_params()`：端口/目录/项目名格式校验
  - 部署到 `/www/rust/soak-toolkit/` 共享目录，支持多项目复用

- **完整性能压测（4 框架 × 3 路由 = 12 组合）**：
  - sz-rust(axum 0.8) / actix / axum / poem 同条件实测
  - 64 并发，10s 时长，wrk 4.1.0
  - 生成 `docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md` 合并报告（评估 + 5 框架 × 5 维度对比）
  - rocket 编译超时结论：`rocket-build-result.json`（status=timeout, >30 分钟）

- **cron 调度配置**：
  - 周日 00:00 UTC 6h soak（`--trigger cron`）
  - 每日 18:00 UTC nightly soak（`--trigger nightly`）

### 变更
- `process-guard.sh` 重构为全参数化（`--soak-dir`/`--protected-process`/`--restart-script`）
- `soak-archive.sh` 添加 `--report-dir` 参数
- `soak-cron-setup.sh` 全参数化（`--cron-marker`/`--soak-runner`/`--work-dir`）
- `run-benchmark.sh` 默认时长 60s→30s，添加汇总 JSON 和 Markdown 报告生成

### 压测结果摘要
| 框架 | /hello RPS | /json RPS | /user/42 RPS |
|------|-----------|----------|-------------|
| sz-rust | 157,526 | 148,321 | 144,267 |
| actix | 194,717 | 190,422 | 183,197 |
| axum | 139,682 | 135,968 | 136,012 |
| poem | 137,898 | 133,062 | 130,842 |

## [0.6.7] - 2026-08-08

### 新增
- **Redis 设备会话存储（P1 完善）**：
  - `RedisDeviceSessionStore` — 实现 `DeviceSessionStore` trait 全部 8 方法（HSET/HGETALL/HGET/HDEL/DEL）
  - `RedisConfig.key_prefix_sessions` 字段（默认 `sso:sessions`）
  - `create_redis_stores_with_devices()` — 三元组工厂方法，共享 ConnectionManager
  - 7 个 Redis 集成测试覆盖全部 8 方法

- **Token 降级 axum 端点（P3 完善）**：
  - `POST /sso/degrade/user` — 用户级降级
  - `POST /sso/degrade/device` — 设备级降级
  - `DELETE /sso/degrade/user/:user_id` — 清除用户降级
  - `DELETE /sso/degrade/device/:user_id/:device_id` — 清除设备降级
  - `GET /sso/degrade/:user_id` — 查询降级状态
  - 3 个降级集成测试（完整流程 / 设备级优先 / TTL 过期）

## [0.6.6] - 2026-08-08

### 新增
- **多设备会话管理（P1）**：
  - `DeviceInfo` / `DeviceSession` / `DeviceSessionConfig` / `DeviceSessionStore` trait / `MemoryDeviceSessionStore`
  - `SsoClaims.device_id` 字段，access/refresh token 均可绑定设备
  - `SsoService::login_with_device()` — 登录并注册设备会话，含 LRU 淘汰
  - `SsoService::list_devices()` / `revoke_device()` / `update_device_active()` / `cleanup_expired_devices()`
  - 设备撤销同时拉黑 access + refresh token 的 jti
  - `revoke_all` 联动清空设备会话，`validate` 联动更新设备活跃时间
  - axum 端点：`GET /sso/devices/:user_id`、`POST /sso/devices/revoke`、`POST /sso/devices/heartbeat`

- **Token 权限降级机制（P3）**：
  - `DegradationEntry` / `DegradationStore` trait / `MemoryDegradationStore`
  - `SsoService::degrade_user()` / `clear_degradation()` / `get_degradation()` — 用户级降级
  - `SsoService::degrade_device()` / `clear_device_degradation()` — 设备级降级
  - `validate` / `validate_with_renewal` 自动应用降级（子集过滤，不能提权）
  - 设备级降级优先于用户级，降级 TTL 自动过期
  - `revoke_all` / `revoke_device` 联动清除降级映射
  - `issue_with_roles()` — 签发携带 roles/permissions 的 token

- **SSO 跨域单点登录（P4）**：
  - `SsoTicket` / `TicketStore` trait / `MemoryTicketStore`
  - `SsoService::generate_ticket()` — 生成一次性 ticket（30 秒 TTL）
  - `SsoService::exchange_ticket()` — ticket 换取 TokenPair（一次性使用）
  - `SsoService::validate_ticket()` — 仅验证不消费

- **审计日志持久化（P5）**：
  - `AuditEvent` / `AuditEventType` 枚举（Login/Logout/Revoke/RevokeAll/RevokeDevice/Degrade/ClearDegradation/TicketGenerate/TicketExchange/RefreshRotated/ReuseDetected/DeviceRegistered/DeviceEvicted）
  - `AuditStore` trait / `MemoryAuditStore`
  - `SsoService::query_audit()` — 查询用户审计事件
  - login / revoke_all / degrade / ticket_generate / ticket_exchange 等关键操作自动记录审计

### 变更
- `Cargo.toml` workspace version 0.6.5 → 0.6.6（semver 兼容）
- `SsoClaims` 新增 `device_id` 字段（`Option<String>`，默认 None）
- `DeviceSession` 新增 `access_jti` 字段（撤销设备时同时拉黑 access token）
- `issue_with_device_and_jti` 签名增加 `roles` / `permissions` 参数
- `MockUserAuth` 测试用户 roles 更新为 `["admin", "user"]`，permissions 为 `["read", "write"]`

### 测试
- 190 个 lib 单元测试 + 21 个集成测试全部通过
- clippy 0 warning（auth-facade + middleware-facade）
- sz-pay 兼容性验证通过

## [0.6.5] - 2026-08-08

### 新增
- **Token 自动续期**（Access Token Auto-Renewal on Validation）：
  - `RenewalConfig` 结构体（`enabled` / `renewal_threshold` / `renewal_ratio` / `access_token_ttl`），可序列化/反序列化
  - `RenewalConfig::should_renew()` — 续期判定算法（threshold=0 特殊处理、ratio 边界）
  - `RenewedToken` 续期结果载体（`access_token` + `expires_at`）
  - `RefreshTokenIssuer::renew_access()` — 仅签发新 accessToken，不签发新 refreshToken，不递增版本号，不撤销旧 token
  - `SsoService::validate_with_renewal()` — 校验 + 自动续期，返回 `(SsoClaims, Option<RenewedToken>)`
  - `SsoService::with_renewal_config()` — 链式设置续期配置
  - axum `/sso/validate` 端点增强：响应新增 `new_access_token` / `new_access_expires_at` 字段（`null` 表示未续期）
  - 中间件续期响应头：`X-Renewed-Access-Token` / `X-Renewed-Expires-At`
  - `SsoMiddlewareConfig::local_with_renewal()` / `local_memory_with_renewal()` — 带续期配置的构造方法
  - 20 个单元测试 + 8 个集成测试 + 6 个边界测试 + 2 个性能基准

### 安全约束
- 续期不签发新 refreshToken（REQ-010）
- 续期不递增版本号（REQ-012）
- 续期不撤销旧 accessToken（REQ-011）
- 续期不绕过黑名单/版本/过期检查（REQ-021/022/023）

### 变更
- `Cargo.toml` workspace version 0.6.4 → 0.6.5（semver 兼容）
- `SsoService::validate()` 签名与行为不变（向后兼容）
- `SsoMiddlewareConfig::local()` / `local_memory()` 签名不变（向后兼容）
- `ValidateResponse` 新增 `Option` 字段，不续期时序列化为 `null`（向后兼容）

## [0.6.4] - 2026-08-08

### 新增
- **远程校验连接池优化**（`remote-validate` feature）：
  - `PoolConfig` 结构体（`pool_max_idle_per_host` / `pool_idle_timeout` / `tcp_keepalive` / `tcp_nodelay`），连接池参数可配置
  - `RemoteValidateConfig::new_checked()` — 返回 `Result`（失败安全，不 panic）
  - `RemoteValidateConfig::new_or_default()` — 失败回退默认 Client + warn 日志
  - `RemoteValidateConfig::from_client()` — 外部注入预配置 `reqwest::Client`
  - `RemoteValidateConfigBuilder` — 链式配置 builder 模式
  - `RefreshTokenError::InvalidConfig` 新变体
  - 10 个单元测试

### 变更
- `Cargo.toml` workspace version 0.6.3 → 0.6.4（semver 兼容，`new()` 签名不变）
- `RemoteValidateConfig::new()` 内部委托 `new_checked().expect()`（向后兼容）

## [0.6.3] - 2026-08-08

### 新增
- **Redis 存储后端**（feature gate `redis-store`）：
  - `auth-facade/src/redis_store.rs` — `RedisRefreshTokenStore`（GET / INCR 原子递增版本号）+ `RedisTokenBlacklist`（EXISTS / SETEX 带 TTL 黑名单）
  - `RedisConfig` 结构体（url / key 前缀 / 超时），Debug 自动脱敏 URL 密码
  - `create_redis_stores` 便捷工厂（共享 ConnectionManager）
  - 使用 `redis::aio::ConnectionManager`（自动重连 + 连接池复用）
  - fail-closed 策略：Redis 故障 → `ServiceUnavailable`，安全优先
  - 9 个单元测试 + 8 个集成测试（真实 Redis，含并发无丢失更新验证）
- **feature gate**：`redis-store`（Redis 存储后端）、`redis-cluster`（Redis 集群支持），默认零 Redis 依赖

### 变更
- `Cargo.toml` workspace version 0.6.2 → 0.6.3（Redis 后端为新增可选 feature，semver 兼容）
- `redis-gateway` feature 修正为 `dep:redis` 语法

## [0.6.2] - 2026-08-08

### 新增
- **SSO 单点登录 + Refresh Token 双 Token 机制**：
  - `auth-facade/src/refresh.rs` — `SsoJwtCodec`（HS256，复用 RustCrypto audited crate）、`SsoClaims`（JwtClaims 超集，新增 token_type/jti/ver）、`RefreshTokenIssuer`（签发+轮换）、`RefreshTokenVerifier`（6 级校验链）、`RefreshTokenRevoker`（单 Token 撤销 + 用户级撤销）、`TokenBlacklist`/`RefreshTokenStore` trait 抽象 + Memory 实现
  - `auth-facade/src/sso.rs` — `SsoService` 认证中心 + `UserAuthService` trait + axum HTTP 端点（login/refresh/revoke/validate/me，feature gate `axum`）
  - `middleware-facade/src/sso_middleware.rs` — `sso_middleware` 本地验签 + 白名单 + `AuthenticatedUser` + 远程校验（feature gate `remote-validate`）
  - 复用攻击检测：已黑名单 refresh token 再刷新 → `ReuseDetected` + 撤销用户所有 Token + tracing::warn! 告警
  - token_version 用户级撤销：`revoke_all` = `increment_version`（O(1)）
  - 22 个单元测试 + 12 个边界测试 + 6 个集成测试 + 3 个基准测试（encode 856ns / verify 2μs / rotate 7μs）
- **feature gate**：`axum`（SsoCenter HTTP 端点）、`remote-validate`（远程校验 + reqwest），默认零网络依赖

### 变更
- `Cargo.toml` workspace version 0.6.1 → 0.6.2（SSO 为新增功能，semver 兼容）
- `sz-rust-sz300/controllers/auth.rs` refresh 端点从空实现替换为调用 `RefreshTokenIssuer::rotate`

## [0.6.1] - 2026-08-07

### 新增
- **SSE (Server-Sent Events) 支持**：
  - `http-facade/src/sse.rs` — 基于 axum 0.8 的 SSE 实现，支持 `Event::data()`/`event()`/`retry()`/`id()`
  - `SseStream` — 将异步迭代器适配为 axum SSE 流
  - 4 个单元测试覆盖：事件构建、流适配、keep-alive、错误处理

### 变更
- `Cargo.toml` workspace version 0.6.0 → 0.6.1（SSE 为新增功能，semver 兼容）

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
