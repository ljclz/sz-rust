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

// P0-DIST-1/2/3：移除 #![allow(dead_code)] — runtime 模块已接入执行链路，
// raft/shard/txn/conflict 模块的公共 API 也被 runtime 使用。
// 仍存在的 dead_code 警告由 cargo check 暴露后逐个评估（保留或接入）。

// Phase 8.1：Raft 共识算法
pub mod raft;
// Phase 8.4：Multi-Raft 分片（Range-based）
pub mod shard;
// Phase 8.7：Percolator 分布式事务
pub mod txn;
// Phase 8.10：多主冲突检测
pub mod conflict;
// P0-DIST-1/2/3：分布式运行时集成层
pub mod runtime;
// P0-DIST 迭代 2：多节点分布式集群（用于多节点 Raft 复制测试）
pub mod cluster;
// P0-DIST 迭代 2：Percolator 跨分片事务协调（基于 DistRuntime 的 2PC）
pub mod dist_txn;
// 阶段 4：生产级 TCP 网络层（替代 InMemoryNetwork）
pub mod network;

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
