//! 鲜视达 SZ-300 后端服务入口

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use sz_rust_observability::MetricsRegistry;
use sz_rust_sz300::{config, db, router, services, state::AppState};
use tokio::signal;
use tokio::sync::watch;
use tracing_subscriber::{fmt, EnvFilter};

/// 示例任务：订单支付超时检查（`order.expire_check`）
///
/// 演示可靠任务队列的延迟任务用法：入队时指定 delay，到点后由 worker
/// 持久化领取并执行；进程重启不丢任务、失败自动退避重试。
/// 真实场景下此处应执行关单动作，示例仅告警。
struct OrderExpireCheckHandler {
    db: Arc<sz_rust_core::orm::Pool>,
}

#[async_trait::async_trait]
impl sz_rust_core::orm::jobs::TaskHandler for OrderExpireCheckHandler {
    async fn handle(&self, payload: &serde_json::Value) -> Result<(), sz_rust_core::orm::JobError> {
        use sz_rust_core::orm::JobError;
        let order_id = payload["order_id"].as_i64().unwrap_or(0);
        if order_id <= 0 {
            return Err(JobError::Permanent("order_id 缺失或非法".to_string()));
        }
        // DB 短暂故障属于临时错误，交给队列退避重试
        let mut conn = self
            .db
            .acquire()
            .await
            .map_err(|e| JobError::Temporary(e.to_string()))?;
        let rows = conn
            .query_with_params(
                "SELECT order_id, status FROM `order` WHERE order_id = ?",
                &[sz_rust_core::orm::Value::I64(order_id)],
            )
            .await
            .map_err(|e| JobError::Temporary(e.to_string()))?;
        match rows
            .first()
            .and_then(|r| r.get("status"))
            .and_then(sz_rust_core::orm::Value::as_i64)
        {
            Some(1) => {
                tracing::warn!(
                    "订单 {} 超时未支付（示例 handler：真实场景应执行关单）",
                    order_id
                );
            }
            Some(_) => {
                tracing::info!("订单 {} 已支付，跳过超时处理", order_id);
            }
            None => {
                tracing::warn!("订单 {} 不存在，跳过超时处理", order_id);
            }
        }
        Ok(())
    }
}

/// 构建 Reranker（环境变量驱动，默认 NoopReranker）
fn build_reranker() -> Arc<dyn sz_rust_ai_facade::rag::Reranker> {
    use sz_rust_ai_facade::rag::reranker::{CrossEncoderReranker, NoopReranker};
    if let Ok(key) = std::env::var("SZ300_RERANKER_API_KEY") {
        let endpoint = std::env::var("SZ300_RERANKER_ENDPOINT")
            .unwrap_or_else(|_| "https://api.cohere.ai/v1/rerank".to_string());
        tracing::info!("Reranker 使用 CrossEncoder（{}）", endpoint);
        Arc::new(CrossEncoderReranker::new(endpoint, key))
    } else {
        Arc::new(NoopReranker::new())
    }
}

/// 构建 Hybrid Retriever
async fn build_hybrid_retriever(
    embedding: Arc<dyn sz_rust_ai_facade::embedding::EmbeddingProvider>,
    vector_store: Arc<dyn sz_rust_ai_facade::embedding::VectorStore>,
) -> Arc<dyn sz_rust_ai_facade::rag::HybridRetrieverTrait> {
    use sz_rust_ai_facade::rag::bm25::Bm25Index;
    use sz_rust_ai_facade::rag::hybrid::HybridRetriever;
    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
    Arc::new(HybridRetriever::new(embedding, vector_store, bm25))
}

/// 构建 LocalEmbedding（环境变量驱动，默认 new_pseudo）
fn build_local_embedding(dim: usize) -> Arc<dyn sz_rust_ai_facade::embedding::EmbeddingProvider> {
    use sz_rust_ai_facade::embedding::LocalEmbedding;
    if let Ok(model_path) = std::env::var("SZ300_LOCAL_EMBEDDING_MODEL") {
        match LocalEmbedding::new(&model_path) {
            Ok(mut emb) => match emb.load_model() {
                Ok(()) => {
                    tracing::info!("LocalEmbedding 模型加载成功（{}）", model_path);
                    Arc::new(emb)
                }
                Err(e) => {
                    tracing::warn!(
                        "LocalEmbedding load_model 失败（{}），降级 new_pseudo: {}",
                        model_path,
                        e
                    );
                    Arc::new(LocalEmbedding::new_pseudo(dim))
                }
            },
            Err(e) => {
                tracing::warn!(
                    "LocalEmbedding 构造失败（{}），降级 new_pseudo: {}",
                    model_path,
                    e
                );
                Arc::new(LocalEmbedding::new_pseudo(dim))
            }
        }
    } else {
        Arc::new(LocalEmbedding::new_pseudo(dim))
    }
}

/// 构建 ToolRegistry（空 registry，后续可扩展）
fn build_tool_registry() -> sz_rust_ai_facade::agent::tool::ToolRegistry {
    sz_rust_ai_facade::agent::tool::ToolRegistry::new()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志 — EnvFilter + JSON 格式（生产环境友好）
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,sz_rust_sz300=info"));
    fmt().with_env_filter(filter).json().init();

    tracing::info!("鲜视达 SZ-300 后端服务启动");

    // 初始化上传目录
    services::file_service::FileService::init().await?;

    // 校验 JWT 配置（SZ_JWT_SECRET 未设置时 panic，阻止启动）
    // 安全铁律：认证密钥必须配置，否则所有受保护端点可被未授权访问
    sz_rust_core::controller::validate_jwt_config();

    // 加载配置（从环境变量读取，密钥不硬编码）
    let config = config::load_config()?;

    // 加载优雅关闭配置
    let shutdown_config = config::ShutdownConfig::from_env();
    tracing::info!(
        "优雅关闭配置: shutdown_timeout={:?}, mqtt_timeout={:?}, force_abort={}",
        shutdown_config.shutdown_timeout,
        shutdown_config.mqtt_timeout(),
        shutdown_config.force_abort_on_timeout
    );

    // 尝试加载框架统一 AppConfig（YAML 配置文件，可选）
    // 对齐 sz-rust-core 的 AppConfig::load_from_dir()，实现框架级配置统一
    match sz_rust_core::config::AppConfig::load_from_dir("config").await {
        Ok(framework_config) => {
            tracing::info!("框架统一 AppConfig 加载成功（config/ 目录）");
            // 框架配置可用于后续框架级功能（缓存、插件、日志等）
            let _ = framework_config;
        }
        Err(e) => {
            tracing::warn!(
                "框架统一 AppConfig 加载失败（非致命，使用环境变量配置）: {}",
                e
            );
        }
    }

    // 初始化 Addon 热加载器（条件编译：启用 hot-reload feature 时生效）
    // 扫描 addons/ 目录中的 .dll/.so/.dylib 文件，动态加载并调用 addon_init 入口
    // 安全说明：libloading 的 unsafe 已收敛于 sz_rust_core::runtime::hot_reload 内部
    #[cfg(feature = "hot-reload")]
    {
        use sz_rust_core::runtime::hot_reload::HotAddonLoader;
        let mut loader = HotAddonLoader::new();
        loader.add_scan_dir("addons");
        let results = loader.scan().await;
        let loaded: Vec<_> = results
            .iter()
            .filter_map(
                |(name, r)| {
                    if r.is_ok() {
                        Some(name.clone())
                    } else {
                        None
                    }
                },
            )
            .collect();
        let failed: Vec<_> = results
            .iter()
            .filter_map(|(name, r)| {
                if let Err(e) = r {
                    Some(format!("{}: {}", name, e))
                } else {
                    None
                }
            })
            .collect();
        if loaded.is_empty() {
            tracing::info!("Addon 热加载：addons/ 目录中未找到共享库（.dll/.so/.dylib）");
        } else {
            tracing::info!(
                "Addon 热加载已启用，已加载 {} 个插件: {:?}",
                loaded.len(),
                loaded
            );
        }
        if !failed.is_empty() {
            tracing::warn!(
                "Addon 热加载：{} 个插件加载失败: {:?}",
                failed.len(),
                failed
            );
        }
    }
    #[cfg(not(feature = "hot-reload"))]
    {
        tracing::info!("Addon 热加载未启用（如需动态插件加载，启用 hot-reload feature）");
    }

    // 初始化 OTLP 分布式追踪（条件编译：启用 otlp / otlp-http feature 时生效）
    // 配置通过 OTEL_* 环境变量传入（对齐 OpenTelemetry 规范）
    #[cfg(feature = "otlp")]
    {
        let otlp_config = sz_rust_observability::otlp::OtlpConfig::from_env();
        match sz_rust_observability::otlp::init_otlp_tracer(&otlp_config) {
            Ok(()) => tracing::info!("OTLP 分布式追踪已启用（gRPC，端口 4317）"),
            Err(e) => tracing::warn!("OTLP 初始化失败（非致命，继续运行）: {}", e),
        }
    }
    #[cfg(not(feature = "otlp"))]
    {
        tracing::info!("OTLP 未启用（如需分布式追踪，启用 otlp feature）");
    }

    // 初始化可观测性 — Prometheus 指标注册中心
    // 注：连接池指标（active/idle/waiters/max）由 /metrics handler 实时读取输出，
    // 不在注册中心预注册 gauge，避免注册后无人更新导致恒 0 误导。
    let metrics_registry = Arc::new(MetricsRegistry::new());
    metrics_registry.register_counter("sz300_requests_total", "Total HTTP requests received");
    metrics_registry.register_histogram(
        "sz300_request_duration_seconds",
        "HTTP request duration in seconds",
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
    );
    tracing::info!("可观测性模块初始化完成（Prometheus /metrics 端点已启用）");

    // 初始化 Capability Registry（能力注册表，用于 AI/MCP 能力发现与调用）
    // 同时初始化 Cap 全局 facade（OnceLock 单例），使 Cap::register/call/metrics 可用
    // 注意：必须用 init_with 注入同一实例——若用 Cap::init() 会创建第二个 registry，
    // Cap::register 注册的能力无法被业务 handler（AppState）访问（双实例缺陷，2026-08-15 修复）
    let capability_registry = Arc::new(sz_rust_capability::CapabilityRegistry::new());
    match sz_rust_capability::Cap::init_with(capability_registry.clone()) {
        Ok(()) => tracing::info!(
            "Capability Registry 初始化完成（Cap facade 已接线，与 AppState 共享实例）"
        ),
        Err(_) => tracing::warn!("Cap facade 已初始化（跳过重复初始化）"),
    }

    // 先加载 RAG 配置并构造 embedding + vector_store（供 AI facade 和 IndustryRag 共用）
    #[allow(clippy::type_complexity)]
    let rag_setup: Option<(
        Arc<sz_rust_rag::config::RagConfig>,
        Arc<dyn sz_rust_ai_facade::embedding::EmbeddingProvider>,
        Arc<dyn sz_rust_ai_facade::embedding::VectorStore>,
    )> = match sz_rust_rag::config::RagConfig::load(std::path::Path::new("config/rag.toml")).await {
        Ok(rag_config) => {
            let rag_config = Arc::new(rag_config);
            tracing::info!(
                "RAG 配置加载成功（embedding_model={}, topk={}, dim={}），开始初始化",
                rag_config.embedding_model,
                rag_config.default_topk,
                rag_config.vector_dimensions
            );

            let embedding = build_local_embedding(rag_config.vector_dimensions);

            #[cfg(feature = "qdrant")]
            let qdrant_store: Option<
                Arc<dyn sz_rust_ai_facade::embedding::VectorStore>,
            > = {
                if let Ok(qdrant_url) = std::env::var("SZ300_QDRANT_URL") {
                    let collection = std::env::var("SZ300_QDRANT_COLLECTION")
                        .unwrap_or_else(|_| "sz300_vectors".to_string());
                    let mut store =
                        sz_rust_vector_db::QdrantVectorStore::new(&qdrant_url, &collection);
                    if let Ok(key) = std::env::var("SZ300_QDRANT_API_KEY") {
                        store = store.with_api_key(&key);
                    }
                    match store.ensure_collection(rag_config.vector_dimensions).await {
                        Ok(()) => {
                            tracing::info!(
                                "RAG VectorStore 使用 Qdrant ({}/{})，dim={}",
                                qdrant_url,
                                collection,
                                rag_config.vector_dimensions
                            );
                            Some(Arc::new(store)
                                as Arc<dyn sz_rust_ai_facade::embedding::VectorStore>)
                        }
                        Err(e) => {
                            tracing::warn!("Qdrant 连接失败（{}），降级为 FileVectorStore", e);
                            None
                        }
                    }
                } else {
                    None
                }
            };
            #[cfg(not(feature = "qdrant"))]
            let qdrant_store: Option<
                Arc<dyn sz_rust_ai_facade::embedding::VectorStore>,
            > = None;

            let vector_store_path = std::path::Path::new("data/rag-vectors.json");
            let vector_store: Arc<dyn sz_rust_ai_facade::embedding::VectorStore> =
                if let Some(store) = qdrant_store {
                    store
                } else {
                    match sz_rust_ai_facade::embedding::FileVectorStore::load(vector_store_path)
                        .await
                    {
                        Ok(vs) => {
                            tracing::info!(
                                "RAG VectorStore 从 {} 加载成功",
                                vector_store_path.display()
                            );
                            Arc::new(vs) as Arc<dyn sz_rust_ai_facade::embedding::VectorStore>
                        }
                        Err(e) => {
                            tracing::warn!(
                                "RAG VectorStore 加载失败（{}），降级为纯内存模式: {}",
                                vector_store_path.display(),
                                e
                            );
                            Arc::new(sz_rust_ai_facade::embedding::FileVectorStore::new_in_memory())
                                as Arc<dyn sz_rust_ai_facade::embedding::VectorStore>
                        }
                    }
                };
            Some((rag_config, embedding, vector_store))
        }
        Err(e) => {
            tracing::info!(
                "RAG 未配置（config/rag.toml 加载失败: {}），ai::chat 将使用原始 prompt",
                e
            );
            None
        }
    };

    // 初始化 AI facade（可选，从环境变量读取 API Key）
    let ai = match std::env::var("SZ300_AI_API_KEY") {
        Ok(api_key) => {
            let base_url = std::env::var("SZ300_AI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let http_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|e| anyhow::anyhow!("构建 reqwest::Client 失败: {e}"))?;
            let audit_http = Arc::new(sz_rust_ai_facade::common::AuditHttpClient::new(
                http_client,
                sz_rust_ai_facade::common::RateLimitConfig::default(),
            ));
            let provider =
                sz_rust_ai_facade::llm::openai::OpenAiProvider::new(api_key, base_url, audit_http);
            let provider_ref: sz_rust_ai_facade::llm::provider::ProviderRef = Arc::new(provider);
            let mut routes = HashMap::new();
            routes.insert("gpt-4o-mini".to_string(), provider_ref.clone());
            routes.insert("gpt-4o".to_string(), provider_ref.clone());
            routes.insert("gpt-4.1-mini".to_string(), provider_ref.clone());
            let router =
                sz_rust_ai_facade::llm::ModelRouter::new(routes, "gpt-4o-mini".to_string());

            // 构造 RagPipeline + ToolRegistry（从 rag_setup 注入）
            let (embedding_opt, vector_store_opt, rag_pipeline, tools) = match &rag_setup {
                Some((_, emb, vs)) => {
                    let pipeline = sz_rust_ai_facade::rag::RagPipeline::new(
                        emb.clone(),
                        vs.clone(),
                        provider_ref.clone(),
                    )
                    .with_reranker(build_reranker());
                    let pipeline = if std::env::var("SZ300_HYBRID_ENABLED").as_deref() == Ok("1") {
                        pipeline.with_hybrid_retriever(
                            build_hybrid_retriever(emb.clone(), vs.clone()).await,
                        )
                    } else {
                        pipeline
                    };
                    let tools = if std::env::var("SZ300_AGENT_ENABLED").as_deref() == Ok("1") {
                        Some(Arc::new(build_tool_registry()))
                    } else {
                        None
                    };
                    (
                        Some(emb.clone()),
                        Some(vs.clone()),
                        Some(Arc::new(pipeline)),
                        tools,
                    )
                }
                None => (None, None, None, None),
            };

            match sz_rust_ai_facade::Ai::init_default(
                router,
                embedding_opt,
                vector_store_opt,
                rag_pipeline,
                tools,
            ) {
                Ok(()) => {
                    tracing::info!("AI facade 初始化完成（OpenAI Provider，默认模型 gpt-4o-mini，RagPipeline + Agent 已接线）");
                    Some(Arc::new(sz_rust_ai_facade::Ai))
                }
                Err(e) => {
                    tracing::warn!("AI facade 初始化失败（非致命）: {e}");
                    None
                }
            }
        }
        Err(_) => {
            tracing::info!(
                "AI facade 未配置（SZ300_AI_API_KEY 未设置，/api/v1/ai/chat 将返回降级响应）"
            );
            None
        }
    };

    // 初始化行业 RAG 知识库（IndustryRag，与 Ai facade 的 RagPipeline 并行共存）
    if let Some((rag_config, embedding, vector_store)) = &rag_setup {
        let knowledge_dir = std::path::Path::new(&rag_config.knowledge_dir);
        let init_result = (async {
            let term_store =
                sz_rust_rag::term::FileTermStore::new(&knowledge_dir.join("terms.json")).await?;
            let rule_store =
                sz_rust_rag::rule::FileRuleStore::new(&knowledge_dir.join("rules.json")).await?;
            let template_store = sz_rust_rag::template::FileTemplateStore::new(
                &knowledge_dir.join("templates.json"),
            )
            .await?;
            let metrics = Arc::new(sz_rust_rag::metrics::RagMetrics::register());
            sz_rust_rag::facade::IndustryRag::init(
                embedding.clone(),
                vector_store.clone(),
                rag_config.clone(),
                Arc::new(term_store),
                Arc::new(rule_store),
                Arc::new(template_store),
                metrics,
            )
        })
        .await;
        match init_result {
            Ok(()) => tracing::info!("RAG 行业知识库初始化完成"),
            Err(e) => tracing::warn!("RAG 初始化失败（非致命，ai::chat 将 fallback）: {}", e),
        }
    }

    // 初始化事件总线（用于业务事件发布/订阅，如 order.created）
    let event_bus = Arc::new(sz_rust_core::plugin::event_bus::InMemoryEventBus::new());
    tracing::info!("事件总线初始化完成（InMemoryEventBus）");

    // 初始化缓存 facade（可选，默认使用内存驱动）
    // 若需 Redis 驱动，设置 SZ300_REDIS_URL 环境变量
    let cache = {
        let cache = sz_rust_cache_facade::Cache::new();
        cache.register_default(sz_rust_cache_facade::MemoryCacheDriver::new());
        Some(Arc::new(cache))
    };
    tracing::info!("缓存 facade 初始化完成（MemoryCacheDriver）");

    // 初始化 SLO 监控器（Google SRE 推荐：1h/5m Page + 6h/30m Ticket 双窗口）
    let slo_monitor = Arc::new(sz_rust_observability::slo::SloMonitor::new(
        sz_rust_observability::slo::SloConfig::default(),
    ));
    tracing::info!("SLO 监控器初始化完成（target=99.9%, Page=1h/5m, Ticket=6h/30m）");

    // 初始化 ORM 钩子注册表（16 事件生命周期钩子，对齐 PHP think-orm Model 钩子）
    let hook_registry = Arc::new(sz_rust_core::hooks::HookRegistry::new());
    tracing::info!("ORM 钩子注册表初始化完成（16 事件）");

    // 初始化数据库连接池
    let pool = Arc::new(db::init_pool(&config).await?);
    // 连接池预热：启动时建立 min_idle 个连接放入空闲队列，
    // 消除首次请求冷启动延迟（sz-orm 原生 prewarm，失败自动降级懒加载）
    pool.prewarm().await;
    tracing::info!(
        "MySQL 连接池初始化完成（max_size={}, min_idle={}），预热已执行",
        pool.max_size(),
        pool.config().min_idle,
    );

    // 连接池动态扩缩容（PoolScaler 决策 + sz-orm Pool::resize 执行，L3 调优）
    // 关键约束：扩容上限对齐 SQLx 池 max_connections（pool.max_size()），
    // 避免两层池容量失配（历史缺陷：SQLx 默认 10 < sz-orm 20，第 11 个并发 acquire 超时）。
    // 仅当 scaler 实际做出扩缩容决策时才 resize，避免启动即缩容。
    {
        use sz_rust_core::orm::pool_scaler::PoolMetrics as ScalerMetrics;
        use sz_rust_core::orm::{PoolScaler, PoolScalerConfig};
        let scaler_config = PoolScalerConfig {
            // 扩容上限 = SQLx 池容量（DB_POOL_MAX 可调），缩容下限 = min_idle
            max_connections: pool.max_size() as usize,
            min_connections: pool.config().min_idle as usize,
            check_interval: Duration::from_secs(30),
            ..PoolScalerConfig::default()
        };
        let scaler = Arc::new(PoolScaler::new(scaler_config.clone()));
        let check_interval = scaler_config.check_interval;
        let pool_clone = pool.clone();
        let scaler_clone = scaler.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(check_interval);
            loop {
                interval.tick().await;
                let status = pool_clone.status().await;
                let orm_metrics = pool_clone.pool_metrics();
                let scaler_metrics = ScalerMetrics {
                    current_connections: (status.idle + status.active) as usize,
                    idle_connections: status.idle as usize,
                    // sz-orm 累计失败含超时/建连失败/池关闭/限流拒绝，作为超时率代理
                    timeout_count: orm_metrics.acquire_failed_count,
                    total_acquire: orm_metrics.acquire_count,
                };
                let target_before = scaler_clone.target_connections();
                scaler_clone.adjust(&scaler_metrics);
                let target = scaler_clone.target_connections();
                if target != target_before {
                    pool_clone.resize(target);
                    tracing::info!(
                        "连接池动态扩缩容: max_size {} -> {}（acquire_timeout_rate={:.3}, idle_rate={:.3}）",
                        pool_clone.max_size(),
                        target,
                        scaler_metrics.timeout_rate(),
                        scaler_metrics.idle_rate(),
                    );
                }
            }
        });
        tracing::info!(
            "连接池动态扩缩容已启用: 目标区间 [{}, {}]，检查间隔 {:?}",
            scaler_config.min_connections,
            scaler_config.max_connections,
            scaler_config.check_interval,
        );
    }
    let pg_pool = match config::pg_config() {
        Ok(pg_cfg) => match db::init_pg_pool(&pg_cfg).await {
            Ok(p) => {
                p.prewarm().await;
                tracing::info!("PostgreSQL 连接池初始化成功（含预热）");
                Some(Arc::new(p))
            }
            Err(e) => {
                tracing::warn!("PostgreSQL 连接池初始化失败（非致命）: {}", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("PostgreSQL 配置加载失败（非致命）: {}", e);
            None
        }
    };
    // 构造 Agent LongTermMemory（SZ300_AGENT_ENABLED=1 时）
    let long_term_memory: Option<Arc<sz_rust_ai_facade::agent::memory::FileLongTermMemoryStore>> =
        if std::env::var("SZ300_AGENT_ENABLED").as_deref() == Ok("1") {
            let store =
                sz_rust_ai_facade::agent::memory::FileLongTermMemoryStore::new("data/agent-memory");
            tracing::info!("Agent LongTermMemory 已启用（data/agent-memory）");
            Some(Arc::new(store))
        } else {
            None
        };
    let app_state = AppState {
        db_pool: pool.clone(),
        pg_pool,
        metrics_registry: metrics_registry.clone(),
        capability_registry: capability_registry.clone(),
        ai,
        event_bus,
        cache,
        slo_monitor,
        hook_registry,
        long_term_memory,
        #[cfg(feature = "admin")]
        db_pool_stats: Arc::new(
            sz_rust_sz300::state::DbPoolStatsAdapter::new(pool.clone())
        ) as Arc<dyn sz_rust_observability::admin::DbPoolStats>,
        #[cfg(feature = "admin")]
        redis_stats: std::env::var("ADMIN_REDIS_URL")
            .ok()
            .and_then(|url| match sz_rust_sz300::state::RedisStatsAdapter::from_url(&url) {
                Ok(adapter) => {
                    Some(Arc::new(adapter) as Arc<dyn sz_rust_observability::admin::RedisStats>)
                }
                Err(e) => {
                    tracing::warn!("Admin Redis 适配器初始化失败（非致命，/api/admin/redis/info 将返回降级响应）: {}", e);
                    None
                }
            }),
    };

    // 初始化 JWT 认证（传入数据库连接池用于密码验证）
    // JWT 密钥从环境变量 SZ300_JWT_SECRET 读取（生产安全要求）
    let jwt_secret = std::env::var("SZ300_JWT_SECRET")
        .expect("SZ300_JWT_SECRET 环境变量未设置 — 请在启动前设置 JWT 密钥");
    services::auth_service::init_auth(&jwt_secret, "sz300", 86400, app_state.db_pool.clone());

    // 初始化可靠任务队列（JobQueue）：持久化 Job 表 + worker 消费
    // 覆盖：任务数据化 / 状态机 / 原子领取 / 退避重试 / 幂等入队 / 死信重放 / 队列健康观测
    let job_queue = sz_rust_core::orm::JobQueue::new(pool.clone());
    job_queue.init_schema().await?;
    tracing::info!("可靠任务队列初始化完成（sz_jobs 表已就绪）");

    // 初始化 MQTT 消费者 — 带优雅退出信号
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // 可靠任务队列 worker：与 MQTT 复用同一优雅关闭信号
    {
        use sz_rust_core::orm::{JobQueueConfig, TaskHandler};
        let handlers: HashMap<String, Arc<dyn TaskHandler>> = HashMap::from([(
            "order.expire_check".to_string(),
            Arc::new(OrderExpireCheckHandler { db: pool.clone() }) as Arc<dyn TaskHandler>,
        )]);
        let queue_clone = job_queue.clone();
        let shutdown_rx_clone = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = queue_clone
                .run_worker(handlers, JobQueueConfig::default(), shutdown_rx_clone)
                .await
            {
                tracing::error!("可靠任务队列 worker 退出异常: {e}");
            }
        });
        tracing::info!("可靠任务队列 worker 已启动（batch=10, poll=1s, max_attempts=8）");
    }
    let app_state_clone = app_state.clone();
    let mut mqtt_handle = tokio::spawn(async move {
        services::mqtt_listener::MqttDispatcher::start_consumer(app_state_clone, shutdown_rx).await;
    });

    // 注册路由
    let app = router::create_router(app_state);

    // 校验 metrics 鉴权配置：生产环境（SZ300_ENV=production）必须配置
    // IP 白名单或 Bearer token，否则启动失败（fail-closed）
    let metrics_auth_config = config::MetricsAuthConfig::from_env();
    let env = std::env::var("SZ300_ENV").unwrap_or_else(|_| "development".to_string());
    metrics_auth_config
        .validate_production(&env)
        .map_err(|e| anyhow::anyhow!("metrics 鉴权配置校验失败（env={env}）: {e}"))?;
    tracing::info!(
        "metrics 鉴权配置: enabled={}, bearer_token={}, allowed_ips={:?}",
        metrics_auth_config.enabled,
        if metrics_auth_config.bearer_token.is_some() {
            "已配置"
        } else {
            "未配置"
        },
        metrics_auth_config.allowed_ips,
    );

    // 启动 HTTP 服务器
    let addr = format!("{}:{}", config.server.host, config.server.port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("监听地址: {}", addr);

    // into_make_service_with_connect_info：为 metrics IP 白名单提供真实客户端 IP
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let ctrl_c = async {
            signal::ctrl_c()
                .await
                .expect("failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to install signal handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }

        tracing::info!("收到关闭信号，正在优雅关闭...");
        // 通知 MQTT 消费者退出
        let _ = shutdown_tx.send(true);
        // 等待 MQTT 任务完成（可配置超时）
        let mqtt_timeout = shutdown_config.mqtt_timeout();
        match tokio::time::timeout(mqtt_timeout, &mut mqtt_handle).await {
            Ok(Ok(())) => {
                tracing::info!("MQTT 消费者已正常退出，HTTP 服务器关闭中...");
            }
            Ok(Err(e)) => {
                tracing::error!("MQTT 消费者退出异常: {e:?}，HTTP 服务器关闭中...");
            }
            Err(_) => {
                if shutdown_config.force_abort_on_timeout {
                    tracing::warn!(
                        "MQTT_CONSUMER_FORCE_QUIT: MQTT 消费者在 {:?} 内未退出，强制中止",
                        mqtt_timeout
                    );
                    mqtt_handle.abort();
                } else {
                    tracing::warn!(
                        "MQTT 消费者在 {:?} 内未退出，继续关闭 HTTP 服务器",
                        mqtt_timeout
                    );
                }
            }
        }
    })
    .await?;

    Ok(())
}
