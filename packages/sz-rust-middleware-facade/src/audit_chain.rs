// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 审计日志链式哈希不可篡改 + 查询（spec §5.17）
//!
//! 链式哈希保证审计记录追加写不可篡改。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 审计记录五要素（spec §6.17 规则 1）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    /// 操作者
    pub actor: String,
    /// 操作类型
    pub action: String,
    /// 操作对象
    pub target: String,
    /// 时间
    pub timestamp: DateTime<Utc>,
    /// 结果
    pub result: String,
    /// 链式哈希（hash_n = SHA256(hash_{n-1} || record_n)）
    pub chain_hash: String,
    /// 脱敏后的字段（spec §6.17 规则 4）
    pub fields: HashMap<String, String>,
}

impl AuditRecord {
    /// 创建审计记录（不含 chain_hash，由 ChainHashAuditor 填充）
    pub fn new(
        actor: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        result: impl Into<String>,
    ) -> Self {
        Self {
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            timestamp: Utc::now(),
            result: result.into(),
            chain_hash: String::new(),
            fields: HashMap::new(),
        }
    }

    /// 添加字段
    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// 审计查询参数（spec §5.17 规则 4）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditQuery {
    /// 操作者过滤
    pub actor: Option<String>,
    /// 时间范围
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// 操作类型过滤
    pub action: Option<String>,
    /// 操作对象过滤
    pub target: Option<String>,
    /// 分页
    pub limit: usize,
}

impl Default for AuditQuery {
    fn default() -> Self {
        Self {
            actor: None,
            time_range: None,
            action: None,
            target: None,
            limit: 100,
        }
    }
}

/// 审计错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum AuditError {
    /// 链式哈希校验失败（spec §5.17 异常 2）
    #[error("链式哈希校验失败，记录可能被篡改: {0}")]
    ChainHashFailed(String),
}

/// 链式哈希审计器
pub struct ChainHashAuditor {
    last_hash: Mutex<String>,
}

impl ChainHashAuditor {
    /// 创建链式哈希审计器（初始哈希为空字符串）
    pub fn new() -> Self {
        Self {
            last_hash: Mutex::new(String::new()),
        }
    }

    /// 计算下一条记录的链式哈希（spec §5.17 规则 2）
    ///
    /// hash_n = SHA256(hash_{n-1} || record_n)
    pub fn next_hash(&self, record: &AuditRecord) -> String {
        let mut hasher = Sha256::new();
        let last = self.last_hash.lock().clone();
        hasher.update(last.as_bytes());
        hasher.update(record.actor.as_bytes());
        hasher.update(record.action.as_bytes());
        hasher.update(record.target.as_bytes());
        hasher.update(record.timestamp.to_rfc3339().as_bytes());
        hasher.update(record.result.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// 追加记录（计算哈希并更新 last_hash）
    pub fn append(&self, record: &mut AuditRecord) {
        let hash = self.next_hash(record);
        record.chain_hash = hash.clone();
        *self.last_hash.lock() = hash;
    }

    /// 校验完整性（逐条重算哈希比对，spec §5.17 异常 2）
    pub fn verify_integrity(&self, records: &[AuditRecord]) -> Result<(), AuditError> {
        let mut expected_prev = String::new();
        for (i, record) in records.iter().enumerate() {
            let mut hasher = Sha256::new();
            hasher.update(expected_prev.as_bytes());
            hasher.update(record.actor.as_bytes());
            hasher.update(record.action.as_bytes());
            hasher.update(record.target.as_bytes());
            hasher.update(record.timestamp.to_rfc3339().as_bytes());
            hasher.update(record.result.as_bytes());
            let expected_hash = hex::encode(hasher.finalize());

            if record.chain_hash != expected_hash {
                return Err(AuditError::ChainHashFailed(format!("记录 {i} 哈希不匹配")));
            }
            expected_prev = record.chain_hash.clone();
        }
        Ok(())
    }

    /// 获取当前哈希
    pub fn current_hash(&self) -> String {
        self.last_hash.lock().clone()
    }
}

impl Default for ChainHashAuditor {
    fn default() -> Self {
        Self::new()
    }
}

/// 审计查询（过滤+分页）
pub fn query_audit(records: &[AuditRecord], q: &AuditQuery) -> Vec<AuditRecord> {
    records
        .iter()
        .filter(|r| {
            if let Some(ref actor) = q.actor {
                if &r.actor != actor {
                    return false;
                }
            }
            if let Some(ref action) = q.action {
                if &r.action != action {
                    return false;
                }
            }
            if let Some(ref target) = q.target {
                if &r.target != target {
                    return false;
                }
            }
            if let Some((start, end)) = q.time_range {
                if r.timestamp < start || r.timestamp > end {
                    return false;
                }
            }
            true
        })
        .take(q.limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_hash_append() {
        let auditor = ChainHashAuditor::new();
        let mut r1 = AuditRecord::new("user1", "create", "article:1", "success");
        auditor.append(&mut r1);
        assert!(!r1.chain_hash.is_empty());

        let mut r2 = AuditRecord::new("user2", "update", "article:1", "success");
        auditor.append(&mut r2);
        assert_ne!(r1.chain_hash, r2.chain_hash);
    }

    #[test]
    fn test_verify_integrity_ok() {
        let auditor = ChainHashAuditor::new();
        let mut records = Vec::new();
        for i in 0..5 {
            let mut r = AuditRecord::new("user1", "action", &format!("target:{i}"), "success");
            auditor.append(&mut r);
            records.push(r);
        }
        assert!(auditor.verify_integrity(&records).is_ok());
    }

    #[test]
    fn test_verify_integrity_tampered() {
        let auditor = ChainHashAuditor::new();
        let mut records = Vec::new();
        for i in 0..3 {
            let mut r = AuditRecord::new("user1", "action", &format!("target:{i}"), "success");
            auditor.append(&mut r);
            records.push(r);
        }
        records[1].result = "tampered".to_string();
        let result = auditor.verify_integrity(&records);
        assert!(matches!(result, Err(AuditError::ChainHashFailed(_))));
    }

    #[test]
    fn test_query_audit_by_actor() {
        let records = vec![
            AuditRecord::new("user1", "create", "t1", "ok"),
            AuditRecord::new("user2", "update", "t2", "ok"),
            AuditRecord::new("user1", "delete", "t3", "ok"),
        ];
        let q = AuditQuery {
            actor: Some("user1".to_string()),
            ..Default::default()
        };
        let result = query_audit(&records, &q);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_query_audit_by_action() {
        let records = vec![
            AuditRecord::new("u1", "create", "t1", "ok"),
            AuditRecord::new("u2", "update", "t2", "ok"),
            AuditRecord::new("u3", "create", "t3", "ok"),
        ];
        let q = AuditQuery {
            action: Some("create".to_string()),
            ..Default::default()
        };
        let result = query_audit(&records, &q);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_query_audit_limit() {
        let records: Vec<AuditRecord> = (0..10)
            .map(|i| AuditRecord::new("u1", "action", &format!("t{i}"), "ok"))
            .collect();
        let q = AuditQuery {
            limit: 3,
            ..Default::default()
        };
        let result = query_audit(&records, &q);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_audit_record_with_field() {
        let record = AuditRecord::new("u1", "login", "system", "success")
            .with_field("ip", "192.168.1.1")
            .with_field("device", "mobile");
        assert_eq!(record.fields.get("ip").unwrap(), "192.168.1.1");
        assert_eq!(record.fields.get("device").unwrap(), "mobile");
    }

    #[test]
    fn test_current_hash() {
        let auditor = ChainHashAuditor::new();
        assert!(auditor.current_hash().is_empty());
        let mut r = AuditRecord::new("u1", "a", "t", "ok");
        auditor.append(&mut r);
        assert_eq!(auditor.current_hash(), r.chain_hash);
    }
}
