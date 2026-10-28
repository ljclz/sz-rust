// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! DebugCollector 请求级收集器（T031）
//!
//! 收集 SQL 查询、缓存命中/未命中、各阶段耗时。
//! 内存超限时截断 + 标注截断。

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// SQL 查询记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlRecord {
    /// SQL 文本
    pub sql: String,
    /// 参数（已脱敏）
    pub params: Vec<String>,
    /// 耗时
    pub elapsed_ms: f64,
    /// 影响行数
    pub rows: u64,
}

/// 缓存操作记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRecord {
    /// 缓存键
    pub key: String,
    /// 命中或未命中
    pub hit: bool,
    /// 操作类型
    pub op: CacheOp,
}

/// 缓存操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheOp {
    /// 读取
    Get,
    /// 写入
    Set,
    /// 删除
    Delete,
}

/// 阶段耗时记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseRecord {
    /// 阶段名
    pub name: String,
    /// 耗时（毫秒）
    pub elapsed_ms: f64,
    /// 开始时间偏移（毫秒，相对于请求开始）
    pub start_offset_ms: f64,
}

/// DebugCollector 收集结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSummary {
    /// SQL 查询列表
    pub sqls: Vec<SqlRecord>,
    /// 缓存操作列表
    pub caches: Vec<CacheRecord>,
    /// 阶段耗时列表
    pub phases: Vec<PhaseRecord>,
    /// 总 SQL 查询数（含截断）
    pub total_sqls: usize,
    /// 总缓存操作数（含截断）
    pub total_caches: usize,
    /// SQL 是否被截断
    pub sqls_truncated: bool,
    /// 缓存是否被截断
    pub caches_truncated: bool,
    /// 缓存命中率
    pub cache_hit_rate: f64,
    /// 总 SQL 耗时（毫秒）
    pub total_sql_ms: f64,
    /// 总请求耗时（毫秒）
    pub total_request_ms: f64,
}

/// DebugCollector 请求级收集器
pub struct DebugCollector {
    sqls: Vec<SqlRecord>,
    caches: Vec<CacheRecord>,
    phases: Vec<PhaseRecord>,
    start: Instant,
    max_sqls: usize,
    max_caches: usize,
    sqls_truncated: bool,
    caches_truncated: bool,
}

impl DebugCollector {
    /// 创建收集器，默认上限 200 SQL / 500 缓存
    pub fn new() -> Self {
        Self::with_limits(200, 500)
    }

    /// 创建收集器，指定上限
    pub fn with_limits(max_sqls: usize, max_caches: usize) -> Self {
        Self {
            sqls: Vec::new(),
            caches: Vec::new(),
            phases: Vec::new(),
            start: Instant::now(),
            max_sqls,
            max_caches,
            sqls_truncated: false,
            caches_truncated: false,
        }
    }

    /// 记时阶段开始
    pub fn start_phase(&self, name: &str) -> PhaseTimer {
        PhaseTimer {
            name: name.to_string(),
            start: Instant::now(),
            request_start: self.start,
        }
    }

    /// 记时阶段结束
    pub fn end_phase(&mut self, timer: PhaseTimer) {
        let elapsed_ms = timer.start.elapsed().as_secs_f64() * 1000.0;
        let start_offset_ms = timer
            .start
            .duration_since(timer.request_start)
            .as_secs_f64()
            * 1000.0;
        self.phases.push(PhaseRecord {
            name: timer.name,
            elapsed_ms,
            start_offset_ms,
        });
    }

    /// 记时 SQL 查询开始
    pub fn start_sql(&self) -> SqlTimer {
        SqlTimer {
            start: Instant::now(),
        }
    }

    /// 记时 SQL 查询结束
    pub fn end_sql(&mut self, timer: SqlTimer, sql: &str, params: Vec<String>, rows: u64) {
        if self.sqls.len() >= self.max_sqls {
            self.sqls_truncated = true;
            return;
        }
        let elapsed_ms = timer.start.elapsed().as_secs_f64() * 1000.0;
        self.sqls.push(SqlRecord {
            sql: sql.to_string(),
            params,
            elapsed_ms,
            rows,
        });
    }

    /// 记录缓存操作
    pub fn record_cache(&mut self, key: &str, hit: bool, op: CacheOp) {
        if self.caches.len() >= self.max_caches {
            self.caches_truncated = true;
            return;
        }
        self.caches.push(CacheRecord {
            key: key.to_string(),
            hit,
            op,
        });
    }

    /// 生成收集摘要
    pub fn summary(&self) -> DebugSummary {
        let total_sqls = self.sqls.len() + if self.sqls_truncated { 1 } else { 0 };
        let total_caches = self.caches.len() + if self.caches_truncated { 1 } else { 0 };

        let cache_hit_rate = if self.caches.is_empty() {
            0.0
        } else {
            let hits = self.caches.iter().filter(|c| c.hit).count();
            hits as f64 / self.caches.len() as f64
        };

        let total_sql_ms: f64 = self.sqls.iter().map(|s| s.elapsed_ms).sum();
        let total_request_ms = self.start.elapsed().as_secs_f64() * 1000.0;

        DebugSummary {
            sqls: self.sqls.clone(),
            caches: self.caches.clone(),
            phases: self.phases.clone(),
            total_sqls,
            total_caches,
            sqls_truncated: self.sqls_truncated,
            caches_truncated: self.caches_truncated,
            cache_hit_rate,
            total_sql_ms,
            total_request_ms,
        }
    }
}

impl Default for DebugCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// 阶段计时器
pub struct PhaseTimer {
    name: String,
    start: Instant,
    request_start: Instant,
}

/// SQL 计时器
pub struct SqlTimer {
    start: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_debug_collector_empty() {
        let collector = DebugCollector::new();
        let summary = collector.summary();
        assert!(summary.sqls.is_empty());
        assert!(summary.caches.is_empty());
        assert!(summary.phases.is_empty());
        assert_eq!(summary.cache_hit_rate, 0.0);
        assert!(!summary.sqls_truncated);
        assert!(!summary.caches_truncated);
    }

    #[test]
    fn test_record_sql() {
        let mut collector = DebugCollector::new();
        let timer = collector.start_sql();
        collector.end_sql(
            timer,
            "SELECT * FROM users WHERE id = $1",
            vec!["1".into()],
            1,
        );

        let summary = collector.summary();
        assert_eq!(summary.sqls.len(), 1);
        assert_eq!(summary.sqls[0].sql, "SELECT * FROM users WHERE id = $1");
        assert_eq!(summary.sqls[0].rows, 1);
        assert!(!summary.sqls_truncated);
    }

    #[test]
    fn test_record_cache_hit() {
        let mut collector = DebugCollector::new();
        collector.record_cache("user:1", true, CacheOp::Get);
        collector.record_cache("user:2", false, CacheOp::Get);

        let summary = collector.summary();
        assert_eq!(summary.caches.len(), 2);
        assert_eq!(summary.cache_hit_rate, 0.5);
    }

    #[test]
    fn test_cache_hit_rate_all_hits() {
        let mut collector = DebugCollector::new();
        collector.record_cache("a", true, CacheOp::Get);
        collector.record_cache("b", true, CacheOp::Get);
        collector.record_cache("c", true, CacheOp::Get);

        let summary = collector.summary();
        assert_eq!(summary.cache_hit_rate, 1.0);
    }

    #[test]
    fn test_phase_timing() {
        let mut collector = DebugCollector::new();
        let timer = collector.start_phase("db_query");
        std::thread::sleep(Duration::from_millis(10));
        collector.end_phase(timer);

        let summary = collector.summary();
        assert_eq!(summary.phases.len(), 1);
        assert_eq!(summary.phases[0].name, "db_query");
        assert!(
            summary.phases[0].elapsed_ms >= 10.0,
            "elapsed should be >= 10ms"
        );
    }

    #[test]
    fn test_sql_truncation() {
        let mut collector = DebugCollector::with_limits(3, 100);
        for i in 0..5 {
            let timer = collector.start_sql();
            collector.end_sql(timer, &format!("SELECT {i}"), vec![], 0);
        }

        let summary = collector.summary();
        assert_eq!(summary.sqls.len(), 3, "should keep only 3 SQLs");
        assert!(summary.sqls_truncated, "should be marked as truncated");
        assert_eq!(summary.total_sqls, 4, "3 kept + 1 truncated marker");
    }

    #[test]
    fn test_cache_truncation() {
        let mut collector = DebugCollector::with_limits(100, 2);
        for i in 0..5 {
            collector.record_cache(&format!("key:{i}"), true, CacheOp::Get);
        }

        let summary = collector.summary();
        assert_eq!(summary.caches.len(), 2);
        assert!(summary.caches_truncated);
        assert_eq!(summary.total_caches, 3);
    }

    #[test]
    fn test_total_sql_ms() {
        let mut collector = DebugCollector::new();
        let timer = collector.start_sql();
        std::thread::sleep(Duration::from_millis(5));
        collector.end_sql(timer, "SELECT 1", vec![], 1);
        let timer = collector.start_sql();
        std::thread::sleep(Duration::from_millis(5));
        collector.end_sql(timer, "SELECT 2", vec![], 1);

        let summary = collector.summary();
        assert!(
            summary.total_sql_ms >= 10.0,
            "total SQL time should be >= 10ms"
        );
    }

    #[test]
    fn test_cache_op_serde() {
        let s = serde_json::to_string(&CacheOp::Get).unwrap();
        assert_eq!(s, "\"get\"");
        let v: CacheOp = serde_json::from_str("\"set\"").unwrap();
        assert_eq!(v, CacheOp::Set);
    }

    #[test]
    fn test_debug_summary_serde() {
        let mut collector = DebugCollector::new();
        collector.record_cache("key", true, CacheOp::Get);
        let summary = collector.summary();
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"hit\":true"));
    }

    #[test]
    fn test_multiple_phases() {
        let mut collector = DebugCollector::new();

        let t1 = collector.start_phase("middleware");
        std::thread::sleep(Duration::from_millis(2));
        collector.end_phase(t1);

        let t2 = collector.start_phase("controller");
        std::thread::sleep(Duration::from_millis(3));
        collector.end_phase(t2);

        let t3 = collector.start_phase("view");
        std::thread::sleep(Duration::from_millis(1));
        collector.end_phase(t3);

        let summary = collector.summary();
        assert_eq!(summary.phases.len(), 3);
        assert_eq!(summary.phases[0].name, "middleware");
        assert_eq!(summary.phases[1].name, "controller");
        assert_eq!(summary.phases[2].name, "view");
        assert!(
            summary.phases[1].start_offset_ms >= 2.0,
            "controller should start after middleware"
        );
    }

    #[test]
    fn test_no_truncation_when_under_limit() {
        let mut collector = DebugCollector::with_limits(10, 10);
        let timer = collector.start_sql();
        collector.end_sql(timer, "SELECT 1", vec![], 0);

        let summary = collector.summary();
        assert!(!summary.sqls_truncated);
        assert!(!summary.caches_truncated);
    }
}
