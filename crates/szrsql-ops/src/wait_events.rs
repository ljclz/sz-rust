//! Phase 7d.20 — 等待事件统计（pg_stat_wait）。
//!
//! 提供类似 PostgreSQL pg_stat_wait / Oracle V$SESSION_WAIT 视图的等待事件
//! 聚合统计：每个 `WaitEvent` 的等待次数、总等待时间、最大/平均等待时间。
//!
//! # 设计
//!
//! - 复用 `ash::WaitEvent` / `WaitClass`（Phase 7d.8 已定义）
//! - `WaitEventStats` 单事件统计（total_waits/total_wait_ms/max_wait_ms/avg_wait_ms）
//! - `WaitEventCollector` 收集器：记录每次等待事件，按事件类型聚合
//! - 提供 `to_pg_stat_wait_rows()` 返回 PostgreSQL pg_stat_wait 视图行格式
//! - 提供 `top_wait_events(n)` 返回按总等待时间降序的 Top N
//!
//! # pg_stat_wait 视图字段
//!
//! ```sql
//! SELECT event, total_waits, total_wait_ms, avg_wait_ms, max_wait_ms
//! FROM pg_stat_wait;
//! ```
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_ops::ash::WaitEvent;
//! use szrsql_ops::wait_events::WaitEventCollector;
//!
//! let mut collector = WaitEventCollector::new();
//! collector.record_wait(WaitEvent::DataFileSequentialRead, 15);  // 15ms
//! collector.record_wait(WaitEvent::DataFileSequentialRead, 25);  // 25ms
//! collector.record_wait(WaitEvent::LogFileSync, 100);             // 100ms
//!
//! let rows = collector.to_pg_stat_wait_rows();
//! assert_eq!(rows.len(), 2);
//! ```

use std::collections::HashMap;

use crate::ash::{WaitClass, WaitEvent};

// =====================================================================
//  WaitEventStats — 单事件统计
// =====================================================================

/// 单个等待事件的聚合统计。
#[derive(Debug, Clone, PartialEq)]
pub struct WaitEventStats {
    /// 等待事件类型。
    pub event: WaitEvent,
    /// 事件名称（Oracle 风格，如 "db file sequential read"）。
    pub event_name: String,
    /// 等待类别。
    pub wait_class: WaitClass,
    /// 总等待次数。
    pub total_waits: u64,
    /// 总等待时间（毫秒）。
    pub total_wait_ms: u64,
    /// 最大单次等待时间（毫秒）。
    pub max_wait_ms: u64,
}

impl WaitEventStats {
    /// 创建空统计。
    pub fn new(event: WaitEvent) -> Self {
        Self {
            event,
            event_name: event.to_string(),
            wait_class: event.wait_class(),
            total_waits: 0,
            total_wait_ms: 0,
            max_wait_ms: 0,
        }
    }

    /// 平均等待时间（毫秒）。
    ///
    /// 0 次等待时返回 0。
    pub fn avg_wait_ms(&self) -> f64 {
        if self.total_waits == 0 {
            0.0
        } else {
            self.total_wait_ms as f64 / self.total_waits as f64
        }
    }

    /// 累加一次等待。
    pub fn record(&mut self, wait_ms: u64) {
        self.total_waits += 1;
        self.total_wait_ms += wait_ms;
        if wait_ms > self.max_wait_ms {
            self.max_wait_ms = wait_ms;
        }
    }
}

// =====================================================================
//  PgStatWaitRow — pg_stat_wait 视图行
// =====================================================================

/// pg_stat_wait 视图单行（PostgreSQL 风格）。
#[derive(Debug, Clone, PartialEq)]
pub struct PgStatWaitRow {
    /// 事件名称（如 "db file sequential read"）。
    pub event: String,
    /// 等待类别（如 "User I/O"）。
    pub wait_class: String,
    /// 总等待次数。
    pub total_waits: u64,
    /// 总等待时间（毫秒）。
    pub total_wait_ms: u64,
    /// 平均等待时间（毫秒）。
    pub avg_wait_ms: f64,
    /// 最大单次等待时间（毫秒）。
    pub max_wait_ms: u64,
}

impl From<&WaitEventStats> for PgStatWaitRow {
    fn from(stats: &WaitEventStats) -> Self {
        Self {
            event: stats.event_name.clone(),
            wait_class: stats.wait_class.to_string(),
            total_waits: stats.total_waits,
            total_wait_ms: stats.total_wait_ms,
            avg_wait_ms: stats.avg_wait_ms(),
            max_wait_ms: stats.max_wait_ms,
        }
    }
}

// =====================================================================
//  WaitEventCollector — 等待事件收集器
// =====================================================================

/// 等待事件收集器：按 `WaitEvent` 类型聚合统计。
///
/// 线程安全策略：本类型非 `Sync`，应由单线程或外部锁保护。
pub struct WaitEventCollector {
    /// 按事件类型聚合的统计。
    stats: HashMap<WaitEvent, WaitEventStats>,
}

impl Default for WaitEventCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitEventCollector {
    /// 创建空收集器。
    pub fn new() -> Self {
        Self {
            stats: HashMap::new(),
        }
    }

    /// 记录一次等待事件。
    ///
    /// - `event`：等待事件类型
    /// - `wait_ms`：本次等待时长（毫秒）
    pub fn record_wait(&mut self, event: WaitEvent, wait_ms: u64) {
        self.stats
            .entry(event)
            .or_insert_with(|| WaitEventStats::new(event))
            .record(wait_ms);
    }

    /// 批量记录等待事件（便捷方法）。
    pub fn record_waits(&mut self, event: WaitEvent, wait_ms_list: &[u64]) {
        for &ms in wait_ms_list {
            self.record_wait(event, ms);
        }
    }

    /// 获取指定事件的统计（若存在）。
    pub fn get(&self, event: WaitEvent) -> Option<&WaitEventStats> {
        self.stats.get(&event)
    }

    /// 已统计的事件类型数量。
    pub fn event_count(&self) -> usize {
        self.stats.len()
    }

    /// 总等待次数（所有事件合计）。
    pub fn total_waits(&self) -> u64 {
        self.stats.values().map(|s| s.total_waits).sum()
    }

    /// 总等待时间（毫秒，所有事件合计）。
    pub fn total_wait_ms(&self) -> u64 {
        self.stats.values().map(|s| s.total_wait_ms).sum()
    }

    /// 清空所有统计。
    pub fn clear(&mut self) {
        self.stats.clear();
    }

    /// 返回按总等待时间降序排列的所有事件统计。
    pub fn sorted_by_total_wait(&self) -> Vec<&WaitEventStats> {
        let mut list: Vec<_> = self.stats.values().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.total_wait_ms));
        list
    }

    /// 返回 Top N 等待事件（按总等待时间降序）。
    pub fn top_wait_events(&self, n: usize) -> Vec<&WaitEventStats> {
        let mut list: Vec<_> = self.stats.values().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.total_wait_ms));
        list.into_iter().take(n).collect()
    }

    /// 转换为 pg_stat_wait 视图行列表（按总等待时间降序）。
    ///
    /// 等价于 `SELECT event, wait_class, total_waits, total_wait_ms, avg_wait_ms, max_wait_ms FROM pg_stat_wait ORDER BY total_wait_ms DESC;`
    pub fn to_pg_stat_wait_rows(&self) -> Vec<PgStatWaitRow> {
        self.sorted_by_total_wait()
            .into_iter()
            .map(PgStatWaitRow::from)
            .collect()
    }

    /// 按等待类别聚合统计（返回每个 WaitClass 的合计）。
    pub fn by_wait_class(&self) -> HashMap<WaitClass, WaitClassStats> {
        let mut by_class: HashMap<WaitClass, WaitClassStats> = HashMap::new();
        for stats in self.stats.values() {
            let class = stats.wait_class;
            let entry = by_class
                .entry(class)
                .or_insert_with(|| WaitClassStats::new(class));
            entry.total_waits += stats.total_waits;
            entry.total_wait_ms += stats.total_wait_ms;
            if stats.max_wait_ms > entry.max_wait_ms {
                entry.max_wait_ms = stats.max_wait_ms;
            }
        }
        by_class
    }
}

// =====================================================================
//  WaitClassStats — 等待类别统计
// =====================================================================

/// 按等待类别聚合的统计。
#[derive(Debug, Clone, PartialEq)]
pub struct WaitClassStats {
    /// 等待类别。
    pub wait_class: WaitClass,
    /// 总等待次数。
    pub total_waits: u64,
    /// 总等待时间（毫秒）。
    pub total_wait_ms: u64,
    /// 最大单次等待时间（毫秒）。
    pub max_wait_ms: u64,
}

impl WaitClassStats {
    /// 创建空统计。
    pub fn new(wait_class: WaitClass) -> Self {
        Self {
            wait_class,
            total_waits: 0,
            total_wait_ms: 0,
            max_wait_ms: 0,
        }
    }

    /// 平均等待时间（毫秒）。
    pub fn avg_wait_ms(&self) -> f64 {
        if self.total_waits == 0 {
            0.0
        } else {
            self.total_wait_ms as f64 / self.total_waits as f64
        }
    }
}

// =====================================================================
//  SessionWaitView — 活跃会话视图
// =====================================================================

/// 活跃会话等待视图单行（类似 pg_stat_activity + pg_stat_wait 联合视图）。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionWaitView {
    /// 会话 ID。
    pub session_id: u32,
    /// 当前等待事件（若空闲则为 None）。
    pub current_wait: Option<WaitEvent>,
    /// 当前等待已耗时（毫秒，若未等待则为 0）。
    pub current_wait_ms: u64,
    /// 会话累计等待次数。
    pub session_total_waits: u64,
    /// 会话累计等待时间（毫秒）。
    pub session_total_wait_ms: u64,
}

impl SessionWaitView {
    /// 创建会话等待视图。
    pub fn new(session_id: u32) -> Self {
        Self {
            session_id,
            current_wait: None,
            current_wait_ms: 0,
            session_total_waits: 0,
            session_total_wait_ms: 0,
        }
    }
}

// =====================================================================
//  SessionWaitTracker — 多会话等待追踪
// =====================================================================

/// 多会话等待事件追踪器：维护每个会话的当前等待 + 累计统计。
pub struct SessionWaitTracker {
    /// 会话级视图。
    sessions: HashMap<u32, SessionWaitView>,
}

impl Default for SessionWaitTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionWaitTracker {
    /// 创建空追踪器。
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// 开始会话等待（设置 current_wait）。
    pub fn begin_wait(&mut self, session_id: u32, event: WaitEvent) {
        let view = self
            .sessions
            .entry(session_id)
            .or_insert_with(|| SessionWaitView::new(session_id));
        view.current_wait = Some(event);
        view.current_wait_ms = 0;
    }

    /// 结束会话等待（清除 current_wait，累加统计）。
    pub fn end_wait(&mut self, session_id: u32, wait_ms: u64) {
        let view = self
            .sessions
            .entry(session_id)
            .or_insert_with(|| SessionWaitView::new(session_id));
        view.session_total_waits += 1;
        view.session_total_wait_ms += wait_ms;
        view.current_wait = None;
        view.current_wait_ms = 0;
    }

    /// 获取会话视图（若存在）。
    pub fn get(&self, session_id: u32) -> Option<&SessionWaitView> {
        self.sessions.get(&session_id)
    }

    /// 返回所有会话视图列表。
    pub fn all_sessions(&self) -> Vec<&SessionWaitView> {
        self.sessions.values().collect()
    }

    /// 返回当前正在等待的会话列表。
    pub fn waiting_sessions(&self) -> Vec<&SessionWaitView> {
        self.sessions
            .values()
            .filter(|s| s.current_wait.is_some())
            .collect()
    }

    /// 会话总数。
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// 正在等待的会话数。
    pub fn waiting_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| s.current_wait.is_some())
            .count()
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== WaitEventStats ====================

    #[test]
    fn test_wait_event_stats_new() {
        let stats = WaitEventStats::new(WaitEvent::DataFileSequentialRead);
        assert_eq!(stats.event, WaitEvent::DataFileSequentialRead);
        assert_eq!(stats.event_name, "db file sequential read");
        assert_eq!(stats.wait_class, WaitClass::UserIo);
        assert_eq!(stats.total_waits, 0);
        assert_eq!(stats.total_wait_ms, 0);
        assert_eq!(stats.max_wait_ms, 0);
        assert_eq!(stats.avg_wait_ms(), 0.0);
    }

    #[test]
    fn test_wait_event_stats_record() {
        let mut stats = WaitEventStats::new(WaitEvent::LogFileSync);
        stats.record(10);
        stats.record(30);
        stats.record(20);

        assert_eq!(stats.total_waits, 3);
        assert_eq!(stats.total_wait_ms, 60);
        assert_eq!(stats.max_wait_ms, 30);
        assert_eq!(stats.avg_wait_ms(), 20.0);
    }

    // ==================== PgStatWaitRow ====================

    #[test]
    fn test_pg_stat_wait_row_from_stats() {
        let mut stats = WaitEventStats::new(WaitEvent::BufferBusy);
        stats.record(5);
        stats.record(15);

        let row = PgStatWaitRow::from(&stats);
        assert_eq!(row.event, "buffer busy waits");
        assert_eq!(row.wait_class, "Concurrency");
        assert_eq!(row.total_waits, 2);
        assert_eq!(row.total_wait_ms, 20);
        assert_eq!(row.avg_wait_ms, 10.0);
        assert_eq!(row.max_wait_ms, 15);
    }

    // ==================== WaitEventCollector ====================

    #[test]
    fn test_collector_record_single_event() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::DataFileSequentialRead, 15);
        c.record_wait(WaitEvent::DataFileSequentialRead, 25);

        let stats = c.get(WaitEvent::DataFileSequentialRead).unwrap();
        assert_eq!(stats.total_waits, 2);
        assert_eq!(stats.total_wait_ms, 40);
        assert_eq!(stats.max_wait_ms, 25);
    }

    #[test]
    fn test_collector_record_multiple_events() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::DataFileSequentialRead, 15);
        c.record_wait(WaitEvent::LogFileSync, 100);
        c.record_wait(WaitEvent::BufferBusy, 5);

        assert_eq!(c.event_count(), 3);
        assert_eq!(c.total_waits(), 3);
        assert_eq!(c.total_wait_ms(), 120);
    }

    #[test]
    fn test_collector_record_waits_batch() {
        let mut c = WaitEventCollector::new();
        c.record_waits(WaitEvent::DataFileSequentialRead, &[10, 20, 30, 40]);

        let stats = c.get(WaitEvent::DataFileSequentialRead).unwrap();
        assert_eq!(stats.total_waits, 4);
        assert_eq!(stats.total_wait_ms, 100);
        assert_eq!(stats.max_wait_ms, 40);
    }

    #[test]
    fn test_collector_clear() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::Cpu, 1);
        assert_eq!(c.event_count(), 1);

        c.clear();
        assert_eq!(c.event_count(), 0);
        assert_eq!(c.total_waits(), 0);
    }

    #[test]
    fn test_collector_sorted_by_total_wait() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::Cpu, 50);
        c.record_wait(WaitEvent::LogFileSync, 200);
        c.record_wait(WaitEvent::DataFileSequentialRead, 100);

        let sorted = c.sorted_by_total_wait();
        assert_eq!(sorted.len(), 3);
        // 降序：LogFileSync(200) > DataFileSequentialRead(100) > Cpu(50)
        assert_eq!(sorted[0].event, WaitEvent::LogFileSync);
        assert_eq!(sorted[1].event, WaitEvent::DataFileSequentialRead);
        assert_eq!(sorted[2].event, WaitEvent::Cpu);
    }

    #[test]
    fn test_collector_top_wait_events() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::Cpu, 50);
        c.record_wait(WaitEvent::LogFileSync, 200);
        c.record_wait(WaitEvent::DataFileSequentialRead, 100);
        c.record_wait(WaitEvent::BufferBusy, 30);

        let top2 = c.top_wait_events(2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].event, WaitEvent::LogFileSync);
        assert_eq!(top2[1].event, WaitEvent::DataFileSequentialRead);
    }

    #[test]
    fn test_collector_to_pg_stat_wait_rows() {
        let mut c = WaitEventCollector::new();
        c.record_wait(WaitEvent::Cpu, 50);
        c.record_wait(WaitEvent::LogFileSync, 200);

        let rows = c.to_pg_stat_wait_rows();
        assert_eq!(rows.len(), 2);
        // 降序：LogFileSync(200) 在前
        assert_eq!(rows[0].event, "log file sync");
        assert_eq!(rows[0].wait_class, "Commit");
        assert_eq!(rows[0].total_wait_ms, 200);
        assert_eq!(rows[1].event, "CPU");
        assert_eq!(rows[1].total_wait_ms, 50);
    }

    #[test]
    fn test_collector_by_wait_class() {
        let mut c = WaitEventCollector::new();
        // User I/O: 50 + 30 = 80
        c.record_wait(WaitEvent::DataFileSequentialRead, 50);
        c.record_wait(WaitEvent::DataFileScatteredRead, 30);
        // Commit: 100
        c.record_wait(WaitEvent::LogFileSync, 100);

        let by_class = c.by_wait_class();
        assert_eq!(by_class.len(), 2);

        let user_io = by_class.get(&WaitClass::UserIo).unwrap();
        assert_eq!(user_io.total_waits, 2);
        assert_eq!(user_io.total_wait_ms, 80);
        assert_eq!(user_io.max_wait_ms, 50);

        let commit = by_class.get(&WaitClass::Commit).unwrap();
        assert_eq!(commit.total_waits, 1);
        assert_eq!(commit.total_wait_ms, 100);
    }

    // ==================== WaitClassStats ====================

    #[test]
    fn test_wait_class_stats_avg() {
        let mut s = WaitClassStats::new(WaitClass::UserIo);
        s.total_waits = 4;
        s.total_wait_ms = 100;
        assert_eq!(s.avg_wait_ms(), 25.0);
    }

    #[test]
    fn test_wait_class_stats_avg_zero() {
        let s = WaitClassStats::new(WaitClass::Idle);
        assert_eq!(s.avg_wait_ms(), 0.0);
    }

    // ==================== SessionWaitTracker ====================

    #[test]
    fn test_session_tracker_begin_end_wait() {
        let mut t = SessionWaitTracker::new();
        t.begin_wait(1, WaitEvent::DataFileSequentialRead);
        assert_eq!(t.waiting_count(), 1);

        t.end_wait(1, 25);
        assert_eq!(t.waiting_count(), 0);

        let view = t.get(1).unwrap();
        assert_eq!(view.session_total_waits, 1);
        assert_eq!(view.session_total_wait_ms, 25);
        assert!(view.current_wait.is_none());
    }

    #[test]
    fn test_session_tracker_multiple_sessions() {
        let mut t = SessionWaitTracker::new();
        t.begin_wait(1, WaitEvent::DataFileSequentialRead);
        t.begin_wait(2, WaitEvent::LogFileSync);
        t.begin_wait(3, WaitEvent::BufferBusy);

        assert_eq!(t.session_count(), 3);
        assert_eq!(t.waiting_count(), 3);

        let waiting = t.waiting_sessions();
        assert_eq!(waiting.len(), 3);

        t.end_wait(2, 50);
        assert_eq!(t.waiting_count(), 2);
    }

    #[test]
    fn test_session_tracker_all_sessions() {
        let mut t = SessionWaitTracker::new();
        t.begin_wait(1, WaitEvent::Cpu);
        t.end_wait(1, 10);
        t.begin_wait(2, WaitEvent::LogFileSync);

        let all = t.all_sessions();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_session_tracker_unknown_session_end() {
        // 对未知会话 end_wait 也应创建视图并累加统计
        let mut t = SessionWaitTracker::new();
        t.end_wait(99, 100);
        let view = t.get(99).unwrap();
        assert_eq!(view.session_total_waits, 1);
        assert_eq!(view.session_total_wait_ms, 100);
    }

    // ==================== 端到端：模拟查询等待分布 ====================

    #[test]
    fn test_end_to_end_wait_distribution() {
        // 模拟：3 类等待事件多次发生，验证 pg_stat_wait 输出
        let mut c = WaitEventCollector::new();
        // 模拟索引扫描 I/O 等待（10 次，每次 5ms）
        for _ in 0..10 {
            c.record_wait(WaitEvent::DataFileSequentialRead, 5);
        }
        // 模拟事务提交日志同步（5 次，每次 50ms）
        for _ in 0..5 {
            c.record_wait(WaitEvent::LogFileSync, 50);
        }
        // 模拟锁等待（2 次，每次 200ms）
        for _ in 0..2 {
            c.record_wait(WaitEvent::EnqueueTxRowLock, 200);
        }

        let rows = c.to_pg_stat_wait_rows();
        assert_eq!(rows.len(), 3);

        // 降序：锁等待(400ms) > 日志同步(250ms) > 顺序读(50ms)
        assert_eq!(rows[0].event, "enq: TX - row lock contention");
        assert_eq!(rows[0].total_wait_ms, 400);
        assert_eq!(rows[0].total_waits, 2);
        assert_eq!(rows[0].max_wait_ms, 200);

        assert_eq!(rows[1].event, "log file sync");
        assert_eq!(rows[1].total_wait_ms, 250);
        assert_eq!(rows[1].total_waits, 5);

        assert_eq!(rows[2].event, "db file sequential read");
        assert_eq!(rows[2].total_wait_ms, 50);
        assert_eq!(rows[2].total_waits, 10);
        assert_eq!(rows[2].avg_wait_ms, 5.0);

        // 总等待次数 = 10 + 5 + 2 = 17
        assert_eq!(c.total_waits(), 17);
        // 总等待时间 = 50 + 250 + 400 = 700
        assert_eq!(c.total_wait_ms(), 700);
    }

    #[test]
    fn test_end_to_end_session_view() {
        // 模拟：3 个会话同时执行查询，部分等待 I/O，部分等待锁
        let mut t = SessionWaitTracker::new();

        // 会话 1：等待 I/O
        t.begin_wait(1, WaitEvent::DataFileSequentialRead);
        // 会话 2：等待锁
        t.begin_wait(2, WaitEvent::EnqueueTxRowLock);
        // 会话 3：CPU 执行（无等待）
        // 不调用 begin_wait，会话 3 不在追踪器中

        assert_eq!(t.waiting_count(), 2);
        let waiting = t.waiting_sessions();
        let waiting_ids: Vec<u32> = waiting.iter().map(|s| s.session_id).collect();
        assert!(waiting_ids.contains(&1));
        assert!(waiting_ids.contains(&2));

        // 会话 1 等待结束
        t.end_wait(1, 30);
        assert_eq!(t.waiting_count(), 1);

        // 会话 2 仍在等待
        let s2 = t.get(2).unwrap();
        assert_eq!(s2.current_wait, Some(WaitEvent::EnqueueTxRowLock));
    }
}
