# Dify 技术吸收文档：基于 sz-rust 现有代码的落地设计

> **编写日期**：2026-08-13  
> **性质**：技术吸收设计（非方向变更），供后续开发参照执行  
> **依据**：`docs/cases/industry-observation-summary.md`（Dify 章节）+ sz-rust 现有代码实测（2026-08-13）  
> **产品方案对应**：`docs/product-technical-plan.md`（P1-T4 / P2-T1 / P2-T3 / P3-T1 / P3-T2）

---

## 一、吸收原则

```
1. 不照搬 Dify 实现，按 sz-rust 现有代码实际吸收
   → 每个吸收点先标注"现有代码位置"，再设计增量

2. 吸收的是"产品能力形态"，不是"Dify 的具体代码"
   → 吸收 RAG 流水线的阶段划分，不抄它的 Python 实现

3. 所有新增 API 遵循 sz-rust 现有风格
   → 对齐现有 trait 命名（LlmProvider / Tool / VectorStore）
   → 对齐现有错误模型（AiError / AddonLoaderError）
   → 对齐现有并发模型（Send + Sync + 'static，parking_lot RwLock）

4. 每个吸收点独立可实施、可验收
   → 有明确的文件级改动清单和验收标准
```

---

## 二、吸收点总览（6 项）

| # | Dify 能力 | sz-rust 吸收项 | 对应产品方案 | 优先级 |
|---|----------|---------------|-------------|--------|
| D-1 | Knowledge Pipeline（摄取→清洗→分块→索引→检索测试） | RAG 流水线阶段补全 | P2-T3 | P1 |
| D-2 | Workflow Studio 画布（节点+连线+测试） | 画布运行时数据模型 + UX 参考 | P3-T1 | P2 |
| D-3 | Plugin Marketplace（模型/工具/数据源/MCP 统一市场） | Capability 统一注册 + 插件清单扩展 | P3-T2 | P1 |
| D-4 | 应用可发布为 MCP 工具 | 插件能力 → MCP 工具导出 | P1-T4 | P1 |
| D-5 | Agent 护栏（guardrails）+ 记忆管理 | AgentOptions 扩展 + 记忆分层 | P2-T1 | P1 |
| D-6 | 发布层监控（日志/反馈/延迟） | Agent 运行可观测性 | — | P2 |

---

## 三、D-1：RAG 知识管道补全（对齐 Dify Knowledge Pipeline）

### 3.1 Dify 的形态

```
Dify Knowledge Pipeline（标准五阶段）：
  数据摄取（文件/网页/云盘）→ 清洗（去重/格式统一）→
  分块（chunking）→ 索引（embedding + vector store）→ 检索测试
```

### 3.2 sz-rust 现有代码实际

```rust
// packages/sz-rust-ai-facade/src/rag/pipeline.rs —— 现有 3 阶段
pub struct RagPipeline { embedding, vector_store, llm, embedding_model, llm_model, metric }
impl RagPipeline {
    pub async fn retrieve(&self, query, topk) -> Result<Vec<VectorHit>, AiError>   // 阶段1：检索（embed query + vector top-k）
    pub async fn assemble(&self, hits, budget) -> Result<String, AiError>          // 阶段2：拼装（字符级截断）
    pub async fn generate(&self, hits, context, query) -> Result<RagResult, AiError> // 阶段3：生成
}
pub enum WarningCode { ContextTruncated, LowRecallScore, RerankerSkipped }        // RerankerSkipped 定义了但从未发出

// packages/sz-rust-ai-facade/src/embedding/batch.rs —— 分块是独立 helper
pub struct BatchChunker;
impl BatchChunker {
    pub fn chunk(texts: &[String], chunk_size: usize) -> Vec<Vec<String>>         // 简单定长分块
    pub async fn embed_chunks<P: EmbeddingProvider>(provider, texts, chunk_size, model) -> ...
}
```

**缺口**：无清洗阶段、无索引生命周期管理（upsert 由调用方做）、无检索测试工具、分块仅定长切割。

### 3.3 吸收设计

#### 新增 crate：`sz-rust-ai-facade/src/rag/knowledge.rs`（模块级，不新建 crate）

```rust
// 对齐 Dify 五阶段，补全为可管理知识管道

/// 知识文档（摄取阶段输入）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub doc_id: String,
    pub source: KnowledgeSource,        // File / Url / Api / Manual
    pub content: String,
    pub metadata: serde_json::Value,    // 行业、租户、标签…
    pub tenant_id: String,
}

/// 清洗配置（Dify 清洗阶段的子集）
#[derive(Debug, Clone)]
pub struct CleaningConfig {
    pub strip_html: bool,               // 去 HTML 标签
    pub dedupe_lines: bool,             // 去重行
    pub normalize_whitespace: bool,     // 归一化空白
    pub max_length: Option<usize>,      // 超长截断
    pub min_length: Option<usize>,      // 过短丢弃
}

/// 分块策略（对齐 Dify 的多种分块模式）
#[derive(Debug, Clone)]
pub enum ChunkStrategy {
    Fixed { size: usize, overlap: usize },          // 定长 + 重叠
    Paragraph { max_chars: usize },                  // 按段落（\n\n）
    Semantic { max_chars: usize },                   // 语义分块（LLM 辅助，可选）
}

/// 知识库管理器 —— 补全"摄取→索引→检索"全生命周期
pub struct KnowledgeBase {
    embedding: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    config: KnowledgeBaseConfig,
    // 可选：元数据索引（doc_id → chunks 映射，用于删除/更新）
    chunk_index: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl KnowledgeBase {
    pub fn new(embedding, vector_store, config) -> Self

    /// 摄取：清洗 → 分块 → embedding → upsert（Dify 摄取+索引阶段）
    pub async fn ingest(&self, docs: &[KnowledgeDocument]) -> Result<IngestReport, AiError>

    /// 删除文档（级联删除其所有 chunk）
    pub async fn delete_document(&self, doc_id: &str, tenant: &str) -> Result<(), AiError>

    /// 检索测试（Dify 的检索测试界面，输出可读报告）
    pub async fn retrieval_test(&self, query: &str, topk: usize, tenant: &str)
        -> Result<RetrievalTestReport, AiError>
}

pub struct IngestReport {
    pub docs_processed: usize,
    pub chunks_created: usize,
    pub chunks_skipped: usize,       // 清洗后过短被丢弃
    pub total_tokens: u32,
    pub warnings: Vec<String>,
}

pub struct RetrievalTestReport {
    pub query: String,
    pub hits: Vec<VectorHit>,
    pub hit_scores: Vec<f32>,
    pub avg_score: f32,
    pub min_score: f32,
    pub suggestions: Vec<String>,    // 如"召回分数偏低，建议提高 topk 或调整分块"
}
```

#### 补全 RagPipeline 本身

```rust
// rag/pipeline.rs 增量：
// 1. 增加 rerank 阶段（当前 RerankerSkipped 永远不发，要么实现要么如实报告）
impl RagPipeline {
    /// 带重排的检索（可选阶段：retrieve → rerank → assemble → generate）
    pub async fn retrieve_with_rerank(
        &self, query: &str, topk: usize, rerank_topk: usize,
    ) -> Result<Vec<VectorHit>, AiError> {
        // 初检 topk*2 → 若配置了重排器则重排取 rerank_topk
        // 未配置重排器：返回初检结果 + WarningCode::RerankerSkipped
    }
}

// 2. 新增 Reranker trait（对齐 Dify 的 RerankModel，默认无实现 = 跳过）
#[async_trait]
pub trait Reranker: Send + Sync {
    fn name(&self) -> &str;
    async fn rerank(&self, query: &str, hits: &[VectorHit], topk: usize)
        -> Result<Vec<VectorHit>, AiError>;
}
```

### 3.4 文件级改动清单

```
新增：
├── packages/sz-rust-ai-facade/src/rag/knowledge.rs   （KnowledgeBase + 清洗/分块/报告）
├── packages/sz-rust-ai-facade/src/rag/reranker.rs    （Reranker trait）
修改：
├── packages/sz-rust-ai-facade/src/rag/mod.rs         （re-export 新类型）
├── packages/sz-rust-ai-facade/src/rag/pipeline.rs    （retrieve_with_rerank）
新增测试：
├── packages/sz-rust-ai-facade/tests/knowledge_base_test.rs
```

### 3.5 验收标准

```
✅ 摄取 3 种来源文档（含 HTML）→ 清洗 → 分块 → 索引成功，IngestReport 字段正确
✅ 删除文档 → 该文档全部 chunk 从 vector store 删除
✅ retrieval_test 返回可读报告，avg_score 合理
✅ 无重排器时 retrieve_with_rerank 返回 RerankerSkipped 警告（修复当前"定义了从未发出"）
✅ 全部通过 cargo test + 铁律检查（tokio::fs、参数化、无 unsafe）
```

---

## 四、D-2：Workflow Studio 画布 —— 运行时数据模型（先于 UI）

### 4.1 Dify 的形态

```
Dify Workflow Studio：
  节点（LLM/知识检索/代码/条件/工具/HTTP请求…）+ 连线 + 运行测试
  图编排 + 节点级输入输出 + 变量系统
```

### 4.2 吸收策略：先做运行时数据模型，UI 是 P3-T1 的事

sz-rust 的画布不能一步到位。**先定义画布运行时的数据模型**（新 crate `sz-rust-workflow`），UI 后续基于该模型渲染。这保证画布 UI 与 SDD Agent 编排（A-2）共用同一套模型。

### 4.3 数据模型设计

```rust
// 新 crate：packages/sz-rust-workflow/src/lib.rs

/// 工作流节点类型（对齐 Dify 节点类型子集）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    Llm { model: String, system_prompt: String, temperature: Option<f32> },
    KnowledgeRetrieval { knowledge_base: String, topk: usize },
    ToolCall { capability: String },          // 对齐 Capability Registry
    Code { language: CodeLanguage, script: String },
    Condition { rules: Vec<ConditionRule> },
    HttpRequest { method: HttpMethod, url: String, headers: HashMap<String, String> },
    Start { variables: Vec<VariableDef> },
    End { output: String },
}

/// 工作流节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    pub kind: WorkflowNodeKind,
    pub inputs: HashMap<String, String>,   // 变量引用，如 "{{start.user_query}}"
    pub retry_policy: Option<RetryPolicy>,
}

/// 工作流边（连线）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,          // 节点 id
    pub to: String,
    pub condition: Option<String>,   // 条件边（条件节点输出分支）
}

/// 工作流定义（YAML 声明式，对齐 Dify 的 app.yaml 思路）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub name: String,
    pub description: String,
    pub version: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
}

/// 工作流运行时
pub struct WorkflowEngine {
    capabilities: Arc<CapabilityRegistry>,   // 节点执行能力来源（ToolCall 节点）
}

impl WorkflowEngine {
    pub fn new(capabilities: Arc<CapabilityRegistry>) -> Self

    /// 执行工作流（拓扑序执行，环检测）
    pub async fn execute(&self, def: &WorkflowDef, inputs: serde_json::Value)
        -> Result<WorkflowRunResult, WorkflowError>

    /// 验证工作流定义（节点引用、边引用、环检测、变量引用检查）
    pub fn validate(&self, def: &WorkflowDef) -> Result<(), Vec<WorkflowError>>
}

pub struct WorkflowRunResult {
    pub node_outputs: Vec<NodeOutput>,   // 每个节点的输出（画布测试面板展示用）
    pub final_output: serde_json::Value,
    pub total_duration_ms: u64,
}
```

### 4.4 与 SDD Agent 的关系

```
sz-rust-workflow（画布运行时）    sz-rust-sdd-agent（企业版编排）
├── 用户拖拽的"业务流"            ├── AI 生成的"开发流"
├── 运行期执行                     ├── 开发期执行
└── 共用 WorkflowNodeKind 词汇     └── tasks.md DAG → WorkflowDef 映射（A-2）
```

### 4.5 文件级改动清单

```
新增 crate：
├── packages/sz-rust-workflow/
│   ├── Cargo.toml
│   └── src/lib.rs（节点/边/定义/引擎/验证）
测试：
└── packages/sz-rust-workflow/tests/engine_test.rs
```

### 4.6 验收标准

```
✅ 定义 3 节点工作流（Start → LLM → End）YAML 加载并执行成功
✅ 环检测：循环依赖报 WorkflowError::CycleDetected
✅ 条件边：Condition 节点按规则选择分支
✅ 变量引用：{{start.x}} 在后续节点正确解析
✅ ToolCall 节点通过 CapabilityRegistry 调用插件能力
```

---

## 五、D-3：插件能力清单扩展（对齐 Dify Plugin Marketplace）

### 5.1 Dify 的形态

```
Dify Plugin Marketplace：模型提供商/工具/数据源/MCP 统一市场
每个插件声明：provider、模型列表、工具、凭据 schema
```

### 5.2 sz-rust 现有代码实际

```rust
// packages/sz-rust-addons-loader/src/manifest.rs —— 现有清单缺少能力声明
pub struct AddonManifest {
    pub name: String, pub title: String, pub identifier: String,
    pub icon: String, pub author: String, pub version: String,
    pub admin: String, pub status: i64,
    #[serde(skip)] pub addon_path: PathBuf,
}
// ❌ 无 capabilities / events / routes / dependencies / licenses 字段

// packages/sz-rust-addons-loader/src/capability_hook.rs —— 已有能力钩子机制
pub use capability_hook::{unregister_plugin_capabilities, validate_capability_naming, CapabilityHook};
// ✅ 插件能力注册/注销/命名校验已有雏形
```

### 5.3 吸收设计：AddonManifest 扩展

```rust
// manifest.rs 增量 —— 向后兼容（serde default），旧清单不受影响

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestCapability {
    pub name: String,             // "market.search_stall"
    pub description: String,
    #[serde(default)] pub tags: Vec<String>,
    #[serde(default)] pub requires_confirmation: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestEvent {
    #[serde(default)] pub publishes: Vec<String>,   // 发布的事件类型
    #[serde(default)] pub subscribes: Vec<String>,  // 订阅的事件类型
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestDependency {
    #[serde(default)] pub plugins: Vec<String>,             // 依赖的插件
    #[serde(default)] pub shared_schemas: Vec<String>,      // 依赖的共享表
}

impl AddonManifest {
    // 新增字段（serde default，向后兼容）
    #[serde(default)] pub capabilities: Vec<ManifestCapability>,
    #[serde(default)] pub events: ManifestEvent,
    #[serde(default)] pub dependencies: ManifestDependency,
    #[serde(default)] pub license: String,   // "apache-2.0" / "mit" / "commercial"
    #[serde(default)] pub sz_rust_version: String,  // 兼容的框架版本 ">=1.1.0"
}
```

### 5.4 与 CapabilityHook 的联动

```
插件注册流程（register 时）：
  AddonLoader::register()
    → 解析 manifest（含新字段）
    → AddonRegistry::upsert(manifest)
    → CapabilityHook::register_plugin_capabilities(
        name, &manifest.capabilities, registry)   // 把能力注册进 CapabilityRegistry

插件注销流程（unregister 时）：
  → unregister_plugin_capabilities(name)          // 已有
```

### 5.5 文件级改动清单

```
修改：
├── packages/sz-rust-addons-loader/src/manifest.rs      （新增 5 字段 + 3 结构体）
├── packages/sz-rust-addons-loader/src/capability_hook.rs（register 侧联动）
├── packages/sz-rust-addons-loader/src/loader.rs        （register() 内调用能力注册）
测试：
├── packages/sz-rust-addons-loader/tests/manifest_ext_test.rs
```

### 5.6 验收标准

```
✅ 旧格式 manifest（无新字段）解析不受影响（serde default 向后兼容）
✅ 新格式 manifest 的能力在插件注册后进入 CapabilityRegistry
✅ 插件注销时能力被移除
✅ validate_capability_naming 校验能力命名规范
```

---

## 六、D-4：插件能力发布为 MCP 工具（对齐 Dify 的 MCP 发布）

### 6.1 Dify 的形态

```
Dify 应用/工具可发布为 MCP 兼容工具，外部 Agent 通过 MCP 协议调用
```

### 6.2 sz-rust 现有代码实际

```rust
// packages/sz-rust-mcp/src/lib.rs —— 现有 21 个工具是静态注册的
pub fn tool_definitions() -> Vec<Value>     // 写死的一组工具
pub fn call_tool(name: &str, args: &Value) -> Result<String, McpError>
// ❌ 插件能力无法动态进入工具列表

// packages/sz-rust-ai-facade/src/mcp_bridge/mod.rs —— 单向：MCP → Agent 工具
pub struct McpToolBridge { allowed_tools: HashSet<String> }
impl McpToolBridge {
    pub async fn call(&self, name, args) -> Result<Value, AiError>   // 调 sz_rust_mcp::call_tool
    pub fn adapters(&self) -> Vec<McpToolAdapter>                    // 转成 Agent Tool
}
// ❌ 反向（Capability → MCP 工具导出）不存在
```

### 6.3 吸收设计：动态 MCP 工具注册

```rust
// sz-rust-mcp 增量：支持运行时注册动态工具

// 新文件：packages/sz-rust-mcp/src/dynamic.rs

/// 动态工具注册表 —— 插件能力通过这里暴露给 MCP 客户端
pub struct McpToolRegistry {
    tools: RwLock<HashMap<String, Box<dyn McpToolHandler>>>,
}

#[async_trait]
pub trait McpToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, args: Value) -> Result<Value, McpError>;
}

impl McpToolRegistry {
    pub fn new() -> Self
    pub fn register(&self, handler: Box<dyn McpToolHandler>)           // 插件激活时注册
    pub fn unregister(&self, name: &str)                                // 插件注销时移除
    pub fn list_definitions(&self) -> Vec<Value>                        // 并入 tool_definitions()
}

// 桥接：Capability → McpToolHandler 适配器
// 新文件：packages/sz-rust-mcp/src/capability_adapter.rs
pub struct CapabilityAdapter {
    name: String,
    description: String,
    schema: Value,
    registry: Arc<CapabilityRegistry>,   // 调用 cap.call()
}
impl McpToolHandler for CapabilityAdapter { ... }

// 使用流程：
// 1. 插件注册能力 → CapabilityRegistry
// 2. McpToolRegistry::register(CapabilityAdapter::new(cap))  // 插件可配置是否导出
// 3. MCP 客户端 tools/list 看到插件能力
// 4. 外部 Agent（如 Claude/Cursor）通过 MCP 调用插件能力
```

### 6.4 文件级改动清单

```
新增：
├── packages/sz-rust-mcp/src/dynamic.rs            （McpToolRegistry + McpToolHandler）
├── packages/sz-rust-mcp/src/capability_adapter.rs （Capability → MCP 适配器）
修改：
├── packages/sz-rust-mcp/src/lib.rs                （tool_definitions 并入动态工具）
└── packages/sz-rust-mcp/src/main.rs               （stdio 循环支持动态列表）
测试：
└── packages/sz-rust-mcp/tests/dynamic_tools_test.rs
```

### 6.5 验收标准

```
✅ 注册一个模拟插件能力 → tools/list 出现该工具 + inputSchema 正确
✅ 外部 MCP 客户端调用该工具 → 能力执行并返回结果
✅ 注销后工具从列表消失
✅ 与现有 21 个静态工具共存
```

---

## 七、D-5：Agent 护栏与记忆分层（对齐 Dify guardrails + 记忆）

### 7.1 Dify 的形态

```
Dify Agent 能力：
  guardrails（护栏：敏感内容过滤、工具权限、输出约束）
  记忆（会话记忆 + 长期记忆，可开关、可清空）
```

### 7.2 sz-rust 现有代码实际

```rust
// packages/sz-rust-ai-facade/src/agent/engine.rs —— AgentOptions 现状
pub struct AgentOptions {
    pub max_steps: Option<u32>,      // 默认 25
    pub max_tokens: Option<u32>,
    pub timeout: Option<Duration>,
    pub allow_tools: Vec<String>,    // 白名单（已有权限护栏雏形）
    pub tenant_id: String,
}

// packages/sz-rust-ai-facade/src/agent/memory.rs
pub struct ShortTermMemory { /* Vec<ChatMessage>, max_messages */ }   // 会话内
pub struct LongTermMemory { memory_id, agent_id, content, embedding, importance, created_at, tenant_id }
// ❌ LongTermMemory 只是数据结构，Agent::run 从未使用它
```

### 7.3 吸收设计

```rust
// agent/engine.rs 增量

/// Agent 护栏配置（对齐 Dify guardrails）
#[derive(Debug, Clone, Default)]
pub struct AgentGuardrails {
    /// 输出内容过滤器（如 ["PASSWORD", "SECRET_KEY"] 模式，命中即截断/打码）
    pub sensitive_patterns: Vec<String>,
    /// 工具调用前回调（可用于风控/审计/人工审批）
    pub tool_pre_hook: Option<Arc<dyn ToolGuardHook>>,
    /// 最大工具调用失败次数（连续失败 N 次强制终止）
    pub max_tool_failures: Option<u32>,
}

#[async_trait]
pub trait ToolGuardHook: Send + Sync {
    /// 返回 Ok(true) 放行；Ok(false) 拒绝；Err 终止 Agent
    async fn check(&self, tool_name: &str, args: &serde_json::Value) -> Result<bool, AiError>;
}

// AgentOptions 扩展（向后兼容：Default 不启用）
pub struct AgentOptions {
    // ...现有字段
    pub guardrails: Option<AgentGuardrails>,
    pub memory: Option<AgentMemoryConfig>,   // None = 无长期记忆
}

/// 长期记忆配置（对齐 Dify 记忆开关）
#[derive(Debug, Clone)]
pub struct AgentMemoryConfig {
    pub long_term: Arc<dyn LongTermMemoryStore>,   // 新 trait：持久化存储
    pub max_recall: usize,                          // 每次注入的记忆条数
    pub importance_threshold: f32,                  // 只回忆重要度以上的
}

/// 长期记忆存储 trait（对齐现有 LongTermMemory 结构）
#[async_trait]
pub trait LongTermMemoryStore: Send + Sync {
    async fn save(&self, mem: LongTermMemory) -> Result<(), AiError>;
    async fn recall(&self, agent_id: &str, query_embedding: &[f32], topk: usize)
        -> Result<Vec<LongTermMemory>, AiError>;
}

// Agent::run 增量逻辑：
// 1. 启动时：memory.recall() → 注入 system prompt（热记忆）
// 2. 循环中：每次工具失败计数，超限 → TerminateReason::Error
// 3. 结束前：将关键步骤（importance 打分）save() → 冷记忆
// 4. 输出前：guardrails.sensitive_patterns 扫描 final_answer，命中打码
```

### 7.4 文件级改动清单

```
修改：
├── packages/sz-rust-ai-facade/src/agent/engine.rs   （AgentGuardrails + 扩展 AgentOptions）
├── packages/sz-rust-ai-facade/src/agent/memory.rs   （LongTermMemoryStore trait）
├── packages/sz-rust-ai-facade/src/agent/mod.rs      （re-export）
测试：
└── packages/sz-rust-ai-facade/tests/agent_guardrails_test.rs
```

### 7.5 验收标准

```
✅ 配置敏感模式 → final_answer 含敏感词时被打码
✅ 配置 tool_pre_hook 拒绝 → 该工具调用被拦截，Agent 继续其他路径
✅ max_tool_failures 达到 → Agent 以 Error 终止（trace 可查）
✅ 启用长期记忆 → 下一次 run 能 recall 到之前的记忆
✅ 现有 AgentOptions::new(tenant_id) 调用不破坏（Default 兼容）
```

---

## 八、D-6：Agent 运行可观测性（对齐 Dify 发布层监控）

### 8.1 Dify 的形态

```
Dify 发布层：日志、反馈、延迟/用量监控（每个应用实例可观测）
```

### 8.2 sz-rust 现有代码实际

```rust
// packages/sz-rust-observability/src/lib.rs —— 已有 Prometheus 基础设施
pub struct MetricsRegistry { counters/gauges/histograms }
impl MetricsRegistry {
    pub fn register_counter(&self, name, help) -> Arc<Counter>
    pub fn register_histogram(&self, name, help, buckets) -> Arc<Histogram>
    pub fn render(&self) -> String    // Prometheus 文本格式
}
// packages/sz-rust-ai-facade/src/common/mod.rs —— AiMetrics 存在（未深究导出）

// AgentTrace 已有：steps / total_tokens / total_duration_ms / terminated_by
```

### 8.3 吸收设计：Agent 指标埋点

```rust
// agent/engine.rs 增量：Agent::run 内埋点（MetricRegistry 可选注入）

pub struct AgentMetrics {
    pub runs_total: Arc<Counter>,          // sz_ai_agent_runs_total{agent, tenant}
    pub runs_failed: Arc<Counter>,         // sz_ai_agent_runs_failed_total
    pub steps_histogram: Arc<Histogram>,   // sz_ai_agent_steps
    pub tokens_histogram: Arc<Histogram>,  // sz_ai_agent_tokens
    pub duration_histogram: Arc<Histogram},// sz_ai_agent_duration_ms
    pub tool_calls_total: Arc<Counter>,    // sz_ai_agent_tool_calls_total{tool}
}

impl AgentMetrics {
    pub fn register(registry: &MetricsRegistry) -> Self
    pub fn record_run(&self, agent: &str, tenant: &str, result: &AgentResult, failed: bool)
}

// Agent::run 增量：
// 1. 开始时 runs_total.inc()
// 2. 每次工具调用 tool_calls_total.inc()（label: tool 名）
// 3. 结束时 record_run：steps/tokens/duration 进 histogram，失败 runs_failed
```

### 8.4 文件级改动清单

```
修改：
├── packages/sz-rust-ai-facade/src/agent/engine.rs   （AgentMetrics 埋点）
├── packages/sz-rust-ai-facade/Cargo.toml            （依赖 sz-rust-observability，已有）
测试：
└── packages/sz-rust-ai-facade/tests/agent_metrics_test.rs
```

### 8.5 验收标准

```
✅ 运行 3 次 Agent（1 次失败）→ render() 输出含 sz_ai_agent_runs_total 3、runs_failed 1
✅ 工具调用次数按 tool 标签正确聚合
✅ 指标不影响 Agent 正常执行（无 panic）
```

---

## 九、实施顺序与依赖

```
D-1 RAG 知识管道（P1）          ← 无依赖，先行
  └── 依赖：BatchChunker 已有、VectorStore 已有

D-3 插件清单扩展（P1）          ← 无依赖，与 D-1 并行
  └── 依赖：AddonManifest 已有、CapabilityHook 已有

D-4 MCP 能力导出（P1）          ← 依赖 D-3（能力清单先有）
  └── 依赖：sz-rust-mcp 已有、CapabilityRegistry（产品方案 P0-1，先行完成）

D-5 Agent 护栏与记忆（P1）      ← 无依赖，与 D-3 并行
  └── 依赖：AgentOptions 已有、LongTermMemory 已有

D-6 Agent 可观测性（P2）        ← 依赖 D-5（复用 AgentOptions 扩展）
  └── 依赖：MetricsRegistry 已有

D-2 工作流运行时（P2）          ← 依赖 D-4（ToolCall 节点调 Capability）
  └── 依赖：CapabilityRegistry、D-3 能力清单
```

```
建议批次：
  批次 1（P1，2-3 周）：D-1 + D-3（并行，无相互依赖）
  批次 2（P1，2-3 周）：D-4 + D-5（并行）
  批次 3（P2，2-3 周）：D-6 + D-2
```

---

## 十、与产品方案的衔接

| 吸收项 | 产品方案任务 | 增量说明 |
|--------|-------------|---------|
| D-1 | P2-T3 行业 RAG 知识库 | 知识管道是行业 RAG 的基础设施 |
| D-2 | P3-T1 可视化画布 | 画布运行时先于 UI，两者共用 WorkflowDef |
| D-3 | P3-T2 插件市场 | 插件清单是市场的基础数据格式 |
| D-4 | P1-T4 MCP 工具扩展 | 从"框架工具"扩展到"插件能力" |
| D-5 | P2-T1 SDD Agent | 护栏与记忆是 SDD Agent 的安全底座 |
| D-6 | （观察汇总 A-6） | Agent 可观测性支撑画布测试面板 |

---

## 十一、更新记录

| 日期 | 变更 |
|------|------|
| 2026-08-13 | 初始版本：6 个吸收点，全部对齐现有代码实际 API |
