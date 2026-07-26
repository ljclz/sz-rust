//! SzRSQL 分布式：Raft/2PC/Percolator。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.17 节。
//!
//! # 模块
//!
//! - [`raft`] — Phase 8.1 Raft 共识算法
//!   - 从零实现的 Raft（Ongaro & Ousterhout 2014）
//!   - Leader 选举 / 日志复制 / 故障恢复 / 成员变更
//!   - 确定性 LCG 驱动选举超时，可复现测试
//! - [`shard`] — Phase 8.4 Multi-Raft 分片（Range-based）
//!   - 基于 Range 的分片策略
//!   - 每个分片由独立 Raft 组管理
//!   - 跨分片扫描聚合
//! - [`txn`] — Phase 8.7 Percolator 分布式事务
//!   - 基于 Google Percolator 论文的跨分片 ACID 事务
//!   - TSO 时间戳服务 + 两阶段提交（prewrite/commit）
//!   - 故障恢复（resolve_lock 前推/回滚）
//! - [`conflict`] — Phase 8.10 多主冲突检测
//!   - 多主（multi-master）场景下的并发写入冲突检测
//!   - 基于 LSN / 时间戳 / 节点 ID 的确定性冲突解决
//!   - 冲突队列管理 + 手动解决（丢弃 / 强制应用 / 合并）
//!   - 冲突日志持久化（二进制编解码）

#![allow(dead_code)]

// Phase 8.1：Raft 共识算法
pub mod raft;
// Phase 8.4：Multi-Raft 分片（Range-based）
pub mod shard;
// Phase 8.7：Percolator 分布式事务
pub mod txn;
// Phase 8.10：多主冲突检测
pub mod conflict;

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
