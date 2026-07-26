//! Phase 8.10 — 多主冲突检测（Multi-Master Conflict Detection）
//!
//! 在多主（multi-master）场景下，多个 Leader 节点可同时接受写入。
//! 当两个 Leader 同时写入同一 key 时，需要检测冲突并解决。
//!
//! # 设计目标
//!
//! 1. **独立模块** — 不直接依赖 Raft 运行时（避免与 Phase 8.1 单主 Raft 冲突），
//!    而是模拟多主场景：多个节点独立接受写入，每个写入带 (node_id, lsn, timestamp, key, value)。
//! 2. **冲突检测** — 同一 key 的两个写操作若来自不同节点，视为冲突。
//! 3. **确定性解决** — 根据 LSN / 时间戳 / 节点 ID 策略选择胜者，结果可复现。
//! 4. **不丢失数据** — 落败者写入冲突队列，支持手动解决（丢弃 / 强制应用 / 合并）。
//! 5. **可审计** — ConflictLog 按时间顺序记录所有冲突事件，支持持久化编解码。
//!
//! # 架构
//!
//! - [`WriteOperation`] — 写操作记录，由 (node_id, lsn) 唯一标识
//! - [`ConflictResolution`] — 冲突解决策略枚举
//! - [`ConflictEntry`] — 冲突记录（含胜者与落败者）
//! - [`MultiMasterDetector`] — 冲突检测器，维护每个 key 的最新已接受写操作
//! - [`ConflictLog`] — 冲突日志，用于审计与回放
//! - [`MultiMasterCluster`] — 多主集群模拟器（测试夹具）
//!
//! # 与 Raft 的关系
//!
//! Phase 8.1 的 Raft 是单主模型：同一时刻只有一个 Leader 接受写入，通过日志复制
//! 保证一致性，不存在多主冲突。本模块模拟的是多主场景（如 CouchDB / Dynamo 风格），
//! 复用 Raft 的 `NodeId` / `Index` 类型别名，但不依赖 Raft 运行时状态机。
//!
//! # 冲突检测逻辑
//!
//! 1. 接受写入 `accept(op)`：查找 key 的现有已接受写操作
//! 2. 若无现有写操作 → 直接接受
//! 3. 若来自同一节点 → 覆盖（非冲突，同节点 LSN 递增更新）
//! 4. 若来自不同节点 → 冲突！根据解决策略选择胜者，落败者入冲突队列
//!
//! 对应 `SzRSQL实施进度.md` Phase 8.10。

#![allow(dead_code)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::raft::{Index, NodeId};

// =====================================================================
//  WriteOperation — 写操作记录
// =====================================================================

/// 多主场景下的写操作记录
///
/// 每个写操作由 (node_id, lsn) 唯一标识，带有物理时间戳或逻辑时间戳，
/// 用于冲突检测。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOperation {
    /// 写入节点 ID
    pub node_id: NodeId,
    /// 该节点上的日志序列号（LSN）
    pub lsn: Index,
    /// 时间戳（物理时间或 TSO 分配的逻辑时间戳）
    pub timestamp: u64,
    /// 写入的键
    pub key: Vec<u8>,
    /// 写入的值
    pub value: Vec<u8>,
}

impl WriteOperation {
    /// 编码为字节序列
    ///
    /// 格式：`[node_id:u64 BE][lsn:u64 BE][timestamp:u64 BE][key_len:u32 BE][key][val_len:u32 BE][value]`
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24 + 4 + self.key.len() + 4 + self.value.len());
        buf.extend_from_slice(&self.node_id.to_be_bytes());
        buf.extend_from_slice(&self.lsn.to_be_bytes());
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.extend_from_slice(&(self.key.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.key);
        buf.extend_from_slice(&(self.value.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.value);
        buf
    }

    /// 从字节序列解码，返回解码结果与剩余未消费的字节切片
    ///
    /// # Errors
    /// 数据格式非法时返回 `ConflictError::CorruptData`。
    pub fn decode_from(data: &[u8]) -> Result<(Self, &[u8]), ConflictError> {
        if data.len() < 28 {
            return Err(ConflictError::CorruptData);
        }
        let node_id = u64::from_be_bytes(data[0..8].try_into().unwrap());
        let lsn = u64::from_be_bytes(data[8..16].try_into().unwrap());
        let timestamp = u64::from_be_bytes(data[16..24].try_into().unwrap());
        let key_len = u32::from_be_bytes(data[24..28].try_into().unwrap()) as usize;
        if data.len() < 28 + key_len + 4 {
            return Err(ConflictError::CorruptData);
        }
        let key = data[28..28 + key_len].to_vec();
        let val_off = 28 + key_len;
        let val_len = u32::from_be_bytes(data[val_off..val_off + 4].try_into().unwrap()) as usize;
        if data.len() < val_off + 4 + val_len {
            return Err(ConflictError::CorruptData);
        }
        let value = data[val_off + 4..val_off + 4 + val_len].to_vec();
        let consumed = val_off + 4 + val_len;
        Ok((
            Self {
                node_id,
                lsn,
                timestamp,
                key,
                value,
            },
            &data[consumed..],
        ))
    }

    /// 从字节序列解码（要求整个切片被消费）
    ///
    /// # Errors
    /// 数据格式非法或存在多余字节时返回 `ConflictError::CorruptData`。
    pub fn decode(data: &[u8]) -> Result<Self, ConflictError> {
        let (op, rest) = Self::decode_from(data)?;
        if !rest.is_empty() {
            return Err(ConflictError::CorruptData);
        }
        Ok(op)
    }
}

// =====================================================================
//  ConflictResolution — 冲突解决策略
// =====================================================================

/// 冲突解决策略
///
/// 当两个写操作冲突时，决定哪个胜出。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// 基于 LSN：LSN 更大的胜出（最新写入胜出，类似 Last-Write-Wins）
    LastLsnWins,
    /// 基于时间戳：时间戳更大的胜出
    LastTimestampWins,
    /// 基于节点 ID：节点 ID 更小的胜出（确定性打破平局）
    NodeIdWins,
}

impl ConflictResolution {
    /// 编码为单字节
    pub fn as_byte(&self) -> u8 {
        match self {
            Self::LastLsnWins => 0x01,
            Self::LastTimestampWins => 0x02,
            Self::NodeIdWins => 0x03,
        }
    }

    /// 从单字节解码
    ///
    /// # Errors
    /// 未知策略字节时返回 `ConflictError::CorruptData`。
    pub fn from_byte(b: u8) -> Result<Self, ConflictError> {
        match b {
            0x01 => Ok(Self::LastLsnWins),
            0x02 => Ok(Self::LastTimestampWins),
            0x03 => Ok(Self::NodeIdWins),
            _ => Err(ConflictError::CorruptData),
        }
    }
}

// =====================================================================
//  ConflictEntry — 冲突记录
// =====================================================================

/// 冲突记录：当检测到冲突时，落败的写操作被放入冲突队列
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEntry {
    /// 胜出的写操作
    pub winner: WriteOperation,
    /// 落败的写操作
    pub loser: WriteOperation,
    /// 冲突检测时间戳
    pub detected_at: u64,
    /// 解决策略
    pub resolution: ConflictResolution,
}

impl ConflictEntry {
    /// 编码为字节序列
    ///
    /// 格式：`[winner:WriteOperation][loser:WriteOperation][detected_at:u64 BE][resolution:u8]`
    pub fn encode(&self) -> Vec<u8> {
        let winner_bytes = self.winner.encode();
        let loser_bytes = self.loser.encode();
        let mut buf = Vec::with_capacity(winner_bytes.len() + loser_bytes.len() + 9);
        buf.extend_from_slice(&winner_bytes);
        buf.extend_from_slice(&loser_bytes);
        buf.extend_from_slice(&self.detected_at.to_be_bytes());
        buf.push(self.resolution.as_byte());
        buf
    }

    /// 从字节序列解码，返回解码结果与剩余未消费的字节切片
    ///
    /// # Errors
    /// 数据格式非法时返回 `ConflictError::CorruptData`。
    pub fn decode_from(data: &[u8]) -> Result<(Self, &[u8]), ConflictError> {
        let (winner, rest) = WriteOperation::decode_from(data)?;
        let (loser, rest) = WriteOperation::decode_from(rest)?;
        if rest.len() < 9 {
            return Err(ConflictError::CorruptData);
        }
        let detected_at = u64::from_be_bytes(rest[0..8].try_into().unwrap());
        let resolution = ConflictResolution::from_byte(rest[8])?;
        Ok((
            Self {
                winner,
                loser,
                detected_at,
                resolution,
            },
            &rest[9..],
        ))
    }
}

// =====================================================================
//  ConflictError — 错误类型
// =====================================================================

/// 冲突检测错误
#[derive(Debug, Clone, Error)]
pub enum ConflictError {
    /// 键不存在
    #[error("key not found: {0:?}")]
    KeyNotFound(Vec<u8>),
    /// 冲突队列已满
    #[error("conflict queue full")]
    QueueFull,
    /// 操作已被解决
    #[error("operation already resolved: node={0}, lsn={1}")]
    AlreadyResolved(NodeId, Index),
    /// 冲突索引越界
    #[error("invalid conflict index: {0}")]
    InvalidIndex(usize),
    /// 数据损坏
    #[error("corrupt data")]
    CorruptData,
}

// =====================================================================
//  AcceptResult — 接受写操作的结果
// =====================================================================

/// 接受写操作的结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptResult {
    /// 无冲突，写操作已接受
    Accepted,
    /// 冲突，本操作胜出，旧操作被放入冲突队列
    WonAsWinner {
        /// 被替换的旧写操作
        displaced: WriteOperation,
    },
    /// 冲突，本操作落败，被放入冲突队列
    LostAsLoser {
        /// 胜出的写操作
        winner: WriteOperation,
    },
}

// =====================================================================
//  ResolveAction — 手动解决冲突的动作
// =====================================================================

/// 手动解决冲突的动作
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveAction {
    /// 丢弃落败者（保持胜者）
    DiscardLoser,
    /// 强制应用落败者（覆盖胜者）
    ApplyLoser,
    /// 合并：两个值都保留（拼接 winner || loser）
    MergeBoth,
}

// =====================================================================
//  MultiMasterDetector — 多主冲突检测器
// =====================================================================

/// 多主冲突检测器
///
/// 维护每个键的最新已接受写操作，当新写入与已有写入冲突时，
/// 根据解决策略选择胜者，落败者写入冲突队列。
///
/// # 设计
///
/// - **接受写入**：`accept(op)` — 检查与现有写入是否冲突
/// - **冲突检测**：同一 key 的两个写操作，如果来自不同节点，则视为冲突
/// - **解决策略**：根据 `ConflictResolution` 选择胜者
/// - **冲突队列**：落败的写操作放入队列，等待手动解决
/// - **手动解决**：`resolve_conflict(index, action)` — 丢弃或强制应用
pub struct MultiMasterDetector {
    /// 每个键的最新已接受写操作（key -> WriteOperation）
    accepted: HashMap<Vec<u8>, WriteOperation>,
    /// 冲突队列
    conflicts: Vec<ConflictEntry>,
    /// 解决策略
    resolution: ConflictResolution,
    /// 当前时间戳（用于记录冲突检测时间）
    current_ts: u64,
    /// 冲突队列最大容量
    max_conflicts: usize,
}

impl MultiMasterDetector {
    /// 创建冲突检测器
    ///
    /// # 参数
    /// - `resolution` — 冲突解决策略
    /// - `max_conflicts` — 冲突队列最大容量（满后覆盖最旧条目）
    pub fn new(resolution: ConflictResolution, max_conflicts: usize) -> Self {
        Self {
            accepted: HashMap::new(),
            conflicts: Vec::new(),
            resolution,
            current_ts: 0,
            max_conflicts,
        }
    }

    /// 更改解决策略
    pub fn set_resolution(&mut self, resolution: ConflictResolution) {
        self.resolution = resolution;
    }

    /// 判断新写操作是否胜出
    ///
    /// 各策略的比较规则：
    /// - `LastLsnWins`：LSN 大的胜；相等则 timestamp 大的胜；都相等则 node_id 小的胜
    /// - `LastTimestampWins`：timestamp 大的胜；相等则 LSN 大的胜；都相等则 node_id 小的胜
    /// - `NodeIdWins`：node_id 小的胜；相等则 LSN 大的胜
    fn new_wins(&self, existing: &WriteOperation, new: &WriteOperation) -> bool {
        match self.resolution {
            ConflictResolution::LastLsnWins => {
                if new.lsn != existing.lsn {
                    new.lsn > existing.lsn
                } else if new.timestamp != existing.timestamp {
                    new.timestamp > existing.timestamp
                } else {
                    new.node_id < existing.node_id
                }
            }
            ConflictResolution::LastTimestampWins => {
                if new.timestamp != existing.timestamp {
                    new.timestamp > existing.timestamp
                } else if new.lsn != existing.lsn {
                    new.lsn > existing.lsn
                } else {
                    new.node_id < existing.node_id
                }
            }
            ConflictResolution::NodeIdWins => {
                if new.node_id != existing.node_id {
                    new.node_id < existing.node_id
                } else {
                    new.lsn > existing.lsn
                }
            }
        }
    }

    /// 将冲突条目加入队列（满时覆盖最旧条目）
    fn push_conflict(&mut self, entry: ConflictEntry) {
        if self.conflicts.len() >= self.max_conflicts && !self.conflicts.is_empty() {
            self.conflicts.remove(0);
        }
        if self.conflicts.len() < self.max_conflicts {
            self.conflicts.push(entry);
        }
    }

    /// 接受写操作，返回是否冲突及解决结果
    ///
    /// # 逻辑
    /// 1. 查找 key 的现有 accepted 写操作
    /// 2. 若无 → 直接接受，返回 `Accepted`
    /// 3. 若来自同一节点 → 覆盖（非冲突，LSN 递增更新）
    /// 4. 若来自不同节点 → 冲突！根据策略选胜者，落败者入冲突队列
    pub fn accept(&mut self, op: WriteOperation) -> AcceptResult {
        let existing = self.accepted.get(&op.key).cloned();
        match existing {
            // 场景 1：无现有写操作 → 直接接受
            None => {
                let key = op.key.clone();
                self.accepted.insert(key, op);
                AcceptResult::Accepted
            }
            // 场景 2：同节点写入 → 覆盖（非冲突）
            Some(existing) if existing.node_id == op.node_id => {
                if op.lsn > existing.lsn {
                    let key = op.key.clone();
                    self.accepted.insert(key, op);
                }
                // 旧 LSN 被忽略，新 LSN 被接受
                AcceptResult::Accepted
            }
            // 场景 3：不同节点 → 冲突
            Some(existing) => {
                let new_wins = self.new_wins(&existing, &op);
                let detected_at = self.current_ts;

                let entry = if new_wins {
                    ConflictEntry {
                        winner: op.clone(),
                        loser: existing.clone(),
                        detected_at,
                        resolution: self.resolution,
                    }
                } else {
                    ConflictEntry {
                        winner: existing.clone(),
                        loser: op.clone(),
                        detected_at,
                        resolution: self.resolution,
                    }
                };

                self.push_conflict(entry);

                if new_wins {
                    let key = op.key.clone();
                    self.accepted.insert(key, op.clone());
                    AcceptResult::WonAsWinner {
                        displaced: existing,
                    }
                } else {
                    AcceptResult::LostAsLoser { winner: existing }
                }
            }
        }
    }

    /// 查询键的最新已接受写操作
    pub fn get(&self, key: &[u8]) -> Option<&WriteOperation> {
        self.accepted.get(key)
    }

    /// 获取冲突队列切片
    pub fn conflicts(&self) -> &[ConflictEntry] {
        &self.conflicts
    }

    /// 获取冲突数量
    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// 手动解决冲突
    ///
    /// # 参数
    /// - `index` — 冲突在队列中的索引
    /// - `action` — 解决动作（丢弃落败者 / 强制应用落败者 / 合并两者）
    ///
    /// # Errors
    /// 索引越界时返回 `ConflictError::InvalidIndex`。
    pub fn resolve_conflict(
        &mut self,
        index: usize,
        action: ResolveAction,
    ) -> Result<(), ConflictError> {
        if index >= self.conflicts.len() {
            return Err(ConflictError::InvalidIndex(index));
        }
        let entry = self.conflicts.remove(index);

        match action {
            ResolveAction::DiscardLoser => {
                // 保持胜者（已在 accepted 中），落败者被丢弃
            }
            ResolveAction::ApplyLoser => {
                // 将落败者作为新 accepted[key]（覆盖胜者）
                let key = entry.loser.key.clone();
                self.accepted.insert(key, entry.loser);
            }
            ResolveAction::MergeBoth => {
                // 合并两个值：winner.value ++ loser.value
                let mut merged_value = entry.winner.value.clone();
                merged_value.extend_from_slice(&entry.loser.value);
                let merged_op = WriteOperation {
                    node_id: entry.winner.node_id,
                    lsn: entry.winner.lsn,
                    timestamp: entry.winner.timestamp,
                    key: entry.winner.key.clone(),
                    value: merged_value,
                };
                self.accepted.insert(entry.winner.key, merged_op);
            }
        }
        Ok(())
    }

    /// 清理已解决的冲突
    ///
    /// 冲突在 `resolve_conflict` 时已从队列移除，此方法目前为空操作，
    /// 保留用于未来扩展（如批量标记后统一清理）。
    pub fn clear_resolved(&mut self) {
        // 冲突在 resolve_conflict 时已被移除，此处无需额外操作
    }

    /// 推进时间戳（current_ts += 1）
    pub fn tick(&mut self) {
        self.current_ts += 1;
    }
}

// =====================================================================
//  ConflictLog — 冲突日志（持久化用）
// =====================================================================

/// 冲突日志：按时间顺序记录所有冲突事件
///
/// 用于审计和回放。可序列化为二进制格式。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConflictLog {
    /// 所有冲突事件（按时间顺序）
    entries: Vec<ConflictEntry>,
    /// 已解决的数量
    resolved_count: usize,
}

impl ConflictLog {
    /// 创建空冲突日志
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条冲突
    pub fn record(&mut self, entry: ConflictEntry) {
        self.entries.push(entry);
    }

    /// 获取所有冲突事件切片
    pub fn entries(&self) -> &[ConflictEntry] {
        &self.entries
    }

    /// 已解决的冲突数量
    pub fn resolved(&self) -> usize {
        self.resolved_count
    }

    /// 未解决的冲突数量
    pub fn pending(&self) -> usize {
        self.entries.len().saturating_sub(self.resolved_count)
    }

    /// 标记指定索引的冲突为已解决
    ///
    /// 若索引越界则不执行任何操作。
    pub fn mark_resolved(&mut self, index: usize) {
        if index < self.entries.len() {
            self.resolved_count += 1;
        }
    }

    /// 编码为二进制字节序列
    ///
    /// 格式：`[entry_count:u32 BE][entries...][resolved_count:u64 BE]`
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + 8);
        buf.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        for entry in &self.entries {
            buf.extend_from_slice(&entry.encode());
        }
        buf.extend_from_slice(&(self.resolved_count as u64).to_be_bytes());
        buf
    }

    /// 从二进制字节序列解码
    ///
    /// # Errors
    /// 数据格式非法时返回 `ConflictError::CorruptData`。
    pub fn decode(data: &[u8]) -> Result<Self, ConflictError> {
        if data.len() < 4 {
            return Err(ConflictError::CorruptData);
        }
        let entry_count = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        let mut rest = &data[4..];
        // 容量按剩余数据下限估算，避免恶意 entry_count 触发超大分配
        // 单条 ConflictEntry 最小编码长度 = 2 * WriteOperation(28) + 9 = 65 字节
        let cap = entry_count.min(rest.len() / 65);
        let mut entries = Vec::with_capacity(cap);
        for _ in 0..entry_count {
            let (entry, remaining) = ConflictEntry::decode_from(rest)?;
            entries.push(entry);
            rest = remaining;
        }
        if rest.len() < 8 {
            return Err(ConflictError::CorruptData);
        }
        let resolved_count = u64::from_be_bytes(rest[0..8].try_into().unwrap()) as usize;
        Ok(Self {
            entries,
            resolved_count,
        })
    }
}

// =====================================================================
//  MultiMasterCluster — 多主集群模拟器（测试用）
// =====================================================================

/// 多主集群模拟器（测试夹具）
///
/// 模拟 N 个节点都能独立接受写入的多主集群。
/// 每个节点维护自己的本地日志（Vec<WriteOperation>），
/// 写入同步到冲突检测器进行冲突检测。
pub struct MultiMasterCluster {
    /// 节点数
    num_nodes: usize,
    /// 每个节点的本地 LSN 计数器
    node_lsns: Vec<Index>,
    /// 冲突检测器
    detector: MultiMasterDetector,
    /// 每个节点的本地日志（已接受的写操作）
    node_logs: Vec<Vec<WriteOperation>>,
}

impl MultiMasterCluster {
    /// 创建多主集群模拟器
    ///
    /// # 参数
    /// - `num_nodes` — 节点数量（节点 ID 为 0..num_nodes）
    /// - `resolution` — 冲突解决策略
    /// - `max_conflicts` — 冲突队列最大容量
    pub fn new(num_nodes: usize, resolution: ConflictResolution, max_conflicts: usize) -> Self {
        Self {
            num_nodes,
            node_lsns: vec![0; num_nodes],
            detector: MultiMasterDetector::new(resolution, max_conflicts),
            node_logs: vec![Vec::new(); num_nodes],
        }
    }

    /// 节点写入（自动生成 LSN 和时间戳）
    ///
    /// 时间戳与 LSN 相同（递增）。仅已接受的写操作记入节点本地日志。
    pub fn write(&mut self, node_id: usize, key: Vec<u8>, value: Vec<u8>) -> AcceptResult {
        self.node_lsns[node_id] += 1;
        let lsn = self.node_lsns[node_id];
        let op = WriteOperation {
            node_id: node_id as NodeId,
            lsn,
            timestamp: lsn,
            key,
            value,
        };
        let result = self.detector.accept(op.clone());
        if matches!(
            result,
            AcceptResult::Accepted | AcceptResult::WonAsWinner { .. }
        ) {
            self.node_logs[node_id].push(op);
        }
        result
    }

    /// 带时间戳的节点写入（自动生成 LSN，时间戳由调用者指定）
    ///
    /// 仅已接受的写操作记入节点本地日志。
    pub fn write_with_ts(
        &mut self,
        node_id: usize,
        key: Vec<u8>,
        value: Vec<u8>,
        timestamp: u64,
    ) -> AcceptResult {
        self.node_lsns[node_id] += 1;
        let lsn = self.node_lsns[node_id];
        let op = WriteOperation {
            node_id: node_id as NodeId,
            lsn,
            timestamp,
            key,
            value,
        };
        let result = self.detector.accept(op.clone());
        if matches!(
            result,
            AcceptResult::Accepted | AcceptResult::WonAsWinner { .. }
        ) {
            self.node_logs[node_id].push(op);
        }
        result
    }

    /// 读取键的最新值
    pub fn read(&self, key: &[u8]) -> Option<&WriteOperation> {
        self.detector.get(key)
    }

    /// 获取冲突队列切片
    pub fn conflicts(&self) -> &[ConflictEntry] {
        self.detector.conflicts()
    }

    /// 手动解决冲突
    ///
    /// # Errors
    /// 索引越界时返回 `ConflictError::InvalidIndex`。
    pub fn resolve_conflict(
        &mut self,
        index: usize,
        action: ResolveAction,
    ) -> Result<(), ConflictError> {
        self.detector.resolve_conflict(index, action)
    }

    /// 获取指定节点的本地日志切片
    pub fn node_log(&self, node_id: usize) -> &[WriteOperation] {
        &self.node_logs[node_id]
    }

    /// 获取冲突检测器的可变引用
    pub fn detector_mut(&mut self) -> &mut MultiMasterDetector {
        &mut self.detector
    }
}

// =====================================================================
//  Phase 8.11 — HLC（Hybrid Logical Clock，混合逻辑时钟）
// =====================================================================
//
// HLC（Kulkarni 2014 "Logical Physical Clocks"）结合物理时钟与逻辑时钟，
// 在节点间时钟偏差下仍能正确排序因果相关的事件。
//
// 每个节点维护 (l, c)：
// - l：物理时间戳部分（毫秒），单调不减
// - c：逻辑计数器，当物理时间相同时递增
//
// # 因果性保证
//
// 若事件 A happened-before 事件 B（A → B），则 hlc(A) < hlc(B)。
// 反之不成立：hlc(A) < hlc(B) 不一定意味着 A → B（可能是并发事件）。

// ---------------------------------------------------------------------
//  HlcTimestamp — HLC 时间戳
// ---------------------------------------------------------------------

/// HLC（Hybrid Logical Clock）时间戳
///
/// 由物理时间戳部分 `l`（毫秒）和逻辑计数器 `c` 组成。
/// 比较规则：`(l1, c1) < (l2, c2)` 当且仅当 `l1 < l2 || (l1 == l2 && c1 < c2)`。
///
/// # 因果性保证
///
/// 若事件 A happened-before 事件 B（A → B），则 `hlc(A) < hlc(B)`。
/// 反之不成立：`hlc(A) < hlc(B)` 不一定意味着 A → B（可能是并发事件）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HlcTimestamp {
    /// 物理时间戳部分（毫秒）
    pub l: u64,
    /// 逻辑计数器
    pub c: u64,
}

impl HlcTimestamp {
    /// 创建 HLC 时间戳
    pub fn new(l: u64, c: u64) -> Self {
        Self { l, c }
    }

    /// 显式比较两个 HLC 时间戳
    ///
    /// 与 derive 的 `Ord` 行为一致：先比较 `l`，`l` 相等时比较 `c`。
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.cmp(other)
    }

    /// 编码为单个 `u128` 用于简单比较
    ///
    /// 格式：`(l << 64) | c`，高 64 位为 `l`，低 64 位为 `c`。
    /// 编码后的大小关系与 `Ord` 一致。
    pub fn to_u128(&self) -> u128 {
        ((self.l as u128) << 64) | (self.c as u128)
    }
}

// ---------------------------------------------------------------------
//  HlcClock — HLC 时钟
// ---------------------------------------------------------------------

/// HLC 时钟
///
/// 每个节点维护一个 HLC 时钟。时钟的物理部分 `l` 来自外部提供的
/// `physical_clock()` 函数（便于测试注入模拟时钟偏差）。
///
/// # 线程安全
///
/// 本实现为单线程版本（测试用）。生产环境需要用 `Mutex<HlcClock>` 或
/// `AtomicU64` 包装。
pub struct HlcClock {
    /// 当前物理时间戳部分
    l: u64,
    /// 当前逻辑计数器
    c: u64,
    /// 物理时钟函数（返回当前物理时间戳，毫秒）
    /// 用 `Box<dyn Fn() -> u64>` 实现依赖注入，便于测试模拟时钟偏差
    physical_clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl HlcClock {
    /// 创建 HLC 时钟
    ///
    /// # 参数
    /// - `physical_clock` — 物理时钟函数（返回当前物理时间戳，毫秒）
    ///
    /// 初始状态为 `(l=0, c=0)`，首次事件时会与物理时钟对齐。
    pub fn new(physical_clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Self {
            l: 0,
            c: 0,
            physical_clock: Box::new(physical_clock),
        }
    }

    /// 本地事件：生成新 HLC 时间戳
    ///
    /// 算法：
    /// ```text
    /// l' = max(l, physical_clock)
    /// if l' == l: c' = c + 1
    /// else:       c' = 0
    /// (l, c) = (l', c')
    /// ```
    pub fn now(&mut self) -> HlcTimestamp {
        let pt = (self.physical_clock)();
        if pt > self.l {
            // 物理时钟推进 → l 更新，c 重置
            self.l = pt;
            self.c = 0;
        } else {
            // 物理时钟未推进（或回退）→ l 不变，c 递增
            self.c += 1;
        }
        HlcTimestamp {
            l: self.l,
            c: self.c,
        }
    }

    /// 发送消息：等同 `now()`，返回当前 HLC 时间戳
    pub fn send(&mut self) -> HlcTimestamp {
        self.now()
    }

    /// 接收消息：合并远端时间戳
    ///
    /// 算法：
    /// ```text
    /// l' = max(l, m.l, physical_clock)
    /// if l' == l && l' == m.l: c' = max(c, m.c) + 1
    /// elif l' == l:             c' = c + 1
    /// elif l' == m.l:           c' = m.c + 1
    /// else:                     c' = 0
    /// (l, c) = (l', c')
    /// ```
    pub fn receive(&mut self, msg: HlcTimestamp) -> HlcTimestamp {
        let pt = (self.physical_clock)();
        let new_l = self.l.max(msg.l).max(pt);

        if new_l == self.l && new_l == msg.l {
            // 本地 l 和消息 l 都等于 new_l → 取较大 c 后 +1
            self.c = self.c.max(msg.c) + 1;
        } else if new_l == self.l {
            // 仅本地 l 等于 new_l → 本地 c +1
            self.c += 1;
        } else if new_l == msg.l {
            // 仅消息 l 等于 new_l → 消息 c +1
            self.c = msg.c + 1;
        } else {
            // 物理时钟最大 → c 重置
            self.c = 0;
        }
        self.l = new_l;
        HlcTimestamp {
            l: self.l,
            c: self.c,
        }
    }

    /// 查看当前时钟值（不推进）
    pub fn peek(&self) -> HlcTimestamp {
        HlcTimestamp {
            l: self.l,
            c: self.c,
        }
    }

    /// 更换物理时钟函数
    pub fn set_physical_clock(&mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) {
        self.physical_clock = Box::new(clock);
    }
}

// ---------------------------------------------------------------------
//  HlcWriteOperation — 带 HLC 时间戳的写操作
// ---------------------------------------------------------------------

/// 带 HLC 时间戳的写操作
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlcWriteOperation {
    /// HLC 时间戳
    pub hlc: HlcTimestamp,
    /// 写入节点 ID
    pub node_id: NodeId,
    /// 写入的键
    pub key: Vec<u8>,
    /// 写入的值
    pub value: Vec<u8>,
}

// ---------------------------------------------------------------------
//  HlcConflictEntry — HLC 冲突记录
// ---------------------------------------------------------------------

/// HLC 冲突记录
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlcConflictEntry {
    /// 胜出的写操作
    pub winner: HlcWriteOperation,
    /// 落败的写操作
    pub loser: HlcWriteOperation,
}

// ---------------------------------------------------------------------
//  HlcAcceptResult — HLC 接受结果
// ---------------------------------------------------------------------

/// HLC 接受结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HlcAcceptResult {
    /// 无冲突，已接受
    Accepted(HlcWriteOperation),
    /// 冲突，本操作胜出
    WonAsWinner {
        /// 被替换的旧写操作
        displaced: HlcWriteOperation,
    },
    /// 冲突，本操作落败
    LostAsLoser {
        /// 胜出的写操作
        winner: HlcWriteOperation,
    },
}

// ---------------------------------------------------------------------
//  HlcMultiMasterDetector — 基于 HLC 的多主冲突检测器
// ---------------------------------------------------------------------

/// 基于 HLC 的多主冲突检测器
///
/// 与 [`MultiMasterDetector`] 类似，但写操作的时间戳来自 HLC 而非物理时间。
/// 冲突解决策略固定为 `LastTimestampWins`（HLC 时间戳大的胜出），
/// 保证了因果排序：后发生的事件 HLC 时间戳更大。
///
/// # 优势
///
/// 在节点间时钟偏差（如 100ms）下，物理时间戳可能导致旧事件被误判为新事件。
/// HLC 通过逻辑计数器 `c` 打破平局，保证了因果正确的排序。
pub struct HlcMultiMasterDetector {
    /// 每个节点的 HLC 时钟
    clocks: HashMap<NodeId, HlcClock>,
    /// 每个键的最新已接受写操作（HLC 时间戳 + WriteOperation）
    accepted: HashMap<Vec<u8>, HlcWriteOperation>,
    /// 冲突队列
    conflicts: Vec<HlcConflictEntry>,
    /// 冲突队列最大容量
    max_conflicts: usize,
}

impl HlcMultiMasterDetector {
    /// 创建基于 HLC 的多主冲突检测器
    ///
    /// # 参数
    /// - `max_conflicts` — 冲突队列最大容量（满后覆盖最旧条目）
    pub fn new(max_conflicts: usize) -> Self {
        Self {
            clocks: HashMap::new(),
            accepted: HashMap::new(),
            conflicts: Vec::new(),
            max_conflicts,
        }
    }

    /// 注册节点及其 HLC 时钟
    pub fn register_node(&mut self, node_id: NodeId, clock: HlcClock) {
        self.clocks.insert(node_id, clock);
    }

    /// 将冲突条目加入队列（满时覆盖最旧条目）
    fn push_conflict(&mut self, entry: HlcConflictEntry) {
        if self.conflicts.len() >= self.max_conflicts && !self.conflicts.is_empty() {
            self.conflicts.remove(0);
        }
        if self.conflicts.len() < self.max_conflicts {
            self.conflicts.push(entry);
        }
    }

    /// 判断新写操作是否胜出（HLC 时间戳大的胜，相等时 node_id 小的胜）
    fn new_wins(new: &HlcWriteOperation, existing: &HlcWriteOperation) -> bool {
        match new.hlc.cmp(&existing.hlc) {
            std::cmp::Ordering::Equal => new.node_id < existing.node_id,
            ord => ord.is_gt(),
        }
    }

    /// 节点写入（内部调用 `clock.now()` 获取 HLC 时间戳）
    ///
    /// # 逻辑
    /// 1. 获取节点的 HLC 时钟，调用 `now()` 生成时间戳
    /// 2. 构造 `HlcWriteOperation`
    /// 3. 查找 key 的现有 accepted 写操作
    /// 4. 若无 → 直接接受
    /// 5. 若同节点 → 覆盖（非冲突，HLC 单调递增）
    /// 6. 若不同节点 → 冲突，HLC 大的胜出
    ///
    /// # Panics
    /// 节点未注册时 panic。
    pub fn write(&mut self, node_id: NodeId, key: Vec<u8>, value: Vec<u8>) -> HlcAcceptResult {
        // 先获取 HLC 时间戳（释放 clocks 的借用）
        let hlc = self
            .clocks
            .get_mut(&node_id)
            .expect("node not registered")
            .now();
        let op = HlcWriteOperation {
            hlc,
            node_id,
            key: key.clone(),
            value,
        };
        self.accept_op(op)
    }

    /// 接收远端写操作（内部调用 `clock.receive(remote_op.hlc)` 更新时钟）
    ///
    /// 与 `write` 不同，`receive` 使用远端操作自带的 HLC 时间戳进行冲突检测，
    /// 但会先通过 `clock.receive()` 更新本地时钟以保证因果性。
    ///
    /// # Panics
    /// 节点未注册时 panic。
    pub fn receive(&mut self, node_id: NodeId, remote_op: HlcWriteOperation) -> HlcAcceptResult {
        // 先更新节点时钟（释放 clocks 的借用）
        self.clocks
            .get_mut(&node_id)
            .expect("node not registered")
            .receive(remote_op.hlc);
        // 使用远端操作原始 HLC 进行冲突检测
        self.accept_op(remote_op)
    }

    /// 内部：接受写操作的统一逻辑
    fn accept_op(&mut self, op: HlcWriteOperation) -> HlcAcceptResult {
        let key = op.key.clone();
        match self.accepted.get(&key).cloned() {
            // 场景 1：无现有写操作 → 直接接受
            None => {
                self.accepted.insert(key, op.clone());
                HlcAcceptResult::Accepted(op)
            }
            // 场景 2：同节点同 HLC（重复消息）→ 幂等接受
            Some(existing) if existing.hlc == op.hlc && existing.node_id == op.node_id => {
                HlcAcceptResult::Accepted(op)
            }
            // 场景 3：同节点不同 HLC → 覆盖（非冲突，HLC 单调递增）
            Some(existing) if existing.node_id == op.node_id => {
                if op.hlc > existing.hlc {
                    self.accepted.insert(key, op.clone());
                }
                HlcAcceptResult::Accepted(op)
            }
            // 场景 4：不同节点 → 冲突
            Some(existing) => {
                if Self::new_wins(&op, &existing) {
                    // 新操作胜出
                    let displaced = existing.clone();
                    self.accepted.insert(key, op.clone());
                    self.push_conflict(HlcConflictEntry {
                        winner: op.clone(),
                        loser: displaced.clone(),
                    });
                    HlcAcceptResult::WonAsWinner { displaced }
                } else {
                    // 新操作落败
                    self.push_conflict(HlcConflictEntry {
                        winner: existing.clone(),
                        loser: op.clone(),
                    });
                    HlcAcceptResult::LostAsLoser {
                        winner: existing.clone(),
                    }
                }
            }
        }
    }

    /// 查询键的最新已接受写操作
    pub fn get(&self, key: &[u8]) -> Option<&HlcWriteOperation> {
        self.accepted.get(key)
    }

    /// 获取冲突队列切片
    pub fn conflicts(&self) -> &[HlcConflictEntry] {
        &self.conflicts
    }

    /// 手动解决冲突
    ///
    /// # 参数
    /// - `index` — 冲突在队列中的索引
    /// - `action` — 解决动作（复用 [`ResolveAction`]）
    ///
    /// # Errors
    /// 索引越界时返回 `ConflictError::InvalidIndex`。
    pub fn resolve_conflict(
        &mut self,
        index: usize,
        action: ResolveAction,
    ) -> Result<(), ConflictError> {
        if index >= self.conflicts.len() {
            return Err(ConflictError::InvalidIndex(index));
        }
        let entry = self.conflicts.remove(index);

        match action {
            ResolveAction::DiscardLoser => {
                // 保持胜者（已在 accepted 中），落败者被丢弃
            }
            ResolveAction::ApplyLoser => {
                // 将落败者作为新 accepted[key]（覆盖胜者）
                let key = entry.loser.key.clone();
                self.accepted.insert(key, entry.loser);
            }
            ResolveAction::MergeBoth => {
                // 合并两个值：winner.value ++ loser.value
                let mut merged_value = entry.winner.value.clone();
                merged_value.extend_from_slice(&entry.loser.value);
                let merged_op = HlcWriteOperation {
                    hlc: entry.winner.hlc,
                    node_id: entry.winner.node_id,
                    key: entry.winner.key.clone(),
                    value: merged_value,
                };
                self.accepted.insert(entry.winner.key, merged_op);
            }
        }
        Ok(())
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::cmp::Ordering;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::sync::Arc;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    /// 构造写操作
    fn make_op(
        node_id: NodeId,
        lsn: Index,
        timestamp: u64,
        key: &[u8],
        value: &[u8],
    ) -> WriteOperation {
        WriteOperation {
            node_id,
            lsn,
            timestamp,
            key: key.to_vec(),
            value: value.to_vec(),
        }
    }

    /// 创建共享时间源 + HLC 时钟（带偏移）
    ///
    /// 返回 `(Arc<AtomicU64>, HlcClock)`，时钟读取 `base_time + offset`。
    /// `offset` 为正表示时钟快，为负表示时钟慢。
    fn make_hlc_clock(base_time: Arc<AtomicU64>, offset: i64) -> HlcClock {
        HlcClock::new(move || {
            let t = base_time.load(AtomicOrdering::SeqCst);
            if offset >= 0 {
                t + offset as u64
            } else {
                t.saturating_sub((-offset) as u64)
            }
        })
    }

    // -----------------------------------------------------------------
    //  1. 基本冲突检测
    // -----------------------------------------------------------------

    #[test]
    fn test_no_conflict_single_node() {
        // 单节点连续写同一 key，无冲突
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        let r1 = detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        assert_eq!(r1, AcceptResult::Accepted);
        let r2 = detector.accept(make_op(1, 2, 2, b"k", b"v2"));
        assert_eq!(r2, AcceptResult::Accepted);
        assert_eq!(detector.conflict_count(), 0);
        // 最新值是 v2
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"v2");
        assert_eq!(op.lsn, 2);
    }

    #[test]
    fn test_no_conflict_different_keys() {
        // 不同节点写不同 key，无冲突
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        let r1 = detector.accept(make_op(1, 1, 1, b"a", b"va"));
        assert_eq!(r1, AcceptResult::Accepted);
        let r2 = detector.accept(make_op(2, 1, 1, b"b", b"vb"));
        assert_eq!(r2, AcceptResult::Accepted);
        assert_eq!(detector.conflict_count(), 0);
        assert_eq!(detector.get(b"a").unwrap().value, b"va");
        assert_eq!(detector.get(b"b").unwrap().value, b"vb");
    }

    #[test]
    fn test_conflict_two_nodes_same_key() {
        // 两节点写同一 key → 冲突检测
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        let r2 = detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        // LSN 相等、timestamp 相等 → node_id 小的（节点 1）胜出
        assert!(matches!(r2, AcceptResult::LostAsLoser { .. }));
        assert_eq!(detector.conflict_count(), 1);
        // 节点 1 的值保留
        assert_eq!(detector.get(b"k").unwrap().value, b"v1");
        // 冲突记录：winner=node1, loser=node2
        let c = &detector.conflicts()[0];
        assert_eq!(c.winner.node_id, 1);
        assert_eq!(c.loser.node_id, 2);
    }

    #[test]
    fn test_conflict_three_nodes_same_key() {
        // 三节点写同一 key → 两次冲突
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        detector.accept(make_op(3, 1, 1, b"k", b"v3"));
        assert_eq!(detector.conflict_count(), 2);
        // 节点 1 始终胜出（node_id 最小）
        assert_eq!(detector.get(b"k").unwrap().value, b"v1");
        // 两个冲突的 winner 都是节点 1
        for c in detector.conflicts() {
            assert_eq!(c.winner.node_id, 1);
        }
    }

    // -----------------------------------------------------------------
    //  2. 解决策略
    // -----------------------------------------------------------------

    #[test]
    fn test_last_lsn_wins() {
        // LastLsnWins：LSN 大的胜出
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        let r = detector.accept(make_op(2, 5, 1, b"k", b"v2"));
        // 节点 2 的 LSN=5 > 节点 1 的 LSN=1 → 节点 2 胜出
        assert!(matches!(r, AcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");
        let c = &detector.conflicts()[0];
        assert_eq!(c.winner.node_id, 2);
        assert_eq!(c.loser.node_id, 1);
    }

    #[test]
    fn test_last_timestamp_wins() {
        // LastTimestampWins：timestamp 大的胜出
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastTimestampWins, 100);
        detector.accept(make_op(1, 1, 5, b"k", b"v1"));
        let r = detector.accept(make_op(2, 1, 10, b"k", b"v2"));
        // 节点 2 的 timestamp=10 > 节点 1 的 timestamp=5 → 节点 2 胜出
        assert!(matches!(r, AcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");
        let c = &detector.conflicts()[0];
        assert_eq!(c.winner.node_id, 2);
        assert_eq!(c.loser.node_id, 1);
    }

    #[test]
    fn test_node_id_wins() {
        // NodeIdWins：node_id 小的胜出
        let mut detector = MultiMasterDetector::new(ConflictResolution::NodeIdWins, 100);
        detector.accept(make_op(3, 1, 1, b"k", b"v3"));
        let r = detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        // 节点 1 的 node_id=1 < 节点 3 的 node_id=3 → 节点 1 胜出
        assert!(matches!(r, AcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.get(b"k").unwrap().value, b"v1");
        let c = &detector.conflicts()[0];
        assert_eq!(c.winner.node_id, 1);
        assert_eq!(c.loser.node_id, 3);
    }

    // -----------------------------------------------------------------
    //  3. 冲突队列和手动解决
    // -----------------------------------------------------------------

    #[test]
    fn test_conflict_queue_grows() {
        // 多次冲突后队列正确增长
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        assert_eq!(detector.conflict_count(), 0);

        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        assert_eq!(detector.conflict_count(), 1);

        detector.accept(make_op(3, 1, 1, b"k", b"v3"));
        assert_eq!(detector.conflict_count(), 2);

        detector.accept(make_op(4, 1, 1, b"k", b"v4"));
        assert_eq!(detector.conflict_count(), 3);
    }

    #[test]
    fn test_resolve_discard_loser() {
        // DiscardLoser：保持胜者
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"winner"));
        detector.accept(make_op(2, 1, 1, b"k", b"loser"));
        assert_eq!(detector.conflict_count(), 1);

        // 解决：丢弃落败者
        detector
            .resolve_conflict(0, ResolveAction::DiscardLoser)
            .unwrap();
        assert_eq!(detector.conflict_count(), 0);
        // 胜者值保留
        assert_eq!(detector.get(b"k").unwrap().value, b"winner");
    }

    #[test]
    fn test_resolve_apply_loser() {
        // ApplyLoser：覆盖胜者为落败者
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"winner"));
        detector.accept(make_op(2, 1, 1, b"k", b"loser"));
        assert_eq!(detector.conflict_count(), 1);

        // 解决：强制应用落败者
        detector
            .resolve_conflict(0, ResolveAction::ApplyLoser)
            .unwrap();
        assert_eq!(detector.conflict_count(), 0);
        // 落败者值覆盖
        assert_eq!(detector.get(b"k").unwrap().value, b"loser");
    }

    #[test]
    fn test_resolve_merge_both() {
        // MergeBoth：合并两个值
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"AAA"));
        detector.accept(make_op(2, 1, 1, b"k", b"BBB"));
        assert_eq!(detector.conflict_count(), 1);

        // 解决：合并
        detector
            .resolve_conflict(0, ResolveAction::MergeBoth)
            .unwrap();
        assert_eq!(detector.conflict_count(), 0);
        // 合并值 = winner || loser = "AAA" || "BBB" = "AAABBB"
        let merged = detector.get(b"k").unwrap();
        assert_eq!(merged.value, b"AAABBB");
        // 合并 op 继承 winner 的 node_id/lsn/timestamp
        assert_eq!(merged.node_id, 1);
        assert_eq!(merged.lsn, 1);
    }

    #[test]
    fn test_resolve_nonexistent_conflict() {
        // 解决不存在的冲突 → 报错
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        let err = detector.resolve_conflict(0, ResolveAction::DiscardLoser);
        assert!(matches!(err, Err(ConflictError::InvalidIndex(0))));

        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        // 只有 1 个冲突，索引 1 越界
        let err = detector.resolve_conflict(1, ResolveAction::DiscardLoser);
        assert!(matches!(err, Err(ConflictError::InvalidIndex(1))));
    }

    // -----------------------------------------------------------------
    //  4. 集成场景
    // -----------------------------------------------------------------

    #[test]
    fn test_multi_master_cluster_basic() {
        // 3 节点集群基本写入 + 读取
        let mut cluster = MultiMasterCluster::new(3, ConflictResolution::LastLsnWins, 100);

        // 各节点写不同 key（无冲突）
        cluster.write(0, b"k0".to_vec(), b"v0".to_vec());
        cluster.write(1, b"k1".to_vec(), b"v1".to_vec());
        cluster.write(2, b"k2".to_vec(), b"v2".to_vec());

        assert_eq!(cluster.read(b"k0").unwrap().value, b"v0");
        assert_eq!(cluster.read(b"k1").unwrap().value, b"v1");
        assert_eq!(cluster.read(b"k2").unwrap().value, b"v2");
        assert_eq!(cluster.conflicts().len(), 0);

        // 节点日志各有 1 条
        assert_eq!(cluster.node_log(0).len(), 1);
        assert_eq!(cluster.node_log(1).len(), 1);
        assert_eq!(cluster.node_log(2).len(), 1);
    }

    #[test]
    fn test_multi_master_cluster_conflict_detection() {
        // 3 节点集群冲突检测
        let mut cluster = MultiMasterCluster::new(3, ConflictResolution::LastLsnWins, 100);

        // 节点 0 和节点 1 同时写同一 key
        cluster.write(0, b"k".to_vec(), b"from0".to_vec());
        cluster.write(1, b"k".to_vec(), b"from1".to_vec());

        // 检测到 1 个冲突
        assert_eq!(cluster.conflicts().len(), 1);
        // 节点 0 胜出（LSN 相等时 node_id 小的胜）
        assert_eq!(cluster.read(b"k").unwrap().value, b"from0");

        // 节点 2 也写同一 key
        cluster.write(2, b"k".to_vec(), b"from2".to_vec());
        assert_eq!(cluster.conflicts().len(), 2);
    }

    #[test]
    fn test_multi_master_cluster_conflict_resolution() {
        // 3 节点集群冲突解决
        let mut cluster = MultiMasterCluster::new(3, ConflictResolution::LastLsnWins, 100);

        cluster.write(0, b"k".to_vec(), b"v0".to_vec());
        cluster.write(1, b"k".to_vec(), b"v1".to_vec());
        assert_eq!(cluster.conflicts().len(), 1);

        // 用 ApplyLoser 强制应用落败者
        cluster
            .resolve_conflict(0, ResolveAction::ApplyLoser)
            .unwrap();
        assert_eq!(cluster.conflicts().len(), 0);
        assert_eq!(cluster.read(b"k").unwrap().value, b"v1");
    }

    #[test]
    fn test_no_data_loss_after_resolution() {
        // 解决冲突后不丢失数据（胜者或落败者至少有一个的值可读）

        // DiscardLoser：胜者值可读
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 10);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        detector
            .resolve_conflict(0, ResolveAction::DiscardLoser)
            .unwrap();
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"v1"); // 胜者值保留

        // ApplyLoser：落败者值可读
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 10);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        detector
            .resolve_conflict(0, ResolveAction::ApplyLoser)
            .unwrap();
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"v2"); // 落败者值应用

        // MergeBoth：两个值合并后可读
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 10);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        detector
            .resolve_conflict(0, ResolveAction::MergeBoth)
            .unwrap();
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"v1v2"); // 合并值
    }

    #[test]
    fn test_queue_full() {
        // 冲突队列满后覆盖最旧条目
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 2);

        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        // 冲突 1：节点 2
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        assert_eq!(detector.conflict_count(), 1);

        // 冲突 2：节点 3
        detector.accept(make_op(3, 1, 1, b"k", b"v3"));
        assert_eq!(detector.conflict_count(), 2);

        // 冲突 3：节点 4 — 队列已满（max=2），覆盖最旧
        detector.accept(make_op(4, 1, 1, b"k", b"v4"));
        assert_eq!(detector.conflict_count(), 2); // 仍为 2

        // 最旧的冲突（节点 2）被淘汰，剩余节点 3 和节点 4 的冲突
        let conflicts = detector.conflicts();
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].loser.node_id, 3);
        assert_eq!(conflicts[1].loser.node_id, 4);
    }

    // -----------------------------------------------------------------
    //  5. 冲突日志
    // -----------------------------------------------------------------

    #[test]
    fn test_conflict_log_record_and_resolve() {
        // 记录冲突 + 标记已解决
        let mut log = ConflictLog::new();
        assert_eq!(log.entries().len(), 0);
        assert_eq!(log.resolved(), 0);
        assert_eq!(log.pending(), 0);

        let entry = ConflictEntry {
            winner: make_op(1, 1, 1, b"k", b"v1"),
            loser: make_op(2, 1, 1, b"k", b"v2"),
            detected_at: 42,
            resolution: ConflictResolution::LastLsnWins,
        };
        log.record(entry);
        assert_eq!(log.entries().len(), 1);
        assert_eq!(log.resolved(), 0);
        assert_eq!(log.pending(), 1);

        log.mark_resolved(0);
        assert_eq!(log.resolved(), 1);
        assert_eq!(log.pending(), 0);

        // 越界索引不执行
        log.mark_resolved(99);
        assert_eq!(log.resolved(), 1);
    }

    #[test]
    fn test_conflict_log_encode_decode() {
        // 编码解码往返测试
        let mut log = ConflictLog::new();
        log.record(ConflictEntry {
            winner: make_op(1, 10, 100, b"key1", b"val1"),
            loser: make_op(2, 20, 200, b"key1", b"val2"),
            detected_at: 42,
            resolution: ConflictResolution::LastLsnWins,
        });
        log.record(ConflictEntry {
            winner: make_op(3, 30, 300, b"key2", b""),
            loser: make_op(4, 40, 400, b"key2", b"nonempty"),
            detected_at: 99,
            resolution: ConflictResolution::NodeIdWins,
        });
        log.mark_resolved(0);

        let encoded = log.encode();
        assert!(!encoded.is_empty());

        let decoded = ConflictLog::decode(&encoded).unwrap();
        assert_eq!(decoded.entries().len(), 2);
        assert_eq!(decoded.resolved(), 1);
        assert_eq!(decoded.pending(), 1);

        // 验证条目内容
        let e0 = &decoded.entries()[0];
        assert_eq!(e0.winner.node_id, 1);
        assert_eq!(e0.winner.lsn, 10);
        assert_eq!(e0.winner.timestamp, 100);
        assert_eq!(e0.winner.key, b"key1");
        assert_eq!(e0.winner.value, b"val1");
        assert_eq!(e0.loser.node_id, 2);
        assert_eq!(e0.detected_at, 42);
        assert_eq!(e0.resolution, ConflictResolution::LastLsnWins);

        let e1 = &decoded.entries()[1];
        assert_eq!(e1.winner.node_id, 3);
        assert_eq!(e1.winner.value, b"");
        assert_eq!(e1.loser.value, b"nonempty");
        assert_eq!(e1.resolution, ConflictResolution::NodeIdWins);

        // 解码损坏数据应报错
        assert!(ConflictLog::decode(&[]).is_err());
        assert!(ConflictLog::decode(&[0xFF, 0xFF, 0xFF, 0xFF]).is_err());
    }

    // -----------------------------------------------------------------
    //  6. 边界情况
    // -----------------------------------------------------------------

    #[test]
    fn test_same_node_overwrite() {
        // 同节点覆盖（非冲突，LSN 递增）
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b"v1"));
        detector.accept(make_op(1, 2, 2, b"k", b"v2"));
        detector.accept(make_op(1, 3, 3, b"k", b"v3"));
        assert_eq!(detector.conflict_count(), 0);
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"v3");
        assert_eq!(op.lsn, 3);
    }

    #[test]
    fn test_same_node_older_lsn_ignored() {
        // 同节点旧 LSN 被忽略
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 5, 5, b"k", b"new"));
        // 旧 LSN 写入应被忽略
        detector.accept(make_op(1, 1, 1, b"k", b"old"));
        assert_eq!(detector.conflict_count(), 0);
        let op = detector.get(b"k").unwrap();
        assert_eq!(op.value, b"new");
        assert_eq!(op.lsn, 5);
    }

    #[test]
    fn test_empty_key() {
        // 空键处理
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"", b"v1"));
        assert_eq!(detector.get(b"").unwrap().value, b"v1");

        // 不同节点写空键 → 冲突
        detector.accept(make_op(2, 1, 1, b"", b"v2"));
        assert_eq!(detector.conflict_count(), 1);

        // 编解码空键
        let op = make_op(1, 1, 1, b"", b"v");
        let encoded = op.encode();
        let decoded = WriteOperation::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    #[test]
    fn test_empty_value() {
        // 空值处理
        let mut detector = MultiMasterDetector::new(ConflictResolution::LastLsnWins, 100);
        detector.accept(make_op(1, 1, 1, b"k", b""));
        assert_eq!(detector.get(b"k").unwrap().value, b"");

        // MergeBoth 合并空值
        detector.accept(make_op(2, 1, 1, b"k", b"v2"));
        detector
            .resolve_conflict(0, ResolveAction::MergeBoth)
            .unwrap();
        // 合并 "" || "v2" = "v2"
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");

        // 编解码空值
        let op = make_op(1, 1, 1, b"k", b"");
        let encoded = op.encode();
        let decoded = WriteOperation::decode(&encoded).unwrap();
        assert_eq!(op, decoded);
    }

    // -----------------------------------------------------------------
    //  7. HLC 混合逻辑时钟（Phase 8.11）
    // -----------------------------------------------------------------

    // 7.1 HLC 基本功能

    #[test]
    fn test_hlc_timestamp_ordering() {
        // HlcTimestamp 的 Ord 排序正确
        let t1 = HlcTimestamp::new(100, 1);
        let t2 = HlcTimestamp::new(100, 2);
        let t3 = HlcTimestamp::new(101, 0);

        // 先比较 l，l 相等时比较 c
        assert!(t1 < t2); // 同 l，c 小的在前
        assert!(t2 < t3); // l 小的在前
        assert!(t1 < t3);

        // to_u128 编码后大小关系一致
        assert!(t1.to_u128() < t2.to_u128());
        assert!(t2.to_u128() < t3.to_u128());

        // compare 方法与 Ord 一致
        assert_eq!(t1.compare(&t2), Ordering::Less);
        assert_eq!(t2.compare(&t1), Ordering::Greater);
        assert_eq!(t1.compare(&t1), Ordering::Equal);
    }

    #[test]
    fn test_hlc_clock_local_events_monotonic() {
        // 同一节点本地事件 HLC 单调递增
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock = make_hlc_clock(time.clone(), 0);

        let t1 = clock.now(); // (1000, 0)
        let t2 = clock.now(); // (1000, 1)
        let t3 = clock.now(); // (1000, 2)

        assert!(t1 < t2);
        assert!(t2 < t3);

        // 推进物理时间 → l 更新，c 重置
        time.store(2000, AtomicOrdering::SeqCst);
        let t4 = clock.now(); // (2000, 0)
        assert!(t3 < t4);
        assert_eq!(t4.l, 2000);
        assert_eq!(t4.c, 0);
    }

    #[test]
    fn test_hlc_clock_send_receive() {
        // send 和 receive 返回的 HLC 正确
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock1 = make_hlc_clock(time.clone(), 0);
        let mut clock2 = make_hlc_clock(time.clone(), 0);

        // clock1 发送消息：首次事件，物理时钟推进，c=0
        let msg = clock1.send();
        assert_eq!(msg.l, 1000);
        assert_eq!(msg.c, 0);

        // clock2 接收消息
        let received = clock2.receive(msg);
        // new_l = max(0, 1000, 1000) = 1000
        // new_l == msg.l → c' = msg.c + 1 = 1
        assert_eq!(received.l, 1000);
        assert_eq!(received.c, 1);

        // clock2 的内部状态已更新
        assert_eq!(clock2.peek(), received);
    }

    #[test]
    fn test_hlc_clock_causality_preserved() {
        // 因果性：A → B 时 hlc(A) < hlc(B)
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock_a = make_hlc_clock(time.clone(), 0);
        let mut clock_b = make_hlc_clock(time.clone(), 0);

        // A 发生事件 a1
        let a1 = clock_a.now(); // (1000, 0)

        // A 发送给 B（消息携带 a1 的时间戳）
        // B 接收消息，发生事件 b1
        let b1 = clock_b.receive(a1); // (1000, 1)

        // 因果性：a1 → b1，所以 hlc(a1) < hlc(b1)
        assert!(a1 < b1);
    }

    #[test]
    fn test_hlc_clock_concurrent_no_ordering() {
        // 并发事件无因果顺序（hlc 可能相等，表示并发）
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock_a = make_hlc_clock(time.clone(), 0);
        let mut clock_b = make_hlc_clock(time.clone(), 0);

        // A 和 B 独立发生事件（无消息交换）→ 并发
        let a1 = clock_a.now(); // (1000, 0)
        let b1 = clock_b.now(); // (1000, 0)

        // 并发事件的 HLC 相等（都读到相同的物理时间，计数器都从 0 开始）
        // 这表示它们之间没有因果关系
        assert_eq!(a1, b1);

        // 但同节点的后续事件 HLC 严格递增
        let a2 = clock_a.now(); // (1000, 1)
        assert!(a1 < a2);

        let b2 = clock_b.now(); // (1000, 1)
        assert!(b1 < b2);
    }

    // 7.2 时钟偏差下的正确性

    #[test]
    fn test_hlc_clock_skew_100ms() {
        // 节点 A 时钟快 100ms，节点 B 时钟慢 100ms，HLC 仍然正确排序因果事件
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock_a = make_hlc_clock(time.clone(), 100); // A 快 100ms → pt=1100
        let mut clock_b = make_hlc_clock(time.clone(), -100); // B 慢 100ms → pt=900

        // A 发生事件
        let a1 = clock_a.now(); // (1100, 0)

        // A 发送给 B
        let b1 = clock_b.receive(a1);
        // B 的 pt=900，但 new_l = max(0, 1100, 900) = 1100
        // new_l == msg.l(1100) → c' = msg.c + 1 = 1
        assert_eq!(b1.l, 1100);
        assert_eq!(b1.c, 1);

        // 因果性：a1 → b1，所以 hlc(a1) < hlc(b1)
        assert!(a1 < b1);
    }

    #[test]
    fn test_hlc_clock_skew_negative() {
        // 节点时钟回退（物理时钟减少），HLC 的 l 部分单调不减
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock = make_hlc_clock(time.clone(), 0);

        // 第一次事件
        let t1 = clock.now();
        assert_eq!(t1.l, 1000);

        // 物理时钟回退
        time.store(500, AtomicOrdering::SeqCst);

        // 第二次事件 — l 不应回退
        let t2 = clock.now();
        assert!(t1 < t2); // HLC 单调递增
        assert_eq!(t2.l, 1000); // l 保持不变（不回退到 500）
        assert_eq!(t2.c, 1); // c 递增
    }

    #[test]
    fn test_hlc_clock_skew_concurrent_writes() {
        // 两节点时钟偏差下并发写，HLC 正确处理因果关系
        let time = Arc::new(AtomicU64::new(1000));
        let mut clock_a = make_hlc_clock(time.clone(), 100); // A 快 100ms → pt=1100
        let mut clock_b = make_hlc_clock(time.clone(), -100); // B 慢 100ms → pt=900

        // A 和 B 并发写（无消息交换）
        let a1 = clock_a.now(); // (1100, 0)
        let b1 = clock_b.now(); // (900, 0)

        // 物理时钟偏差导致 A 的 HLC > B 的 HLC（并发事件，不代表因果）
        assert!(a1 > b1);

        // 但如果 B 先收到 A 的消息，再写，则 B 的 HLC > A 的 HLC（因果保证）
        let b2 = clock_b.receive(a1);
        // new_l = max(900, 1100, 900) = 1100
        // new_l == msg.l(1100) → c' = 0 + 1 = 1
        assert!(b2 > a1); // 因果顺序保证
    }

    // 7.3 HLC 多主冲突检测

    #[test]
    fn test_hlc_multi_master_no_conflict() {
        // 不同节点写不同 key，无冲突
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0));
        detector.register_node(2, make_hlc_clock(time.clone(), 0));

        let r1 = detector.write(1, b"k1".to_vec(), b"v1".to_vec());
        let r2 = detector.write(2, b"k2".to_vec(), b"v2".to_vec());

        assert!(matches!(r1, HlcAcceptResult::Accepted(_)));
        assert!(matches!(r2, HlcAcceptResult::Accepted(_)));
        assert_eq!(detector.conflicts().len(), 0);

        assert_eq!(detector.get(b"k1").unwrap().value, b"v1");
        assert_eq!(detector.get(b"k2").unwrap().value, b"v2");
    }

    #[test]
    fn test_hlc_multi_master_conflict_detection() {
        // 两节点写同一 key → 冲突检测，HLC 大的胜出
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0)); // pt=1000
        detector.register_node(2, make_hlc_clock(time.clone(), 100)); // pt=1100

        // 节点 1 先写
        let r1 = detector.write(1, b"k".to_vec(), b"v1".to_vec());
        assert!(matches!(r1, HlcAcceptResult::Accepted(_)));

        // 节点 2 后写同一 key
        let r2 = detector.write(2, b"k".to_vec(), b"v2".to_vec());

        // 节点 2 的 HLC=(1100,0) > 节点 1 的 HLC=(1000,0) → 节点 2 胜出
        assert!(matches!(r2, HlcAcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.conflicts().len(), 1);
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");

        // 冲突记录：winner=node2, loser=node1
        let c = &detector.conflicts()[0];
        assert_eq!(c.winner.node_id, 2);
        assert_eq!(c.loser.node_id, 1);
    }

    #[test]
    fn test_hlc_multi_master_causality_ordering() {
        // A 发送给 B 后 B 写同一 key，B 的写操作 HLC > A → B 胜出（因果正确）
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0));
        detector.register_node(2, make_hlc_clock(time.clone(), 0));

        // 节点 1 写 key K
        let r1 = detector.write(1, b"k".to_vec(), b"v1".to_vec());
        let op1 = match r1 {
            HlcAcceptResult::Accepted(op) => op,
            _ => panic!("expected Accepted"),
        };
        // op1.hlc = (1000, 0)

        // 节点 2 接收节点 1 的写操作（更新时钟，建立因果关系）
        detector.receive(2, op1.clone());
        // 节点 2 的时钟更新为 (1000, 1)

        // 节点 2 写同一 key
        let r2 = detector.write(2, b"k".to_vec(), b"v2".to_vec());
        // 节点 2 的 now() → (1000, 2) > op1.hlc=(1000,0) → B 胜出
        assert!(matches!(r2, HlcAcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");

        // 因果正确：op2.hlc > op1.hlc
        let op2 = detector.get(b"k").unwrap();
        assert!(op2.hlc > op1.hlc);
    }

    #[test]
    fn test_hlc_multi_master_clock_skew() {
        // 时钟偏差 100ms 下，旧节点的写操作不会误判为新操作
        // 场景：节点 A（慢时钟 -100ms）先写，节点 B（快时钟 +100ms）后写
        // 有因果关系（A → B）时，HLC 保证 B 胜出
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), -100)); // A 慢 100ms → pt=900
        detector.register_node(2, make_hlc_clock(time.clone(), 100)); // B 快 100ms → pt=1100

        // 节点 A（慢时钟）先写
        let r1 = detector.write(1, b"k".to_vec(), b"v1".to_vec());
        let op1 = match r1 {
            HlcAcceptResult::Accepted(op) => op,
            _ => panic!("expected Accepted"),
        };
        // op1.hlc = (900, 0)

        // 节点 B 接收 A 的写操作（建立因果关系）
        detector.receive(2, op1.clone());
        // 节点 B 的时钟：receive((900,0)), pt=1100
        // new_l = max(0, 900, 1100) = 1100, c=0

        // 节点 B 写同一 key
        let r2 = detector.write(2, b"k".to_vec(), b"v2".to_vec());
        // 节点 B 的 now() → pt=1100 <= l=1100, c=1 → (1100, 1)
        // (1100, 1) > (900, 0) → B 胜出（因果正确）
        assert!(matches!(r2, HlcAcceptResult::WonAsWinner { .. }));
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");
    }

    // 7.4 集成测试

    #[test]
    fn test_hlc_multi_master_three_nodes() {
        // 3 节点集群，时钟偏差各异，HLC 正确排序
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0)); // 正常 pt=1000
        detector.register_node(2, make_hlc_clock(time.clone(), 100)); // 快 100ms pt=1100
        detector.register_node(3, make_hlc_clock(time.clone(), -100)); // 慢 100ms pt=900

        // 节点 1 写 key A
        let r1 = detector.write(1, b"A".to_vec(), b"v1".to_vec());
        let op1 = match r1 {
            HlcAcceptResult::Accepted(op) => op,
            _ => panic!("expected Accepted"),
        };
        // op1.hlc = (1000, 0)

        // 节点 2 接收并写 key A
        detector.receive(2, op1.clone());
        let r2 = detector.write(2, b"A".to_vec(), b"v2".to_vec());
        assert!(matches!(r2, HlcAcceptResult::WonAsWinner { .. }));
        let op2 = detector.get(b"A").unwrap().clone();
        // op2.hlc = (1100, 1)

        // 节点 3 接收节点 2 的写并写 key A
        detector.receive(3, op2.clone());
        let r3 = detector.write(3, b"A".to_vec(), b"v3".to_vec());
        assert!(matches!(r3, HlcAcceptResult::WonAsWinner { .. }));
        let op3 = detector.get(b"A").unwrap().clone();
        // op3.hlc = (1100, 3)

        // 最终节点 3 的值胜出
        assert_eq!(op3.value, b"v3");

        // HLC 单调递增
        assert!(op1.hlc < op2.hlc);
        assert!(op2.hlc < op3.hlc);
    }

    #[test]
    fn test_hlc_multi_master_resolve_conflict() {
        // 冲突解决后状态正确
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0)); // pt=1000
        detector.register_node(2, make_hlc_clock(time.clone(), 100)); // pt=1100

        // 制造冲突
        detector.write(1, b"k".to_vec(), b"v1".to_vec());
        detector.write(2, b"k".to_vec(), b"v2".to_vec());
        assert_eq!(detector.conflicts().len(), 1);

        // 节点 2 胜出（HLC 更大）
        assert_eq!(detector.get(b"k").unwrap().value, b"v2");

        // 解决：强制应用落败者（节点 1 的值）
        detector
            .resolve_conflict(0, ResolveAction::ApplyLoser)
            .unwrap();
        assert_eq!(detector.conflicts().len(), 0);
        assert_eq!(detector.get(b"k").unwrap().value, b"v1");
    }

    #[test]
    fn test_hlc_multi_master_long_chain() {
        // 长链因果：A→B→C→A→B，HLC 单调递增
        let time = Arc::new(AtomicU64::new(1000));
        let mut detector = HlcMultiMasterDetector::new(100);
        detector.register_node(1, make_hlc_clock(time.clone(), 0));
        detector.register_node(2, make_hlc_clock(time.clone(), 0));
        detector.register_node(3, make_hlc_clock(time.clone(), 0));

        // A 写
        let r1 = detector.write(1, b"k".to_vec(), b"v1".to_vec());
        let op1 = match r1 {
            HlcAcceptResult::Accepted(op) => op,
            _ => panic!("expected Accepted"),
        };
        // op1.hlc = (1000, 0)

        // A → B: B 接收并写
        detector.receive(2, op1.clone());
        let r2 = detector.write(2, b"k".to_vec(), b"v2".to_vec());
        assert!(matches!(r2, HlcAcceptResult::WonAsWinner { .. }));
        let op2 = detector.get(b"k").unwrap().clone();
        // op2.hlc = (1000, 2)

        // B → C: C 接收并写
        detector.receive(3, op2.clone());
        let r3 = detector.write(3, b"k".to_vec(), b"v3".to_vec());
        assert!(matches!(r3, HlcAcceptResult::WonAsWinner { .. }));
        let op3 = detector.get(b"k").unwrap().clone();
        // op3.hlc = (1000, 4)

        // C → A: A 接收并写
        detector.receive(1, op3.clone());
        let r4 = detector.write(1, b"k".to_vec(), b"v4".to_vec());
        assert!(matches!(r4, HlcAcceptResult::WonAsWinner { .. }));
        let op4 = detector.get(b"k").unwrap().clone();
        // op4.hlc = (1000, 6)

        // A → B: B 接收并写
        detector.receive(2, op4.clone());
        let r5 = detector.write(2, b"k".to_vec(), b"v5".to_vec());
        assert!(matches!(r5, HlcAcceptResult::WonAsWinner { .. }));
        let op5 = detector.get(b"k").unwrap().clone();
        // op5.hlc = (1000, 8)

        // HLC 单调递增
        assert!(op1.hlc < op2.hlc);
        assert!(op2.hlc < op3.hlc);
        assert!(op3.hlc < op4.hlc);
        assert!(op4.hlc < op5.hlc);

        // 最终值
        assert_eq!(op5.value, b"v5");
    }
}
