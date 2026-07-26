//! ASH 采样 + AWR 报告（Active Session History + Automatic Workload Repository）— Phase 7d.8
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.8 ASH 采样 + AWR 报告设计。
//!
//! # 设计
//!
//! 借鉴 Oracle ASH/AWR：
//! - **ASH（Active Session History）** — 每 1 秒采样活动会话，记录正在执行的 SQL、
//!   等待事件、用户、客户端等。采样数据保留在内存环形缓冲区，用于实时诊断。
//! - **AWR（Automatic Workload Repository）** — 基于 ASH 数据生成快照报告，
//!   包含 Top SQL、Top Wait Events、物理 I/O 等统计。
//!
//! ## 验证标准
//!
//! - 运行混合负载（10 线程）→ ASH 采样 10 分钟 → AWR 报告
//! - AWR 报告包含 Top SQL/等待事件/物理 IO（格式类似 Oracle）

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// ASH 默认采样间隔（秒） — Oracle 默认 1 秒
pub const DEFAULT_ASH_SAMPLE_INTERVAL_SECS: u64 = 1;

/// ASH 内存缓冲区默认容量（采样数） — 保留最近 N 个采样
pub const DEFAULT_ASH_BUFFER_CAPACITY: usize = 10_000;

/// AWR 快照默认保留时间（秒）
pub const DEFAULT_AWR_RETENTION_SECS: u64 = 7 * 24 * 3_600; // 7 天

/// Top N 默认数量
pub const DEFAULT_TOP_N: usize = 10;

// =====================================================================
//  SessionId — 会话 ID
// =====================================================================

/// 会话 ID
pub type SessionId = u32;

/// SQL ID（简化为 u32）
pub type SqlId = u32;

// =====================================================================
//  SessionState — 会话状态
// =====================================================================

/// 会话状态 — 采样时会话正在做什么
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// 活动状态 — 正在 CPU 上执行
    Active,
    /// 等待状态 — 正在等待某个事件
    Waiting,
    /// 空闲状态 — 无任务
    Idle,
}

impl SessionState {
    /// 状态名称
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Active => "ACTIVE",
            SessionState::Waiting => "WAITING",
            SessionState::Idle => "IDLE",
        }
    }

    /// 是否活动
    pub fn is_active(&self) -> bool {
        matches!(self, SessionState::Active)
    }

    /// 是否等待
    pub fn is_waiting(&self) -> bool {
        matches!(self, SessionState::Waiting)
    }

    /// 是否空闲
    pub fn is_idle(&self) -> bool {
        matches!(self, SessionState::Idle)
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  WaitEvent — 等待事件
// =====================================================================

/// 等待事件 — 借鉴 Oracle 等待事件分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaitEvent {
    /// CPU 执行（非等待事件，表示在 CPU 上运行）
    Cpu,
    /// 数据文件读取（db file sequential read，索引扫描）
    DataFileSequentialRead,
    /// 数据文件分散读取（db file scattered read，全表扫描）
    DataFileScatteredRead,
    /// 日志文件同步（log file sync，事务提交等待）
    LogFileSync,
    /// 日志文件并行写入（log file parallel write）
    LogFileParallelWrite,
    /// 锁等待（enq: TX row lock contention）
    EnqueueTxRowLock,
    /// 缓冲区忙等待（buffer busy waits）
    BufferBusy,
    /// 库缓存锁（library cache lock/parse）
    LibraryCacheLock,
    /// SQL*Net 消息（客户端通信）
    SqlNetMessageFromClient,
    /// 磁盘排序（direct path write temp）
    DirectPathWriteTemp,
    /// 空闲等待（Idle 类，如 SQL*Net message from client 空闲）
    Idle,
}

impl WaitEvent {
    /// 事件名称（Oracle 风格）
    pub fn as_str(&self) -> &'static str {
        match self {
            WaitEvent::Cpu => "CPU",
            WaitEvent::DataFileSequentialRead => "db file sequential read",
            WaitEvent::DataFileScatteredRead => "db file scattered read",
            WaitEvent::LogFileSync => "log file sync",
            WaitEvent::LogFileParallelWrite => "log file parallel write",
            WaitEvent::EnqueueTxRowLock => "enq: TX - row lock contention",
            WaitEvent::BufferBusy => "buffer busy waits",
            WaitEvent::LibraryCacheLock => "library cache lock",
            WaitEvent::SqlNetMessageFromClient => "SQL*Net message from client",
            WaitEvent::DirectPathWriteTemp => "direct path write temp",
            WaitEvent::Idle => "idle",
        }
    }

    /// 等待类别
    pub fn wait_class(&self) -> WaitClass {
        match self {
            WaitEvent::Cpu => WaitClass::Cpu,
            WaitEvent::DataFileSequentialRead | WaitEvent::DataFileScatteredRead => {
                WaitClass::UserIo
            }
            WaitEvent::LogFileSync | WaitEvent::LogFileParallelWrite => WaitClass::Commit,
            WaitEvent::EnqueueTxRowLock => WaitClass::Application,
            WaitEvent::BufferBusy => WaitClass::Concurrency,
            WaitEvent::LibraryCacheLock => WaitClass::Concurrency,
            WaitEvent::SqlNetMessageFromClient | WaitEvent::Idle => WaitClass::Idle,
            WaitEvent::DirectPathWriteTemp => WaitClass::UserIo,
        }
    }

    /// 是否空闲等待
    pub fn is_idle(&self) -> bool {
        matches!(self, WaitEvent::Idle | WaitEvent::SqlNetMessageFromClient)
    }
}

impl std::fmt::Display for WaitEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  WaitClass — 等待类别
// =====================================================================

/// 等待类别 — Oracle 风格分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaitClass {
    /// CPU
    Cpu,
    /// 用户 I/O（磁盘读写）
    UserIo,
    /// 提交（日志同步）
    Commit,
    /// 应用程序（锁）
    Application,
    /// 并发（buffer busy / library cache lock）
    Concurrency,
    /// 空闲
    Idle,
}

impl WaitClass {
    /// 类别名称
    pub fn as_str(&self) -> &'static str {
        match self {
            WaitClass::Cpu => "CPU",
            WaitClass::UserIo => "User I/O",
            WaitClass::Commit => "Commit",
            WaitClass::Application => "Application",
            WaitClass::Concurrency => "Concurrency",
            WaitClass::Idle => "Idle",
        }
    }
}

impl std::fmt::Display for WaitClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  SqlInfo — SQL 信息
// =====================================================================

/// SQL 信息 — ASH 采样时记录的 SQL 上下文
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlInfo {
    /// SQL ID
    pub sql_id: SqlId,
    /// SQL 文本（截断到 200 字符）
    pub sql_text: String,
    /// 用户名
    pub username: String,
    /// 客户端机器
    pub machine: String,
    /// 客户端程序
    pub program: String,
}

impl SqlInfo {
    /// 构造 SQL 信息
    pub fn new(
        sql_id: SqlId,
        sql_text: impl Into<String>,
        username: impl Into<String>,
        machine: impl Into<String>,
        program: impl Into<String>,
    ) -> Self {
        let sql_text = sql_text.into();
        let sql_text = if sql_text.len() > 200 {
            sql_text.chars().take(200).collect()
        } else {
            sql_text
        };
        Self {
            sql_id,
            sql_text,
            username: username.into(),
            machine: machine.into(),
            program: program.into(),
        }
    }

    /// SQL 文本长度
    pub fn sql_text_len(&self) -> usize {
        self.sql_text.len()
    }
}

// =====================================================================
//  AshSample — ASH 采样
// =====================================================================

/// ASH 采样 — 单次会话采样记录
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AshSample {
    /// 采样时间戳（秒）
    pub timestamp: u64,
    /// 会话 ID
    pub session_id: SessionId,
    /// 会话状态
    pub state: SessionState,
    /// 等待事件（Waiting 状态时有值）
    pub wait_event: WaitEvent,
    /// SQL 信息
    pub sql_info: Option<SqlInfo>,
    /// 物理读字节数（本次采样累计）
    pub physical_read_bytes: u64,
    /// 物理写字节数（本次采样累计）
    pub physical_write_bytes: u64,
}

impl AshSample {
    /// 构造活动状态采样（CPU 上执行）
    pub fn active(
        timestamp: u64,
        session_id: SessionId,
        sql_info: Option<SqlInfo>,
        physical_read_bytes: u64,
        physical_write_bytes: u64,
    ) -> Self {
        Self {
            timestamp,
            session_id,
            state: SessionState::Active,
            wait_event: WaitEvent::Cpu,
            sql_info,
            physical_read_bytes,
            physical_write_bytes,
        }
    }

    /// 构造等待状态采样
    pub fn waiting(
        timestamp: u64,
        session_id: SessionId,
        wait_event: WaitEvent,
        sql_info: Option<SqlInfo>,
        physical_read_bytes: u64,
        physical_write_bytes: u64,
    ) -> Self {
        Self {
            timestamp,
            session_id,
            state: SessionState::Waiting,
            wait_event,
            sql_info,
            physical_read_bytes,
            physical_write_bytes,
        }
    }

    /// 构造空闲状态采样
    pub fn idle(timestamp: u64, session_id: SessionId) -> Self {
        Self {
            timestamp,
            session_id,
            state: SessionState::Idle,
            wait_event: WaitEvent::Idle,
            sql_info: None,
            physical_read_bytes: 0,
            physical_write_bytes: 0,
        }
    }

    /// 是否活动状态
    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    /// 是否等待状态
    pub fn is_waiting(&self) -> bool {
        self.state.is_waiting()
    }

    /// 是否空闲状态
    pub fn is_idle(&self) -> bool {
        self.state.is_idle()
    }

    /// 物理读写总字节数
    pub fn physical_io_bytes(&self) -> u64 {
        self.physical_read_bytes + self.physical_write_bytes
    }
}

// =====================================================================
//  AshCollector — ASH 采样器
// =====================================================================

/// ASH 采样器 — 收集会话采样到环形缓冲区
pub struct AshCollector {
    /// 采样缓冲区（环形，新采样追加到末尾，超出容量时丢弃最旧的）
    samples: Vec<AshSample>,
    /// 缓冲区容量
    capacity: usize,
    /// 采样间隔（秒）
    sample_interval_secs: u64,
    /// 上次采样时间戳
    last_sample_time: u64,
    /// 总采样数（含已丢弃的）
    total_sampled: u64,
    /// 丢弃的采样数
    dropped_samples: u64,
}

impl AshCollector {
    /// 构造默认 ASH 采样器
    pub fn new() -> Self {
        Self::with_capacity_and_interval(
            DEFAULT_ASH_BUFFER_CAPACITY,
            DEFAULT_ASH_SAMPLE_INTERVAL_SECS,
        )
    }

    /// 构造自定义容量和间隔的 ASH 采样器
    pub fn with_capacity_and_interval(capacity: usize, sample_interval_secs: u64) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            capacity,
            sample_interval_secs,
            last_sample_time: 0,
            total_sampled: 0,
            dropped_samples: 0,
        }
    }

    /// 获取采样间隔（秒）
    pub fn sample_interval_secs(&self) -> u64 {
        self.sample_interval_secs
    }

    /// 获取缓冲区容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前缓冲区采样数
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 总采样数（含已丢弃的）
    pub fn total_sampled(&self) -> u64 {
        self.total_sampled
    }

    /// 丢弃的采样数
    pub fn dropped_samples(&self) -> u64 {
        self.dropped_samples
    }

    /// 是否到达采样时间（当前时间 - 上次采样 >= 间隔）
    pub fn should_sample(&self, now: u64) -> bool {
        now.saturating_sub(self.last_sample_time) >= self.sample_interval_secs
    }

    /// 记录一个采样（不检查时间间隔）
    ///
    /// 如果缓冲区满，丢弃最旧的采样。
    pub fn record(&mut self, sample: AshSample) {
        if self.samples.len() >= self.capacity {
            self.samples.remove(0);
            self.dropped_samples += 1;
        }
        self.last_sample_time = sample.timestamp;
        self.total_sampled += 1;
        self.samples.push(sample);
    }

    /// 采样指定会话列表（按时间间隔判断）
    ///
    /// 返回实际记录的采样数
    pub fn sample_sessions(&mut self, now: u64, samples: Vec<AshSample>) -> usize {
        if !self.should_sample(now) && !self.samples.is_empty() {
            return 0;
        }
        let count = samples.len();
        for s in samples {
            self.record(s);
        }
        count
    }

    /// 获取所有采样（按时间顺序）
    pub fn samples(&self) -> &[AshSample] {
        &self.samples
    }

    /// 获取指定时间范围内的采样
    pub fn samples_in_range(&self, start: u64, end: u64) -> Vec<&AshSample> {
        self.samples
            .iter()
            .filter(|s| s.timestamp >= start && s.timestamp <= end)
            .collect()
    }

    /// 清空采样缓冲区
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// 获取缓冲区引用（可变）
    pub fn samples_mut(&mut self) -> &mut Vec<AshSample> {
        &mut self.samples
    }
}

impl Default for AshCollector {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  AwrSnapshot — AWR 快照
// =====================================================================

/// AWR 快照 — 某段时间内的统计汇总
#[derive(Debug, Clone)]
pub struct AwrSnapshot {
    /// 快照开始时间戳
    pub start_time: u64,
    /// 快照结束时间戳
    pub end_time: u64,
    /// 采样数
    pub sample_count: usize,
    /// 唯一会话数
    pub session_count: usize,
    /// 唯一 SQL 数
    pub sql_count: usize,
    /// 按等待事件统计（事件 → 采样数）
    pub wait_event_counts: HashMap<WaitEvent, usize>,
    /// 按等待类别统计（类别 → 采样数）
    pub wait_class_counts: HashMap<WaitClass, usize>,
    /// 按 SQL 统计（sql_id → 采样数）
    pub sql_counts: HashMap<SqlId, usize>,
    /// 按 SQL 统计（sql_id → SQL 文本）
    pub sql_texts: HashMap<SqlId, String>,
    /// 物理读总字节数
    pub total_physical_read_bytes: u64,
    /// 物理写总字节数
    pub total_physical_write_bytes: u64,
    /// 活动状态采样数
    pub active_count: usize,
    /// 等待状态采样数
    pub waiting_count: usize,
    /// 空闲状态采样数
    pub idle_count: usize,
}

impl AwrSnapshot {
    /// 从采样列表构造 AWR 快照
    pub fn from_samples(start_time: u64, end_time: u64, samples: &[AshSample]) -> Self {
        let mut wait_event_counts: HashMap<WaitEvent, usize> = HashMap::new();
        let mut wait_class_counts: HashMap<WaitClass, usize> = HashMap::new();
        let mut sql_counts: HashMap<SqlId, usize> = HashMap::new();
        let mut sql_texts: HashMap<SqlId, String> = HashMap::new();
        let mut sessions = std::collections::HashSet::new();
        let mut active_count = 0usize;
        let mut waiting_count = 0usize;
        let mut idle_count = 0usize;
        let mut total_physical_read_bytes = 0u64;
        let mut total_physical_write_bytes = 0u64;

        for s in samples {
            *wait_event_counts.entry(s.wait_event).or_insert(0) += 1;
            *wait_class_counts
                .entry(s.wait_event.wait_class())
                .or_insert(0) += 1;
            if let Some(info) = &s.sql_info {
                *sql_counts.entry(info.sql_id).or_insert(0) += 1;
                sql_texts.insert(info.sql_id, info.sql_text.clone());
            }
            sessions.insert(s.session_id);
            match s.state {
                SessionState::Active => active_count += 1,
                SessionState::Waiting => waiting_count += 1,
                SessionState::Idle => idle_count += 1,
            }
            total_physical_read_bytes += s.physical_read_bytes;
            total_physical_write_bytes += s.physical_write_bytes;
        }

        Self {
            start_time,
            end_time,
            sample_count: samples.len(),
            session_count: sessions.len(),
            sql_count: sql_counts.len(),
            wait_event_counts,
            wait_class_counts,
            sql_counts,
            sql_texts,
            total_physical_read_bytes,
            total_physical_write_bytes,
            active_count,
            waiting_count,
            idle_count,
        }
    }

    /// 快照持续时间（秒）
    pub fn duration_secs(&self) -> u64 {
        self.end_time.saturating_sub(self.start_time)
    }

    /// 物理读写总字节数
    pub fn total_physical_io_bytes(&self) -> u64 {
        self.total_physical_read_bytes + self.total_physical_write_bytes
    }

    /// 平均每秒活动会话数（DB Time / elapsed）
    pub fn avg_active_sessions(&self) -> f64 {
        let elapsed = self.duration_secs();
        if elapsed == 0 {
            return 0.0;
        }
        self.active_count as f64 / elapsed as f64
    }

    /// 活动比例（active / total）
    pub fn active_ratio(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.active_count as f64 / self.sample_count as f64
    }

    /// 等待比例（waiting / total）
    pub fn waiting_ratio(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.waiting_count as f64 / self.sample_count as f64
    }

    /// 空闲比例（idle / total）
    pub fn idle_ratio(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.idle_count as f64 / self.sample_count as f64
    }

    /// Top N 等待事件（按采样数降序）
    pub fn top_wait_events(&self, n: usize) -> Vec<(WaitEvent, usize)> {
        let mut events: Vec<_> = self.wait_event_counts.iter().collect();
        events.sort_by(|a, b| b.1.cmp(a.1));
        events.into_iter().take(n).map(|(e, c)| (*e, *c)).collect()
    }

    /// Top N 等待类别（按采样数降序）
    pub fn top_wait_classes(&self, n: usize) -> Vec<(WaitClass, usize)> {
        let mut classes: Vec<_> = self.wait_class_counts.iter().collect();
        classes.sort_by(|a, b| b.1.cmp(a.1));
        classes
            .into_iter()
            .take(n)
            .map(|(c, count)| (*c, *count))
            .collect()
    }

    /// Top N SQL（按采样数降序）
    pub fn top_sqls(&self, n: usize) -> Vec<(SqlId, usize)> {
        let mut sqls: Vec<_> = self.sql_counts.iter().collect();
        sqls.sort_by(|a, b| b.1.cmp(a.1));
        sqls.into_iter().take(n).map(|(s, c)| (*s, *c)).collect()
    }

    /// 获取 SQL 文本
    pub fn sql_text(&self, sql_id: SqlId) -> Option<&str> {
        self.sql_texts.get(&sql_id).map(|s| s.as_str())
    }
}

// =====================================================================
//  AwrReport — AWR 报告
// =====================================================================

/// AWR 报告 — 基于 AWR 快照生成可读报告
pub struct AwrReport {
    /// 快照
    pub snapshot: AwrSnapshot,
    /// Top N
    pub top_n: usize,
}

impl AwrReport {
    /// 从快照构造 AWR 报告
    pub fn new(snapshot: AwrSnapshot) -> Self {
        Self {
            snapshot,
            top_n: DEFAULT_TOP_N,
        }
    }

    /// 设置 Top N
    pub fn with_top_n(mut self, n: usize) -> Self {
        self.top_n = n;
        self
    }

    /// 生成文本报告（类似 Oracle AWR）
    pub fn render(&self) -> String {
        let mut report = String::new();
        let snap = &self.snapshot;

        // 标题
        report.push_str(
            "================================================================================\n",
        );
        report.push_str("                        SzRSQL AWR Report\n");
        report.push_str(
            "================================================================================\n\n",
        );

        // 快照信息
        report.push_str("Snapshot Information\n");
        report.push_str("--------------------\n");
        report.push_str(&format!("  Start Time:      {}\n", snap.start_time));
        report.push_str(&format!("  End Time:        {}\n", snap.end_time));
        report.push_str(&format!("  Duration (sec):  {}\n", snap.duration_secs()));
        report.push_str(&format!("  Samples:         {}\n", snap.sample_count));
        report.push_str(&format!("  Sessions:        {}\n", snap.session_count));
        report.push_str(&format!("  SQLs:            {}\n", snap.sql_count));
        report.push_str(&format!(
            "  Avg Active Sess: {:.2}\n",
            snap.avg_active_sessions()
        ));
        report.push('\n');

        // 会话状态汇总
        report.push_str("Session State Summary\n");
        report.push_str("---------------------\n");
        report.push_str(&format!(
            "  ACTIVE:  {:6} ({:.2}%)\n",
            snap.active_count,
            snap.active_ratio() * 100.0
        ));
        report.push_str(&format!(
            "  WAITING: {:6} ({:.2}%)\n",
            snap.waiting_count,
            snap.waiting_ratio() * 100.0
        ));
        report.push_str(&format!(
            "  IDLE:    {:6} ({:.2}%)\n",
            snap.idle_count,
            snap.idle_ratio() * 100.0
        ));
        report.push('\n');

        // Top N 等待事件
        report.push_str(&format!("Top {} Wait Events\n", self.top_n));
        report.push_str("------------------\n");
        report.push_str("  Event                                     Count    %\n");
        report.push_str("  ----------------------------------------  -------  ----\n");
        let top_events = snap.top_wait_events(self.top_n);
        let total = snap.sample_count.max(1);
        for (event, count) in &top_events {
            report.push_str(&format!(
                "  {:<40}  {:>7}  {:4.1}%\n",
                event.as_str(),
                count,
                (*count as f64 / total as f64) * 100.0
            ));
        }
        report.push('\n');

        // Top N 等待类别
        report.push_str(&format!("Top {} Wait Classes\n", self.top_n));
        report.push_str("-------------------\n");
        report.push_str("  Class            Count    %\n");
        report.push_str("  ---------------  -------  ----\n");
        let top_classes = snap.top_wait_classes(self.top_n);
        for (class, count) in &top_classes {
            report.push_str(&format!(
                "  {:<15}  {:>7}  {:4.1}%\n",
                class.as_str(),
                count,
                (*count as f64 / total as f64) * 100.0
            ));
        }
        report.push('\n');

        // Top N SQL
        report.push_str(&format!("Top {} SQL (by activity)\n", self.top_n));
        report.push_str("-------------------------\n");
        report.push_str("  SQL ID  Count    %    SQL Text\n");
        report.push_str("  ------  -------  ---  --------------------------------\n");
        let top_sqls = snap.top_sqls(self.top_n);
        for (sql_id, count) in &top_sqls {
            let text = snap
                .sql_text(*sql_id)
                .unwrap_or("<unknown>")
                .chars()
                .take(50)
                .collect::<String>();
            report.push_str(&format!(
                "  {:>6}  {:>7}  {:3.1}%  {}\n",
                sql_id,
                count,
                (*count as f64 / total as f64) * 100.0,
                text
            ));
        }
        report.push('\n');

        // 物理 I/O 统计
        report.push_str("Physical I/O\n");
        report.push_str("------------\n");
        report.push_str(&format!(
            "  Physical Read:  {} bytes ({:.2} MB)\n",
            snap.total_physical_read_bytes,
            snap.total_physical_read_bytes as f64 / (1024.0 * 1024.0)
        ));
        report.push_str(&format!(
            "  Physical Write: {} bytes ({:.2} MB)\n",
            snap.total_physical_write_bytes,
            snap.total_physical_write_bytes as f64 / (1024.0 * 1024.0)
        ));
        report.push_str(&format!(
            "  Total I/O:      {} bytes ({:.2} MB)\n",
            snap.total_physical_io_bytes(),
            snap.total_physical_io_bytes() as f64 / (1024.0 * 1024.0)
        ));
        report.push('\n');

        // 报告尾部
        report.push_str(
            "================================================================================\n",
        );
        report.push_str("                              End of Report\n");
        report.push_str(
            "================================================================================\n",
        );

        report
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成混合负载采样（模拟 10 线程并发）
///
/// `start_time` 起始时间戳，`duration_secs` 持续时间，`thread_count` 线程数
pub fn generate_mixed_workload_samples(
    start_time: u64,
    duration_secs: u64,
    thread_count: u32,
) -> Vec<AshSample> {
    let mut samples = Vec::new();
    let mut state = 0x1234_5678u64;

    for t in 0..duration_secs {
        let timestamp = start_time + t;
        for session_id in 1..=thread_count {
            // LCG 伪随机选择状态
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let rand = state % 100;

            let (state_enum, wait_event) = if rand < 40 {
                (SessionState::Active, WaitEvent::Cpu)
            } else if rand < 60 {
                (SessionState::Waiting, WaitEvent::DataFileSequentialRead)
            } else if rand < 75 {
                (SessionState::Waiting, WaitEvent::DataFileScatteredRead)
            } else if rand < 85 {
                (SessionState::Waiting, WaitEvent::LogFileSync)
            } else if rand < 92 {
                (SessionState::Waiting, WaitEvent::EnqueueTxRowLock)
            } else if rand < 97 {
                (SessionState::Waiting, WaitEvent::BufferBusy)
            } else {
                (SessionState::Idle, WaitEvent::Idle)
            };

            // 模拟 SQL 信息（每个会话固定一个 SQL）
            let sql_id = session_id;
            let sql_text = format!("SELECT * FROM table_{} WHERE id = ?", session_id);
            let sql_info = if state_enum != SessionState::Idle {
                Some(SqlInfo::new(
                    sql_id,
                    sql_text,
                    format!("user_{}", session_id),
                    format!("host_{}", session_id),
                    format!("app_{}", session_id),
                ))
            } else {
                None
            };

            // 模拟物理 I/O（Active/Waiting 状态有 I/O）
            let (read_bytes, write_bytes) = if state_enum == SessionState::Idle {
                (0, 0)
            } else {
                let io_rand = (state >> 32) % 8192;
                (io_rand * 1024, (io_rand / 4) * 1024)
            };

            samples.push(AshSample {
                timestamp,
                session_id,
                state: state_enum,
                wait_event,
                sql_info,
                physical_read_bytes: read_bytes,
                physical_write_bytes: write_bytes,
            });
        }
    }
    samples
}

/// 生成简单顺序负载采样（单线程顺序执行）
pub fn generate_sequential_samples(
    start_time: u64,
    duration_secs: u64,
    session_id: SessionId,
) -> Vec<AshSample> {
    let mut samples = Vec::new();
    for t in 0..duration_secs {
        let timestamp = start_time + t;
        let state = if t % 5 == 0 {
            SessionState::Waiting
        } else {
            SessionState::Active
        };
        let wait_event = if state == SessionState::Waiting {
            WaitEvent::DataFileSequentialRead
        } else {
            WaitEvent::Cpu
        };
        let sql_info = Some(SqlInfo::new(
            1,
            "SELECT * FROM orders WHERE id = ?",
            "app_user",
            "localhost",
            "order_service",
        ));
        samples.push(AshSample {
            timestamp,
            session_id,
            state,
            wait_event,
            sql_info,
            physical_read_bytes: 4096,
            physical_write_bytes: 1024,
        });
    }
    samples
}

/// 生成纯等待负载采样（锁等待场景）
pub fn generate_lock_wait_samples(
    start_time: u64,
    duration_secs: u64,
    session_id: SessionId,
) -> Vec<AshSample> {
    let mut samples = Vec::new();
    for t in 0..duration_secs {
        let timestamp = start_time + t;
        let sql_info = Some(SqlInfo::new(
            2,
            "UPDATE accounts SET balance = balance - 100 WHERE id = ?",
            "txn_user",
            "192.168.1.10",
            "txn_app",
        ));
        samples.push(AshSample {
            timestamp,
            session_id,
            state: SessionState::Waiting,
            wait_event: WaitEvent::EnqueueTxRowLock,
            sql_info,
            physical_read_bytes: 0,
            physical_write_bytes: 0,
        });
    }
    samples
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  SessionState 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_session_state_as_str() {
        assert_eq!(SessionState::Active.as_str(), "ACTIVE");
        assert_eq!(SessionState::Waiting.as_str(), "WAITING");
        assert_eq!(SessionState::Idle.as_str(), "IDLE");
    }

    #[test]
    fn test_session_state_predicates() {
        assert!(SessionState::Active.is_active());
        assert!(!SessionState::Active.is_waiting());
        assert!(!SessionState::Active.is_idle());

        assert!(!SessionState::Waiting.is_active());
        assert!(SessionState::Waiting.is_waiting());
        assert!(!SessionState::Waiting.is_idle());

        assert!(!SessionState::Idle.is_active());
        assert!(!SessionState::Idle.is_waiting());
        assert!(SessionState::Idle.is_idle());
    }

    #[test]
    fn test_session_state_display() {
        assert_eq!(format!("{}", SessionState::Active), "ACTIVE");
        assert_eq!(format!("{}", SessionState::Waiting), "WAITING");
        assert_eq!(format!("{}", SessionState::Idle), "IDLE");
    }

    // -----------------------------------------------------------------
    //  WaitEvent 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_wait_event_as_str() {
        assert_eq!(WaitEvent::Cpu.as_str(), "CPU");
        assert_eq!(
            WaitEvent::DataFileSequentialRead.as_str(),
            "db file sequential read"
        );
        assert_eq!(
            WaitEvent::DataFileScatteredRead.as_str(),
            "db file scattered read"
        );
        assert_eq!(WaitEvent::LogFileSync.as_str(), "log file sync");
        assert_eq!(
            WaitEvent::EnqueueTxRowLock.as_str(),
            "enq: TX - row lock contention"
        );
        assert_eq!(WaitEvent::BufferBusy.as_str(), "buffer busy waits");
        assert_eq!(WaitEvent::Idle.as_str(), "idle");
    }

    #[test]
    fn test_wait_event_wait_class() {
        assert_eq!(WaitEvent::Cpu.wait_class(), WaitClass::Cpu);
        assert_eq!(
            WaitEvent::DataFileSequentialRead.wait_class(),
            WaitClass::UserIo
        );
        assert_eq!(
            WaitEvent::DataFileScatteredRead.wait_class(),
            WaitClass::UserIo
        );
        assert_eq!(WaitEvent::LogFileSync.wait_class(), WaitClass::Commit);
        assert_eq!(
            WaitEvent::EnqueueTxRowLock.wait_class(),
            WaitClass::Application
        );
        assert_eq!(WaitEvent::BufferBusy.wait_class(), WaitClass::Concurrency);
        assert_eq!(
            WaitEvent::LibraryCacheLock.wait_class(),
            WaitClass::Concurrency
        );
        assert_eq!(
            WaitEvent::SqlNetMessageFromClient.wait_class(),
            WaitClass::Idle
        );
        assert_eq!(WaitEvent::Idle.wait_class(), WaitClass::Idle);
        assert_eq!(
            WaitEvent::DirectPathWriteTemp.wait_class(),
            WaitClass::UserIo
        );
    }

    #[test]
    fn test_wait_event_is_idle() {
        assert!(WaitEvent::Idle.is_idle());
        assert!(WaitEvent::SqlNetMessageFromClient.is_idle());
        assert!(!WaitEvent::Cpu.is_idle());
        assert!(!WaitEvent::DataFileSequentialRead.is_idle());
    }

    #[test]
    fn test_wait_event_display() {
        assert_eq!(format!("{}", WaitEvent::Cpu), "CPU");
        assert_eq!(
            format!("{}", WaitEvent::DataFileSequentialRead),
            "db file sequential read"
        );
    }

    // -----------------------------------------------------------------
    //  WaitClass 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_wait_class_as_str() {
        assert_eq!(WaitClass::Cpu.as_str(), "CPU");
        assert_eq!(WaitClass::UserIo.as_str(), "User I/O");
        assert_eq!(WaitClass::Commit.as_str(), "Commit");
        assert_eq!(WaitClass::Application.as_str(), "Application");
        assert_eq!(WaitClass::Concurrency.as_str(), "Concurrency");
        assert_eq!(WaitClass::Idle.as_str(), "Idle");
    }

    #[test]
    fn test_wait_class_display() {
        assert_eq!(format!("{}", WaitClass::Cpu), "CPU");
        assert_eq!(format!("{}", WaitClass::UserIo), "User I/O");
    }

    // -----------------------------------------------------------------
    //  SqlInfo 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sql_info_new() {
        let info = SqlInfo::new(
            42,
            "SELECT * FROM users WHERE id = ?",
            "app_user",
            "localhost",
            "my_app",
        );
        assert_eq!(info.sql_id, 42);
        assert_eq!(info.sql_text, "SELECT * FROM users WHERE id = ?");
        assert_eq!(info.username, "app_user");
        assert_eq!(info.machine, "localhost");
        assert_eq!(info.program, "my_app");
    }

    #[test]
    fn test_sql_info_truncates_long_text() {
        let long_sql = "A".repeat(300);
        let info = SqlInfo::new(1, long_sql.clone(), "u", "m", "p");
        assert_eq!(info.sql_text.len(), 200);
        assert_eq!(info.sql_text, "A".repeat(200));
    }

    #[test]
    fn test_sql_info_preserves_short_text() {
        let short_sql = "SELECT 1";
        let info = SqlInfo::new(1, short_sql, "u", "m", "p");
        assert_eq!(info.sql_text, short_sql);
        assert_eq!(info.sql_text_len(), 8);
    }

    // -----------------------------------------------------------------
    //  AshSample 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_ash_sample_active() {
        let sample = AshSample::active(1000, 1, None, 4096, 1024);
        assert_eq!(sample.timestamp, 1000);
        assert_eq!(sample.session_id, 1);
        assert!(sample.is_active());
        assert!(!sample.is_waiting());
        assert!(!sample.is_idle());
        assert_eq!(sample.wait_event, WaitEvent::Cpu);
        assert_eq!(sample.physical_io_bytes(), 5120);
    }

    #[test]
    fn test_ash_sample_waiting() {
        let sample = AshSample::waiting(1000, 1, WaitEvent::DataFileSequentialRead, None, 4096, 0);
        assert!(sample.is_waiting());
        assert_eq!(sample.wait_event, WaitEvent::DataFileSequentialRead);
        assert_eq!(sample.physical_io_bytes(), 4096);
    }

    #[test]
    fn test_ash_sample_idle() {
        let sample = AshSample::idle(1000, 1);
        assert!(sample.is_idle());
        assert_eq!(sample.wait_event, WaitEvent::Idle);
        assert_eq!(sample.physical_io_bytes(), 0);
        assert!(sample.sql_info.is_none());
    }

    #[test]
    fn test_ash_sample_with_sql_info() {
        let sql_info = SqlInfo::new(1, "SELECT 1", "u", "m", "p");
        let sample = AshSample::active(1000, 1, Some(sql_info.clone()), 0, 0);
        assert_eq!(sample.sql_info, Some(sql_info));
    }

    // -----------------------------------------------------------------
    //  AshCollector 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_ash_collector_new() {
        let collector = AshCollector::new();
        assert_eq!(collector.capacity(), DEFAULT_ASH_BUFFER_CAPACITY);
        assert_eq!(
            collector.sample_interval_secs(),
            DEFAULT_ASH_SAMPLE_INTERVAL_SECS
        );
        assert!(collector.is_empty());
        assert_eq!(collector.total_sampled(), 0);
        assert_eq!(collector.dropped_samples(), 0);
    }

    #[test]
    fn test_ash_collector_with_capacity_and_interval() {
        let collector = AshCollector::with_capacity_and_interval(100, 5);
        assert_eq!(collector.capacity(), 100);
        assert_eq!(collector.sample_interval_secs(), 5);
    }

    #[test]
    fn test_ash_collector_record() {
        let mut collector = AshCollector::new();
        let sample = AshSample::active(1000, 1, None, 0, 0);
        collector.record(sample.clone());

        assert_eq!(collector.len(), 1);
        assert_eq!(collector.total_sampled(), 1);
        assert_eq!(collector.dropped_samples(), 0);
        assert_eq!(collector.samples()[0], sample);
    }

    #[test]
    fn test_ash_collector_record_many() {
        let mut collector = AshCollector::new();
        for i in 0..100 {
            collector.record(AshSample::active(1000 + i, 1, None, 0, 0));
        }
        assert_eq!(collector.len(), 100);
        assert_eq!(collector.total_sampled(), 100);
    }

    #[test]
    fn test_ash_collector_eviction() {
        let mut collector = AshCollector::with_capacity_and_interval(5, 1);
        for i in 0..10 {
            collector.record(AshSample::active(i, 1, None, 0, 0));
        }
        assert_eq!(collector.len(), 5); // 容量 5
        assert_eq!(collector.total_sampled(), 10);
        assert_eq!(collector.dropped_samples(), 5);
        // 应保留最后 5 个（时间戳 5~9）
        assert_eq!(collector.samples()[0].timestamp, 5);
        assert_eq!(collector.samples()[4].timestamp, 9);
    }

    #[test]
    fn test_ash_collector_should_sample() {
        let mut collector = AshCollector::with_capacity_and_interval(100, 5);
        collector.last_sample_time = 100;

        assert!(!collector.should_sample(104)); // 间隔不足
        assert!(collector.should_sample(105)); // 间隔恰好 5
        assert!(collector.should_sample(110)); // 间隔超过 5
    }

    #[test]
    fn test_ash_collector_sample_sessions() {
        let mut collector = AshCollector::with_capacity_and_interval(100, 1);
        let samples = vec![
            AshSample::active(1000, 1, None, 0, 0),
            AshSample::active(1000, 2, None, 0, 0),
        ];
        let count = collector.sample_sessions(1000, samples);
        assert_eq!(count, 2);
        assert_eq!(collector.len(), 2);
    }

    #[test]
    fn test_ash_collector_sample_sessions_skip_if_too_soon() {
        let mut collector = AshCollector::with_capacity_and_interval(100, 10);
        // 第一次采样（无 last_sample_time，应允许）
        let samples1 = vec![AshSample::active(100, 1, None, 0, 0)];
        let count1 = collector.sample_sessions(100, samples1);
        assert_eq!(count1, 1);

        // 第二次采样（间隔不足，应跳过）
        let samples2 = vec![AshSample::active(105, 1, None, 0, 0)];
        let count2 = collector.sample_sessions(105, samples2);
        assert_eq!(count2, 0);
        assert_eq!(collector.len(), 1);
    }

    #[test]
    fn test_ash_collector_samples_in_range() {
        let mut collector = AshCollector::with_capacity_and_interval(100, 1);
        for t in 0..10 {
            collector.record(AshSample::active(t, 1, None, 0, 0));
        }
        let in_range = collector.samples_in_range(3, 7);
        assert_eq!(in_range.len(), 5);
        for s in &in_range {
            assert!(s.timestamp >= 3 && s.timestamp <= 7);
        }
    }

    #[test]
    fn test_ash_collector_clear() {
        let mut collector = AshCollector::new();
        collector.record(AshSample::active(1000, 1, None, 0, 0));
        assert!(!collector.is_empty());

        collector.clear();
        assert!(collector.is_empty());
    }

    // -----------------------------------------------------------------
    //  AwrSnapshot 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_awr_snapshot_from_empty_samples() {
        let snapshot = AwrSnapshot::from_samples(0, 100, &[]);
        assert_eq!(snapshot.sample_count, 0);
        assert_eq!(snapshot.session_count, 0);
        assert_eq!(snapshot.sql_count, 0);
        assert_eq!(snapshot.duration_secs(), 100);
    }

    #[test]
    fn test_awr_snapshot_from_samples() {
        let samples = vec![
            AshSample::active(
                10,
                1,
                Some(SqlInfo::new(1, "SELECT 1", "u", "m", "p")),
                4096,
                1024,
            ),
            AshSample::waiting(
                11,
                2,
                WaitEvent::DataFileSequentialRead,
                Some(SqlInfo::new(2, "SELECT 2", "u", "m", "p")),
                8192,
                0,
            ),
            AshSample::idle(12, 3),
        ];
        let snapshot = AwrSnapshot::from_samples(10, 12, &samples);

        assert_eq!(snapshot.sample_count, 3);
        assert_eq!(snapshot.session_count, 3);
        assert_eq!(snapshot.sql_count, 2);
        assert_eq!(snapshot.active_count, 1);
        assert_eq!(snapshot.waiting_count, 1);
        assert_eq!(snapshot.idle_count, 1);
        assert_eq!(snapshot.total_physical_read_bytes, 4096 + 8192);
        assert_eq!(snapshot.total_physical_write_bytes, 1024);
    }

    #[test]
    fn test_awr_snapshot_duration_secs() {
        let snapshot = AwrSnapshot::from_samples(100, 200, &[]);
        assert_eq!(snapshot.duration_secs(), 100);
    }

    #[test]
    fn test_awr_snapshot_avg_active_sessions() {
        let samples = vec![
            AshSample::active(0, 1, None, 0, 0),
            AshSample::active(1, 2, None, 0, 0),
            AshSample::idle(2, 3),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 10, &samples);
        assert!((snapshot.avg_active_sessions() - 0.2).abs() < 1e-9); // 2 active / 10 sec
    }

    #[test]
    fn test_awr_snapshot_ratios() {
        let samples = vec![
            AshSample::active(0, 1, None, 0, 0),
            AshSample::active(1, 2, None, 0, 0),
            AshSample::waiting(2, 3, WaitEvent::LogFileSync, None, 0, 0),
            AshSample::idle(3, 4),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 10, &samples);
        assert!((snapshot.active_ratio() - 0.5).abs() < 1e-9);
        assert!((snapshot.waiting_ratio() - 0.25).abs() < 1e-9);
        assert!((snapshot.idle_ratio() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn test_awr_snapshot_ratios_empty() {
        let snapshot = AwrSnapshot::from_samples(0, 10, &[]);
        assert_eq!(snapshot.active_ratio(), 0.0);
        assert_eq!(snapshot.waiting_ratio(), 0.0);
        assert_eq!(snapshot.idle_ratio(), 0.0);
    }

    #[test]
    fn test_awr_snapshot_top_wait_events() {
        let samples = vec![
            AshSample::active(0, 1, None, 0, 0),
            AshSample::active(1, 1, None, 0, 0),
            AshSample::active(2, 1, None, 0, 0),
            AshSample::waiting(3, 1, WaitEvent::LogFileSync, None, 0, 0),
            AshSample::waiting(4, 1, WaitEvent::DataFileSequentialRead, None, 0, 0),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 5, &samples);
        let top = snapshot.top_wait_events(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, WaitEvent::Cpu);
        assert_eq!(top[0].1, 3);
        assert_eq!(top[1].1, 1);
    }

    #[test]
    fn test_awr_snapshot_top_wait_classes() {
        let samples = vec![
            AshSample::active(0, 1, None, 0, 0),
            AshSample::waiting(1, 1, WaitEvent::DataFileSequentialRead, None, 0, 0),
            AshSample::waiting(2, 1, WaitEvent::DataFileScatteredRead, None, 0, 0),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 3, &samples);
        let top = snapshot.top_wait_classes(2);
        assert_eq!(top.len(), 2);
        // User I/O 应该最多（2 次），CPU 其次（1 次）
        assert_eq!(top[0].0, WaitClass::UserIo);
        assert_eq!(top[0].1, 2);
    }

    #[test]
    fn test_awr_snapshot_top_sqls() {
        let samples = vec![
            AshSample::active(
                0,
                1,
                Some(SqlInfo::new(10, "SELECT 1", "u", "m", "p")),
                0,
                0,
            ),
            AshSample::active(
                1,
                1,
                Some(SqlInfo::new(10, "SELECT 1", "u", "m", "p")),
                0,
                0,
            ),
            AshSample::active(
                2,
                1,
                Some(SqlInfo::new(20, "SELECT 2", "u", "m", "p")),
                0,
                0,
            ),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 3, &samples);
        let top = snapshot.top_sqls(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, 10);
        assert_eq!(top[0].1, 2);
        assert_eq!(top[1].0, 20);
        assert_eq!(top[1].1, 1);
    }

    #[test]
    fn test_awr_snapshot_sql_text() {
        let samples = vec![AshSample::active(
            0,
            1,
            Some(SqlInfo::new(42, "SELECT 1", "u", "m", "p")),
            0,
            0,
        )];
        let snapshot = AwrSnapshot::from_samples(0, 1, &samples);
        assert_eq!(snapshot.sql_text(42), Some("SELECT 1"));
        assert_eq!(snapshot.sql_text(99), None);
    }

    #[test]
    fn test_awr_snapshot_total_physical_io_bytes() {
        let samples = vec![
            AshSample::active(0, 1, None, 1000, 2000),
            AshSample::active(1, 1, None, 3000, 4000),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 2, &samples);
        assert_eq!(snapshot.total_physical_read_bytes, 4000);
        assert_eq!(snapshot.total_physical_write_bytes, 6000);
        assert_eq!(snapshot.total_physical_io_bytes(), 10000);
    }

    // -----------------------------------------------------------------
    //  AwrReport 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_awr_report_new() {
        let snapshot = AwrSnapshot::from_samples(0, 100, &[]);
        let report = AwrReport::new(snapshot);
        assert_eq!(report.top_n, DEFAULT_TOP_N);
    }

    #[test]
    fn test_awr_report_with_top_n() {
        let snapshot = AwrSnapshot::from_samples(0, 100, &[]);
        let report = AwrReport::new(snapshot).with_top_n(5);
        assert_eq!(report.top_n, 5);
    }

    #[test]
    fn test_awr_report_render_contains_sections() {
        let samples = vec![
            AshSample::active(
                0,
                1,
                Some(SqlInfo::new(1, "SELECT * FROM users", "u", "m", "p")),
                4096,
                1024,
            ),
            AshSample::waiting(
                1,
                1,
                WaitEvent::DataFileSequentialRead,
                Some(SqlInfo::new(1, "SELECT * FROM users", "u", "m", "p")),
                8192,
                0,
            ),
        ];
        let snapshot = AwrSnapshot::from_samples(0, 10, &samples);
        let report = AwrReport::new(snapshot);
        let text = report.render();

        // 报告应包含各节标题
        assert!(text.contains("SzRSQL AWR Report"));
        assert!(text.contains("Snapshot Information"));
        assert!(text.contains("Session State Summary"));
        assert!(text.contains("Top") && text.contains("Wait Events"));
        assert!(text.contains("Top") && text.contains("Wait Classes"));
        assert!(text.contains("Top") && text.contains("SQL"));
        assert!(text.contains("Physical I/O"));
        assert!(text.contains("End of Report"));
    }

    #[test]
    fn test_awr_report_render_contains_sql_text() {
        let samples = vec![AshSample::active(
            0,
            1,
            Some(SqlInfo::new(42, "SELECT 1", "u", "m", "p")),
            0,
            0,
        )];
        let snapshot = AwrSnapshot::from_samples(0, 1, &samples);
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(text.contains("SELECT 1"));
        assert!(text.contains("42")); // SQL ID
    }

    #[test]
    fn test_awr_report_render_contains_wait_event() {
        let samples = vec![AshSample::waiting(0, 1, WaitEvent::LogFileSync, None, 0, 0)];
        let snapshot = AwrSnapshot::from_samples(0, 1, &samples);
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(text.contains("log file sync"));
    }

    #[test]
    fn test_awr_report_render_contains_physical_io() {
        let samples = vec![AshSample::active(0, 1, None, 1_048_576, 524_288)]; // 1MB read, 512KB write
        let snapshot = AwrSnapshot::from_samples(0, 1, &samples);
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(text.contains("1.00 MB")); // read
        assert!(text.contains("0.50 MB")); // write
    }

    #[test]
    fn test_awr_report_render_empty_snapshot() {
        let snapshot = AwrSnapshot::from_samples(0, 10, &[]);
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(text.contains("SzRSQL AWR Report"));
        assert!(text.contains("Samples:         0"));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_generate_mixed_workload_samples_basic() {
        let samples = generate_mixed_workload_samples(1000, 10, 5);
        // 10 秒 × 5 线程 = 50 个采样
        assert_eq!(samples.len(), 50);

        // 第一个采样时间戳应为 1000
        assert_eq!(samples[0].timestamp, 1000);

        // 最后一个采样时间戳应为 1000 + 9 = 1009
        assert_eq!(samples[49].timestamp, 1009);

        // 会话 ID 范围 1~5
        for s in &samples {
            assert!(s.session_id >= 1 && s.session_id <= 5);
        }
    }

    #[test]
    fn test_generate_mixed_workload_samples_states() {
        let samples = generate_mixed_workload_samples(1000, 100, 10);
        // 应该有 Active/Waiting/Idle 三种状态
        let has_active = samples.iter().any(|s| s.is_active());
        let has_waiting = samples.iter().any(|s| s.is_waiting());
        let has_idle = samples.iter().any(|s| s.is_idle());

        // 由于伪随机，大样本下应该都有（极小概率缺失，但 1000 个样本足够）
        assert!(has_active, "should have active samples");
        assert!(has_waiting, "should have waiting samples");
        assert!(has_idle, "should have idle samples");
    }

    #[test]
    fn test_generate_mixed_workload_samples_sql_info() {
        let samples = generate_mixed_workload_samples(1000, 10, 5);
        // Active/Waiting 应有 SQL 信息，Idle 应无
        for s in &samples {
            if s.is_idle() {
                assert!(s.sql_info.is_none(), "idle sample should have no sql_info");
            } else {
                assert!(s.sql_info.is_some(), "non-idle sample should have sql_info");
            }
        }
    }

    #[test]
    fn test_generate_sequential_samples() {
        let samples = generate_sequential_samples(1000, 20, 1);
        assert_eq!(samples.len(), 20);
        assert_eq!(samples[0].timestamp, 1000);
        assert_eq!(samples[19].timestamp, 1019);

        // 每 5 秒一次 Waiting
        let waiting_count = samples.iter().filter(|s| s.is_waiting()).count();
        let active_count = samples.iter().filter(|s| s.is_active()).count();
        assert_eq!(waiting_count, 4); // t=0,5,10,15
        assert_eq!(active_count, 16);
    }

    #[test]
    fn test_generate_lock_wait_samples() {
        let samples = generate_lock_wait_samples(1000, 10, 1);
        assert_eq!(samples.len(), 10);

        // 全部应是 Waiting + EnqueueTxRowLock
        for s in &samples {
            assert!(s.is_waiting());
            assert_eq!(s.wait_event, WaitEvent::EnqueueTxRowLock);
            assert!(s.sql_info.is_some());
        }
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    /// 集成测试：完整 ASH 采样 + AWR 报告工作流
    #[test]
    fn test_integration_full_workflow() {
        // 1. 生成混合负载采样（10 线程 × 60 秒 = 600 采样）
        let samples = generate_mixed_workload_samples(1000, 60, 10);
        assert_eq!(samples.len(), 600);

        // 2. 收集到 ASH 采样器
        let mut collector = AshCollector::with_capacity_and_interval(10_000, 1);
        for s in samples {
            collector.record(s);
        }
        assert_eq!(collector.len(), 600);

        // 3. 生成 AWR 快照
        let snapshot = AwrSnapshot::from_samples(1000, 1059, collector.samples());

        // 4. 生成 AWR 报告
        let report = AwrReport::new(snapshot).with_top_n(10);
        let text = report.render();

        // 报告应包含关键内容
        assert!(text.contains("SzRSQL AWR Report"));
        assert!(text.contains("Top 10 Wait Events"));
        assert!(text.contains("Top 10 SQL"));
        assert!(text.contains("Physical I/O"));
    }

    /// 集成测试：ASH 采样器自动丢弃旧采样
    #[test]
    fn test_integration_collector_eviction() {
        let mut collector = AshCollector::with_capacity_and_interval(100, 1);
        // 写入 200 个采样，应丢弃 100 个
        for i in 0..200 {
            collector.record(AshSample::active(i, 1, None, 0, 0));
        }
        assert_eq!(collector.len(), 100);
        assert_eq!(collector.total_sampled(), 200);
        assert_eq!(collector.dropped_samples(), 100);
        // 保留时间戳 100~199
        assert_eq!(collector.samples()[0].timestamp, 100);
        assert_eq!(collector.samples()[99].timestamp, 199);
    }

    /// 集成测试：AWR 报告格式类似 Oracle（包含 Top SQL/等待事件/物理 IO）
    #[test]
    fn test_integration_awr_report_oracle_like_format() {
        let samples = generate_mixed_workload_samples(1000, 60, 10);
        let snapshot = AwrSnapshot::from_samples(1000, 1059, &samples);
        let report = AwrReport::new(snapshot).with_top_n(5);
        let text = report.render();

        // Oracle AWR 报告特征：
        // 1. 标题
        assert!(text.contains("AWR Report"));
        // 2. 快照信息
        assert!(text.contains("Snapshot Information"));
        assert!(text.contains("Start Time"));
        assert!(text.contains("End Time"));
        assert!(text.contains("Duration"));
        // 3. Top SQL
        assert!(text.contains("Top 5 SQL"));
        assert!(text.contains("SQL ID"));
        assert!(text.contains("SQL Text"));
        // 4. Top 等待事件
        assert!(text.contains("Top 5 Wait Events"));
        assert!(text.contains("Event"));
        assert!(text.contains("Count"));
        // 5. 物理 IO
        assert!(text.contains("Physical I/O"));
        assert!(text.contains("Physical Read"));
        assert!(text.contains("Physical Write"));
    }

    /// 集成测试：多线程混合负载场景（10 线程 × 10 分钟 = 6000 采样）
    #[test]
    fn test_integration_10_threads_10_minutes() {
        // 模拟 10 线程 × 600 秒 = 6000 采样
        let samples = generate_mixed_workload_samples(0, 600, 10);
        assert_eq!(samples.len(), 6000);

        // 会话数应为 10
        let snapshot = AwrSnapshot::from_samples(0, 599, &samples);
        assert_eq!(snapshot.session_count, 10);

        // 应有多个 SQL（10 个会话，每个一个 SQL）
        assert!(snapshot.sql_count >= 5); // Idle 采样无 SQL，所以可能少于 10

        // 报告应正常生成
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(!text.is_empty());
    }

    /// 集成测试：采样时间范围过滤
    #[test]
    fn test_integration_time_range_filter() {
        let mut collector = AshCollector::new();
        for t in 0..100 {
            collector.record(AshSample::active(t, 1, None, 0, 0));
        }

        let in_range = collector.samples_in_range(25, 75);
        assert_eq!(in_range.len(), 51); // [25, 75] 共 51 个时间戳

        for s in &in_range {
            assert!(s.timestamp >= 25 && s.timestamp <= 75);
        }
    }

    /// 集成测试：等待事件统计准确性
    #[test]
    fn test_integration_wait_event_stats_accuracy() {
        let mut samples = Vec::new();
        // 30 个 CPU
        for i in 0..30 {
            samples.push(AshSample::active(i, 1, None, 0, 0));
        }
        // 20 个 db file sequential read
        for i in 30..50 {
            samples.push(AshSample::waiting(
                i,
                1,
                WaitEvent::DataFileSequentialRead,
                None,
                0,
                0,
            ));
        }
        // 10 个 log file sync
        for i in 50..60 {
            samples.push(AshSample::waiting(i, 1, WaitEvent::LogFileSync, None, 0, 0));
        }

        let snapshot = AwrSnapshot::from_samples(0, 60, &samples);
        assert_eq!(snapshot.wait_event_counts.get(&WaitEvent::Cpu), Some(&30));
        assert_eq!(
            snapshot
                .wait_event_counts
                .get(&WaitEvent::DataFileSequentialRead),
            Some(&20)
        );
        assert_eq!(
            snapshot.wait_event_counts.get(&WaitEvent::LogFileSync),
            Some(&10)
        );

        let top = snapshot.top_wait_events(3);
        assert_eq!(top[0].0, WaitEvent::Cpu);
        assert_eq!(top[0].1, 30);
    }

    /// 集成测试：SQL 排名准确性
    #[test]
    fn test_integration_sql_ranking() {
        let mut samples = Vec::new();
        // SQL 1: 50 次
        for _ in 0..50 {
            samples.push(AshSample::active(
                0,
                1,
                Some(SqlInfo::new(1, "SELECT 1", "u", "m", "p")),
                0,
                0,
            ));
        }
        // SQL 2: 30 次
        for _ in 0..30 {
            samples.push(AshSample::active(
                0,
                1,
                Some(SqlInfo::new(2, "SELECT 2", "u", "m", "p")),
                0,
                0,
            ));
        }
        // SQL 3: 10 次
        for _ in 0..10 {
            samples.push(AshSample::active(
                0,
                1,
                Some(SqlInfo::new(3, "SELECT 3", "u", "m", "p")),
                0,
                0,
            ));
        }

        let snapshot = AwrSnapshot::from_samples(0, 100, &samples);
        let top = snapshot.top_sqls(3);
        assert_eq!(top[0].0, 1);
        assert_eq!(top[0].1, 50);
        assert_eq!(top[1].0, 2);
        assert_eq!(top[1].1, 30);
        assert_eq!(top[2].0, 3);
        assert_eq!(top[2].1, 10);
    }

    /// 集成测试：物理 I/O 累加准确性
    #[test]
    fn test_integration_physical_io_accumulation() {
        let mut samples = Vec::new();
        for i in 0..100 {
            samples.push(AshSample::active(0, 1, None, i * 1000, i * 500));
        }
        let snapshot = AwrSnapshot::from_samples(0, 100, &samples);

        // 总读 = sum(0, 1000, 2000, ..., 99000) = 1000 * (0+99)*100/2 = 4950000
        assert_eq!(
            snapshot.total_physical_read_bytes,
            (0..100u64).map(|i| i * 1000).sum::<u64>()
        );
        assert_eq!(
            snapshot.total_physical_write_bytes,
            (0..100u64).map(|i| i * 500).sum::<u64>()
        );
    }

    /// 集成测试：纯锁等待场景
    #[test]
    fn test_integration_pure_lock_wait_scenario() {
        let samples = generate_lock_wait_samples(0, 30, 1);
        let snapshot = AwrSnapshot::from_samples(0, 30, &samples);

        // 全部应是在等待 EnqueueTxRowLock
        assert_eq!(snapshot.waiting_count, 30);
        assert_eq!(snapshot.active_count, 0);
        assert_eq!(snapshot.idle_count, 0);
        assert_eq!(
            snapshot.wait_event_counts.get(&WaitEvent::EnqueueTxRowLock),
            Some(&30)
        );
        assert_eq!(
            snapshot.wait_class_counts.get(&WaitClass::Application),
            Some(&30)
        );

        // 报告应突出显示锁等待
        let report = AwrReport::new(snapshot);
        let text = report.render();
        assert!(text.contains("enq: TX - row lock contention"));
        assert!(text.contains("Application"));
    }

    /// 集成测试：ASH 采样间隔控制
    #[test]
    fn test_integration_sample_interval_control() {
        let mut collector = AshCollector::with_capacity_and_interval(1000, 5);

        // t=0 第一次采样，应允许
        assert_eq!(
            collector.sample_sessions(0, vec![AshSample::active(0, 1, None, 0, 0)]),
            1
        );

        // t=3 间隔不足，应跳过
        assert_eq!(
            collector.sample_sessions(3, vec![AshSample::active(3, 1, None, 0, 0)]),
            0
        );

        // t=5 间隔恰好，应允许
        assert_eq!(
            collector.sample_sessions(5, vec![AshSample::active(5, 1, None, 0, 0)]),
            1
        );

        // t=10 间隔超过，应允许
        assert_eq!(
            collector.sample_sessions(10, vec![AshSample::active(10, 1, None, 0, 0)]),
            1
        );

        assert_eq!(collector.len(), 3);
    }

    /// 大规模测试：10 线程 × 600 秒（10 分钟）混合负载（#[ignore] 默认跳过）
    #[test]
    #[ignore = "大规模测试：10 线程 × 600 秒混合负载 ASH/AWR"]
    fn test_integration_large_scale_10_threads_10_minutes() {
        let samples = generate_mixed_workload_samples(0, 600, 10);
        assert_eq!(samples.len(), 6000);

        let mut collector = AshCollector::new();
        for s in samples {
            collector.record(s);
        }

        let snapshot = AwrSnapshot::from_samples(0, 599, collector.samples());
        let report = AwrReport::new(snapshot).with_top_n(20);
        let text = report.render();

        assert!(text.contains("Top 20 Wait Events"));
        assert!(text.contains("Top 20 SQL"));
    }
}
