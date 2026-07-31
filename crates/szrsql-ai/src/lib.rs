//! SzRSQL AI 引擎：自动 Embedding 生命周期 / NL2SQL / HNSW 向量索引 / RAG。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 模块
//!
//! - [`embedding`] — Phase 7b.2 自动 Embedding 生命周期
//!   - `HashingEmbedder` — 确定性本地嵌入模型（hashing trick）
//!   - `HnswIndex` — 分层可导航小世界图（Malkov & Yashunin 2016）
//!   - `EmbeddingLifecycle` — DDL 声明 → INSERT 自动嵌入 → HNSW 自动索引 → 搜索
//! - [`nl2sql`] — Phase 7b.3 NL2SQL 自然语言查询
//!   - `Nl2SqlEngine` — 基于规则的本地 NL2SQL 引擎（中英文混合）
//!   - 多阶段管道：文本预处理 → 意图分类 → 槽位填充 → SQL 生成
//!   - Spider 风格合成测试集验证准确率 >= 70%
//! - [`llm_cache`] — Phase 7b.4 LLM 缓存层
//!   - `LlmCache` — LRU 淘汰 + CDC 表级失效
//!   - 100000 条重复查询缓存命中率 >= 60%
//!   - CDC 事件触发缓存失效 → 下次查询重新生成
//! - [`rag`] — Phase 7b.5 RAG 集成
//!   - `RagEngine` — 检索增强生成（Embedding + NL2SQL + LLM 缓存）
//!   - 命名空间隔离 + 文档索引 + 过滤 + 引用追踪
//!   - `rag_ask('哪些商品需补货？', '库存低于安全库存', '零售助手')`
//! - [`mcp`] — Phase 7b.6 MCP Server
//!   - `McpServer` — JSON-RPC 2.0 over stdio，LLM 通过 MCP 协议调用 SzRSQL 工具
//!   - 工具：`list_tables` / `describe_table` / `execute_sql` / `get_stats`
//!   - `McpBackend` trait — 可注入自定义后端（含 `MockBackend` 用于测试）
//! - [`auto_index`] — Phase 7b.7 自治运维（索引推荐）
//!   - `AutoIndexEngine` — 自动分析慢查询 + 推荐索引 + 验证加速比
//!   - 100000 条查询负载 → 慢查询识别 → 索引推荐 → 创建索引 → 加速比 >= 2x
//! - [`auto_ops`] — Phase 7b.8 自治运维（异常检测 + 容量预测）
//!   - `AnomalyDetector` — 4 类异常检测（全表扫描/死锁/超时/高频错误）+ 4 级严重级别
//!   - `CapacityPredictor` — 线性回归 + R² 拟合优度 + 留一法交叉验证 MAPE
//!   - 异常召回率 >= 90%，容量预测误差 < 20%

#![allow(dead_code)]

pub mod auto_index;
pub mod auto_ops;
pub mod embedding;
pub mod index;
pub mod llm_cache;
pub mod mcp;
pub mod mcp_server;
pub mod nl2sql;
pub mod proactive_insights;
pub mod rag;

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }
}
