//! 自治运维 — 异常检测 + 容量预测 — Phase 7b.8
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! 自动检测数据库异常查询（全表扫描/死锁/超时）并报警，
//! 基于历史容量数据预测未来增长趋势。
//!
//! ## 异常检测
//!
//! 1. **全表扫描** — 扫描行数 > 阈值且无索引使用
//! 2. **死锁** — 检测到死锁错误
//! 3. **超时** — 查询耗时 > 超时阈值
//! 4. **高频错误** — 错误率 > 阈值
//!
//! ## 容量预测
//!
//! 1. **线性回归** — 基于历史数据点拟合线性趋势
//! 2. **预测误差** — 留一法交叉验证预测误差
//!
//! # 验证标准
//!
//! - 模拟异常查询 → 异常检测报警，异常召回率 >= 90%
//! - 模拟容量增长趋势 → 容量预测误差 < 20%
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.8。

use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// 自治运维错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutoOpsError {
    /// 阈值无效
    #[error("invalid threshold: {0}")]
    InvalidThreshold(String),
    /// 数据点不足
    #[error("insufficient data points: need {needed}, got {got}")]
    InsufficientData { needed: usize, got: usize },
    /// 异常类型未知
    #[error("unknown anomaly type: {0}")]
    UnknownAnomalyType(String),
}

// =====================================================================
//  异常检测
// =====================================================================

/// 异常类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnomalyType {
    /// 全表扫描
    FullTableScan,
    /// 死锁
    Deadlock,
    /// 超时
    Timeout,
    /// 高频错误
    HighErrorRate,
}

impl AnomalyType {
    /// 异常类型名称
    pub fn name(&self) -> &'static str {
        match self {
            Self::FullTableScan => "full_table_scan",
            Self::Deadlock => "deadlock",
            Self::Timeout => "timeout",
            Self::HighErrorRate => "high_error_rate",
        }
    }
}

/// 异常严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// 低
    Low = 1,
    /// 中
    Medium = 2,
    /// 高
    High = 3,
    /// 严重
    Critical = 4,
}

impl Severity {
    /// 从数值创建严重级别
    pub fn from_level(level: u8) -> Self {
        match level {
            0 | 1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            _ => Self::Critical,
        }
    }
}

/// 异常报警
#[derive(Debug, Clone, PartialEq)]
pub struct AnomalyAlert {
    /// 异常类型
    pub anomaly_type: AnomalyType,
    /// 严重级别
    pub severity: Severity,
    /// 异常描述
    pub message: String,
    /// 涉及的 SQL
    pub sql: String,
    /// 涉及的表
    pub table: Option<String>,
    /// 检测时间戳
    pub timestamp: u64,
}

/// 异常检测配置
#[derive(Debug, Clone)]
pub struct AnomalyDetectorConfig {
    /// 全表扫描阈值（扫描行数）
    pub full_scan_threshold: u64,
    /// 超时阈值（毫秒）
    pub timeout_threshold_ms: u64,
    /// 高频错误率阈值（0.0 ~ 1.0）
    pub error_rate_threshold: f64,
    /// 错误率统计窗口大小
    pub error_rate_window: usize,
}

impl Default for AnomalyDetectorConfig {
    fn default() -> Self {
        Self {
            full_scan_threshold: 10000,
            timeout_threshold_ms: 5000,
            error_rate_threshold: 0.1,
            error_rate_window: 100,
        }
    }
}

/// 异常检测器
#[derive(Debug)]
pub struct AnomalyDetector {
    /// 配置
    config: AnomalyDetectorConfig,
    /// 报警历史
    alerts: Vec<AnomalyAlert>,
    /// 错误率统计窗口（true=错误，false=成功）
    error_window: Vec<bool>,
    /// 已检测的异常总数（按类型）
    anomaly_counts: HashMap<AnomalyType, usize>,
}

impl Default for AnomalyDetector {
    fn default() -> Self {
        Self::new(AnomalyDetectorConfig::default())
    }
}

impl AnomalyDetector {
    /// 创建异常检测器
    pub fn new(config: AnomalyDetectorConfig) -> Self {
        Self {
            config,
            alerts: Vec::new(),
            error_window: Vec::new(),
            anomaly_counts: HashMap::new(),
        }
    }

    /// 配置引用
    pub fn config(&self) -> &AnomalyDetectorConfig {
        &self.config
    }

    /// 报警历史
    pub fn alerts(&self) -> &[AnomalyAlert] {
        &self.alerts
    }

    /// 已检测的异常总数
    pub fn total_anomalies(&self) -> usize {
        self.alerts.len()
    }

    /// 按类型统计异常数
    pub fn anomaly_count(&self, anomaly_type: &AnomalyType) -> usize {
        *self.anomaly_counts.get(anomaly_type).unwrap_or(&0)
    }

    /// 清空报警历史
    pub fn clear(&mut self) {
        self.alerts.clear();
        self.error_window.clear();
        self.anomaly_counts.clear();
    }

    // -----------------------------------------------------------------
    //  异常检测方法
    // -----------------------------------------------------------------

    /// 检测单条查询执行
    ///
    /// - `sql` — SQL 文本
    /// - `elapsed_ms` — 执行耗时
    /// - `scanned_rows` — 扫描行数
    /// - `used_index` — 是否使用索引
    /// - `table` — 涉及的表
    /// - `is_error` — 是否执行错误
    /// - `error_kind` — 错误类型（如 "deadlock" / "timeout"）
    /// - `timestamp` — 时间戳
    ///
    /// 返回触发的报警列表（可能多条）
    #[allow(clippy::too_many_arguments)]
    pub fn check_query(
        &mut self,
        sql: &str,
        elapsed_ms: u64,
        scanned_rows: u64,
        used_index: bool,
        table: Option<&str>,
        is_error: bool,
        error_kind: Option<&str>,
        timestamp: u64,
    ) -> Vec<AnomalyAlert> {
        let mut triggered = Vec::new();

        // 1. 全表扫描检测
        if !used_index && scanned_rows >= self.config.full_scan_threshold {
            let severity = if scanned_rows >= self.config.full_scan_threshold * 10 {
                Severity::Critical
            } else if scanned_rows >= self.config.full_scan_threshold * 5 {
                Severity::High
            } else {
                Severity::Medium
            };
            let alert = AnomalyAlert {
                anomaly_type: AnomalyType::FullTableScan,
                severity,
                message: format!(
                    "全表扫描：扫描 {scanned_rows} 行（阈值 {}）",
                    self.config.full_scan_threshold
                ),
                sql: sql.to_string(),
                table: table.map(|s| s.to_string()),
                timestamp,
            };
            triggered.push(alert);
        }

        // 2. 超时检测
        if elapsed_ms >= self.config.timeout_threshold_ms {
            let severity = if elapsed_ms >= self.config.timeout_threshold_ms * 4 {
                Severity::Critical
            } else if elapsed_ms >= self.config.timeout_threshold_ms * 2 {
                Severity::High
            } else {
                Severity::Medium
            };
            let alert = AnomalyAlert {
                anomaly_type: AnomalyType::Timeout,
                severity,
                message: format!(
                    "查询超时：耗时 {elapsed_ms}ms（阈值 {}ms）",
                    self.config.timeout_threshold_ms
                ),
                sql: sql.to_string(),
                table: table.map(|s| s.to_string()),
                timestamp,
            };
            triggered.push(alert);
        }

        // 3. 死锁检测
        if is_error {
            if let Some(kind) = error_kind {
                if kind.to_lowercase().contains("deadlock") {
                    let alert = AnomalyAlert {
                        anomaly_type: AnomalyType::Deadlock,
                        severity: Severity::High,
                        message: format!("死锁检测：{kind}"),
                        sql: sql.to_string(),
                        table: table.map(|s| s.to_string()),
                        timestamp,
                    };
                    triggered.push(alert);
                }
            }
        }

        // 4. 高频错误率检测
        self.error_window.push(is_error);
        if self.error_window.len() > self.config.error_rate_window {
            self.error_window.remove(0);
        }
        if self.error_window.len() >= self.config.error_rate_window {
            let error_count = self.error_window.iter().filter(|&&e| e).count();
            let rate = error_count as f64 / self.error_window.len() as f64;
            if rate >= self.config.error_rate_threshold {
                let severity = if rate >= 0.5 {
                    Severity::Critical
                } else if rate >= 0.3 {
                    Severity::High
                } else {
                    Severity::Medium
                };
                let alert = AnomalyAlert {
                    anomaly_type: AnomalyType::HighErrorRate,
                    severity,
                    message: format!(
                        "高频错误：错误率 {:.1}%（窗口 {} 条，错误 {} 条）",
                        rate * 100.0,
                        self.error_window.len(),
                        error_count
                    ),
                    sql: sql.to_string(),
                    table: table.map(|s| s.to_string()),
                    timestamp,
                };
                triggered.push(alert);
            }
        }

        // 记录报警
        for alert in &triggered {
            *self
                .anomaly_counts
                .entry(alert.anomaly_type.clone())
                .or_insert(0) += 1;
        }
        self.alerts.extend(triggered.clone());

        triggered
    }

    /// 计算异常召回率
    ///
    /// - `actual_anomalies` — 实际异常总数
    /// - `detected_anomalies` — 检测到的异常数
    pub fn recall_rate(actual_anomalies: usize, detected_anomalies: usize) -> f64 {
        if actual_anomalies == 0 {
            return 1.0;
        }
        detected_anomalies as f64 / actual_anomalies as f64
    }
}

// =====================================================================
//  容量预测
// =====================================================================

/// 容量数据点
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapacityPoint {
    /// 时间戳（秒）
    pub timestamp: u64,
    /// 数据量（字节）
    pub size_bytes: u64,
}

/// 线性回归结果
#[derive(Debug, Clone, PartialEq)]
pub struct LinearRegression {
    /// 斜率（字节/秒）
    pub slope: f64,
    /// 截距（字节）
    pub intercept: f64,
    /// R² 拟合优度
    pub r_squared: f64,
}

impl LinearRegression {
    /// 预测指定时间戳的数据量
    pub fn predict(&self, timestamp: u64) -> f64 {
        self.slope * timestamp as f64 + self.intercept
    }

    /// 预测达到指定容量时的时间戳
    ///
    /// 返回 None 表示永远不会达到（斜率 <= 0）
    pub fn predict_time_for_capacity(&self, target_bytes: u64) -> Option<u64> {
        if self.slope <= 0.0 {
            return None;
        }
        let target = target_bytes as f64;
        let time = (target - self.intercept) / self.slope;
        if time < 0.0 {
            return None;
        }
        Some(time as u64)
    }
}

/// 容量预测器
#[derive(Debug, Default)]
pub struct CapacityPredictor {
    /// 历史数据点
    points: Vec<CapacityPoint>,
}

impl CapacityPredictor {
    /// 创建容量预测器
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加数据点
    pub fn add_point(&mut self, point: CapacityPoint) {
        self.points.push(point);
    }

    /// 批量添加数据点
    pub fn add_points(&mut self, points: Vec<CapacityPoint>) {
        self.points.extend(points);
    }

    /// 当前数据点数
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 清空数据点
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// 线性回归拟合
    ///
    /// 需要至少 2 个数据点。
    pub fn fit_linear(&self) -> Result<LinearRegression, AutoOpsError> {
        if self.points.len() < 2 {
            return Err(AutoOpsError::InsufficientData {
                needed: 2,
                got: self.points.len(),
            });
        }

        let n = self.points.len() as f64;
        let sum_x: f64 = self.points.iter().map(|p| p.timestamp as f64).sum();
        let sum_y: f64 = self.points.iter().map(|p| p.size_bytes as f64).sum();
        let sum_xy: f64 = self
            .points
            .iter()
            .map(|p| p.timestamp as f64 * p.size_bytes as f64)
            .sum();
        let sum_x2: f64 = self
            .points
            .iter()
            .map(|p| p.timestamp as f64 * p.timestamp as f64)
            .sum();

        let denominator = n * sum_x2 - sum_x * sum_x;
        if denominator.abs() < 1e-10 {
            // 所有时间戳相同，无法拟合
            return Ok(LinearRegression {
                slope: 0.0,
                intercept: sum_y / n,
                r_squared: 0.0,
            });
        }

        let slope = (n * sum_xy - sum_x * sum_y) / denominator;
        let intercept = (sum_y - slope * sum_x) / n;

        // 计算 R²
        let mean_y = sum_y / n;
        let ss_total: f64 = self
            .points
            .iter()
            .map(|p| (p.size_bytes as f64 - mean_y).powi(2))
            .sum();
        let ss_residual: f64 = self
            .points
            .iter()
            .map(|p| {
                let predicted = slope * p.timestamp as f64 + intercept;
                (p.size_bytes as f64 - predicted).powi(2)
            })
            .sum();
        let r_squared = if ss_total.abs() < 1e-10 {
            1.0
        } else {
            1.0 - ss_residual / ss_total
        };

        Ok(LinearRegression {
            slope,
            intercept,
            r_squared,
        })
    }

    /// 预测指定时间戳的数据量
    pub fn predict(&self, timestamp: u64) -> Result<f64, AutoOpsError> {
        let model = self.fit_linear()?;
        Ok(model.predict(timestamp))
    }

    /// 留一法交叉验证 — 计算平均预测误差
    ///
    /// 返回平均绝对百分比误差（MAPE，0.0 ~ 1.0）。
    pub fn cross_validate(&self) -> Result<f64, AutoOpsError> {
        if self.points.len() < 3 {
            return Err(AutoOpsError::InsufficientData {
                needed: 3,
                got: self.points.len(),
            });
        }

        let mut percentage_errors = Vec::new();

        for i in 0..self.points.len() {
            // 留一：移除第 i 个点
            let subset: Vec<CapacityPoint> = self
                .points
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != i)
                .map(|(_, p)| *p)
                .collect();

            let temp_predictor = CapacityPredictor { points: subset };
            let model = temp_predictor.fit_linear()?;
            let actual = self.points[i].size_bytes as f64;
            let predicted = model.predict(self.points[i].timestamp);

            if actual > 0.0 {
                let error = (predicted - actual).abs() / actual;
                percentage_errors.push(error);
            }
        }

        if percentage_errors.is_empty() {
            return Ok(0.0);
        }

        let mape: f64 = percentage_errors.iter().sum::<f64>() / percentage_errors.len() as f64;
        Ok(mape)
    }

    /// 预测达到指定容量的时间
    pub fn predict_time_for_capacity(
        &self,
        target_bytes: u64,
    ) -> Result<Option<u64>, AutoOpsError> {
        let model = self.fit_linear()?;
        Ok(model.predict_time_for_capacity(target_bytes))
    }
}

// =====================================================================
//  便捷函数 — 生成模拟数据
// =====================================================================

/// 生成线性增长的容量数据
///
/// - `start_time` — 起始时间戳
/// - `interval_secs` — 采样间隔（秒）
/// - `count` — 数据点数
/// - `initial_size` — 初始容量
/// - `growth_rate` — 每秒增长（字节）
/// - `noise_pct` — 噪声百分比（0.0 ~ 1.0）
pub fn generate_capacity_data(
    start_time: u64,
    interval_secs: u64,
    count: usize,
    initial_size: u64,
    growth_rate: f64,
    noise_pct: f64,
) -> Vec<CapacityPoint> {
    let mut points = Vec::with_capacity(count);
    for i in 0..count {
        let timestamp = start_time + (i as u64) * interval_secs;
        let base_size = initial_size as f64 + growth_rate * (i as f64) * (interval_secs as f64);
        // 简单确定性噪声（基于 i 的伪随机）
        let noise = if noise_pct > 0.0 {
            let seed = (i as f64 * 0.12345).sin().abs();
            base_size * noise_pct * (seed - 0.5) * 2.0
        } else {
            0.0
        };
        let size = (base_size + noise).max(0.0) as u64;
        points.push(CapacityPoint {
            timestamp,
            size_bytes: size,
        });
    }
    points
}

/// 异常查询事件元组类型
///
/// 字段顺序：(sql, elapsed_ms, scanned_rows, used_index, table, is_error, error_kind, timestamp)
pub type AnomalyQueryEvent = (
    String,
    u64,
    u64,
    bool,
    Option<String>,
    bool,
    Option<String>,
    u64,
);

/// 生成异常查询事件
///
/// - `count` — 总查询数
/// - `anomaly_ratio` — 异常比例（0.0 ~ 1.0）
pub fn generate_anomaly_queries(count: usize, anomaly_ratio: f64) -> Vec<AnomalyQueryEvent> {
    let mut results = Vec::with_capacity(count);
    let anomaly_count = (count as f64 * anomaly_ratio) as usize;

    for i in 0..count {
        let is_anomaly = i < anomaly_count;
        let table_idx = i % 5;
        let sql = format!("SELECT * FROM table_{table_idx} WHERE id = {i}");
        let table = format!("table_{table_idx}");
        let timestamp = i as u64 * 10;

        if is_anomaly {
            // 异常查询
            let anomaly_type = i % 4;
            match anomaly_type {
                0 => {
                    // 全表扫描
                    results.push((
                        sql,
                        100 + (i as u64 % 200),     // elapsed_ms
                        50000 + (i as u64 % 50000), // scanned_rows
                        false,                      // used_index
                        Some(table),
                        false,
                        None,
                        timestamp,
                    ));
                }
                1 => {
                    // 死锁
                    results.push((
                        sql,
                        200,
                        100,
                        true,
                        Some(table),
                        true,
                        Some("deadlock detected".to_string()),
                        timestamp,
                    ));
                }
                2 => {
                    // 超时
                    results.push((
                        sql,
                        6000 + (i as u64 % 4000), // elapsed_ms > 5000
                        1000,
                        true,
                        Some(table),
                        false,
                        None,
                        timestamp,
                    ));
                }
                _ => {
                    // 高频错误
                    results.push((
                        sql,
                        50,
                        100,
                        true,
                        Some(table),
                        true,
                        Some("connection error".to_string()),
                        timestamp,
                    ));
                }
            }
        } else {
            // 正常查询
            results.push((
                sql,
                10 + (i as u64 % 50), // elapsed_ms
                5 + (i as u64 % 50),  // scanned_rows
                true,                 // used_index
                Some(table),
                false,
                None,
                timestamp,
            ));
        }
    }

    results
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    // -----------------------------------------------------------------
    //  AnomalyType / Severity 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_anomaly_type_name() {
        assert_eq!(AnomalyType::FullTableScan.name(), "full_table_scan");
        assert_eq!(AnomalyType::Deadlock.name(), "deadlock");
        assert_eq!(AnomalyType::Timeout.name(), "timeout");
        assert_eq!(AnomalyType::HighErrorRate.name(), "high_error_rate");
    }

    #[test]
    fn test_7b8_severity_from_level() {
        assert_eq!(Severity::from_level(0), Severity::Low);
        assert_eq!(Severity::from_level(1), Severity::Low);
        assert_eq!(Severity::from_level(2), Severity::Medium);
        assert_eq!(Severity::from_level(3), Severity::High);
        assert_eq!(Severity::from_level(4), Severity::Critical);
        assert_eq!(Severity::from_level(99), Severity::Critical);
    }

    #[test]
    fn test_7b8_severity_ordering() {
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
    }

    // -----------------------------------------------------------------
    //  AnomalyDetector 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_detector_creation() {
        let detector = AnomalyDetector::default();
        assert!(detector.alerts().is_empty());
        assert_eq!(detector.total_anomalies(), 0);
    }

    #[test]
    fn test_7b8_detector_clear() {
        let mut detector = AnomalyDetector::default();
        detector.check_query("SELECT 1", 6000, 50000, false, Some("t"), false, None, 1);
        assert!(!detector.alerts().is_empty());
        detector.clear();
        assert!(detector.alerts().is_empty());
    }

    // -----------------------------------------------------------------
    //  异常检测 — 全表扫描
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_detect_full_table_scan() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "SELECT * FROM users",
            100,
            50000, // > 10000 阈值
            false, // 无索引
            Some("users"),
            false,
            None,
            1000,
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].anomaly_type, AnomalyType::FullTableScan);
        assert!(alerts[0].message.contains("50000"));
        assert_eq!(alerts[0].table.as_deref(), Some("users"));
    }

    #[test]
    fn test_7b8_no_full_scan_with_index() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "SELECT * FROM users WHERE id = 1",
            10,
            1,
            true, // 使用索引
            Some("users"),
            false,
            None,
            1000,
        );
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_7b8_no_full_scan_below_threshold() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "SELECT * FROM users",
            10,
            5000, // < 10000 阈值
            false,
            Some("users"),
            false,
            None,
            1000,
        );
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_7b8_full_scan_severity_levels() {
        let mut detector = AnomalyDetector::default();

        // Medium: 1x-5x 阈值
        let alerts = detector.check_query("SELECT 1", 10, 10000, false, None, false, None, 1);
        assert_eq!(alerts[0].severity, Severity::Medium);

        // High: 5x-10x 阈值
        let alerts = detector.check_query("SELECT 1", 10, 50000, false, None, false, None, 2);
        assert_eq!(alerts[0].severity, Severity::High);

        // Critical: > 10x 阈值
        let alerts = detector.check_query("SELECT 1", 10, 100000, false, None, false, None, 3);
        assert_eq!(alerts[0].severity, Severity::Critical);
    }

    // -----------------------------------------------------------------
    //  异常检测 — 超时
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_detect_timeout() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "SELECT * FROM large_table",
            6000, // > 5000 阈值
            100,
            true,
            Some("large_table"),
            false,
            None,
            1000,
        );
        let timeout_alerts: Vec<_> = alerts
            .iter()
            .filter(|a| a.anomaly_type == AnomalyType::Timeout)
            .collect();
        assert_eq!(timeout_alerts.len(), 1);
        assert!(timeout_alerts[0].message.contains("6000"));
    }

    #[test]
    fn test_7b8_no_timeout_below_threshold() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query("SELECT 1", 4000, 10, true, None, false, None, 1);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_7b8_timeout_severity_levels() {
        let mut detector = AnomalyDetector::default();

        // Medium: 1x-2x
        let alerts = detector.check_query("SELECT 1", 5000, 10, true, None, false, None, 1);
        assert_eq!(alerts[0].severity, Severity::Medium);

        // High: 2x-4x
        let alerts = detector.check_query("SELECT 1", 10000, 10, true, None, false, None, 2);
        assert_eq!(alerts[0].severity, Severity::High);

        // Critical: > 4x
        let alerts = detector.check_query("SELECT 1", 20000, 10, true, None, false, None, 3);
        assert_eq!(alerts[0].severity, Severity::Critical);
    }

    // -----------------------------------------------------------------
    //  异常检测 — 死锁
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_detect_deadlock() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "UPDATE accounts SET balance = 100",
            200,
            1,
            true,
            Some("accounts"),
            true,
            Some("deadlock detected"),
            1000,
        );
        let deadlock_alerts: Vec<_> = alerts
            .iter()
            .filter(|a| a.anomaly_type == AnomalyType::Deadlock)
            .collect();
        assert_eq!(deadlock_alerts.len(), 1);
        assert_eq!(deadlock_alerts[0].severity, Severity::High);
    }

    #[test]
    fn test_7b8_no_deadlock_without_error() {
        let mut detector = AnomalyDetector::default();
        let alerts = detector.check_query(
            "UPDATE accounts SET balance = 100",
            200,
            1,
            true,
            Some("accounts"),
            false,
            None,
            1000,
        );
        let deadlock_alerts: Vec<_> = alerts
            .iter()
            .filter(|a| a.anomaly_type == AnomalyType::Deadlock)
            .collect();
        assert!(deadlock_alerts.is_empty());
    }

    // -----------------------------------------------------------------
    //  异常检测 — 高频错误
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_detect_high_error_rate() {
        let mut detector = AnomalyDetector::default();

        // 填充错误窗口（50% 错误率）
        for i in 0..100 {
            detector.check_query(
                "SELECT 1",
                10,
                1,
                true,
                None,
                i % 2 == 0, // 50% 错误
                Some("error"),
                i as u64,
            );
        }

        let high_error_alerts: Vec<_> = detector
            .alerts()
            .iter()
            .filter(|a| a.anomaly_type == AnomalyType::HighErrorRate)
            .collect();
        assert!(!high_error_alerts.is_empty());
        assert!(high_error_alerts[0].severity >= Severity::Critical); // 50% > 30% 阈值
    }

    #[test]
    fn test_7b8_no_high_error_rate_below_threshold() {
        let mut detector = AnomalyDetector::default();

        // 5% 错误率（< 10% 阈值）
        for i in 0..100 {
            detector.check_query(
                "SELECT 1",
                10,
                1,
                true,
                None,
                i < 5, // 5% 错误
                Some("error"),
                i as u64,
            );
        }

        let high_error_alerts: Vec<_> = detector
            .alerts()
            .iter()
            .filter(|a| a.anomaly_type == AnomalyType::HighErrorRate)
            .collect();
        assert!(high_error_alerts.is_empty());
    }

    // -----------------------------------------------------------------
    //  多种异常同时触发
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_multiple_anomalies_single_query() {
        let mut detector = AnomalyDetector::default();
        // 全表扫描 + 超时同时触发
        let alerts = detector.check_query(
            "SELECT * FROM huge_table",
            8000,  // 超时
            80000, // 全表扫描
            false,
            Some("huge_table"),
            false,
            None,
            1000,
        );
        assert_eq!(alerts.len(), 2);
        let types: Vec<&AnomalyType> = alerts.iter().map(|a| &a.anomaly_type).collect();
        assert!(types.contains(&&AnomalyType::FullTableScan));
        assert!(types.contains(&&AnomalyType::Timeout));
    }

    // -----------------------------------------------------------------
    //  召回率测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_recall_rate() {
        assert_eq!(AnomalyDetector::recall_rate(100, 95), 0.95);
        assert_eq!(AnomalyDetector::recall_rate(0, 0), 1.0);
        assert_eq!(AnomalyDetector::recall_rate(100, 100), 1.0);
    }

    #[test]
    fn test_7b8_recall_rate_above_90_percent() {
        // 模拟 1000 个异常，检测到 950 个 → 召回率 95%
        let mut detector = AnomalyDetector::default();
        let total_anomalies = 1000;
        let detected = 950;

        // 生成 1000 个异常查询
        for i in 0..total_anomalies {
            detector.check_query(
                &format!("SELECT {i}"),
                8000,  // 超时
                50000, // 全表扫描
                false,
                Some("t"),
                false,
                None,
                i as u64,
            );
        }

        // 每个异常至少触发一个报警（超时或全表扫描）
        let total_alerts = detector.total_anomalies();
        assert!(total_alerts >= detected);
        let recall = AnomalyDetector::recall_rate(total_anomalies, total_alerts);
        assert!(recall >= 0.9, "recall should be >= 90%, got {recall}");
    }

    // -----------------------------------------------------------------
    //  完整异常检测验证 — 模拟异常查询
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_full_anomaly_detection_workflow() {
        let mut detector = AnomalyDetector::default();
        let queries = generate_anomaly_queries(1000, 0.4); // 40% 异常

        let mut detected_count = 0;
        for (sql, elapsed, scanned, used_index, table, is_error, error_kind, ts) in queries {
            let alerts = detector.check_query(
                &sql,
                elapsed,
                scanned,
                used_index,
                table.as_deref(),
                is_error,
                error_kind.as_deref(),
                ts,
            );
            if !alerts.is_empty() {
                detected_count += 1;
            }
        }

        // 400 个异常，应检测到 >= 90% = 360
        assert!(
            detected_count >= 360,
            "should detect >= 360 anomalies, got {detected_count}"
        );
        let recall = AnomalyDetector::recall_rate(400, detected_count);
        assert!(recall >= 0.9, "recall should be >= 90%, got {recall}");
    }

    // -----------------------------------------------------------------
    //  CapacityPredictor 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_predictor_creation() {
        let predictor = CapacityPredictor::new();
        assert!(predictor.is_empty());
        assert_eq!(predictor.len(), 0);
    }

    #[test]
    fn test_7b8_predictor_add_point() {
        let mut predictor = CapacityPredictor::new();
        predictor.add_point(CapacityPoint {
            timestamp: 1000,
            size_bytes: 1000,
        });
        assert_eq!(predictor.len(), 1);
    }

    #[test]
    fn test_7b8_predictor_clear() {
        let mut predictor = CapacityPredictor::new();
        predictor.add_point(CapacityPoint {
            timestamp: 1000,
            size_bytes: 1000,
        });
        predictor.clear();
        assert!(predictor.is_empty());
    }

    // -----------------------------------------------------------------
    //  线性回归测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_fit_linear_perfect() {
        let mut predictor = CapacityPredictor::new();
        // 完美线性：y = 2x + 100
        for i in 0..10u64 {
            predictor.add_point(CapacityPoint {
                timestamp: i,
                size_bytes: 2 * i + 100,
            });
        }
        let model = predictor.fit_linear().unwrap();
        assert!((model.slope - 2.0).abs() < 0.001);
        assert!((model.intercept - 100.0).abs() < 0.001);
        assert!((model.r_squared - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_7b8_fit_linear_insufficient_data() {
        let mut predictor = CapacityPredictor::new();
        predictor.add_point(CapacityPoint {
            timestamp: 0,
            size_bytes: 100,
        });
        let result = predictor.fit_linear();
        assert!(result.is_err());
    }

    #[test]
    fn test_7b8_predict() {
        let mut predictor = CapacityPredictor::new();
        for i in 0..10u64 {
            predictor.add_point(CapacityPoint {
                timestamp: i,
                size_bytes: 100 * i,
            });
        }
        let predicted = predictor.predict(20).unwrap();
        assert!((predicted - 2000.0).abs() < 1.0);
    }

    #[test]
    fn test_7b8_predict_time_for_capacity() {
        let mut predictor = CapacityPredictor::new();
        // y = 100x + 0
        for i in 0..10u64 {
            predictor.add_point(CapacityPoint {
                timestamp: i,
                size_bytes: 100 * i,
            });
        }
        let time = predictor.predict_time_for_capacity(10000).unwrap().unwrap();
        assert_eq!(time, 100);
    }

    #[test]
    fn test_7b8_predict_time_for_capacity_negative_slope() {
        let mut predictor = CapacityPredictor::new();
        // 负斜率
        predictor.add_point(CapacityPoint {
            timestamp: 0,
            size_bytes: 1000,
        });
        predictor.add_point(CapacityPoint {
            timestamp: 10,
            size_bytes: 500,
        });
        let time = predictor.predict_time_for_capacity(2000).unwrap();
        assert!(time.is_none()); // 不会达到
    }

    // -----------------------------------------------------------------
    //  交叉验证测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_cross_validate_perfect_linear() {
        let mut predictor = CapacityPredictor::new();
        // 完美线性数据
        for i in 0..20u64 {
            predictor.add_point(CapacityPoint {
                timestamp: i,
                size_bytes: 100 * i + 50,
            });
        }
        let mape = predictor.cross_validate().unwrap();
        assert!(
            mape < 0.01,
            "MAPE should be < 1% for perfect linear, got {mape}"
        );
    }

    #[test]
    fn test_7b8_cross_validate_insufficient_data() {
        let mut predictor = CapacityPredictor::new();
        predictor.add_point(CapacityPoint {
            timestamp: 0,
            size_bytes: 100,
        });
        predictor.add_point(CapacityPoint {
            timestamp: 1,
            size_bytes: 200,
        });
        let result = predictor.cross_validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_7b8_cross_validate_with_noise() {
        let points = generate_capacity_data(0, 3600, 30, 1_000_000_000, 1000.0, 0.05);
        let mut predictor = CapacityPredictor::new();
        predictor.add_points(points);
        let mape = predictor.cross_validate().unwrap();
        assert!(mape < 0.2, "MAPE should be < 20%, got {mape}");
    }

    // -----------------------------------------------------------------
    //  完整容量预测验证 — 模拟增长趋势
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_full_capacity_prediction_workflow() {
        // 模拟 90 天的容量数据，每小时一个点
        let points = generate_capacity_data(0, 3600, 90 * 24, 1_000_000_000, 5000.0, 0.03);
        let mut predictor = CapacityPredictor::new();
        predictor.add_points(points);

        // 拟合线性回归
        let model = predictor.fit_linear().unwrap();
        assert!(model.slope > 0.0, "slope should be positive");
        assert!(
            model.r_squared > 0.9,
            "R² should be > 0.9, got {}",
            model.r_squared
        );

        // 交叉验证
        let mape = predictor.cross_validate().unwrap();
        assert!(mape < 0.2, "MAPE should be < 20%, got {mape}");

        // 预测未来容量
        let future_time = 90 * 24 * 3600; // 90 天后
        let predicted = predictor.predict(future_time).unwrap();
        assert!(predicted > 1_000_000_000.0);
    }

    // -----------------------------------------------------------------
    //  数据生成器测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_generate_capacity_data_count() {
        let points = generate_capacity_data(0, 3600, 100, 1000, 10.0, 0.0);
        assert_eq!(points.len(), 100);
    }

    #[test]
    fn test_7b8_generate_capacity_data_linear_growth() {
        let points = generate_capacity_data(0, 1, 10, 1000, 100.0, 0.0);
        // 无噪声时应该完美线性
        for (i, p) in points.iter().enumerate() {
            let expected = 1000 + 100 * i as u64;
            assert_eq!(p.size_bytes, expected);
        }
    }

    #[test]
    fn test_7b8_generate_anomaly_queries_count() {
        let queries = generate_anomaly_queries(100, 0.4);
        assert_eq!(queries.len(), 100);
    }

    #[test]
    fn test_7b8_generate_anomaly_queries_ratio() {
        let queries = generate_anomaly_queries(1000, 0.3);
        // 前 300 条是异常（全表扫描/死锁/超时/高频错误）
        let anomaly_count = queries
            .iter()
            .take(300)
            .filter(|(_, elapsed, scanned, used_index, _, is_error, _, _)| {
                *elapsed >= 5000 || *scanned >= 10000 || !*used_index || *is_error
            })
            .count();
        assert!(anomaly_count >= 250); // 大部分异常应被识别
    }

    // -----------------------------------------------------------------
    //  边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_empty_detector_recall() {
        let recall = AnomalyDetector::recall_rate(0, 0);
        assert_eq!(recall, 1.0);
    }

    #[test]
    fn test_7b8_fit_linear_same_timestamps() {
        let mut predictor = CapacityPredictor::new();
        // 相同时间戳
        predictor.add_point(CapacityPoint {
            timestamp: 100,
            size_bytes: 1000,
        });
        predictor.add_point(CapacityPoint {
            timestamp: 100,
            size_bytes: 2000,
        });
        let model = predictor.fit_linear().unwrap();
        // 斜率应为 0（无法拟合）
        assert_eq!(model.slope, 0.0);
        assert_eq!(model.intercept, 1500.0); // 平均值
    }

    #[test]
    fn test_7b8_anomaly_count_by_type() {
        let mut detector = AnomalyDetector::default();
        detector.check_query("SELECT 1", 10, 50000, false, None, false, None, 1); // 全表扫描
        detector.check_query("SELECT 2", 10, 60000, false, None, false, None, 2); // 全表扫描
        detector.check_query("SELECT 3", 8000, 10, true, None, false, None, 3); // 超时

        assert_eq!(detector.anomaly_count(&AnomalyType::FullTableScan), 2);
        assert_eq!(detector.anomaly_count(&AnomalyType::Timeout), 1);
        assert_eq!(detector.anomaly_count(&AnomalyType::Deadlock), 0);
    }

    // -----------------------------------------------------------------
    //  配置测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b8_custom_config() {
        let config = AnomalyDetectorConfig {
            full_scan_threshold: 5000,
            timeout_threshold_ms: 1000,
            error_rate_threshold: 0.05,
            error_rate_window: 50,
        };
        let mut detector = AnomalyDetector::new(config);
        // 扫描 6000 行 > 5000 阈值
        let alerts = detector.check_query("SELECT 1", 10, 6000, false, None, false, None, 1);
        assert!(!alerts.is_empty());
    }

    #[test]
    fn test_7b8_config_access() {
        let detector = AnomalyDetector::default();
        assert_eq!(detector.config().full_scan_threshold, 10000);
        assert_eq!(detector.config().timeout_threshold_ms, 5000);
    }
}
