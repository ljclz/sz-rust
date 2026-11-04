// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 基准压测套件核心逻辑（spec §5.10）
//!
//! 回归检测 + 性能预算门禁 + 基线管理 + 火焰图配置。

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::BenchmarkError;

/// 基准结果（spec §5.10）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkResult {
    /// 基准名称
    pub name: String,
    /// 平均耗时（纳秒）
    pub mean_ns: f64,
    /// 吞吐量（ops/s）
    pub throughput: f64,
    /// 分配数
    pub allocations: u64,
}

impl BenchmarkResult {
    /// 创建新基准结果
    pub fn new(name: impl Into<String>, mean_ns: f64, throughput: f64, allocations: u64) -> Self {
        Self {
            name: name.into(),
            mean_ns,
            throughput,
            allocations,
        }
    }
}

/// 回归检测报告（spec §5.10）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegressionReport {
    /// 各基准的回归百分比（正数=回归，负数=提升）
    pub regressions: HashMap<String, f64>,
    /// 是否超过门禁
    pub exceeded_gate: bool,
}

/// 基线文件（JSON 序列化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineFile {
    /// 版本标识
    pub version: String,
    /// 基线结果
    pub results: Vec<BenchmarkResult>,
}

/// 基准套件配置（spec §6.10）
#[derive(Debug, Clone)]
pub struct BenchmarkSuiteConfig {
    /// 回归阈值（百分比，spec §6.10 规则 2）
    pub regression_threshold: f64,
    /// 性能预算门禁（百分比，spec §6.10 规则 3）
    pub budget_gate: f64,
    /// 采样次数（默认 ≥ 10，spec §6.10 规则 4）
    pub sample_size: usize,
    /// 基线路径
    pub baseline_path: PathBuf,
    /// 火焰图输出目录
    pub flamegraph_dir: PathBuf,
}

impl Default for BenchmarkSuiteConfig {
    fn default() -> Self {
        Self {
            regression_threshold: 10.0,
            budget_gate: 15.0,
            sample_size: 10,
            baseline_path: PathBuf::from("benchmarks/baseline.json"),
            flamegraph_dir: PathBuf::from("benchmarks/flamegraphs"),
        }
    }
}

impl BenchmarkSuiteConfig {
    /// 验证配置（spec §6.10 规则 2/3/4）
    pub fn validate(&self) -> Result<(), BenchmarkError> {
        if self.regression_threshold <= 0.0 {
            return Err(BenchmarkError::Config(format!(
                "regression_threshold 必须为正数，当前: {}",
                self.regression_threshold
            )));
        }
        if self.budget_gate <= 0.0 {
            return Err(BenchmarkError::Config(format!(
                "budget_gate 必须为正数，当前: {}",
                self.budget_gate
            )));
        }
        if self.sample_size < 10 {
            return Err(BenchmarkError::Config(format!(
                "sample_size 必须 >= 10，当前: {}",
                self.sample_size
            )));
        }
        Ok(())
    }
}

/// 基准套件（spec §5.10）
pub struct BenchmarkSuite {
    config: BenchmarkSuiteConfig,
}

impl BenchmarkSuite {
    /// 创建基准套件
    pub fn new(config: BenchmarkSuiteConfig) -> Result<Self, BenchmarkError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// 获取配置引用
    pub fn config(&self) -> &BenchmarkSuiteConfig {
        &self.config
    }

    /// 计算单个基准的回归百分比
    ///
    /// 回归百分比 = (current - baseline) / baseline * 100
    /// 正数 = 回归（变慢），负数 = 提升（变快）
    pub fn calc_regression_pct(baseline_ns: f64, current_ns: f64) -> Result<f64, BenchmarkError> {
        if baseline_ns <= 0.0 {
            return Err(BenchmarkError::Internal(format!(
                "基线耗时必须为正数，当前: {}",
                baseline_ns
            )));
        }
        Ok((current_ns - baseline_ns) / baseline_ns * 100.0)
    }

    /// 回归检测：与基线对比（spec §5.10 规则 2/3）
    pub async fn detect_regression(
        &self,
        results: &[BenchmarkResult],
    ) -> Result<RegressionReport, BenchmarkError> {
        let baseline = self.load_baseline().await?;
        let baseline_map: HashMap<&str, &BenchmarkResult> = baseline
            .results
            .iter()
            .map(|r| (r.name.as_str(), r))
            .collect();

        let mut regressions = HashMap::new();
        let mut exceeded_gate = false;

        for result in results {
            if let Some(base) = baseline_map.get(result.name.as_str()) {
                let pct = Self::calc_regression_pct(base.mean_ns, result.mean_ns)?;
                regressions.insert(result.name.clone(), pct);
                if pct > self.config.budget_gate {
                    exceeded_gate = true;
                }
            }
        }

        Ok(RegressionReport {
            regressions,
            exceeded_gate,
        })
    }

    /// 更新基线（需显式确认，spec §5.10 规则 5）
    pub async fn update_baseline(
        &self,
        results: &[BenchmarkResult],
        version: &str,
    ) -> Result<(), BenchmarkError> {
        let baseline = BaselineFile {
            version: version.to_string(),
            results: results.to_vec(),
        };
        let json = serde_json::to_string_pretty(&baseline)
            .map_err(|e| BenchmarkError::Internal(format!("序列化基线失败: {e}")))?;

        if let Some(dir) = self.config.baseline_path.parent() {
            if !dir.as_os_str().is_empty() {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|e| BenchmarkError::Internal(format!("创建目录失败: {e}")))?;
            }
        }
        tokio::fs::write(&self.config.baseline_path, json)
            .await
            .map_err(|e| BenchmarkError::Internal(format!("写入基线失败: {e}")))?;
        Ok(())
    }

    /// 加载基线
    pub async fn load_baseline(&self) -> Result<BaselineFile, BenchmarkError> {
        let content = tokio::fs::read_to_string(&self.config.baseline_path)
            .await
            .map_err(|_| BenchmarkError::BaselineMissing)?;
        serde_json::from_str(&content)
            .map_err(|e| BenchmarkError::Internal(format!("解析基线失败: {e}")))
    }

    /// 检查基线是否存在
    pub async fn baseline_exists(&self) -> bool {
        tokio::fs::metadata(&self.config.baseline_path)
            .await
            .is_ok()
    }

    /// 火焰图输出路径（spec §5.10 规则 4）
    pub fn flamegraph_path(&self, bench_name: &str) -> PathBuf {
        self.config.flamegraph_dir.join(format!("{bench_name}.svg"))
    }

    /// 验证基准结果（spec §5.10 禁止项：禁止幻影测试）
    pub fn validate_results(results: &[BenchmarkResult]) -> Result<(), BenchmarkError> {
        for r in results {
            if r.name.is_empty() {
                return Err(BenchmarkError::Config("基准名称不能为空".to_string()));
            }
            if r.mean_ns <= 0.0 {
                return Err(BenchmarkError::Config(format!(
                    "基准 {} 的 mean_ns 必须为正数",
                    r.name
                )));
            }
            if r.throughput < 0.0 {
                return Err(BenchmarkError::Config(format!(
                    "基准 {} 的 throughput 不能为负数",
                    r.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_baseline_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "sz-rust-bench-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn make_config(path: PathBuf) -> BenchmarkSuiteConfig {
        BenchmarkSuiteConfig {
            regression_threshold: 10.0,
            budget_gate: 15.0,
            sample_size: 10,
            baseline_path: path,
            flamegraph_dir: PathBuf::from("benchmarks/flamegraphs"),
        }
    }

    #[test]
    fn test_config_validate_ok() {
        let config = make_config(temp_baseline_path());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_negative_threshold() {
        let mut config = make_config(temp_baseline_path());
        config.regression_threshold = -1.0;
        assert!(matches!(config.validate(), Err(BenchmarkError::Config(_))));
    }

    #[test]
    fn test_config_validate_zero_budget_gate() {
        let mut config = make_config(temp_baseline_path());
        config.budget_gate = 0.0;
        assert!(matches!(config.validate(), Err(BenchmarkError::Config(_))));
    }

    #[test]
    fn test_config_validate_small_sample_size() {
        let mut config = make_config(temp_baseline_path());
        config.sample_size = 5;
        assert!(matches!(config.validate(), Err(BenchmarkError::Config(_))));
    }

    #[test]
    fn test_calc_regression_pct() {
        assert_eq!(
            BenchmarkSuite::calc_regression_pct(100.0, 120.0).unwrap(),
            20.0
        );
        assert_eq!(
            BenchmarkSuite::calc_regression_pct(100.0, 80.0).unwrap(),
            -20.0
        );
        assert_eq!(
            BenchmarkSuite::calc_regression_pct(100.0, 100.0).unwrap(),
            0.0
        );
    }

    #[test]
    fn test_calc_regression_pct_zero_baseline() {
        assert!(BenchmarkSuite::calc_regression_pct(0.0, 100.0).is_err());
    }

    #[test]
    fn test_validate_results_ok() {
        let results = vec![
            BenchmarkResult::new("bench1", 100.0, 10000.0, 10),
            BenchmarkResult::new("bench2", 200.0, 5000.0, 20),
        ];
        assert!(BenchmarkSuite::validate_results(&results).is_ok());
    }

    #[test]
    fn test_validate_results_empty_name() {
        let results = vec![BenchmarkResult::new("", 100.0, 1000.0, 1)];
        assert!(matches!(
            BenchmarkSuite::validate_results(&results),
            Err(BenchmarkError::Config(_))
        ));
    }

    #[test]
    fn test_validate_results_negative_mean() {
        let results = vec![BenchmarkResult::new("bench1", -1.0, 1000.0, 1)];
        assert!(matches!(
            BenchmarkSuite::validate_results(&results),
            Err(BenchmarkError::Config(_))
        ));
    }

    #[test]
    fn test_validate_results_negative_throughput() {
        let results = vec![BenchmarkResult::new("bench1", 100.0, -1.0, 1)];
        assert!(matches!(
            BenchmarkSuite::validate_results(&results),
            Err(BenchmarkError::Config(_))
        ));
    }

    #[tokio::test]
    async fn test_baseline_round_trip() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let results = vec![
            BenchmarkResult::new("bench1", 100.0, 10000.0, 10),
            BenchmarkResult::new("bench2", 200.0, 5000.0, 20),
        ];
        suite.update_baseline(&results, "v1.7.0").await.unwrap();
        let loaded = suite.load_baseline().await.unwrap();
        assert_eq!(loaded.version, "v1.7.0");
        assert_eq!(loaded.results, results);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_detect_regression_no_baseline() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let results = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        let err = suite.detect_regression(&results).await.unwrap_err();
        assert!(matches!(err, BenchmarkError::BaselineMissing));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_detect_regression_within_gate() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let baseline = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        suite.update_baseline(&baseline, "v1.0.0").await.unwrap();
        let current = vec![BenchmarkResult::new("bench1", 105.0, 9523.0, 10)];
        let report = suite.detect_regression(&current).await.unwrap();
        assert!(!report.exceeded_gate);
        assert_eq!(
            report.regressions.get("bench1").copied().unwrap_or(0.0),
            5.0
        );
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_detect_regression_exceeded_gate() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let baseline = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        suite.update_baseline(&baseline, "v1.0.0").await.unwrap();
        let current = vec![BenchmarkResult::new("bench1", 120.0, 8333.0, 10)];
        let report = suite.detect_regression(&current).await.unwrap();
        assert!(report.exceeded_gate);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_detect_regression_improvement() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let baseline = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        suite.update_baseline(&baseline, "v1.0.0").await.unwrap();
        let current = vec![BenchmarkResult::new("bench1", 90.0, 11111.0, 10)];
        let report = suite.detect_regression(&current).await.unwrap();
        assert!(!report.exceeded_gate);
        assert_eq!(
            report.regressions.get("bench1").copied().unwrap_or(0.0),
            -10.0
        );
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_baseline_exists() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        assert!(!suite.baseline_exists().await);
        let results = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        suite.update_baseline(&results, "v1.0.0").await.unwrap();
        assert!(suite.baseline_exists().await);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[test]
    fn test_flamegraph_path() {
        let config = make_config(temp_baseline_path());
        let suite = BenchmarkSuite::new(config).unwrap();
        let path = suite.flamegraph_path("routing_bench");
        assert!(path.to_string_lossy().ends_with("routing_bench.svg"));
    }

    #[test]
    fn test_default_config() {
        let config = BenchmarkSuiteConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.sample_size, 10);
    }

    #[tokio::test]
    async fn test_detect_regression_multiple_benchmarks() {
        let path = temp_baseline_path();
        let suite = BenchmarkSuite::new(make_config(path.clone())).unwrap();
        let baseline = vec![
            BenchmarkResult::new("routing", 100.0, 10000.0, 10),
            BenchmarkResult::new("serialize", 50.0, 20000.0, 5),
            BenchmarkResult::new("cache", 10.0, 100000.0, 2),
        ];
        suite.update_baseline(&baseline, "v1.0.0").await.unwrap();
        let current = vec![
            BenchmarkResult::new("routing", 110.0, 9090.0, 11),
            BenchmarkResult::new("serialize", 45.0, 22222.0, 4),
            BenchmarkResult::new("cache", 13.0, 76923.0, 3),
        ];
        let report = suite.detect_regression(&current).await.unwrap();
        assert!(report.exceeded_gate);
        assert_eq!(report.regressions.len(), 3);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_update_baseline_creates_parent_dir() {
        let mut base = std::env::temp_dir();
        base.push(format!(
            "sz-rust-bench-dir-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let baseline_path = base.join("sub").join("baseline.json");
        let config = BenchmarkSuiteConfig {
            regression_threshold: 10.0,
            budget_gate: 15.0,
            sample_size: 10,
            baseline_path: baseline_path.clone(),
            flamegraph_dir: PathBuf::from("benchmarks/flamegraphs"),
        };
        let suite = BenchmarkSuite::new(config).unwrap();
        let results = vec![BenchmarkResult::new("bench1", 100.0, 10000.0, 10)];
        suite.update_baseline(&results, "v1.0.0").await.unwrap();
        assert!(baseline_path.exists());
        let _ = tokio::fs::remove_dir_all(&base).await;
    }
}
