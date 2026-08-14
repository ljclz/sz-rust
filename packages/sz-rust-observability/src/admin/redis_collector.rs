//! Redis 状态采集器
//!
//! 提供 Redis 服务器实时状态采集能力，供 `GET /api/admin/redis/info` 端点使用。
//!
//! ## 数据结构（对齐 FssAdmin `RedisMonitorService::getFullInfo()`）
//!
//! ```json
//! {
//!   "connected": true,
//!   "uptime_in_seconds": 86400,
//!   "uptime_in_days": 1,
//!   "connected_clients": 12,
//!   "used_memory": "1.20M",
//!   "variable": {
//!     "used_memory": 1258291,
//!     "used_memory_peak": 2097152,
//!     "used_memory_rss": 3000000,
//!     "mem_fragmentation_ratio": 2.38,
//!     "keyspace_hits": 5000,
//!     "keyspace_misses": 100,
//!     "expired_keys": 50,
//!     "evicted_keys": 0,
//!     "instantaneous_ops_per_sec": 120,
//!     "instantaneous_input_kbps": 5.2,
//!     "instantaneous_output_kbps": 8.7,
//!     "total_commands_processed": 1000000,
//!     "redis_version": "7.2.3",
//!     "redis_mode": "standalone",
//!     "os": "Linux 5.15.0",
//!     "arch_bits": 64,
//!     "mem_allocator": "jemalloc-5.3.0",
//!     "role": "master",
//!     "tcp_port": 6379,
//!     "aof_enabled": 1,
//!     "rdb_changes_since_last_save": 0,
//!     "total_connections_received": 500
//!   }
//! }
//! ```
//!
//! ## 降级策略
//!
//! 当 Redis 不可达时返回 `Err(RedisCollectError)`，调用方应返回 HTTP 503。

use serde::Serialize;
use std::fmt;

/// Redis 实时状态信息（`GET /api/admin/redis/info` 响应体 data 字段）
///
/// 对齐 FssAdmin `RedisMonitorService::getFullInfo()` 的扁平 + variable 嵌套结构。
#[derive(Debug, Clone, Default, Serialize)]
pub struct RedisInfo {
    /// 是否可连通（PING 探活结果）
    pub connected: bool,
    /// 运行时长（秒）
    pub uptime_in_seconds: u64,
    /// 运行时长（天，由 uptime_in_seconds / 86400 计算）
    pub uptime_in_days: u64,
    /// 当前客户端连接数
    pub connected_clients: u64,
    /// 已用内存（人类可读字符串，如 "1.20M"）
    pub used_memory: String,
    /// 详细指标集合（内存 / 命中率 / 持久化 / CPU / 命令统计等）
    pub variable: RedisVariable,
}

/// Redis 详细指标（`RedisInfo.variable` 字段）
///
/// 对齐 FssAdmin `variable` 对象，包含所有核心运维指标。
#[derive(Debug, Clone, Default, Serialize)]
pub struct RedisVariable {
    // ---- 内存指标（原始字节数）----
    /// 已用内存（bytes）
    pub used_memory: u64,
    /// 内存峰值（bytes）
    pub used_memory_peak: u64,
    /// RSS 内存（bytes，操作系统实际分配）
    pub used_memory_rss: u64,
    /// 内存碎片率（used_memory_rss / used_memory）
    pub mem_fragmentation_ratio: f64,

    // ---- 缓存效率指标 ----
    /// 缓存命中次数
    pub keyspace_hits: u64,
    /// 缓存未命中次数
    pub keyspace_misses: u64,
    /// 因过期被删除的 key 数量
    pub expired_keys: u64,
    /// 因 maxmemory 策略被驱逐的 key 数量
    pub evicted_keys: u64,

    // ---- 性能指标 ----
    /// 瞬时 QPS（ops/sec）
    pub instantaneous_ops_per_sec: u64,
    /// 瞬时网络入站带宽（KB/s）
    pub instantaneous_input_kbps: f64,
    /// 瞬时网络出站带宽（KB/s）
    pub instantaneous_output_kbps: f64,
    /// 累计处理命令数
    pub total_commands_processed: u64,

    // ---- 服务器信息 ----
    /// Redis 版本（如 "7.2.3"）
    pub redis_version: String,
    /// 运行模式（standalone / cluster / sentinel）
    pub redis_mode: String,
    /// 操作系统信息
    pub os: String,
    /// 架构位数（32 / 64）
    pub arch_bits: u64,
    /// 内存分配器（如 "jemalloc-5.3.0"）
    pub mem_allocator: String,
    /// 主从角色（master / slave）
    pub role: String,
    /// 监听端口
    pub tcp_port: u64,
    /// AOF 持久化是否启用（0 / 1）
    pub aof_enabled: u64,
    /// 距离上次 RDB 保存的变更次数
    pub rdb_changes_since_last_save: u64,
    /// 累计接收连接数
    pub total_connections_received: u64,
}

/// Redis 采集错误
#[derive(Debug, Clone)]
pub struct RedisCollectError {
    /// 错误描述
    pub message: String,
}

impl fmt::Display for RedisCollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Redis collect error: {}", self.message)
    }
}

impl std::error::Error for RedisCollectError {}

/// Redis 状态采集 trait
///
/// 由应用层实现，将具体的 Redis 客户端适配为采集器可理解的接口。
pub trait RedisStats: Send + Sync {
    /// 采集当前 Redis 状态
    fn info(&self) -> Result<RedisInfo, RedisCollectError>;
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 解析 Redis INFO 命令输出为键值对
    fn parse_info_output(raw: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        map
    }

    /// 从 INFO 解析结果中提取 [`RedisInfo`]
    fn build_redis_info(connected: bool, map: &HashMap<String, String>) -> RedisInfo {
        fn parse_u64(map: &HashMap<String, String>, key: &str) -> u64 {
            map.get(key)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        }
        fn parse_f64(map: &HashMap<String, String>, key: &str) -> f64 {
            map.get(key)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        }

        let uptime = parse_u64(map, "uptime_in_seconds");
        let used_mem = parse_u64(map, "used_memory");
        let peak_mem = parse_u64(map, "used_memory_peak");
        let rss_mem = parse_u64(map, "used_memory_rss");

        let variable = RedisVariable {
            used_memory: used_mem,
            used_memory_peak: peak_mem,
            used_memory_rss: rss_mem,
            mem_fragmentation_ratio: if used_mem > 0 {
                rss_mem as f64 / used_mem as f64
            } else {
                0.0
            },
            keyspace_hits: parse_u64(map, "keyspace_hits"),
            keyspace_misses: parse_u64(map, "keyspace_misses"),
            expired_keys: parse_u64(map, "expired_keys"),
            evicted_keys: parse_u64(map, "evicted_keys"),
            instantaneous_ops_per_sec: parse_u64(map, "instantaneous_ops_per_sec"),
            instantaneous_input_kbps: parse_f64(map, "instantaneous_input_kbps"),
            instantaneous_output_kbps: parse_f64(map, "instantaneous_output_kbps"),
            total_commands_processed: parse_u64(map, "total_commands_processed"),
            redis_version: map.get("redis_version").cloned().unwrap_or_default(),
            redis_mode: map.get("redis_mode").cloned().unwrap_or_default(),
            os: map.get("os").cloned().unwrap_or_default(),
            arch_bits: parse_u64(map, "arch_bits"),
            mem_allocator: map.get("mem_allocator").cloned().unwrap_or_default(),
            role: map.get("role").cloned().unwrap_or_default(),
            tcp_port: parse_u64(map, "tcp_port"),
            aof_enabled: parse_u64(map, "aof_enabled"),
            rdb_changes_since_last_save: parse_u64(map, "rdb_changes_since_last_save"),
            total_connections_received: parse_u64(map, "total_connections_received"),
        };

        RedisInfo {
            connected,
            uptime_in_seconds: uptime,
            uptime_in_days: uptime / 86400,
            connected_clients: parse_u64(map, "connected_clients"),
            used_memory: map.get("used_memory_human").cloned().unwrap_or_default(),
            variable,
        }
    }

    const SAMPLE_INFO: &str = "# Server
redis_version:7.2.3
redis_mode:standalone
os:Linux 5.15.0-generic x86_64
arch_bits:64
tcp_port:6379
uptime_in_seconds:86400
role:master

# Clients
connected_clients:12

# Memory
used_memory:1258291
used_memory_human:1.20M
used_memory_peak:2097152
used_memory_rss:3000000
mem_fragmentation_ratio:2.38
mem_allocator:jemalloc-5.3.0

# Stats
total_commands_processed:1000000
instantaneous_ops_per_sec:120
instantaneous_input_kbps:5.2
instantaneous_output_kbps:8.7
total_connections_received:500
keyspace_hits:5000
keyspace_misses:100
expired_keys:50
evicted_keys:0

# Persistence
loading:0
rdb_changes_since_last_save:0
aof_enabled:1
";

    #[test]
    fn test_parse_info_output_extracts_fields() {
        let map = parse_info_output(SAMPLE_INFO);
        assert_eq!(map.get("redis_version"), Some(&"7.2.3".to_string()));
        assert_eq!(map.get("redis_mode"), Some(&"standalone".to_string()));
        assert_eq!(map.get("role"), Some(&"master".to_string()));
        assert_eq!(map.get("connected_clients"), Some(&"12".to_string()));
        assert_eq!(map.get("used_memory_human"), Some(&"1.20M".to_string()));
        assert_eq!(map.get("uptime_in_seconds"), Some(&"86400".to_string()));
        assert_eq!(map.get("keyspace_hits"), Some(&"5000".to_string()));
        assert_eq!(
            map.get("mem_fragmentation_ratio"),
            Some(&"2.38".to_string())
        );
    }

    #[test]
    fn test_build_redis_info_from_parsed_map() {
        let map = parse_info_output(SAMPLE_INFO);
        let info = build_redis_info(true, &map);

        assert!(info.connected);
        assert_eq!(info.uptime_in_seconds, 86400);
        assert_eq!(info.uptime_in_days, 1); // 86400 / 86400
        assert_eq!(info.connected_clients, 12);
        assert_eq!(info.used_memory, "1.20M");

        let v = &info.variable;
        assert_eq!(v.redis_version, "7.2.3");
        assert_eq!(v.redis_mode, "standalone");
        assert_eq!(v.role, "master");
        assert_eq!(v.used_memory, 1258291);
        assert_eq!(v.used_memory_peak, 2097152);
        assert_eq!(v.used_memory_rss, 3000000);
        assert!((v.mem_fragmentation_ratio - 2.38).abs() < 0.01);
        assert_eq!(v.keyspace_hits, 5000);
        assert_eq!(v.keyspace_misses, 100);
        assert_eq!(v.expired_keys, 50);
        assert_eq!(v.evicted_keys, 0);
        assert_eq!(v.instantaneous_ops_per_sec, 120);
        assert_eq!(v.total_commands_processed, 1000000);
        assert_eq!(v.tcp_port, 6379);
        assert_eq!(v.aof_enabled, 1);
    }

    #[test]
    fn test_build_redis_info_missing_fields_defaults() {
        let map = HashMap::new();
        let info = build_redis_info(false, &map);

        assert!(!info.connected);
        assert_eq!(info.uptime_in_seconds, 0);
        assert_eq!(info.uptime_in_days, 0);
        assert_eq!(info.connected_clients, 0);
        assert_eq!(info.variable.redis_version, "");
        assert_eq!(info.variable.used_memory, 0);
        assert_eq!(info.variable.keyspace_hits, 0);
    }

    #[test]
    fn test_redis_info_serializes_to_json() {
        let info = RedisInfo {
            connected: true,
            uptime_in_seconds: 86400,
            uptime_in_days: 1,
            connected_clients: 42,
            used_memory: "3.5M".to_string(),
            variable: RedisVariable {
                redis_version: "7.2.3".to_string(),
                redis_mode: "standalone".to_string(),
                role: "master".to_string(),
                used_memory: 3670016,
                keyspace_hits: 5000,
                keyspace_misses: 100,
                mem_fragmentation_ratio: 1.05,
                instantaneous_ops_per_sec: 120,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&info).unwrap();

        assert!(json.contains("\"connected\":true"));
        assert!(json.contains("\"uptime_in_seconds\":86400"));
        assert!(json.contains("\"uptime_in_days\":1"));
        assert!(json.contains("\"variable\""));
        assert!(json.contains("\"keyspace_hits\":5000"));
        assert!(json.contains("\"mem_fragmentation_ratio\":1.05"));
        assert!(json.contains("\"instantaneous_ops_per_sec\":120"));
    }

    #[test]
    fn test_redis_collect_error_display() {
        let err = RedisCollectError {
            message: "connection refused".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("connection refused"));
        assert!(msg.contains("Redis collect error"));
    }

    #[test]
    fn test_hit_rate_derivation() {
        // 验证命中率可由 keyspace_hits / (hits + misses) 推导
        let map = parse_info_output(SAMPLE_INFO);
        let info = build_redis_info(true, &map);
        let hits = info.variable.keyspace_hits as f64;
        let misses = info.variable.keyspace_misses as f64;
        let hit_rate = hits / (hits + misses) * 100.0;
        assert!((hit_rate - 98.04).abs() < 0.1); // 5000/(5000+100) ≈ 98.04%
    }
}
