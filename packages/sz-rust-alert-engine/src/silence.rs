// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 告警静默管理器
//!
//! 对应 tasks.md §6.3：同类告警静默期（默认 30min）内不重复推送。
//!
//! ## 设计
//!
//! - `SilenceManager` 维护 `rule_id -> SilenceEntry` 映射
//! - `record_alert(rule_id)` 记录告警触发时间
//! - `should_alert(rule_id)` 检查是否在静默期内：若距上次告警 < default_silence，返回 false

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// 静默条目
#[derive(Debug, Clone)]
pub struct SilenceEntry {
    /// 规则 ID
    pub rule_id: String,
    /// 最后一次告警时间
    pub last_alert_at: Instant,
    /// 静默时长（覆盖默认值）
    pub silence_duration: Duration,
}

/// 告警静默管理器
///
/// 同类告警在静默期内不重复推送（spec 5.7.1 规则 4）
pub struct SilenceManager {
    silences: RwLock<HashMap<String, SilenceEntry>>,
    default_silence: Duration,
}

impl SilenceManager {
    /// 创建静默管理器（默认静默期 30 分钟）
    pub fn new() -> Self {
        Self::with_default_silence(Duration::from_secs(30 * 60))
    }

    /// 创建指定默认静默期的管理器
    pub fn with_default_silence(default_silence: Duration) -> Self {
        Self {
            silences: RwLock::new(HashMap::new()),
            default_silence,
        }
    }

    /// 检查规则是否应该告警（静默期内返回 false）
    pub fn should_alert(&self, rule_id: &str) -> bool {
        let silences = self.silences.read().unwrap();
        match silences.get(rule_id) {
            Some(entry) => {
                let elapsed = entry.last_alert_at.elapsed();
                elapsed >= entry.silence_duration
            }
            None => true,
        }
    }

    /// 记录告警触发（更新最后告警时间）
    pub fn record_alert(&self, rule_id: &str) {
        let mut silences = self.silences.write().unwrap();
        silences.insert(
            rule_id.to_string(),
            SilenceEntry {
                rule_id: rule_id.to_string(),
                last_alert_at: Instant::now(),
                silence_duration: self.default_silence,
            },
        );
    }

    /// 记录告警触发（指定自定义静默期）
    pub fn record_alert_with_silence(&self, rule_id: &str, silence: Duration) {
        let mut silences = self.silences.write().unwrap();
        silences.insert(
            rule_id.to_string(),
            SilenceEntry {
                rule_id: rule_id.to_string(),
                last_alert_at: Instant::now(),
                silence_duration: silence,
            },
        );
    }

    /// 清除指定规则的静默记录
    pub fn clear(&self, rule_id: &str) {
        let mut silences = self.silences.write().unwrap();
        silences.remove(rule_id);
    }

    /// 清除所有静默记录
    pub fn clear_all(&self) {
        let mut silences = self.silences.write().unwrap();
        silences.clear();
    }

    /// 获取当前静默中的规则数量
    pub fn silenced_count(&self) -> usize {
        let silences = self.silences.read().unwrap();
        silences
            .iter()
            .filter(|(_, entry)| entry.last_alert_at.elapsed() < entry.silence_duration)
            .count()
    }

    /// 获取默认静默期
    pub fn default_silence(&self) -> Duration {
        self.default_silence
    }
}

impl Default for SilenceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_should_alert_no_record() {
        let mgr = SilenceManager::new();
        assert!(mgr.should_alert("rule-1"), "未记录的规则应允许告警");
    }

    #[test]
    fn test_silence_after_record() {
        let mgr = SilenceManager::with_default_silence(Duration::from_millis(100));
        mgr.record_alert("rule-1");
        assert!(!mgr.should_alert("rule-1"), "记录后应立即进入静默期");
    }

    #[test]
    fn test_silence_expires() {
        let mgr = SilenceManager::with_default_silence(Duration::from_millis(50));
        mgr.record_alert("rule-1");
        assert!(!mgr.should_alert("rule-1"));
        thread::sleep(Duration::from_millis(60));
        assert!(mgr.should_alert("rule-1"), "静默期过后应允许告警");
    }

    #[test]
    fn test_different_rules_independent() {
        let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));
        mgr.record_alert("rule-1");
        assert!(!mgr.should_alert("rule-1"));
        assert!(mgr.should_alert("rule-2"), "不同规则不受静默影响");
    }

    #[test]
    fn test_custom_silence_duration() {
        let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));
        mgr.record_alert_with_silence("rule-1", Duration::from_millis(30));
        assert!(!mgr.should_alert("rule-1"));
        thread::sleep(Duration::from_millis(40));
        assert!(mgr.should_alert("rule-1"), "自定义静默期应生效");
    }

    #[test]
    fn test_clear_silence() {
        let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));
        mgr.record_alert("rule-1");
        assert!(!mgr.should_alert("rule-1"));
        mgr.clear("rule-1");
        assert!(mgr.should_alert("rule-1"), "清除后应允许告警");
    }

    #[test]
    fn test_clear_all() {
        let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));
        mgr.record_alert("rule-1");
        mgr.record_alert("rule-2");
        mgr.clear_all();
        assert!(mgr.should_alert("rule-1"));
        assert!(mgr.should_alert("rule-2"));
    }

    #[test]
    fn test_silenced_count() {
        let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));
        mgr.record_alert("rule-1");
        mgr.record_alert("rule-2");
        assert_eq!(mgr.silenced_count(), 2);
        mgr.clear("rule-1");
        assert_eq!(mgr.silenced_count(), 1);
    }

    #[test]
    fn test_default_silence_is_30min() {
        let mgr = SilenceManager::new();
        assert_eq!(mgr.default_silence(), Duration::from_secs(30 * 60));
    }
}
