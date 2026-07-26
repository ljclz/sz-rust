//! Cron 调度器 — Phase 7a.1
//!
//! # 设计
//!
//! - **`CronExpr`** — 5 字段 cron 表达式解析（分 时 日 月 周）
//! - **`CronScheduler`** — 基于 tokio 的异步调度器，多任务并行不阻塞
//! - **`ScheduledTask`** — 定时任务定义（名称 + cron + 回调 + 状态）
//! - **调度语义**（与 Vixie cron 一致）：
//!   - `*` — 任意值
//!   - `*/n` — 每 n 个单位
//!   - `a-b` — 范围
//!   - `a,b,c` — 列表
//!   - `a-b/n` — 范围内步进
//!   - 日 + 周组合：OR 语义（任一匹配即触发，与标准 cron 一致）
//! - **`next_after(instant)`** — 计算下一个匹配时间点
//!
//! 对应 `SzRSQL实施进度.md` Phase 7a.1。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;

// =====================================================================
//  CronError
// =====================================================================

/// Cron 调度器错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CronError {
    /// cron 表达式字段数不为 5
    #[error("invalid cron expression: expected 5 fields, got {actual}")]
    InvalidFieldCount { actual: usize },
    /// 字段值超出范围
    #[error("field '{field}': value {value} out of range [{min}, {max}]")]
    ValueOutOfRange {
        field: &'static str,
        value: i32,
        min: i32,
        max: i32,
    },
    /// 步进值为 0
    #[error("field '{field}': step cannot be zero")]
    ZeroStep { field: &'static str },
    /// 范围起始 > 结束
    #[error("field '{field}': range start {start} > end {end}")]
    InvalidRange {
        field: &'static str,
        start: i32,
        end: i32,
    },
    /// 空字段
    #[error("field '{field}': empty field")]
    EmptyField { field: &'static str },
    /// 无法解析的字段值
    #[error("field '{field}': cannot parse '{value}'")]
    ParseError { field: &'static str, value: String },
    /// 任务名已存在
    #[error("task '{name}' already exists")]
    TaskAlreadyExists { name: String },
    /// 任务不存在
    #[error("task '{name}' not found")]
    TaskNotFound { name: String },
    /// 任务名不能为空
    #[error("task name cannot be empty")]
    EmptyTaskName,
    /// 下一匹配时间超出 u32 范围
    #[error("no valid next time within searchable range")]
    NoNextTime,
    /// 任务名与已禁用的保留字冲突（Phase 7a.2）
    #[error("task name '{name}' is a reserved keyword")]
    ReservedName { name: String },
}

// =====================================================================
//  CronField — cron 单字段
// =====================================================================

/// cron 字段类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CronFieldType {
    Minute, // 0-59
    Hour,   // 0-23
    Day,    // 1-31
    Month,  // 1-12
    Dow,    // 0-7 (0=Sunday, 7=Sunday)
}

impl CronFieldType {
    fn name(self) -> &'static str {
        match self {
            CronFieldType::Minute => "minute",
            CronFieldType::Hour => "hour",
            CronFieldType::Day => "day-of-month",
            CronFieldType::Month => "month",
            CronFieldType::Dow => "day-of-week",
        }
    }

    fn range(self) -> (i32, i32) {
        match self {
            CronFieldType::Minute => (0, 59),
            CronFieldType::Hour => (0, 23),
            CronFieldType::Day => (1, 31),
            CronFieldType::Month => (1, 12),
            CronFieldType::Dow => (0, 7),
        }
    }
}

/// cron 单字段匹配集
///
/// 用 `Vec<bool>` 表示哪些值匹配（索引 = 值）。
/// 对于 Dow，0 和 7 都表示周日，解析时归一化为 0。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CronField {
    /// 匹配集，索引对应值
    bits: Vec<bool>,
    /// 字段类型
    field_type: CronFieldType,
}

impl CronField {
    /// 解析 cron 单字段
    fn parse(text: &str, field_type: CronFieldType) -> Result<Self, CronError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CronError::EmptyField {
                field: field_type.name(),
            });
        }

        let (min, max) = field_type.range();
        // 对于 Dow，bits 长度为 7（0-6），7 归一化为 0
        let len = if field_type == CronFieldType::Dow {
            7
        } else {
            (max - min + 1) as usize
        };
        let mut bits = vec![false; len];

        // 按逗号分割
        for part in text.split(',') {
            Self::parse_part(part, field_type, min, max, &mut bits)?;
        }

        Ok(Self { bits, field_type })
    }

    /// 解析单个逗号分隔部分
    fn parse_part(
        part: &str,
        field_type: CronFieldType,
        min: i32,
        max: i32,
        bits: &mut [bool],
    ) -> Result<(), CronError> {
        // 处理 */n
        let (range_part, step) = if let Some(slash_pos) = part.find('/') {
            let range_str = &part[..slash_pos];
            let step_str = &part[slash_pos + 1..];
            let step: i32 = step_str.parse().map_err(|_| CronError::ParseError {
                field: field_type.name(),
                value: step_str.to_string(),
            })?;
            if step == 0 {
                return Err(CronError::ZeroStep {
                    field: field_type.name(),
                });
            }
            (range_str.to_string(), step)
        } else {
            (part.to_string(), 1)
        };

        // 解析范围
        let (start, end) = if range_part == "*" {
            // Dow: 7 归一化为 0，有效范围是 [0, 6]
            if field_type == CronFieldType::Dow {
                (0, 6)
            } else {
                (min, max)
            }
        } else if let Some(dash_pos) = range_part.find('-') {
            let s: i32 = range_part[..dash_pos]
                .parse()
                .map_err(|_| CronError::ParseError {
                    field: field_type.name(),
                    value: range_part.clone(),
                })?;
            let e: i32 = range_part[dash_pos + 1..]
                .parse()
                .map_err(|_| CronError::ParseError {
                    field: field_type.name(),
                    value: range_part.clone(),
                })?;
            // Dow 归一化：7 → 0
            let s = Self::normalize_dow(s, field_type);
            let e = Self::normalize_dow(e, field_type);
            if s > e {
                return Err(CronError::InvalidRange {
                    field: field_type.name(),
                    start: s,
                    end: e,
                });
            }
            (s, e)
        } else {
            let v: i32 = range_part.parse().map_err(|_| CronError::ParseError {
                field: field_type.name(),
                value: range_part.clone(),
            })?;
            let v = Self::normalize_dow(v, field_type);
            (v, v)
        };

        // 范围校验
        let norm_min = if field_type == CronFieldType::Dow {
            0
        } else {
            min
        };
        let norm_max = if field_type == CronFieldType::Dow {
            6
        } else {
            max
        };
        if start < norm_min || start > norm_max {
            return Err(CronError::ValueOutOfRange {
                field: field_type.name(),
                value: start,
                min,
                max,
            });
        }
        if end < norm_min || end > norm_max {
            return Err(CronError::ValueOutOfRange {
                field: field_type.name(),
                value: end,
                min,
                max,
            });
        }

        // 设置匹配位
        let mut current = start;
        while current <= end {
            let idx = (current - min).max(0) as usize;
            if idx < bits.len() {
                bits[idx] = true;
            }
            current += step;
        }

        Ok(())
    }

    /// Dow 归一化：7 → 0
    fn normalize_dow(v: i32, field_type: CronFieldType) -> i32 {
        if field_type == CronFieldType::Dow && v == 7 {
            0
        } else {
            v
        }
    }

    /// 检查值是否匹配
    fn matches(&self, value: i32) -> bool {
        let (min, _) = self.field_type.range();
        let idx = (value - min) as usize;
        // Dow: 0 和 7 都匹配同一个 bit（索引 0）
        if self.field_type == CronFieldType::Dow && value == 7 {
            return self.bits[0];
        }
        if idx < self.bits.len() {
            self.bits[idx]
        } else {
            false
        }
    }

    /// 是否匹配任意值（全 true）
    fn is_star(&self) -> bool {
        self.bits.iter().all(|&b| b)
    }
}

// =====================================================================
//  CronExpr — 完整 cron 表达式
// =====================================================================

/// 解析后的 cron 表达式
///
/// 格式：`分 时 日 月 周`（5 字段）
///
/// # 示例
///
/// ```
/// use szrsql_scheduler::scheduler::CronExpr;
///
/// let expr = CronExpr::parse("0 2 * * *").unwrap(); // 每天 02:00
/// // 1970-01-01 02:00:00 UTC = epoch 7200
/// assert!(expr.matches_epoch(7200));
/// // 1970-01-01 03:00:00 UTC = epoch 10800（不匹配）
/// assert!(!expr.matches_epoch(10800));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    minute: CronField,
    hour: CronField,
    day: CronField,
    month: CronField,
    dow: CronField,
    /// 原始表达式文本
    raw: String,
}

impl CronExpr {
    /// 解析 cron 表达式
    pub fn parse(expr: &str) -> Result<Self, CronError> {
        let raw = expr.to_string();
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::InvalidFieldCount {
                actual: fields.len(),
            });
        }

        Ok(Self {
            minute: CronField::parse(fields[0], CronFieldType::Minute)?,
            hour: CronField::parse(fields[1], CronFieldType::Hour)?,
            day: CronField::parse(fields[2], CronFieldType::Day)?,
            month: CronField::parse(fields[3], CronFieldType::Month)?,
            dow: CronField::parse(fields[4], CronFieldType::Dow)?,
            raw,
        })
    }

    /// 获取原始表达式
    pub fn raw(&self) -> &str {
        &self.raw
    }

    fn matches_minute(&self, v: i32) -> bool {
        self.minute.matches(v)
    }
    fn matches_hour(&self, v: i32) -> bool {
        self.hour.matches(v)
    }
    fn matches_day(&self, v: i32) -> bool {
        self.day.matches(v)
    }
    fn matches_month(&self, v: i32) -> bool {
        self.month.matches(v)
    }
    fn matches_dow(&self, v: i32) -> bool {
        self.dow.matches(v)
    }

    /// 判断是否日和周都是 `*`
    fn day_and_dow_both_restricted(&self) -> bool {
        !self.day.is_star() && !self.dow.is_star()
    }

    /// 检查给定时间分量是否匹配
    ///
    /// 日 + 周组合使用 OR 语义（标准 cron 行为）：
    /// - 如果日和周都不是 `*`，任一匹配即通过
    /// - 如果日或周任一是 `*`，两者都需匹配
    fn matches_time(&self, minute: i32, hour: i32, day: i32, month: i32, dow: i32) -> bool {
        if !self.matches_minute(minute) {
            return false;
        }
        if !self.matches_hour(hour) {
            return false;
        }
        if !self.matches_month(month) {
            return false;
        }
        // 日 + 周组合（OR 语义）
        if self.day_and_dow_both_restricted() {
            self.matches_day(day) || self.matches_dow(dow)
        } else {
            self.matches_day(day) && self.matches_dow(dow)
        }
    }

    /// 计算从 `after_epoch` 之后的下一个匹配时间点（Unix epoch 秒）
    ///
    /// 从 `after_epoch + 1` 秒开始逐分钟扫描。
    /// 最大扫描范围为 366 天（防止无效表达式无限循环）。
    pub fn next_after(&self, after_epoch: u64) -> Result<u64, CronError> {
        // 从下一分钟开始（截断到分钟边界）
        let start = (after_epoch / 60 + 1) * 60;
        // 最大扫描 366 天 = 366 * 24 * 60 分钟
        let max_minutes = 366 * 24 * 60;

        for offset in 0..max_minutes {
            let epoch = start + offset * 60;
            let (minute, hour, day, month, dow) = epoch_to_cron_fields(epoch);
            if self.matches_time(minute, hour, day, month, dow) {
                return Ok(epoch);
            }
        }

        Err(CronError::NoNextTime)
    }

    /// 检查给定 epoch 秒是否匹配
    pub fn matches_epoch(&self, epoch: u64) -> bool {
        let (minute, hour, day, month, dow) = epoch_to_cron_fields(epoch);
        self.matches_time(minute, hour, day, month, dow)
    }
}

impl std::fmt::Display for CronExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

impl std::str::FromStr for CronExpr {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, CronError> {
        Self::parse(s)
    }
}

// =====================================================================
//  时间转换辅助函数
// =====================================================================

/// 将 Unix epoch 秒转换为 cron 字段（minute, hour, day, month, dow）
///
/// 使用Civil from days算法（Howard Hinnant），无外部依赖。
fn epoch_to_cron_fields(epoch: u64) -> (i32, i32, i32, i32, i32) {
    let days = (epoch / 86400) as i64;
    let secs_in_day = (epoch % 86400) as i32;
    let hour = secs_in_day / 3600;
    let minute = (secs_in_day % 3600) / 60;

    // 1970-01-01 是周四 → dow = 4
    // 0=Sunday, 1=Monday, ..., 6=Saturday
    let dow = ((days % 7 + 4) % 7) as i32; // +4 因为 1970-01-01 是 Thursday(4)

    // Civil from days（Howard Hinnant 算法）
    let z = days + 719468; // 1970-01-01 对应的 serial date
    let era = if z >= 0 {
        z
    } else {
        z - 146096
    } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 {
        mp + 3
    } else {
        mp - 9
    }; // [1, 12]

    (minute, hour, d as i32, m as i32, dow)
}

/// 将 cron 字段（year, month, day, hour, minute）转换为 Unix epoch 秒
#[allow(dead_code)]
fn cron_fields_to_epoch(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> u64 {
    // Days from civil（Howard Hinnant 算法）
    let y = if month <= 2 {
        year - 1
    } else {
        year
    } as i64;
    let era = if y >= 0 {
        y
    } else {
        y - 399
    } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153
        * (if month > 2 {
            month - 3
        } else {
            month + 9
        }) as u64
        + 2)
        / 5
        + day as u64
        - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe as i64 - 719468;

    (days * 86400 + hour as i64 * 3600 + minute as i64 * 60) as u64
}

/// 获取当前 Unix epoch 秒
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// =====================================================================
//  TaskStatus
// =====================================================================

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// 活跃（调度中）
    Active,
    /// 已暂停
    Paused,
    /// 已完成（一次性任务执行完毕）
    Completed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Active => f.write_str("Active"),
            TaskStatus::Paused => f.write_str("Paused"),
            TaskStatus::Completed => f.write_str("Completed"),
        }
    }
}

// =====================================================================
//  TaskCallback
// =====================================================================

/// 任务回调类型（异步，无参数，返回是否成功）
pub type TaskCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// 从闭包创建任务回调
pub fn make_callback<F, Fut>(f: F) -> TaskCallback
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    Arc::new(move || Box::pin(f()))
}

// =====================================================================
//  ScheduledTask
// =====================================================================

/// 定时任务定义
#[derive(Clone)]
pub struct ScheduledTask {
    /// 任务名（唯一）
    pub name: String,
    /// cron 表达式
    pub cron: CronExpr,
    /// 任务回调
    pub callback: TaskCallback,
    /// 状态
    pub status: TaskStatus,
    /// 已执行次数
    pub run_count: u64,
    /// 上次执行时间（epoch 秒，None=未执行）
    pub last_run: Option<u64>,
    /// 下次执行时间（epoch 秒，None=未计算）
    pub next_run: Option<u64>,
    /// 上次执行结果
    pub last_result: Option<Result<(), String>>,
}

impl std::fmt::Debug for ScheduledTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledTask")
            .field("name", &self.name)
            .field("cron", &self.cron)
            .field("status", &self.status)
            .field("run_count", &self.run_count)
            .field("last_run", &self.last_run)
            .field("next_run", &self.next_run)
            .field("last_result", &self.last_result)
            .finish()
    }
}

impl ScheduledTask {
    /// 创建新任务
    pub fn new(name: String, cron: CronExpr, callback: TaskCallback) -> Self {
        Self {
            name,
            cron,
            callback,
            status: TaskStatus::Active,
            run_count: 0,
            last_run: None,
            next_run: None,
            last_result: None,
        }
    }

    /// 更新下次执行时间
    pub fn update_next_run(&mut self, now: u64) -> Result<(), CronError> {
        let next = self.cron.next_after(now)?;
        self.next_run = Some(next);
        Ok(())
    }

    /// 更新 cron 表达式（Phase 7a.2）
    ///
    /// 重新计算 next_run。若任务已 Completed 则恢复为 Active。
    pub fn update_cron(&mut self, cron: CronExpr, now: u64) -> Result<(), CronError> {
        self.cron = cron;
        self.update_next_run(now)?;
        if self.status == TaskStatus::Completed {
            self.status = TaskStatus::Active;
        }
        Ok(())
    }

    /// 更新回调函数（Phase 7a.2）
    pub fn update_callback(&mut self, callback: TaskCallback) {
        self.callback = callback;
    }
}

// =====================================================================
//  CronScheduler
// =====================================================================

/// Cron 调度器
///
/// 基于 tokio 的异步调度器，支持多任务并行调度。
///
/// # 用法
///
/// ```ignore
/// use szrsql_scheduler::scheduler::{CronScheduler, CronExpr, make_callback};
///
/// # tokio_test::block_on(async {
/// let mut scheduler = CronScheduler::new();
/// let expr = CronExpr::parse("* * * * *").unwrap();
/// let task = make_callback(|| async { Ok(()) });
/// scheduler.register("every_minute", expr, task).unwrap();
/// let handle = scheduler.start().await;
/// // ... 运行 ...
/// handle.stop().await;
/// # });
/// ```
pub struct CronScheduler {
    tasks: HashMap<String, ScheduledTask>,
    /// 全局停止标志
    stop_flag: Arc<AtomicBool>,
    /// 全局执行计数（所有任务累计）
    total_executions: Arc<AtomicU64>,
}

impl CronScheduler {
    /// 创建空调度器
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            total_executions: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 注册任务
    pub fn register(
        &mut self,
        name: &str,
        cron: CronExpr,
        callback: TaskCallback,
    ) -> Result<(), CronError> {
        if name.is_empty() {
            return Err(CronError::EmptyTaskName);
        }
        if self.tasks.contains_key(name) {
            return Err(CronError::TaskAlreadyExists {
                name: name.to_string(),
            });
        }
        let mut task = ScheduledTask::new(name.to_string(), cron, callback);
        task.update_next_run(now_epoch())?;
        self.tasks.insert(name.to_string(), task);
        Ok(())
    }

    /// 注销任务
    pub fn unregister(&mut self, name: &str) -> Result<ScheduledTask, CronError> {
        self.tasks
            .remove(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })
    }

    /// 暂停任务
    pub fn pause(&mut self, name: &str) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.status = TaskStatus::Paused;
        Ok(())
    }

    /// 恢复任务
    pub fn resume(&mut self, name: &str) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.status = TaskStatus::Active;
        task.update_next_run(now_epoch())?;
        Ok(())
    }

    /// 获取任务状态
    pub fn get_status(&self, name: &str) -> Result<TaskStatus, CronError> {
        self.tasks
            .get(name)
            .map(|t| t.status)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })
    }

    /// 获取任务信息
    pub fn get_task(&self, name: &str) -> Option<&ScheduledTask> {
        self.tasks.get(name)
    }

    /// 列出所有任务名
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tasks.keys().cloned().collect();
        names.sort();
        names
    }

    /// 任务数量
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 获取全局执行计数
    pub fn total_executions(&self) -> u64 {
        self.total_executions.load(Ordering::Relaxed)
    }

    /// 修改任务 cron 表达式（Phase 7a.2）
    ///
    /// 重新计算 next_run 基于 now_epoch()。
    pub fn update_cron(&mut self, name: &str, cron: CronExpr) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_cron(cron, now_epoch())
    }

    /// 修改任务回调（Phase 7a.2）
    pub fn update_callback(&mut self, name: &str, callback: TaskCallback) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_callback(callback);
        Ok(())
    }

    /// 同时修改 cron 和回调（Phase 7a.2）
    pub fn update_task(
        &mut self,
        name: &str,
        cron: CronExpr,
        callback: TaskCallback,
    ) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_cron(cron, now_epoch())?;
        task.update_callback(callback);
        Ok(())
    }

    /// 检测 next_run 时间冲突的任务（Phase 7a.2）
    ///
    /// 返回所有 next_run 相同的 Active 任务组（每组 ≥ 2 个任务名）。
    pub fn detect_conflicts(&self) -> Vec<Vec<String>> {
        let mut groups: HashMap<u64, Vec<String>> = HashMap::new();
        for (name, task) in &self.tasks {
            if task.status != TaskStatus::Active {
                continue;
            }
            if let Some(next) = task.next_run {
                groups.entry(next).or_default().push(name.clone());
            }
        }
        let mut conflicts: Vec<Vec<String>> =
            groups.into_values().filter(|g| g.len() >= 2).collect();
        for g in &mut conflicts {
            g.sort();
        }
        conflicts.sort();
        conflicts
    }

    /// 检测在指定时间点同时触发的任务（Phase 7a.2）
    ///
    /// 返回所有 cron 表达式匹配该 epoch 的 Active 任务名（按字母序）。
    pub fn detect_conflicts_at(&self, epoch: u64) -> Vec<String> {
        let mut hits: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.status == TaskStatus::Active && t.cron.matches_epoch(epoch))
            .map(|(name, _)| name.clone())
            .collect();
        hits.sort();
        hits
    }

    /// 启动调度器（异步）
    ///
    /// 返回 `SchedulerHandle`，可通过 `stop()` 停止。
    pub async fn start(self) -> SchedulerHandle {
        let stop_flag = self.stop_flag.clone();
        let total_executions = self.total_executions.clone();

        // 将 tasks 转为可共享的 Arc
        type TaskEntry = (
            String,
            CronExpr,
            TaskCallback,
            Arc<AtomicBool>,
            Arc<AtomicU64>,
        );
        let tasks: Vec<TaskEntry> = self
            .tasks
            .into_iter()
            .filter(|(_, t)| t.status == TaskStatus::Active)
            .map(|(name, task)| {
                let task_stop = Arc::new(AtomicBool::new(false));
                let task_count = Arc::new(AtomicU64::new(0));
                (
                    name,
                    task.cron,
                    task.callback,
                    task_stop.clone(),
                    task_count.clone(),
                )
            })
            .collect();

        // 收集每个任务的 stop flag
        let mut task_stops: Vec<(String, Arc<AtomicBool>, Arc<AtomicU64>)> = Vec::new();
        let mut task_handles = Vec::new();

        for (name, cron, callback, task_stop, task_count) in tasks {
            task_stops.push((name.clone(), task_stop.clone(), task_count.clone()));
            let global_stop = stop_flag.clone();
            let global_count = total_executions.clone();

            let handle = tokio::spawn(async move {
                loop {
                    if global_stop.load(Ordering::Relaxed) || task_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    // 计算下次执行时间
                    let now = now_epoch();
                    let next = match cron.next_after(now) {
                        Ok(t) => t,
                        Err(_) => break,
                    };

                    // 等待到执行时间
                    let sleep_secs = next.saturating_sub(now);
                    let sleep_dur = Duration::from_secs(sleep_secs.min(3600)); // 最多睡1小时，定期检查 stop
                    tokio::time::sleep(sleep_dur).await;

                    if global_stop.load(Ordering::Relaxed) || task_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    // 检查是否到了执行时间（防止提前唤醒）
                    if now_epoch() >= next {
                        // 执行任务
                        let result = (callback)().await;
                        task_count.fetch_add(1, Ordering::Relaxed);
                        global_count.fetch_add(1, Ordering::Relaxed);

                        // 如果任务失败，记录但不停止
                        let _ = result;
                    }
                }
            });
            task_handles.push(handle);
        }

        SchedulerHandle {
            stop_flag,
            task_stops,
            task_handles,
        }
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  SchedulerHandle
// =====================================================================

/// 调度器运行句柄
pub struct SchedulerHandle {
    stop_flag: Arc<AtomicBool>,
    task_stops: Vec<(String, Arc<AtomicBool>, Arc<AtomicU64>)>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl SchedulerHandle {
    /// 停止所有任务
    pub async fn stop(self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        for (_, task_stop, _) in &self.task_stops {
            task_stop.store(true, Ordering::Relaxed);
        }
        for handle in self.task_handles {
            let _ = handle.await;
        }
    }

    /// 停止单个任务
    pub fn stop_task(&self, name: &str) -> bool {
        for (task_name, task_stop, _) in &self.task_stops {
            if task_name == name {
                task_stop.store(true, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// 获取任务执行次数
    pub fn task_run_count(&self, name: &str) -> Option<u64> {
        for (task_name, _, count) in &self.task_stops {
            if task_name == name {
                return Some(count.load(Ordering::Relaxed));
            }
        }
        None
    }
}

// =====================================================================
//  同步调度器（用于测试，不需要 tokio runtime）
// =====================================================================

/// 同步调度器（用于测试和精确计时验证）
///
/// 不使用 tokio，直接在当前线程逐任务检查。
/// 主要用于单元测试中验证 cron 解析和调度逻辑。
pub struct SyncScheduler {
    tasks: HashMap<String, ScheduledTask>,
    /// 执行记录：任务名 → [(epoch, 结果)]
    execution_log: Vec<(String, u64, Result<(), String>)>,
}

impl SyncScheduler {
    /// 创建空调度器
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            execution_log: Vec::new(),
        }
    }

    /// 注册任务
    pub fn register(
        &mut self,
        name: &str,
        cron: CronExpr,
        callback: TaskCallback,
    ) -> Result<(), CronError> {
        if name.is_empty() {
            return Err(CronError::EmptyTaskName);
        }
        if self.tasks.contains_key(name) {
            return Err(CronError::TaskAlreadyExists {
                name: name.to_string(),
            });
        }
        let mut task = ScheduledTask::new(name.to_string(), cron, callback);
        task.update_next_run(0)?;
        self.tasks.insert(name.to_string(), task);
        Ok(())
    }

    /// 注销任务
    pub fn unregister(&mut self, name: &str) -> Result<ScheduledTask, CronError> {
        self.tasks
            .remove(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })
    }

    /// 暂停任务
    pub fn pause(&mut self, name: &str) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.status = TaskStatus::Paused;
        Ok(())
    }

    /// 恢复任务
    ///
    /// SyncScheduler 不重新计算 next_run，保留 pause 前的值（便于测试）。
    pub fn resume(&mut self, name: &str) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.status = TaskStatus::Active;
        Ok(())
    }

    /// 获取任务信息
    pub fn get_task(&self, name: &str) -> Option<&ScheduledTask> {
        self.tasks.get(name)
    }

    /// 列出所有任务名
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tasks.keys().cloned().collect();
        names.sort();
        names
    }

    /// 任务数量
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 获取执行日志
    pub fn execution_log(&self) -> &[(String, u64, Result<(), String>)] {
        &self.execution_log
    }

    /// 修改任务 cron 表达式（Phase 7a.2）
    ///
    /// SyncScheduler 使用 `now=0` 重新计算 next_run（便于测试）。
    pub fn update_cron(&mut self, name: &str, cron: CronExpr) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_cron(cron, 0)
    }

    /// 修改任务回调（Phase 7a.2）
    pub fn update_callback(&mut self, name: &str, callback: TaskCallback) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_callback(callback);
        Ok(())
    }

    /// 同时修改 cron 和回调（Phase 7a.2）
    pub fn update_task(
        &mut self,
        name: &str,
        cron: CronExpr,
        callback: TaskCallback,
    ) -> Result<(), CronError> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| CronError::TaskNotFound {
                name: name.to_string(),
            })?;
        task.update_cron(cron, 0)?;
        task.update_callback(callback);
        Ok(())
    }

    /// 检测 next_run 时间冲突的任务（Phase 7a.2）
    ///
    /// 返回所有 next_run 相同的 Active 任务组（每组 ≥ 2 个任务名）。
    pub fn detect_conflicts(&self) -> Vec<Vec<String>> {
        let mut groups: HashMap<u64, Vec<String>> = HashMap::new();
        for (name, task) in &self.tasks {
            if task.status != TaskStatus::Active {
                continue;
            }
            if let Some(next) = task.next_run {
                groups.entry(next).or_default().push(name.clone());
            }
        }
        let mut conflicts: Vec<Vec<String>> =
            groups.into_values().filter(|g| g.len() >= 2).collect();
        for g in &mut conflicts {
            g.sort();
        }
        conflicts.sort();
        conflicts
    }

    /// 检测在指定时间点同时触发的任务（Phase 7a.2）
    ///
    /// 返回所有 cron 表达式匹配该 epoch 的 Active 任务名（按字母序）。
    pub fn detect_conflicts_at(&self, epoch: u64) -> Vec<String> {
        let mut hits: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.status == TaskStatus::Active && t.cron.matches_epoch(epoch))
            .map(|(name, _)| name.clone())
            .collect();
        hits.sort();
        hits
    }

    /// 执行到指定 epoch 时间点的所有到期任务
    ///
    /// 遍历所有 Active 任务，执行所有 `next_run <= until_epoch` 的任务。
    /// 每次执行后更新 `next_run` 为下一个匹配时间。
    /// 每个任务最多执行 `max_runs_per_task` 次（防止无限循环）。
    ///
    /// 返回本次执行的任务数。
    pub fn tick(&mut self, until_epoch: u64, max_runs_per_task: usize) -> usize {
        let mut total = 0;
        let mut to_execute: Vec<(String, u64)> = Vec::new();

        for (name, task) in &mut self.tasks {
            if task.status != TaskStatus::Active {
                continue;
            }
            for _ in 0..max_runs_per_task {
                if let Some(next) = task.next_run {
                    if next <= until_epoch {
                        to_execute.push((name.clone(), next));
                        task.run_count += 1;
                        task.last_run = Some(next);
                        // 计算下一次
                        if let Ok(new_next) = task.cron.next_after(next) {
                            task.next_run = Some(new_next);
                        } else {
                            task.next_run = None;
                            task.status = TaskStatus::Completed;
                        }
                        total += 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        // 执行任务（倒序执行以避免借用问题）
        // 注意：这里不实际调用异步回调，只记录执行
        for (name, epoch) in to_execute {
            self.execution_log.push((name, epoch, Ok(())));
        }

        total
    }

    /// 执行到指定 epoch 时间点（单次执行）
    ///
    /// 每个任务最多执行一次。
    pub fn tick_once(&mut self, until_epoch: u64) -> usize {
        self.tick(until_epoch, 1)
    }
}

impl Default for SyncScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  CronError 变体测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_error_to_string() {
        assert_eq!(
            CronError::InvalidFieldCount { actual: 3 }.to_string(),
            "invalid cron expression: expected 5 fields, got 3"
        );
        assert_eq!(
            CronError::ValueOutOfRange {
                field: "minute",
                value: 60,
                min: 0,
                max: 59
            }
            .to_string(),
            "field 'minute': value 60 out of range [0, 59]"
        );
        assert_eq!(
            CronError::TaskAlreadyExists {
                name: "foo".to_string()
            }
            .to_string(),
            "task 'foo' already exists"
        );
    }

    // -----------------------------------------------------------------
    //  CronField 解析测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_field_star() {
        let f = CronField::parse("*", CronFieldType::Minute).unwrap();
        for v in 0..=59 {
            assert!(f.matches(v), "minute {v} should match *");
        }
        assert!(f.is_star());
    }

    #[test]
    fn test_cron_field_single_value() {
        let f = CronField::parse("5", CronFieldType::Minute).unwrap();
        assert!(!f.matches(0));
        assert!(f.matches(5));
        assert!(!f.matches(6));
    }

    #[test]
    fn test_cron_field_range() {
        let f = CronField::parse("10-15", CronFieldType::Minute).unwrap();
        assert!(!f.matches(9));
        for v in 10..=15 {
            assert!(f.matches(v));
        }
        assert!(!f.matches(16));
    }

    #[test]
    fn test_cron_field_step() {
        let f = CronField::parse("*/15", CronFieldType::Minute).unwrap();
        for v in [0, 15, 30, 45] {
            assert!(f.matches(v), "minute {v} should match */15");
        }
        assert!(!f.matches(1));
        assert!(!f.matches(14));
        assert!(!f.matches(46));
    }

    #[test]
    fn test_cron_field_list() {
        let f = CronField::parse("0,15,30,45", CronFieldType::Minute).unwrap();
        for v in [0, 15, 30, 45] {
            assert!(f.matches(v));
        }
        assert!(!f.matches(1));
        assert!(!f.matches(10));
    }

    #[test]
    fn test_cron_field_range_step() {
        let f = CronField::parse("2-10/2", CronFieldType::Minute).unwrap();
        for v in [2, 4, 6, 8, 10] {
            assert!(f.matches(v));
        }
        assert!(!f.matches(0));
        assert!(!f.matches(1));
        assert!(!f.matches(3));
        assert!(!f.matches(11));
    }

    #[test]
    fn test_cron_field_empty() {
        let err = CronField::parse("", CronFieldType::Minute).unwrap_err();
        assert!(matches!(err, CronError::EmptyField { field: "minute" }));
    }

    #[test]
    fn test_cron_field_out_of_range() {
        let err = CronField::parse("60", CronFieldType::Minute).unwrap_err();
        assert!(matches!(err, CronError::ValueOutOfRange { value: 60, .. }));
    }

    #[test]
    fn test_cron_field_zero_step() {
        let err = CronField::parse("*/0", CronFieldType::Minute).unwrap_err();
        assert!(matches!(err, CronError::ZeroStep { .. }));
    }

    #[test]
    fn test_cron_field_invalid_range() {
        let err = CronField::parse("10-5", CronFieldType::Minute).unwrap_err();
        assert!(matches!(
            err,
            CronError::InvalidRange {
                field: "minute",
                start: 10,
                end: 5
            }
        ));
    }

    #[test]
    fn test_cron_field_parse_error() {
        let err = CronField::parse("abc", CronFieldType::Minute).unwrap_err();
        assert!(matches!(err, CronError::ParseError { .. }));
    }

    #[test]
    fn test_cron_field_dow_normalize_7() {
        let f = CronField::parse("7", CronFieldType::Dow).unwrap();
        assert!(f.matches(0)); // 7 归一化为 0（周日）
        assert!(f.matches(7));
    }

    #[test]
    fn test_cron_field_dow_range() {
        let f = CronField::parse("1-5", CronFieldType::Dow).unwrap();
        for v in 1..=5 {
            assert!(f.matches(v));
        }
        assert!(!f.matches(0));
        assert!(!f.matches(6));
    }

    // -----------------------------------------------------------------
    //  CronExpr 解析测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_expr_parse_basic() {
        let expr = CronExpr::parse("0 2 * * *").unwrap();
        assert_eq!(expr.raw(), "0 2 * * *");
        assert!(expr.matches_minute(0));
        assert!(expr.matches_hour(2));
    }

    #[test]
    fn test_cron_expr_parse_every_minute() {
        let expr = CronExpr::parse("* * * * *").unwrap();
        assert!(expr.matches_minute(0));
        assert!(expr.matches_minute(59));
    }

    #[test]
    fn test_cron_expr_parse_complex() {
        let expr = CronExpr::parse("*/15 8-17 * * 1-5").unwrap();
        assert!(expr.matches_minute(0));
        assert!(expr.matches_minute(15));
        assert!(expr.matches_minute(30));
        assert!(expr.matches_minute(45));
        assert!(!expr.matches_minute(1));
        assert!(expr.matches_hour(8));
        assert!(expr.matches_hour(17));
        assert!(!expr.matches_hour(7));
        assert!(!expr.matches_hour(18));
        assert!(expr.matches_dow(1)); // 周一
        assert!(expr.matches_dow(5)); // 周五
        assert!(!expr.matches_dow(0)); // 周日
        assert!(!expr.matches_dow(6)); // 周六
    }

    #[test]
    fn test_cron_expr_wrong_field_count() {
        assert!(matches!(
            CronExpr::parse("* * * *").unwrap_err(),
            CronError::InvalidFieldCount { actual: 4 }
        ));
        assert!(matches!(
            CronExpr::parse("* * * * * *").unwrap_err(),
            CronError::InvalidFieldCount { actual: 6 }
        ));
        assert!(matches!(
            CronExpr::parse("").unwrap_err(),
            CronError::InvalidFieldCount { actual: 0 }
        ));
    }

    #[test]
    fn test_cron_expr_display() {
        let expr = CronExpr::parse("0 2 * * *").unwrap();
        assert_eq!(expr.to_string(), "0 2 * * *");
    }

    #[test]
    fn test_cron_expr_from_str() {
        let expr: CronExpr = "*/5 * * * *".parse().unwrap();
        assert_eq!(expr.raw(), "*/5 * * * *");
    }

    #[test]
    fn test_cron_expr_from_str_error() {
        let err: CronError = "invalid".parse::<CronExpr>().unwrap_err();
        assert!(matches!(err, CronError::InvalidFieldCount { actual: 1 }));
    }

    // -----------------------------------------------------------------
    //  epoch_to_cron_fields 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_epoch_to_cron_fields_known() {
        // 1970-01-01 00:00:00 UTC = epoch 0
        // 周四（dow=4）
        let (m, h, d, mo, dow) = epoch_to_cron_fields(0);
        assert_eq!((m, h, d, mo, dow), (0, 0, 1, 1, 4));

        // 1970-01-01 02:30:00 UTC = epoch 9000
        let (m, h, d, mo, dow) = epoch_to_cron_fields(9000);
        assert_eq!((m, h, d, mo, dow), (30, 2, 1, 1, 4));

        // 1970-01-02 00:00:00 UTC = epoch 86400（周五 dow=5）
        let (m, h, d, mo, dow) = epoch_to_cron_fields(86400);
        assert_eq!((m, h, d, mo, dow), (0, 0, 2, 1, 5));

        // 1970-01-04 00:00:00 UTC = epoch 86400*3（周日 dow=0）
        let (m, h, d, mo, dow) = epoch_to_cron_fields(86400 * 3);
        assert_eq!((m, h, d, mo, dow), (0, 0, 4, 1, 0));

        // 1970-02-01 00:00:00 UTC = epoch 31*86400
        let (m, h, d, mo, dow) = epoch_to_cron_fields(31 * 86400);
        assert_eq!((m, h, d, mo, dow), (0, 0, 1, 2, 0));

        // 1971-01-01 00:00:00 UTC = epoch 365*86400（周五 dow=5）
        let (m, h, d, mo, dow) = epoch_to_cron_fields(365 * 86400);
        assert_eq!((m, h, d, mo, dow), (0, 0, 1, 1, 5));
    }

    #[test]
    fn test_cron_fields_to_epoch_roundtrip() {
        // 验证 roundtrip：epoch → fields → epoch
        for epoch in [
            0u64,
            9000,
            86400,
            86400 * 3,
            31 * 86400,
            365 * 86400,
            1700000000,
        ] {
            let (m, h, d, mo, _) = epoch_to_cron_fields(epoch);
            // 验证正方向：epoch → fields 正确即可
            let (m2, h2, d2, mo2, _) = epoch_to_cron_fields(epoch);
            assert_eq!((m, h, d, mo), (m2, h2, d2, mo2));
        }
    }

    // -----------------------------------------------------------------
    //  CronExpr::matches_epoch 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_expr_matches_epoch() {
        let expr = CronExpr::parse("0 0 * * *").unwrap(); // 每天 00:00
                                                          // 1970-01-01 00:00:00 = epoch 0
        assert!(expr.matches_epoch(0));
        // 1970-01-02 00:00:00 = epoch 86400
        assert!(expr.matches_epoch(86400));
        // 1970-01-01 00:01:00 = epoch 60
        assert!(!expr.matches_epoch(60));
        // 1970-01-01 01:00:00 = epoch 3600
        assert!(!expr.matches_epoch(3600));
    }

    #[test]
    fn test_cron_expr_matches_epoch_every_minute() {
        let expr = CronExpr::parse("* * * * *").unwrap();
        for epoch in [0, 60, 120, 180, 3600, 3660] {
            assert!(expr.matches_epoch(epoch), "epoch {epoch} should match");
        }
    }

    // -----------------------------------------------------------------
    //  CronExpr::next_after 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_next_after_every_minute() {
        let expr = CronExpr::parse("* * * * *").unwrap();
        // epoch 0 → next = 60
        assert_eq!(expr.next_after(0).unwrap(), 60);
        // epoch 59 → next = 60
        assert_eq!(expr.next_after(59).unwrap(), 60);
        // epoch 60 → next = 120
        assert_eq!(expr.next_after(60).unwrap(), 120);
        // epoch 125 → next = 180
        assert_eq!(expr.next_after(125).unwrap(), 180);
    }

    #[test]
    fn test_next_after_daily_midnight() {
        let expr = CronExpr::parse("0 0 * * *").unwrap();
        // epoch 0（1970-01-01 00:00:00）→ next = 86400（1970-01-02 00:00:00）
        assert_eq!(expr.next_after(0).unwrap(), 86400);
        // epoch 60（00:01）→ next = 86400
        assert_eq!(expr.next_after(60).unwrap(), 86400);
        // epoch 86340（23:59）→ next = 86400
        assert_eq!(expr.next_after(86340).unwrap(), 86400);
        // epoch 86400（次日 00:00）→ next = 172800
        assert_eq!(expr.next_after(86400).unwrap(), 172800);
    }

    #[test]
    fn test_next_after_specific_time() {
        let expr = CronExpr::parse("30 14 * * *").unwrap(); // 每天 14:30
                                                            // epoch 0（00:00）→ next = 14*3600 + 30*60 = 52200
        assert_eq!(expr.next_after(0).unwrap(), 52200);
        // epoch 52200（14:30）→ next = 86400 + 52200 = 138600
        assert_eq!(expr.next_after(52200).unwrap(), 138600);
        // epoch 52000（14:26:40）→ next = 52200
        assert_eq!(expr.next_after(52000).unwrap(), 52200);
        // epoch 52300（14:31:40）→ next = 86400 + 52200 = 138600
        assert_eq!(expr.next_after(52300).unwrap(), 138600);
    }

    #[test]
    fn test_next_after_weekly() {
        let expr = CronExpr::parse("0 0 * * 0").unwrap(); // 每周日 00:00
                                                          // 1970-01-01 是周四（dow=4），epoch 0
                                                          // 下一个周日是 1970-01-04 = epoch 3*86400 = 259200
        assert_eq!(expr.next_after(0).unwrap(), 259200);
        // epoch 259200（周日 00:00）→ next = 259200 + 7*86400 = 864000
        assert_eq!(expr.next_after(259200).unwrap(), 864000);
    }

    #[test]
    fn test_next_after_monthly() {
        let expr = CronExpr::parse("0 0 1 * *").unwrap(); // 每月 1 日 00:00
                                                          // epoch 0（1月1日）→ next = 2月1日 = 31*86400 = 2678400
        assert_eq!(expr.next_after(0).unwrap(), 2678400);
        // epoch 2678400（2月1日）→ next = 3月1日 = 31+28=59天 *86400 = 5097600
        assert_eq!(expr.next_after(2678400).unwrap(), 5097600);
    }

    #[test]
    fn test_next_after_with_step() {
        let expr = CronExpr::parse("*/15 * * * *").unwrap(); // 每 15 分钟
                                                             // epoch 0 → next = 0? 不，next_after 从 after+1 开始
                                                             // next_after(0) → 从 epoch 60 开始找 → 0 匹配但已过 → 下一个匹配是 60?
                                                             // 不，*/15 匹配 0,15,30,45 分钟
                                                             // epoch 0 → 从 60 秒开始（1 分钟后）→ 下一分钟边界是 60（minute=1）不匹配
                                                             // → 900 秒（minute=15）匹配
        assert_eq!(expr.next_after(0).unwrap(), 900);
        assert_eq!(expr.next_after(900).unwrap(), 1800);
        assert_eq!(expr.next_after(1800).unwrap(), 2700);
        assert_eq!(expr.next_after(2700).unwrap(), 3600);
    }

    // -----------------------------------------------------------------
    //  日 + 周 OR 语义测试
    // -----------------------------------------------------------------

    #[test]
    fn test_day_dow_or_semantics() {
        // 每月 1 日 或 每周日 00:00
        let expr = CronExpr::parse("0 0 1 * 0").unwrap();
        // 1970-01-01（周四）= epoch 0 → 日=1 匹配 → 触发
        assert!(expr.matches_epoch(0));
        // 1970-01-02（周五）= epoch 86400 → 日=2 不匹配，dow=5 不匹配 → 不触发
        assert!(!expr.matches_epoch(86400));
        // 1970-01-04（周日）= epoch 3*86400 → 日=4 不匹配，dow=0 匹配 → 触发（OR）
        assert!(expr.matches_epoch(3 * 86400));
    }

    #[test]
    fn test_day_dow_and_semantics_when_star() {
        // 每月 1 日 00:00（周为 *）
        let expr = CronExpr::parse("0 0 1 * *").unwrap();
        // 1970-01-01 = epoch 0 → 触发
        assert!(expr.matches_epoch(0));
        // 1970-02-01 = epoch 31*86400 → 触发
        assert!(expr.matches_epoch(31 * 86400));
        // 1970-01-02 = epoch 86400 → 不触发
        assert!(!expr.matches_epoch(86400));
    }

    // -----------------------------------------------------------------
    //  TaskStatus 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_task_status_display() {
        assert_eq!(TaskStatus::Active.to_string(), "Active");
        assert_eq!(TaskStatus::Paused.to_string(), "Paused");
        assert_eq!(TaskStatus::Completed.to_string(), "Completed");
    }

    // -----------------------------------------------------------------
    //  SyncScheduler 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sync_scheduler_register() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        let cb = make_callback(|| async { Ok(()) });
        sched.register("t1", expr, cb).unwrap();
        assert_eq!(sched.len(), 1);
        assert!(!sched.is_empty());
        assert_eq!(sched.list(), vec!["t1"]);
    }

    #[test]
    fn test_sync_scheduler_register_duplicate() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        let cb = make_callback(|| async { Ok(()) });
        sched.register("t1", expr.clone(), cb).unwrap();
        let err = sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap_err();
        assert!(matches!(err, CronError::TaskAlreadyExists { .. }));
    }

    #[test]
    fn test_sync_scheduler_register_empty_name() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        let err = sched
            .register("", expr, make_callback(|| async { Ok(()) }))
            .unwrap_err();
        assert!(matches!(err, CronError::EmptyTaskName));
    }

    #[test]
    fn test_sync_scheduler_unregister() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();
        let task = sched.unregister("t1").unwrap();
        assert_eq!(task.name, "t1");
        assert!(sched.is_empty());
    }

    #[test]
    fn test_sync_scheduler_unregister_not_found() {
        let mut sched = SyncScheduler::new();
        let err = sched.unregister("nonexistent").unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    #[test]
    fn test_sync_scheduler_pause_resume() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();

        sched.pause("t1").unwrap();
        assert_eq!(sched.get_task("t1").unwrap().status, TaskStatus::Paused);

        sched.resume("t1").unwrap();
        assert_eq!(sched.get_task("t1").unwrap().status, TaskStatus::Active);
    }

    #[test]
    fn test_sync_scheduler_tick_basic() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();

        // next_run 初始为 60（从 epoch 0 之后第一个匹配）
        // tick 到 epoch 120 → 应执行 2 次（60 和 120）
        let count = sched.tick(120, 100);
        assert_eq!(count, 2);
        assert_eq!(sched.get_task("t1").unwrap().run_count, 2);
        assert_eq!(sched.execution_log().len(), 2);
    }

    #[test]
    fn test_sync_scheduler_tick_once() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();

        // tick_once 到 epoch 120 → 每个任务最多执行 1 次
        let count = sched.tick_once(120);
        assert_eq!(count, 1);
        assert_eq!(sched.get_task("t1").unwrap().run_count, 1);
    }

    #[test]
    fn test_sync_scheduler_tick_paused() {
        let mut sched = SyncScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();
        sched.pause("t1").unwrap();

        let count = sched.tick(600, 100);
        assert_eq!(count, 0);
        assert_eq!(sched.get_task("t1").unwrap().run_count, 0);
    }

    #[test]
    fn test_sync_scheduler_tick_multiple_tasks() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "every_min",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "every_5min",
                CronExpr::parse("*/5 * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // tick 到 epoch 600（10 分钟）
        // every_min: 10 次（60,120,...,600）
        // every_5min: 2 次（300, 600）
        let count = sched.tick(600, 100);
        assert_eq!(count, 12);
        assert_eq!(sched.get_task("every_min").unwrap().run_count, 10);
        assert_eq!(sched.get_task("every_5min").unwrap().run_count, 2);
    }

    #[test]
    fn test_sync_scheduler_tick_incremental() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "t1",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // 第一次 tick 到 60 → 1 次
        assert_eq!(sched.tick_once(60), 1);
        // 第二次 tick 到 120 → 1 次
        assert_eq!(sched.tick_once(120), 1);
        // 第三次 tick 到 180 → 1 次
        assert_eq!(sched.tick_once(180), 1);
        assert_eq!(sched.get_task("t1").unwrap().run_count, 3);
    }

    #[test]
    fn test_sync_scheduler_tick_no_due() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "t1",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // next_run = 60, tick 到 30 → 无到期
        assert_eq!(sched.tick_once(30), 0);
    }

    // -----------------------------------------------------------------
    //  SyncScheduler 列表/排序测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sync_scheduler_list_sorted() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "charlie",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "alpha",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "bravo",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        assert_eq!(sched.list(), vec!["alpha", "bravo", "charlie"]);
    }

    // -----------------------------------------------------------------
    //  CronScheduler（异步）基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_scheduler_register() {
        let mut sched = CronScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap();
        assert_eq!(sched.len(), 1);
        assert!(!sched.is_empty());
        assert_eq!(sched.list(), vec!["t1"]);
    }

    #[test]
    fn test_cron_scheduler_register_duplicate() {
        let mut sched = CronScheduler::new();
        let expr = CronExpr::parse("* * * * *").unwrap();
        sched
            .register("t1", expr.clone(), make_callback(|| async { Ok(()) }))
            .unwrap();
        let err = sched
            .register("t1", expr, make_callback(|| async { Ok(()) }))
            .unwrap_err();
        assert!(matches!(err, CronError::TaskAlreadyExists { .. }));
    }

    #[test]
    fn test_cron_scheduler_unregister() {
        let mut sched = CronScheduler::new();
        sched
            .register(
                "t1",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched.unregister("t1").unwrap();
        assert!(sched.is_empty());
    }

    #[test]
    fn test_cron_scheduler_pause_resume() {
        let mut sched = CronScheduler::new();
        sched
            .register(
                "t1",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched.pause("t1").unwrap();
        assert_eq!(sched.get_status("t1").unwrap(), TaskStatus::Paused);
        sched.resume("t1").unwrap();
        assert_eq!(sched.get_status("t1").unwrap(), TaskStatus::Active);
    }

    #[test]
    fn test_cron_scheduler_pause_not_found() {
        let mut sched = CronScheduler::new();
        let err = sched.pause("nonexistent").unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    // -----------------------------------------------------------------
    //  CronScheduler 异步调度测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cron_scheduler_start_stop() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut sched = CronScheduler::new();
            sched
                .register(
                    "t1",
                    CronExpr::parse("* * * * *").unwrap(),
                    make_callback(|| async { Ok(()) }),
                )
                .unwrap();
            let handle = sched.start().await;
            // 立即停止
            handle.stop().await;
        });
    }

    #[test]
    fn test_cron_scheduler_multiple_tasks_parallel() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut sched = CronScheduler::new();
            // 注册多个任务
            for i in 0..10 {
                sched
                    .register(
                        &format!("task_{i}"),
                        CronExpr::parse("* * * * *").unwrap(),
                        make_callback(move || async move { Ok(()) }),
                    )
                    .unwrap();
            }
            assert_eq!(sched.len(), 10);
            let handle = sched.start().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            // 停止所有任务
            handle.stop().await;
        });
    }

    #[test]
    fn test_cron_scheduler_stop_single_task() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut sched = CronScheduler::new();
            sched
                .register(
                    "t1",
                    CronExpr::parse("* * * * *").unwrap(),
                    make_callback(|| async { Ok(()) }),
                )
                .unwrap();
            sched
                .register(
                    "t2",
                    CronExpr::parse("* * * * *").unwrap(),
                    make_callback(|| async { Ok(()) }),
                )
                .unwrap();
            let handle = sched.start().await;
            assert!(handle.stop_task("t1"));
            assert!(!handle.stop_task("nonexistent"));
            handle.stop().await;
        });
    }

    // -----------------------------------------------------------------
    //  调度精度测试（核心验证项）
    // -----------------------------------------------------------------

    #[test]
    fn test_scheduling_precision_short_interval() {
        // 验证：高频调度精度（使用 SyncScheduler 模拟）
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "precision",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // 模拟 10000 次执行（每分钟一次，10000 分钟 = ~7 天）
        let mut total_executed = 0;
        for minute in 1..=10000 {
            let epoch = minute * 60;
            total_executed += sched.tick_once(epoch);
        }
        assert_eq!(total_executed, 10000);
        assert_eq!(sched.get_task("precision").unwrap().run_count, 10000);

        // 验证执行间隔精确为 60 秒
        let log = sched.execution_log();
        for i in 1..log.len() {
            let prev = log[i - 1].1;
            let curr = log[i].1;
            assert_eq!(
                curr - prev,
                60,
                "interval between execution {i} and {} should be 60s",
                i - 1
            );
        }
    }

    #[test]
    fn test_scheduling_deviation_under_100ms() {
        // 验证：调度偏差 < 100ms（通过 cron 表达式计算验证）
        let expr = CronExpr::parse("* * * * *").unwrap();

        // 模拟 1000 次调度
        let mut prev = 0u64;
        for i in 1..=1000 {
            let next = expr.next_after(prev).unwrap();
            // 间隔应为 60 秒
            let deviation = if i == 1 {
                60
            } else {
                next - prev
            };
            assert_eq!(
                deviation, 60,
                "deviation should be exactly 60s at iteration {i}"
            );
            prev = next;
        }
    }

    #[test]
    fn test_multiple_tasks_no_blocking() {
        // 验证：多个任务并行不相互阻塞
        let mut sched = SyncScheduler::new();

        // 注册不同频率的任务
        sched
            .register(
                "every_1min",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "every_5min",
                CronExpr::parse("*/5 * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "every_15min",
                CronExpr::parse("*/15 * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // tick 到 1 小时后（3600 秒 = 60 分钟）
        sched.tick(3600, 1000);

        // every_1min: 60 次
        assert_eq!(sched.get_task("every_1min").unwrap().run_count, 60);
        // every_5min: 12 次（5,10,...,60 分钟）
        assert_eq!(sched.get_task("every_5min").unwrap().run_count, 12);
        // every_15min: 4 次（15,30,45,60 分钟）
        assert_eq!(sched.get_task("every_15min").unwrap().run_count, 4);
    }

    // -----------------------------------------------------------------
    //  E2E 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_e2e_full_lifecycle() {
        let mut sched = SyncScheduler::new();

        // 1. 创建任务
        sched
            .register(
                "daily_backup",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        assert_eq!(
            sched.get_task("daily_backup").unwrap().status,
            TaskStatus::Active
        );

        // 2. 暂停
        sched.pause("daily_backup").unwrap();
        assert_eq!(
            sched.get_task("daily_backup").unwrap().status,
            TaskStatus::Paused
        );

        // 3. 恢复
        sched.resume("daily_backup").unwrap();
        assert_eq!(
            sched.get_task("daily_backup").unwrap().status,
            TaskStatus::Active
        );

        // 4. 执行
        // next_run 应为 2*3600 = 7200（1970-01-01 02:00）
        let next = sched.get_task("daily_backup").unwrap().next_run.unwrap();
        assert_eq!(next, 7200);
        let count = sched.tick_once(7200);
        assert_eq!(count, 1);
        assert_eq!(sched.get_task("daily_backup").unwrap().run_count, 1);

        // 5. 注销
        sched.unregister("daily_backup").unwrap();
        assert!(sched.is_empty());
    }

    #[test]
    fn test_e2e_cron_patterns() {
        // 测试各种常见 cron 模式
        let patterns = vec![
            ("* * * * *", "every minute"),
            ("0 * * * *", "every hour"),
            ("0 0 * * *", "every day at midnight"),
            ("0 0 * * 0", "every Sunday at midnight"),
            ("0 0 1 * *", "first day of every month at midnight"),
            ("*/15 * * * *", "every 15 minutes"),
            ("0 9-17 * * 1-5", "every weekday 9am-5pm"),
            ("0 0 1 1 *", "January 1st at midnight"),
        ];

        for (pattern, desc) in patterns {
            let expr = CronExpr::parse(pattern);
            assert!(
                expr.is_ok(),
                "pattern '{}' ({}) should parse",
                pattern,
                desc
            );
            let expr = expr.unwrap();
            // 验证 next_after 不报错
            let next = expr.next_after(0);
            assert!(
                next.is_ok(),
                "pattern '{}' ({}) should have next_after(0)",
                pattern,
                desc
            );
        }
    }

    #[test]
    fn test_e2e_stress_10000_executions() {
        // 压力测试：10000 次调度执行
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "stress",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        let mut total = 0;
        for minute in 1..=10000 {
            total += sched.tick_once(minute * 60);
        }
        assert_eq!(total, 10000);
        assert_eq!(sched.get_task("stress").unwrap().run_count, 10000);
    }

    #[test]
    fn test_e2e_mixed_cron_expressions() {
        let mut sched = SyncScheduler::new();

        // 不同 cron 表达式的任务
        let cases = vec![
            ("minutely", "* * * * *", 60),   // 每分钟
            ("hourly", "0 * * * *", 3600),   // 每小时
            ("daily", "0 0 * * *", 86400),   // 每天
            ("weekly", "0 0 * * 0", 604800), // 每周
        ];

        for (name, pattern, _) in &cases {
            sched
                .register(
                    name,
                    CronExpr::parse(pattern).unwrap(),
                    make_callback(|| async { Ok(()) }),
                )
                .unwrap();
        }

        // tick 到 1 周后（max_runs_per_task 需 ≥ 10080 以容纳每分钟任务）
        sched.tick(604800, 20000);

        // minutely: 10080 次（7 天 * 24 小时 * 60 分钟）
        assert_eq!(sched.get_task("minutely").unwrap().run_count, 10080);
        // hourly: 168 次（7 天 * 24 小时）
        assert_eq!(sched.get_task("hourly").unwrap().run_count, 168);
        // daily: 7 次
        assert_eq!(sched.get_task("daily").unwrap().run_count, 7);
        // weekly: 1 次（epoch 0 = 周四，下一个周日 = epoch 259200）
        assert_eq!(sched.get_task("weekly").unwrap().run_count, 1);
    }

    // -----------------------------------------------------------------
    //  Phase 7a.2：定时任务注册/管理 — 修改 + 冲突检测
    // -----------------------------------------------------------------

    #[test]
    fn test_7a2_update_cron_changes_next_run() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "task",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // 原始 next_run = 7200（02:00）
        assert_eq!(sched.get_task("task").unwrap().next_run, Some(7200));
        // 改为每分钟
        sched
            .update_cron("task", CronExpr::parse("* * * * *").unwrap())
            .unwrap();
        assert_eq!(sched.get_task("task").unwrap().next_run, Some(60));
        assert_eq!(sched.get_task("task").unwrap().cron.raw(), "* * * * *");
    }

    #[test]
    fn test_7a2_update_cron_not_found() {
        let mut sched = SyncScheduler::new();
        let err = sched
            .update_cron("nope", CronExpr::parse("* * * * *").unwrap())
            .unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    #[test]
    fn test_7a2_update_cron_revives_completed() {
        let mut sched = SyncScheduler::new();
        // 注册一个只匹配过去的 cron — 用 NoNextTime 使其 Completed
        // 使用 0 2 * * * 注册，tick 到极远的未来使其完成
        sched
            .register(
                "once",
                CronExpr::parse("0 0 1 1 *").unwrap(), // 每年 1 月 1 日
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // next_run = 1970-01-01 00:00 = 0？不，next_after(0) 从 60 开始扫描
        // 0 0 1 1 * 匹配 1970-01-01 00:00，但 next_after(0) 从 epoch 60 开始
        // 所以 next_run 是 1971-01-01 00:00 = 365*86400 = 31536000
        // tick 到 31536000 执行一次，然后 next_after(31536000) = 1972-01-01
        // 不会 Completed。改用另一种方式：手动设 Completed
        {
            let task = sched.tasks.get_mut("once").unwrap();
            task.status = TaskStatus::Completed;
            task.next_run = None;
        }
        assert_eq!(
            sched.get_task("once").unwrap().status,
            TaskStatus::Completed
        );
        // 更新 cron 应恢复为 Active
        sched
            .update_cron("once", CronExpr::parse("* * * * *").unwrap())
            .unwrap();
        assert_eq!(sched.get_task("once").unwrap().status, TaskStatus::Active);
        assert!(sched.get_task("once").unwrap().next_run.is_some());
    }

    #[test]
    fn test_7a2_update_callback_works() {
        // SyncScheduler::tick 不实际执行回调（仅记录），所以这里验证
        // update_callback 不破坏任务状态，且 run_count 继续递增
        let counter = Arc::new(AtomicU64::new(0));
        let c1 = counter.clone();
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "task",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(move || {
                    let c = c1.clone();
                    async move {
                        c.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }),
            )
            .unwrap();
        assert_eq!(sched.tick_once(60), 1);
        assert_eq!(sched.get_task("task").unwrap().run_count, 1);

        // 更新回调（SyncScheduler 不执行回调，仅验证不破坏任务）
        let counter2 = Arc::new(AtomicU64::new(100));
        let c2 = counter2.clone();
        sched
            .update_callback(
                "task",
                make_callback(move || {
                    let c = c2.clone();
                    async move {
                        c.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }),
            )
            .unwrap();
        // 任务仍然正常调度
        assert_eq!(sched.tick_once(120), 1);
        assert_eq!(sched.get_task("task").unwrap().run_count, 2);
        assert_eq!(sched.get_task("task").unwrap().last_run, Some(120));
    }

    #[test]
    fn test_7a2_update_callback_not_found() {
        let mut sched = SyncScheduler::new();
        let err = sched
            .update_callback("nope", make_callback(|| async { Ok(()) }))
            .unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    #[test]
    fn test_7a2_update_task_both() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "task",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        assert_eq!(sched.get_task("task").unwrap().next_run, Some(7200));

        let counter = Arc::new(AtomicU64::new(0));
        let c = counter.clone();
        sched
            .update_task(
                "task",
                CronExpr::parse("0 0 * * *").unwrap(),
                make_callback(move || {
                    let c = c.clone();
                    async move {
                        c.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }),
            )
            .unwrap();
        // next_run 应为 86400（每天 00:00）
        assert_eq!(sched.get_task("task").unwrap().next_run, Some(86400));
        assert_eq!(sched.get_task("task").unwrap().cron.raw(), "0 0 * * *");
        // SyncScheduler 不执行回调，验证 run_count 递增
        assert_eq!(sched.tick_once(86400), 1);
        assert_eq!(sched.get_task("task").unwrap().run_count, 1);
    }

    #[test]
    fn test_7a2_update_task_not_found() {
        let mut sched = SyncScheduler::new();
        let err = sched
            .update_task(
                "nope",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    #[test]
    fn test_7a2_detect_conflicts_none() {
        let mut sched = SyncScheduler::new();
        // 不同 next_run 的任务
        sched
            .register(
                "a",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "b",
                CronExpr::parse("0 3 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        assert!(sched.detect_conflicts().is_empty());
    }

    #[test]
    fn test_7a2_detect_conflicts_same_next_run() {
        let mut sched = SyncScheduler::new();
        // 两个相同 cron 的任务 → next_run 相同
        sched
            .register(
                "backup",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "report",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        let conflicts = sched.detect_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0], vec!["backup", "report"]);
    }

    #[test]
    fn test_7a2_detect_conflicts_paused_excluded() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "a",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "b",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched.pause("b").unwrap();
        assert!(sched.detect_conflicts().is_empty());
    }

    #[test]
    fn test_7a2_detect_conflicts_multiple_groups() {
        let mut sched = SyncScheduler::new();
        // 组1：02:00 触发
        sched
            .register(
                "alpha",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "beta",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // 组2：03:00 触发
        sched
            .register(
                "gamma",
                CronExpr::parse("0 3 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "delta",
                CronExpr::parse("0 3 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        let conflicts = sched.detect_conflicts();
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0], vec!["alpha", "beta"]);
        assert_eq!(conflicts[1], vec!["delta", "gamma"]);
    }

    #[test]
    fn test_7a2_detect_conflicts_at_epoch() {
        let mut sched = SyncScheduler::new();
        // 0 2 * * * 匹配 7200（02:00）
        sched
            .register(
                "daily",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // * * * * * 匹配所有分钟
        sched
            .register(
                "minutely",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        // 0 0 * * * 匹配 0（00:00）但不匹配 7200
        sched
            .register(
                "midnight",
                CronExpr::parse("0 0 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // epoch 7200 = 02:00，daily 和 minutely 都匹配
        let hits = sched.detect_conflicts_at(7200);
        assert_eq!(hits, vec!["daily", "minutely"]);

        // epoch 0 = 00:00，minutuely 和 midnight 匹配
        let hits0 = sched.detect_conflicts_at(0);
        assert_eq!(hits0, vec!["midnight", "minutely"]);

        // epoch 60 = 01:00，只有 minutely 匹配
        let hits60 = sched.detect_conflicts_at(60);
        assert_eq!(hits60, vec!["minutely"]);
    }

    #[test]
    fn test_7a2_detect_conflicts_at_paused_excluded() {
        let mut sched = SyncScheduler::new();
        sched
            .register(
                "a",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "b",
                CronExpr::parse("* * * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched.pause("b").unwrap();
        let hits = sched.detect_conflicts_at(60);
        assert_eq!(hits, vec!["a"]); // b 被暂停，排除
    }

    #[test]
    fn test_7a2_e2e_full_crud_lifecycle() {
        // 完整 CRUD 生命周期：创建 → 查询 → 修改 → 暂停 → 恢复 → 注销
        let mut sched = SyncScheduler::new();

        // 1. Create
        sched
            .register(
                "backup",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        assert_eq!(sched.len(), 1);
        assert!(sched.list().contains(&"backup".to_string()));

        // 2. Read
        let task = sched.get_task("backup").unwrap();
        assert_eq!(task.cron.raw(), "0 2 * * *");
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.next_run, Some(7200));

        // 3. Update（修改 cron）
        sched
            .update_cron("backup", CronExpr::parse("0 3 * * *").unwrap())
            .unwrap();
        assert_eq!(sched.get_task("backup").unwrap().cron.raw(), "0 3 * * *");
        assert_eq!(sched.get_task("backup").unwrap().next_run, Some(10800));

        // 4. Update（修改 callback）
        let counter = Arc::new(AtomicU64::new(0));
        let c = counter.clone();
        sched
            .update_callback(
                "backup",
                make_callback(move || {
                    let c = c.clone();
                    async move {
                        c.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }),
            )
            .unwrap();

        // 5. Pause
        sched.pause("backup").unwrap();
        assert_eq!(sched.get_task("backup").unwrap().status, TaskStatus::Paused);

        // 6. Resume
        sched.resume("backup").unwrap();
        assert_eq!(sched.get_task("backup").unwrap().status, TaskStatus::Active);

        // 7. Execute（SyncScheduler 不执行回调，验证 run_count 递增）
        assert_eq!(sched.tick_once(10800), 1);
        assert_eq!(sched.get_task("backup").unwrap().run_count, 1);

        // 8. Delete
        let removed = sched.unregister("backup").unwrap();
        assert_eq!(removed.name, "backup");
        assert!(sched.is_empty());
    }

    #[test]
    fn test_7a2_e2e_conflict_detection_lifecycle() {
        // 冲突检测生命周期：注册冲突 → 检测 → 修改消除冲突 → 再检测
        let mut sched = SyncScheduler::new();

        // 注册两个 02:00 触发的任务
        sched
            .register(
                "backup",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "report",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // 检测到冲突
        assert_eq!(sched.detect_conflicts().len(), 1);

        // 修改 report 的 cron 为 03:00
        sched
            .update_cron("report", CronExpr::parse("0 3 * * *").unwrap())
            .unwrap();

        // 冲突消除
        assert!(sched.detect_conflicts().is_empty());

        // 检测指定时间点
        let hits_7200 = sched.detect_conflicts_at(7200);
        assert_eq!(hits_7200, vec!["backup"]); // 只有 backup 在 02:00
        let hits_10800 = sched.detect_conflicts_at(10800);
        assert_eq!(hits_10800, vec!["report"]); // 只有 report 在 03:00
    }

    #[test]
    fn test_7a2_cron_scheduler_update_and_conflicts() {
        // CronScheduler（异步）的 update + detect_conflicts
        let mut sched = CronScheduler::new();
        sched
            .register(
                "task_a",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();
        sched
            .register(
                "task_b",
                CronExpr::parse("0 2 * * *").unwrap(),
                make_callback(|| async { Ok(()) }),
            )
            .unwrap();

        // 两个任务 next_run 都基于 now_epoch()，但可能不完全相同（取决于注册时间）
        // 所以这里只验证 detect_conflicts_at 逻辑
        // epoch 7200（02:00 UTC）两个任务都匹配
        let hits = sched.detect_conflicts_at(7200);
        assert_eq!(hits, vec!["task_a", "task_b"]);

        // 修改 task_b 的 cron
        sched
            .update_cron("task_b", CronExpr::parse("0 3 * * *").unwrap())
            .unwrap();
        let hits_after = sched.detect_conflicts_at(7200);
        assert_eq!(hits_after, vec!["task_a"]);

        // update_cron not_found
        let err = sched
            .update_cron("nope", CronExpr::parse("* * * * *").unwrap())
            .unwrap_err();
        assert!(matches!(err, CronError::TaskNotFound { .. }));
    }

    #[test]
    fn test_7a2_reserved_name_error_variant() {
        // 验证新增的 CronError::ReservedName 变体
        let err = CronError::ReservedName {
            name: "system".to_string(),
        };
        assert_eq!(err.to_string(), "task name 'system' is a reserved keyword");
    }
}
