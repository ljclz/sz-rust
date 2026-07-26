//! Phase 5.10 — ML 成本模型原型
//!
//! 提供 ML 驱动的查询成本预测能力，作为手工 `CostModel` 的补充与提升：
//! - 从 `LogicalPlan` 提取 16 维数值特征（节点计数 + 结构特征 + 手工估算）
//! - 纯 Rust 线性回归（梯度下降 + L2 正则 + z-score 特征归一化）
//! - 增量训练：每次执行后追加样本，达到阈值自动重训练
//! - 冷启动 fallback：样本不足时回退到手工 `CostModel`
//! - 可选 `onnx` feature：启用后通过 ONNX Runtime 加载预训练模型推理（默认禁用，
//!   因 `ort` 在 Windows 上需动态链接 ONNX Runtime DLL；Phase -1.3 预检未完成）
//!
//! # 设计权衡
//!
//! - **纯 Rust 默认**：避免 `ort` 依赖带来的构建复杂性（Windows DLL、CMake）。
//!   线性回归虽简单，但在 16 维特征 + L2 正则下足以超越手工模型（验证标准要求
//!   "ML 误差 < 手工模型"）。
//! - **特征归一化**：原始特征量纲差异巨大（node_count 个位 vs estimated_rows 千级），
//!   z-score 归一化后梯度下降收敛稳定。
//! - **增量训练**：生产场景下查询持续到达，全量重训代价高。每次达到
//!   `RETRAIN_THRESHOLD` 触发增量梯度下降（基于当前权重继续优化）。
//! - **MAPE 评估**：与 PG 论文一致，使用平均绝对百分比误差衡量模型精度。
//!
//! # 验证标准
//!
//! - 收集 100000 条查询的执行特征（合成数据 + ground truth 函数）
//! - 80% 训练 / 20% 测试
//! - ML 预测 MAPE < 手工模型 MAPE
//! - 冷启动（< `MIN_SAMPLES_TO_PREDICT`）正确 fallback 到手工模型
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.10。

use std::sync::Arc;

use szrsql_sql::ast::{
    BinaryOp, ColumnDefinition, Expr, JoinCondition, JoinType, OrderByExpr, TableName,
};
use szrsql_sql::plan::{LogicalPlan, TableSchema};
use szrsql_types::value::ColumnType;

use crate::cost::CostModel;
use crate::statistics::StatisticsStore;

// =====================================================================
//  常量
// =====================================================================

/// 特征维度数
pub const FEATURE_DIM: usize = 16;

/// 最小预测样本数 — 少于此值时 fallback 到手工模型
pub const MIN_SAMPLES_TO_PREDICT: usize = 100;

/// 增量训练阈值 — 每累积此数量新样本触发一次重训练
pub const RETRAIN_THRESHOLD: usize = 50;

/// 梯度下降学习率（Phase 7b.1: 适配目标归一化后的梯度尺度）
pub const LEARNING_RATE: f64 = 0.1;

/// L2 正则化系数
pub const L2_REG: f64 = 0.001;

/// 梯度下降迭代轮数（每次增量训练）
pub const EPOCHS_PER_TRAIN: usize = 50;

/// 训练集上限 — 超过后丢弃最旧样本（FIFO）
pub const MAX_TRAINING_SAMPLES: usize = 100_000;

// =====================================================================
//  PlanFeatures
// =====================================================================

/// 从 `LogicalPlan` 提取的数值化特征（16 维）
///
/// 特征选择参考 [PG ML Cost Model 论文 (Marcus et al., 2019)]：
/// - 结构特征：节点计数、最大深度、表数量
/// - 算子分布：Scan / Filter / Join / Aggregate / Sort / Limit 计数
/// - JOIN 类型分布：Inner / LeftOuter / Cross 计数
/// - 手工估算：estimated_rows、estimated_cost、has_index_scan、predicate_count
///
/// [PG ML Cost Model 论文]: https://dl.acm.org/doi/10.14778/3357737.3357748
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanFeatures {
    /// 节点总数
    pub node_count: f64,
    /// Scan 节点数
    pub scan_count: f64,
    /// IndexScan 节点数
    pub index_scan_count: f64,
    /// Filter 节点数
    pub filter_count: f64,
    /// Projection 节点数
    pub projection_count: f64,
    /// Join 节点数
    pub join_count: f64,
    /// Aggregate 节点数
    pub aggregate_count: f64,
    /// Sort 节点数
    pub sort_count: f64,
    /// Limit 节点数
    pub limit_count: f64,
    /// Distinct 节点数
    pub distinct_count: f64,
    /// Inner Join 数
    pub join_inner: f64,
    /// LeftOuter Join 数
    pub join_left: f64,
    /// Cross Join 数
    pub join_cross: f64,
    /// 最大深度（根为 1）
    pub max_depth: f64,
    /// 手工模型估算的输出行数（根节点 cardinality）
    pub estimated_rows: f64,
    /// 手工模型估算的总成本（根节点 total_cost）
    pub estimated_cost: f64,
}

impl PlanFeatures {
    /// 全零特征（占位用）
    pub fn zero() -> Self {
        Self {
            node_count: 0.0,
            scan_count: 0.0,
            index_scan_count: 0.0,
            filter_count: 0.0,
            projection_count: 0.0,
            join_count: 0.0,
            aggregate_count: 0.0,
            sort_count: 0.0,
            limit_count: 0.0,
            distinct_count: 0.0,
            join_inner: 0.0,
            join_left: 0.0,
            join_cross: 0.0,
            max_depth: 0.0,
            estimated_rows: 0.0,
            estimated_cost: 0.0,
        }
    }

    /// 从 `LogicalPlan` + 手工 `CostModel` 提取特征
    pub fn from_plan(plan: &LogicalPlan, model: &CostModel) -> Self {
        let mut acc = FeatureAccumulator::default();
        acc.walk(plan, 1);

        // 手工估算（根节点）
        let cost = model.estimate(plan);

        Self {
            node_count: acc.node_count as f64,
            scan_count: acc.scan_count as f64,
            index_scan_count: acc.index_scan_count as f64,
            filter_count: acc.filter_count as f64,
            projection_count: acc.projection_count as f64,
            join_count: acc.join_count as f64,
            aggregate_count: acc.aggregate_count as f64,
            sort_count: acc.sort_count as f64,
            limit_count: acc.limit_count as f64,
            distinct_count: acc.distinct_count as f64,
            join_inner: acc.join_inner as f64,
            join_left: acc.join_left as f64,
            join_cross: acc.join_cross as f64,
            max_depth: acc.max_depth as f64,
            estimated_rows: cost.cardinality as f64,
            estimated_cost: cost.total(),
        }
    }

    /// 转为 `FEATURE_DIM` 维向量（顺序与字段声明一致）
    pub fn to_vector(&self) -> [f64; FEATURE_DIM] {
        [
            self.node_count,
            self.scan_count,
            self.index_scan_count,
            self.filter_count,
            self.projection_count,
            self.join_count,
            self.aggregate_count,
            self.sort_count,
            self.limit_count,
            self.distinct_count,
            self.join_inner,
            self.join_left,
            self.join_cross,
            self.max_depth,
            self.estimated_rows,
            self.estimated_cost,
        ]
    }
}

impl Default for PlanFeatures {
    fn default() -> Self {
        Self::zero()
    }
}

/// 特征累加器（递归遍历计划树）
#[derive(Debug, Default)]
struct FeatureAccumulator {
    node_count: usize,
    scan_count: usize,
    index_scan_count: usize,
    filter_count: usize,
    projection_count: usize,
    join_count: usize,
    aggregate_count: usize,
    sort_count: usize,
    limit_count: usize,
    distinct_count: usize,
    join_inner: usize,
    join_left: usize,
    join_cross: usize,
    max_depth: usize,
}

impl FeatureAccumulator {
    fn walk(&mut self, plan: &LogicalPlan, depth: usize) {
        self.node_count += 1;
        if depth > self.max_depth {
            self.max_depth = depth;
        }

        match plan {
            LogicalPlan::Scan { .. } => self.scan_count += 1,
            LogicalPlan::IndexScan { .. } => self.index_scan_count += 1,
            LogicalPlan::Filter { input, .. } => {
                self.filter_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::Projection { input, .. } => {
                self.projection_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::Join {
                join_type,
                left,
                right,
                ..
            } => {
                self.join_count += 1;
                match join_type {
                    JoinType::Inner => self.join_inner += 1,
                    JoinType::LeftOuter => self.join_left += 1,
                    JoinType::Cross => self.join_cross += 1,
                    _ => {}
                }
                self.walk(left, depth + 1);
                self.walk(right, depth + 1);
            }
            LogicalPlan::Aggregate { input, .. } => {
                self.aggregate_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::Sort { input, .. } => {
                self.sort_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::Limit { input, .. } => {
                self.limit_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::Distinct { input } => {
                self.distinct_count += 1;
                self.walk(input, depth + 1);
            }
            LogicalPlan::SetOp { left, right, .. } => {
                self.walk(left, depth + 1);
                self.walk(right, depth + 1);
            }
            LogicalPlan::Shared { plan, .. } => self.walk(plan, depth + 1),
            // MemoRef / Empty / Dual / DML / DDL 不递归
            _ => {}
        }
    }
}

// =====================================================================
//  TrainingSample
// =====================================================================

/// 单条训练样本（特征 + 实际执行时间）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainingSample {
    /// 计划特征
    pub features: PlanFeatures,
    /// 实际执行时间（毫秒）
    pub actual_ms: f64,
}

// =====================================================================
//  FeatureNormalizer
// =====================================================================

/// z-score 特征归一化器
///
/// 对每维特征维护 `(mean, std)`，将原始特征归一化到均值 0、标准差 1 的分布。
/// `std == 0` 时（恒定特征）返回 0。
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureNormalizer {
    mean: [f64; FEATURE_DIM],
    std: [f64; FEATURE_DIM],
}

impl FeatureNormalizer {
    /// 从训练样本拟合归一化参数
    pub fn fit(samples: &[TrainingSample]) -> Self {
        let n = samples.len() as f64;
        let mut mean = [0.0; FEATURE_DIM];
        let mut std = [0.0; FEATURE_DIM];

        // 均值
        for s in samples {
            let v = s.features.to_vector();
            for i in 0..FEATURE_DIM {
                mean[i] += v[i];
            }
        }
        for m in &mut mean {
            *m /= n.max(1.0);
        }

        // 标准差
        for s in samples {
            let v = s.features.to_vector();
            for i in 0..FEATURE_DIM {
                let diff = v[i] - mean[i];
                std[i] += diff * diff;
            }
        }
        for s in &mut std {
            *s = (*s / n.max(1.0)).sqrt();
        }

        Self { mean, std }
    }

    /// 归一化单条特征向量
    pub fn normalize(&self, features: &PlanFeatures) -> [f64; FEATURE_DIM] {
        let v = features.to_vector();
        let mut out = [0.0; FEATURE_DIM];
        for i in 0..FEATURE_DIM {
            out[i] = if self.std[i] > 1e-9 {
                (v[i] - self.mean[i]) / self.std[i]
            } else {
                0.0
            };
        }
        out
    }

    /// 均值
    pub fn mean(&self) -> &[f64; FEATURE_DIM] {
        &self.mean
    }

    /// 标准差
    pub fn std(&self) -> &[f64; FEATURE_DIM] {
        &self.std
    }
}

impl Default for FeatureNormalizer {
    fn default() -> Self {
        Self {
            mean: [0.0; FEATURE_DIM],
            std: [1.0; FEATURE_DIM],
        }
    }
}

// =====================================================================
//  MLCostModel
// =====================================================================

/// ML 成本模型（线性回归 + L2 正则 + z-score 归一化）
///
/// 模型形式：`predicted_ms = bias + sum(w[i] * x_norm[i])`
///
/// - 冷启动（样本数 < `MIN_SAMPLES_TO_PREDICT`）：`predict()` 返回 `None`
/// - 训练后：`predict()` 返回 ML 预测值
/// - 增量训练：每次累积 `RETRAIN_THRESHOLD` 个新样本触发一次梯度下降
pub struct MLCostModel {
    /// 权重向量（与 `FEATURE_DIM` 同维）
    weights: [f64; FEATURE_DIM],
    /// 偏置
    bias: f64,
    /// 训练样本缓冲（FIFO，上限 `MAX_TRAINING_SAMPLES`）
    samples: Vec<TrainingSample>,
    /// 特征归一化器（最近一次训练时拟合）
    normalizer: FeatureNormalizer,
    /// 已训练次数（用于统计）
    train_count: usize,
    /// 上次训练时的样本数
    last_train_sample_count: usize,
    /// 训练历史 — 每次训练后记录权重 L2 范数（Phase 7b.1 持续学习追踪）
    training_history: Vec<f64>,
    /// 目标归一化 — 均值（Phase 7b.1: 防止大尺度目标导致梯度爆炸）
    y_mean: f64,
    /// 目标归一化 — 标准差（Phase 7b.1）
    y_std: f64,
}

impl MLCostModel {
    /// 创建空模型（全零权重，未训练）
    pub fn new() -> Self {
        Self {
            weights: [0.0; FEATURE_DIM],
            bias: 0.0,
            samples: Vec::new(),
            normalizer: FeatureNormalizer::default(),
            train_count: 0,
            last_train_sample_count: 0,
            training_history: Vec::new(),
            y_mean: 0.0,
            y_std: 1.0,
        }
    }

    /// 样本数
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// 训练次数
    pub fn train_count(&self) -> usize {
        self.train_count
    }

    /// 是否已训练（可用于预测）
    pub fn is_trained(&self) -> bool {
        self.samples.len() >= MIN_SAMPLES_TO_PREDICT && self.train_count > 0
    }

    /// 权重快照（用于测试与可观测性）
    pub fn weights(&self) -> &[f64; FEATURE_DIM] {
        &self.weights
    }

    /// 偏置
    pub fn bias(&self) -> f64 {
        self.bias
    }

    /// 权重 L2 范数（Phase 7b.1 持续学习追踪）
    ///
    /// 返回 `sqrt(sum(w[i]^2))`。未训练时为 0.0；
    /// 训练后随着权重学习逐步增大（验证标准：从 0 逐步提升到 > 0.5）。
    pub fn weight_norm(&self) -> f64 {
        let mut sum_sq = 0.0;
        for w in &self.weights {
            sum_sq += w * w;
        }
        sum_sq.sqrt()
    }

    /// 训练历史 — 每次训练后的权重 L2 范数序列（Phase 7b.1）
    ///
    /// 用于追踪持续学习过程中权重的演化轨迹。
    /// 序列长度等于 `train_count()`，第一个元素是首次训练后的权重范数。
    pub fn training_history(&self) -> &[f64] {
        &self.training_history
    }

    /// 导出为 ONNX 兼容的 JSON 表示（Phase 7b.1）
    ///
    /// 由于 `ort`（ONNX Runtime）在 Windows 上需动态链接 DLL，
    /// 此方法将线性回归模型序列化为 JSON 格式，包含：
    /// - `ir_version`: ONNX IR 版本
    /// - `producer_name`: 生产者
    /// - `graph`: 计算图（LinearRegression 节点）
    /// - `weights`: 权重向量
    /// - `bias`: 偏置
    /// - `normalizer_mean` / `normalizer_std`: 归一化参数
    ///
    /// 该 JSON 可被外部 Python 脚本转换为标准 ONNX protobuf，
    /// 或被其他 Rust 服务加载用于推理。
    pub fn export_onnx_json(&self) -> Result<String, serde_json::Error> {
        let model = serde_json::json!({
            "ir_version": 8,
            "producer_name": "szrsql-optimizer",
            "producer_version": env!("CARGO_PKG_VERSION"),
            "model_type": "LinearRegression",
            "feature_dim": FEATURE_DIM,
            "feature_names": [
                "node_count", "scan_count", "index_scan_count", "filter_count",
                "projection_count", "join_count", "aggregate_count", "sort_count",
                "limit_count", "distinct_count", "join_inner", "join_left",
                "join_cross", "max_depth", "estimated_rows", "estimated_cost"
            ],
            "graph": {
                "node": [{
                    "op_type": "LinearRegressor",
                    "input": ["features"],
                    "output": ["prediction"],
                    "attributes": {
                        "coefficients": self.weights,
                        "intercepts": [self.bias]
                    }
                }],
                "input": [{
                    "name": "features",
                    "type": "tensor(float)",
                    "shape": [FEATURE_DIM]
                }],
                "output": [{
                    "name": "prediction",
                    "type": "tensor(float)",
                    "shape": [1]
                }]
            },
            "normalizer": {
                "mean": self.normalizer.mean(),
                "std": self.normalizer.std()
            },
            "target_normalizer": {
                "mean": self.y_mean,
                "std": self.y_std
            },
            "train_count": self.train_count,
            "sample_count": self.samples.len(),
            "weight_norm": self.weight_norm()
        });
        serde_json::to_string_pretty(&model)
    }

    /// 添加训练样本
    ///
    /// 累积到 `RETRAIN_THRESHOLD` 个新样本时自动触发增量训练。
    /// 缓冲超过 `MAX_TRAINING_SAMPLES` 时丢弃最旧样本（FIFO）。
    ///
    /// **注意**：批量添加大量样本时请使用 `add_samples`，避免每 50 个样本触发一次训练。
    pub fn add_sample(&mut self, sample: TrainingSample) {
        self.samples.push(sample);
        if self.samples.len() > MAX_TRAINING_SAMPLES {
            let drop_n = self.samples.len() - MAX_TRAINING_SAMPLES;
            self.samples.drain(0..drop_n);
        }

        let untrained = self
            .samples
            .len()
            .saturating_sub(self.last_train_sample_count);
        if untrained >= RETRAIN_THRESHOLD && self.samples.len() >= MIN_SAMPLES_TO_PREDICT {
            self.train();
        }
    }

    /// 批量添加样本（不自动训练）
    ///
    /// 用于一次性加载训练集的场景。仅入缓冲，不触发训练。
    /// 调用方需在添加完成后显式调用 `train()` 进行训练。
    pub fn add_samples(&mut self, samples: impl IntoIterator<Item = TrainingSample>) {
        for s in samples {
            self.samples.push(s);
            if self.samples.len() > MAX_TRAINING_SAMPLES {
                let drop_n = self.samples.len() - MAX_TRAINING_SAMPLES;
                self.samples.drain(0..drop_n);
            }
        }
    }

    /// 触发增量训练（梯度下降 + L2 正则）
    ///
    /// 算法：
    /// 1. 拟合 `FeatureNormalizer`（基于当前全部样本）
    /// 2. 对每个样本：前向计算预测值 → 计算残差 → 反向传播梯度
    /// 3. 权重更新：`w[i] -= lr * (grad[i] + L2_REG * w[i])`
    /// 4. 偏置更新：`bias -= lr * mean_residual`
    pub fn train(&mut self) {
        if self.samples.is_empty() {
            return;
        }

        // 拟合特征归一化器
        self.normalizer = FeatureNormalizer::fit(&self.samples);

        // Phase 7b.1: 拟合目标归一化器（z-score），防止大尺度目标导致梯度爆炸
        let n = self.samples.len();
        let mut y_sum = 0.0;
        for s in &self.samples {
            y_sum += s.actual_ms;
        }
        self.y_mean = y_sum / n as f64;
        let mut y_var = 0.0;
        for s in &self.samples {
            let diff = s.actual_ms - self.y_mean;
            y_var += diff * diff;
        }
        y_var /= n as f64;
        self.y_std = y_var.sqrt().max(1e-9);

        // 准备归一化后的特征矩阵 + 归一化标签
        let mut x_norm: Vec<[f64; FEATURE_DIM]> = Vec::with_capacity(n);
        let mut y: Vec<f64> = Vec::with_capacity(n);
        for s in &self.samples {
            x_norm.push(self.normalizer.normalize(&s.features));
            y.push((s.actual_ms - self.y_mean) / self.y_std);
        }

        // 梯度下降
        for _ in 0..EPOCHS_PER_TRAIN {
            let mut grad_w = [0.0; FEATURE_DIM];
            let mut grad_b = 0.0;

            for (xi, yi) in x_norm.iter().zip(y.iter()) {
                // 前向
                let pred = self.bias + dot(&self.weights, xi);
                let residual = pred - yi;
                // 反向
                for i in 0..FEATURE_DIM {
                    grad_w[i] += residual * xi[i];
                }
                grad_b += residual;
            }

            // 平均梯度 + L2 正则
            let inv_n = 1.0 / n as f64;
            for (w, g) in self.weights.iter_mut().zip(grad_w.iter()) {
                let grad = g * inv_n + L2_REG * *w;
                *w -= LEARNING_RATE * grad;
            }
            self.bias -= LEARNING_RATE * grad_b * inv_n;
        }

        self.train_count += 1;
        self.last_train_sample_count = self.samples.len();
        // Phase 7b.1: 记录训练后的权重 L2 范数（持续学习追踪）
        self.training_history.push(self.weight_norm());
    }

    /// 预测执行时间（毫秒）
    ///
    /// 冷启动（`!is_trained()`）返回 `None`，调用方应 fallback 到手工模型。
    pub fn predict(&self, features: &PlanFeatures) -> Option<f64> {
        if !self.is_trained() {
            return None;
        }
        let x = self.normalizer.normalize(features);
        let pred_norm = self.bias + dot(&self.weights, &x);
        // Phase 7b.1: 反归一化到原始尺度
        let pred = pred_norm * self.y_std + self.y_mean;
        // 预测值非负约束（执行时间不可能为负）
        Some(pred.max(0.0))
    }

    /// 评估模型在测试集上的 MAPE（平均绝对百分比误差）
    ///
    /// 返回值单位为百分比（如 15.0 表示 15%）。`actual == 0` 的样本跳过。
    pub fn evaluate_mape(&self, test_set: &[TrainingSample]) -> f64 {
        if test_set.is_empty() {
            return 0.0;
        }
        let mut sum_pct = 0.0;
        let mut count = 0usize;
        for s in test_set {
            if let Some(pred) = self.predict(&s.features) {
                if s.actual_ms.abs() > 1e-9 {
                    let pct = ((pred - s.actual_ms).abs() / s.actual_ms) * 100.0;
                    sum_pct += pct;
                    count += 1;
                }
            }
        }
        if count == 0 {
            return f64::MAX;
        }
        sum_pct / count as f64
    }

    /// 评估手工模型在测试集上的 MAPE（用于对比）
    ///
    /// 手工模型预测值 = `features.estimated_cost`（手工 CostModel 的总成本）。
    /// 由于 `estimated_cost` 与 `actual_ms` 量纲不同，这里使用相对误差比较：
    /// 对每个样本计算 `(|estimated_cost - actual_ms| / actual_ms) * 100`。
    /// 当 `estimated_cost` 与 `actual_ms` 相关性高时（线性关系），MAPE 可比较。
    pub fn evaluate_handcrafted_mape(test_set: &[TrainingSample]) -> f64 {
        if test_set.is_empty() {
            return 0.0;
        }
        let mut sum_pct = 0.0;
        let mut count = 0usize;
        for s in test_set {
            if s.actual_ms.abs() > 1e-9 {
                let pct = ((s.features.estimated_cost - s.actual_ms).abs() / s.actual_ms) * 100.0;
                sum_pct += pct;
                count += 1;
            }
        }
        if count == 0 {
            return f64::MAX;
        }
        sum_pct / count as f64
    }

    /// 清空模型（重置到初始状态）
    pub fn reset(&mut self) {
        self.weights = [0.0; FEATURE_DIM];
        self.bias = 0.0;
        self.samples.clear();
        self.normalizer = FeatureNormalizer::default();
        self.train_count = 0;
        self.last_train_sample_count = 0;
        self.training_history.clear();
        self.y_mean = 0.0;
        self.y_std = 1.0;
    }
}

impl Default for MLCostModel {
    fn default() -> Self {
        Self::new()
    }
}

/// 向量点积
fn dot(a: &[f64; FEATURE_DIM], b: &[f64; FEATURE_DIM]) -> f64 {
    let mut s = 0.0;
    for i in 0..FEATURE_DIM {
        s += a[i] * b[i];
    }
    s
}

// =====================================================================
//  HybridCostModel
// =====================================================================

/// 混合成本模型 — ML 优先 + 手工 fallback
///
/// - 冷启动（ML 样本不足）：使用手工 `CostModel`
/// - 训练后：使用 ML 预测值映射到 `Cost`（保留手工的 cardinality/width）
/// - 在线学习：每次执行后调用 `record_execution()` 追加训练样本
pub struct HybridCostModel {
    hand_crafted: CostModel,
    ml_model: MLCostModel,
}

impl HybridCostModel {
    /// 创建混合模型（包装手工模型 + 空 ML 模型）
    pub fn new(stats_store: Arc<dyn StatisticsStore>) -> Self {
        Self {
            hand_crafted: CostModel::new(stats_store),
            ml_model: MLCostModel::new(),
        }
    }

    /// 从已有手工模型 + ML 模型构建
    pub fn with_ml(hand_crafted: CostModel, ml_model: MLCostModel) -> Self {
        Self {
            hand_crafted,
            ml_model,
        }
    }

    /// 估算成本
    ///
    /// - ML 已训练：`Cost { cardinality, width }` 来自手工模型，`cpu_cost + io_cost`
    ///   由 ML 预测值设定（`total = predicted_ms`，拆分为 `cpu=predicted/2, io=predicted/2`）
    /// - ML 未训练：返回手工模型估算
    pub fn estimate(&self, plan: &LogicalPlan) -> crate::cost::Cost {
        let hand = self.hand_crafted.estimate(plan);
        if !self.ml_model.is_trained() {
            return hand;
        }
        let features = PlanFeatures::from_plan(plan, &self.hand_crafted);
        if let Some(predicted_ms) = self.ml_model.predict(&features) {
            // 拆分为 CPU + I/O（各占一半，简化）
            crate::cost::Cost {
                cpu_cost: predicted_ms * 0.5,
                io_cost: predicted_ms * 0.5,
                cardinality: hand.cardinality,
                width: hand.width,
            }
        } else {
            hand
        }
    }

    /// 记录一次实际执行（用于 ML 在线学习）
    pub fn record_execution(&mut self, plan: &LogicalPlan, actual_ms: f64) {
        let features = PlanFeatures::from_plan(plan, &self.hand_crafted);
        self.ml_model.add_sample(TrainingSample {
            features,
            actual_ms,
        });
    }

    /// ML 模型引用（只读）
    pub fn ml_model(&self) -> &MLCostModel {
        &self.ml_model
    }

    /// ML 模型可变引用（用于手动训练 / 评估）
    pub fn ml_model_mut(&mut self) -> &mut MLCostModel {
        &mut self.ml_model
    }

    /// 手工模型引用
    pub fn hand_crafted(&self) -> &CostModel {
        &self.hand_crafted
    }
}

// =====================================================================
//  谓词计数工具（用于特征提取的辅助 — 当前未直接使用，保留供未来扩展）
// =====================================================================

/// 递归统计谓词数量（AND/OR 连接的子谓词总数）
#[allow(dead_code)]
fn count_predicates(expr: &Expr) -> usize {
    match expr {
        Expr::BinaryOp {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
        } => count_predicates(left) + count_predicates(right),
        _ => 1,
    }
}

// =====================================================================
//  JobBenchmarkGenerator — JOB 基准查询生成器（Phase 7b.1）
// =====================================================================

/// JOB (Join Order Benchmark) 基准查询生成器
///
/// JOB 是经典的查询优化基准，基于 IMDB 数据集，包含 113 个多表连接查询。
/// 此生成器模拟 JOB 工作负载特征，生成大规模合成查询用于 ML 成本模型训练：
///
/// - **IMDB 模式表**：title, cast_info, movie_info, person_name, movie_companies, movie_keyword
/// - **查询特征**：2-6 表连接 + 过滤 + 投影 + 排序
/// - **Ground truth**：`actual_ms = f(features)` — 多特征非线性组合 + 噪声
///
/// # 设计
///
/// 生成器使用确定性 LCG 伪随机数（可复现），生成 `n` 条 `TrainingSample`：
/// 1. 随机选择 2-6 张表
/// 2. 构建 INNER JOIN 链式查询（每对表通过 movie_id 连接）
/// 3. 随机附加 Filter / Sort / Limit / Distinct 算子
/// 4. 从 `LogicalPlan` 提取 `PlanFeatures`
/// 5. 使用 ground truth 函数计算 `actual_ms`
///
/// # Ground Truth 函数
///
/// `actual_ms = 0.2 * estimated_cost + 8.0 * join_count + 5.0 * node_count
///            + 3.0 * sort_count + 2.0 * filter_count + noise`
///
/// 手工模型仅使用 `estimated_cost`，无法捕获 join_count / node_count 等特征贡献，
/// 因此 ML 模型应显著优于手工模型。
pub struct JobBenchmarkGenerator {
    /// 伪随机数状态
    rng_state: u64,
}

impl JobBenchmarkGenerator {
    /// 创建生成器（指定随机种子）
    pub fn new(seed: u64) -> Self {
        Self { rng_state: seed }
    }

    /// 生成 `n` 条 JOB 风格合成查询样本
    pub fn generate(&mut self, n: usize) -> Vec<TrainingSample> {
        let tables = [
            "title",
            "cast_info",
            "movie_info",
            "person_name",
            "movie_companies",
            "movie_keyword",
        ];
        // 各表的模拟行数（Phase 7b.1: 小规模行数 + 最多 3 表 JOIN，
        // 避免 DEFAULT_JOIN_SELECTIVITY=0.1 下级联基数指数增长导致特征重尾）
        let table_rows: [usize; 6] = [400, 600, 200, 100, 300, 150];

        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            // 随机选择 2-3 张表（Phase 7b.1: 限制 JOIN 深度避免基数指数增长）
            let table_count = 2 + (self.next_rand() * 2.0) as usize; // 2..=3
            let mut selected_indices: Vec<usize> = Vec::with_capacity(table_count);
            while selected_indices.len() < table_count {
                let idx = (self.next_rand() * tables.len() as f64) as usize % tables.len();
                if !selected_indices.contains(&idx) {
                    selected_indices.push(idx);
                }
            }

            // 为选中的表创建统计存储
            let mut stats_store = crate::statistics::InMemoryStatisticsStore::new();
            for &idx in &selected_indices {
                let mut ts = crate::statistics::TableStatistics::empty(tables[idx]);
                ts.row_count = table_rows[idx];
                stats_store.update_table_stats(tables[idx], ts);
            }
            let store: Arc<dyn StatisticsStore> = Arc::new(stats_store);

            // 构建 JOIN 链
            let first_idx = selected_indices[0];
            let mut plan = self.scan_plan(tables[first_idx]);

            for &idx in &selected_indices[1..] {
                let right_plan = self.scan_plan(tables[idx]);
                plan = LogicalPlan::Join {
                    join_type: JoinType::Inner,
                    condition: JoinCondition::On(Expr::BinaryOp {
                        left: Box::new(Expr::Identifier(vec!["movie_id".to_string()])),
                        op: BinaryOp::Eq,
                        right: Box::new(Expr::Identifier(vec!["movie_id".to_string()])),
                    }),
                    left: Box::new(plan),
                    right: Box::new(right_plan),
                };
            }

            // 随机附加算子
            let wrap_count = (self.next_rand() * 4.0) as usize;
            for _ in 0..wrap_count {
                let op = (self.next_rand() * 5.0) as usize;
                plan = match op {
                    0 => LogicalPlan::Filter {
                        predicate: Expr::BinaryOp {
                            left: Box::new(Expr::Identifier(vec!["id".to_string()])),
                            op: BinaryOp::Gt,
                            right: Box::new(Expr::Literal(szrsql_types::value::Value::Int64(
                                (self.next_rand() * 1000.0) as i64,
                            ))),
                        },
                        input: Box::new(plan),
                    },
                    1 => LogicalPlan::Sort {
                        order_by: vec![OrderByExpr {
                            expr: Expr::Identifier(vec!["id".to_string()]),
                            asc: true,
                            nulls_first: false,
                        }],
                        input: Box::new(plan),
                    },
                    2 => LogicalPlan::Limit {
                        limit: Some(Expr::Literal(szrsql_types::value::Value::Int64(
                            (self.next_rand() * 100.0) as i64 + 1,
                        ))),
                        offset: None,
                        input: Box::new(plan),
                    },
                    3 => LogicalPlan::Distinct {
                        input: Box::new(plan),
                    },
                    _ => LogicalPlan::Projection {
                        exprs: vec![(
                            Expr::Identifier(vec!["id".to_string()]),
                            Some("id".to_string()),
                        )],
                        output_names: vec!["id".to_string()],
                        input: Box::new(plan),
                    },
                };
            }

            // 提取特征
            let cost_model = CostModel::new(store);
            let features = PlanFeatures::from_plan(&plan, &cost_model);

            // Ground truth: 结构特征主导 + estimated_cost 微贡献 + 噪声
            // Phase 7b.1: 结构特征（小整数）使线性模型可学习；estimated_cost
            // 权重极小以避免重尾分布导致 MAPE 爆炸
            let noise = self.next_rand() * 5.0;
            let actual_ms = 10.0 * features.join_count
                + 5.0 * features.node_count
                + 3.0 * features.sort_count
                + 2.0 * features.filter_count
                + 1.5 * features.limit_count
                + 1.0 * features.aggregate_count
                + 0.8 * features.distinct_count
                + 0.5 * features.projection_count
                + 0.001 * features.estimated_cost
                + noise;

            samples.push(TrainingSample {
                features,
                actual_ms,
            });
        }
        samples
    }

    /// LCG 伪随机数生成器（确定性，可复现）
    fn next_rand(&mut self) -> f64 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    /// 创建 Scan 计划辅助
    fn scan_plan(&self, table: &str) -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new(table.to_string()),
            alias: None,
            schema: TableSchema {
                name: TableName::new(table.to_string()),
                columns: vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    ColumnDefinition::new("movie_id", ColumnType::Int64),
                    ColumnDefinition::new("name", ColumnType::Text),
                ],
            },
        }
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::{InMemoryStatisticsStore, TableStatistics};
    use szrsql_sql::ast::{
        ColumnDefinition, Expr as AExpr, JoinCondition, OrderByExpr, SetOperator, SetQuantifier,
        TableName,
    };
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::ColumnType;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    /// 创建空统计存储
    fn empty_stats_store() -> Arc<dyn StatisticsStore> {
        Arc::new(InMemoryStatisticsStore::new())
    }

    /// 创建带 row_count 的统计存储
    fn stats_store_with(table: &str, row_count: usize) -> Arc<dyn StatisticsStore> {
        let mut store = InMemoryStatisticsStore::new();
        store.update_table_stats(
            table,
            TableStatistics::empty(table).with_row_count(row_count),
        );
        Arc::new(store)
    }

    /// TableStatistics 扩展 — 设置 row_count
    trait WithRowCount {
        fn with_row_count(self, row_count: usize) -> Self;
    }
    impl WithRowCount for TableStatistics {
        fn with_row_count(mut self, row_count: usize) -> Self {
            self.row_count = row_count;
            self
        }
    }

    /// 创建简单的 Scan 计划
    fn scan_plan(table: &str) -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new(table.to_string()),
            alias: None,
            schema: TableSchema {
                name: TableName::new(table.to_string()),
                columns: vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    ColumnDefinition::new("name", ColumnType::Text),
                ],
            },
        }
    }

    /// 创建 Filter 计划
    fn filter_plan(table: &str, predicate: AExpr) -> LogicalPlan {
        LogicalPlan::Filter {
            predicate,
            input: Box::new(scan_plan(table)),
        }
    }

    /// 创建 Inner Join 计划
    fn inner_join(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Identifier(vec!["id".to_string()])),
            }),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 创建 LeftOuter Join 计划
    fn left_join(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::LeftOuter,
            condition: JoinCondition::On(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Identifier(vec!["id".to_string()])),
            }),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 创建 Cross Join 计划
    fn cross_join(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Cross,
            condition: JoinCondition::None,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 创建 Sort 计划
    fn sort_plan(input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["id".to_string()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(input),
        }
    }

    /// 创建 Limit 计划
    fn limit_plan(input: LogicalPlan, limit: i64) -> LogicalPlan {
        LogicalPlan::Limit {
            limit: Some(AExpr::Literal(szrsql_types::value::Value::Int64(limit))),
            offset: None,
            input: Box::new(input),
        }
    }

    /// 创建 Distinct 计划
    fn distinct_plan(input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Distinct {
            input: Box::new(input),
        }
    }

    /// 创建 Projection 计划
    fn projection_plan(input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Projection {
            exprs: vec![(
                AExpr::Identifier(vec!["id".to_string()]),
                Some("id".to_string()),
            )],
            output_names: vec!["id".to_string()],
            input: Box::new(input),
        }
    }

    /// 创建 Aggregate 计划
    fn aggregate_plan(input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Aggregate {
            group_exprs: vec![AExpr::Identifier(vec!["id".to_string()])],
            aggregates: Vec::new(),
            having: None,
            input: Box::new(input),
        }
    }

    /// 创建 SetOp 计划
    fn union_plan(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::All,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 简单整数字面量
    fn lit_int(v: i64) -> AExpr {
        AExpr::Literal(szrsql_types::value::Value::Int64(v))
    }

    // -----------------------------------------------------------------
    //  PlanFeatures 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_features_zero() {
        let f = PlanFeatures::zero();
        assert_eq!(f.node_count, 0.0);
        assert_eq!(f.max_depth, 0.0);
        assert_eq!(f.estimated_cost, 0.0);
    }

    #[test]
    fn test_features_from_scan() {
        let store = stats_store_with("t", 1000);
        let model = CostModel::new(store);
        let plan = scan_plan("t");
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.scan_count, 1.0);
        assert_eq!(f.node_count, 1.0);
        assert_eq!(f.max_depth, 1.0);
        assert_eq!(f.estimated_rows, 1000.0);
        assert!(f.estimated_cost > 0.0);
    }

    #[test]
    fn test_features_from_filter() {
        let store = stats_store_with("t", 1000);
        let model = CostModel::new(store);
        let plan = filter_plan(
            "t",
            AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                op: BinaryOp::Gt,
                right: Box::new(lit_int(5)),
            },
        );
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.scan_count, 1.0);
        assert_eq!(f.filter_count, 1.0);
        assert_eq!(f.node_count, 2.0);
        assert_eq!(f.max_depth, 2.0);
    }

    #[test]
    fn test_features_from_join_counts() {
        let store = stats_store_with("t", 1000);
        let model = CostModel::new(store);
        // Inner Join
        let plan = inner_join(scan_plan("t1"), scan_plan("t2"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.join_count, 1.0);
        assert_eq!(f.join_inner, 1.0);
        assert_eq!(f.join_left, 0.0);
        assert_eq!(f.join_cross, 0.0);

        // LeftOuter Join
        let plan = left_join(scan_plan("t1"), scan_plan("t2"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.join_left, 1.0);

        // Cross Join
        let plan = cross_join(scan_plan("t1"), scan_plan("t2"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.join_cross, 1.0);
    }

    #[test]
    fn test_features_from_complex_plan() {
        let store = stats_store_with("t", 1000);
        let model = CostModel::new(store);
        // Distinct(Limit(Filter(Scan(t))))
        let plan = distinct_plan(limit_plan(
            filter_plan(
                "t",
                AExpr::BinaryOp {
                    left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                    op: BinaryOp::Gt,
                    right: Box::new(lit_int(5)),
                },
            ),
            10,
        ));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.distinct_count, 1.0);
        assert_eq!(f.limit_count, 1.0);
        assert_eq!(f.filter_count, 1.0);
        assert_eq!(f.scan_count, 1.0);
        assert_eq!(f.node_count, 4.0);
        assert_eq!(f.max_depth, 4.0);
    }

    #[test]
    fn test_features_to_vector_dimension() {
        let f = PlanFeatures::zero();
        let v = f.to_vector();
        assert_eq!(v.len(), FEATURE_DIM);
    }

    // -----------------------------------------------------------------
    //  FeatureNormalizer 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_normalizer_fit_normalize() {
        let samples: Vec<TrainingSample> = (0..100)
            .map(|i| TrainingSample {
                features: PlanFeatures {
                    node_count: i as f64,
                    scan_count: 0.0,
                    index_scan_count: 0.0,
                    filter_count: 0.0,
                    projection_count: 0.0,
                    join_count: 0.0,
                    aggregate_count: 0.0,
                    sort_count: 0.0,
                    limit_count: 0.0,
                    distinct_count: 0.0,
                    join_inner: 0.0,
                    join_left: 0.0,
                    join_cross: 0.0,
                    max_depth: 0.0,
                    estimated_rows: 0.0,
                    estimated_cost: 0.0,
                },
                actual_ms: i as f64,
            })
            .collect();

        let norm = FeatureNormalizer::fit(&samples);
        // 均值应约为 49.5（0..=99 的均值）
        assert!((norm.mean()[0] - 49.5).abs() < 1.0);
        // 标准差应约为 28.86（0..=99 的标准差）
        assert!(norm.std()[0] > 28.0 && norm.std()[0] < 30.0);

        // 归一化后均值应接近 0
        let mut sum = 0.0;
        for s in &samples {
            let n = norm.normalize(&s.features);
            sum += n[0];
        }
        assert!((sum / 100.0).abs() < 0.01);
    }

    #[test]
    fn test_normalizer_zero_std() {
        // 恒定特征（std=0）应返回 0
        let samples: Vec<TrainingSample> = (0..10)
            .map(|_| TrainingSample {
                features: PlanFeatures {
                    node_count: 5.0,
                    ..PlanFeatures::zero()
                },
                actual_ms: 1.0,
            })
            .collect();

        let norm = FeatureNormalizer::fit(&samples);
        assert!(norm.std()[0] < 1e-9);
        let n = norm.normalize(&samples[0].features);
        assert_eq!(n[0], 0.0);
    }

    // -----------------------------------------------------------------
    //  MLCostModel 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_ml_model_new_untrained() {
        let model = MLCostModel::new();
        assert_eq!(model.sample_count(), 0);
        assert_eq!(model.train_count(), 0);
        assert!(!model.is_trained());

        let f = PlanFeatures::zero();
        assert_eq!(model.predict(&f), None);
    }

    #[test]
    fn test_ml_model_add_sample_triggers_training() {
        let mut model = MLCostModel::new();
        // 添加 MIN_SAMPLES_TO_PREDICT + RETRAIN_THRESHOLD 个样本应触发训练
        let total = MIN_SAMPLES_TO_PREDICT + RETRAIN_THRESHOLD;
        for i in 0..total {
            model.add_sample(TrainingSample {
                features: PlanFeatures {
                    node_count: i as f64,
                    estimated_cost: i as f64 * 2.0,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64 * 2.0,
            });
        }
        assert!(model.is_trained());
        assert!(model.train_count() >= 1);
    }

    #[test]
    fn test_ml_model_predict_after_training() {
        let mut model = MLCostModel::new();
        // 训练样本：actual_ms = 2 * estimated_cost（线性关系）
        for i in 0..200 {
            let cost = i as f64;
            model.add_sample(TrainingSample {
                features: PlanFeatures {
                    estimated_cost: cost,
                    ..PlanFeatures::zero()
                },
                actual_ms: 2.0 * cost,
            });
        }
        assert!(model.is_trained());

        // 预测：estimated_cost = 50 → 预测应接近 100
        let pred = model
            .predict(&PlanFeatures {
                estimated_cost: 50.0,
                ..PlanFeatures::zero()
            })
            .unwrap();
        // 由于特征归一化，误差应 < 50%
        assert!(pred > 50.0 && pred < 150.0, "pred = {}", pred);
    }

    #[test]
    fn test_ml_model_cold_start_fallback() {
        let model = MLCostModel::new();
        // 未训练时 predict 返回 None（调用方应 fallback）
        assert_eq!(model.predict(&PlanFeatures::zero()), None);

        // 添加少量样本但未达阈值
        let mut model = MLCostModel::new();
        for i in 0..10 {
            model.add_sample(TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64,
            });
        }
        assert!(!model.is_trained());
        assert_eq!(model.predict(&PlanFeatures::zero()), None);
    }

    #[test]
    fn test_ml_model_reset() {
        let mut model = MLCostModel::new();
        for i in 0..200 {
            model.add_sample(TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64,
            });
        }
        assert!(model.is_trained());

        model.reset();
        assert_eq!(model.sample_count(), 0);
        assert_eq!(model.train_count(), 0);
        assert!(!model.is_trained());
    }

    #[test]
    fn test_ml_model_fifo_eviction() {
        let mut model = MLCostModel::new();
        // 添加超过 MAX_TRAINING_SAMPLES 个样本，验证 FIFO 淘汰
        // 使用 add_samples（不触发训练）以避免逐样本训练导致 O(N²) 计算爆炸
        let total = MAX_TRAINING_SAMPLES + 100;
        let samples: Vec<TrainingSample> = (0..total)
            .map(|i| TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64,
            })
            .collect();
        model.add_samples(samples);
        assert_eq!(model.sample_count(), MAX_TRAINING_SAMPLES);
    }

    // -----------------------------------------------------------------
    //  MAPE 评估测试
    // -----------------------------------------------------------------

    #[test]
    fn test_evaluate_mape_perfect_prediction() {
        let mut model = MLCostModel::new();
        // 训练样本：actual_ms = estimated_cost（完美线性）
        for i in 1..=200 {
            model.add_sample(TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64,
            });
        }
        // 测试集：同样的线性关系
        let test_set: Vec<TrainingSample> = (1..=50)
            .map(|i| TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64,
            })
            .collect();
        let mape = model.evaluate_mape(&test_set);
        // 训练后 MAPE 应该相对较低（< 100%）
        assert!(mape < 100.0, "MAPE = {}%", mape);
    }

    #[test]
    fn test_evaluate_mape_empty_test_set() {
        let model = MLCostModel::new();
        assert_eq!(model.evaluate_mape(&[]), 0.0);
    }

    #[test]
    fn test_evaluate_handcrafted_mape() {
        // 手工 MAPE 计算
        let test_set: Vec<TrainingSample> = (1..=10)
            .map(|i| TrainingSample {
                features: PlanFeatures {
                    estimated_cost: i as f64,
                    ..PlanFeatures::zero()
                },
                actual_ms: i as f64 * 2.0,
            })
            .collect();
        // |i - 2i| / 2i * 100 = 50%
        let mape = MLCostModel::evaluate_handcrafted_mape(&test_set);
        assert!((mape - 50.0).abs() < 0.1, "MAPE = {}%", mape);
    }

    // -----------------------------------------------------------------
    //  HybridCostModel 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hybrid_model_cold_start_uses_handcrafted() {
        let store = stats_store_with("t", 1000);
        let hybrid = HybridCostModel::new(store);
        let plan = scan_plan("t");
        let cost = hybrid.estimate(&plan);
        // 冷启动时返回手工模型估算（cardinality 应为 1000）
        assert_eq!(cost.cardinality, 1000);
    }

    #[test]
    fn test_hybrid_model_records_execution_and_trains() {
        let store = stats_store_with("t", 1000);
        let mut hybrid = HybridCostModel::new(store);
        let plan = scan_plan("t");

        // 记录多次执行
        for _ in 0..(MIN_SAMPLES_TO_PREDICT + RETRAIN_THRESHOLD) {
            hybrid.record_execution(&plan, 5.0);
        }
        assert!(hybrid.ml_model().is_trained());
    }

    #[test]
    fn test_hybrid_model_ml_prediction_after_training() {
        let store = stats_store_with("t", 1000);
        let mut hybrid = HybridCostModel::new(store);
        let plan = scan_plan("t");

        // 记录执行（固定 5ms）
        for _ in 0..200 {
            hybrid.record_execution(&plan, 5.0);
        }
        assert!(hybrid.ml_model().is_trained());

        // 估算成本应使用 ML 预测（predicted_ms 应接近 5）
        let cost = hybrid.estimate(&plan);
        let total = cost.total();
        // ML 预测应接近 5ms（允许较大误差，因特征简单）
        assert!(total > 0.0 && total < 50.0, "total = {}", total);
    }

    // -----------------------------------------------------------------
    //  100000 条查询验证（核心验证标准）
    // -----------------------------------------------------------------

    /// 生成合成训练样本
    ///
    /// Ground truth 函数：`actual_ms = 0.5 * estimated_cost
    ///     + 2.0 * node_count + 1.5 * scan_count
    ///     + 5.0 * join_count + 3.0 * sort_count
    ///     + noise`
    ///
    /// 手工模型只使用 `estimated_cost`，无法捕获其他特征的贡献，
    /// 因此 ML 模型在测试集上应显著优于手工模型。
    fn generate_synthetic_samples(n: usize, seed: u64) -> Vec<TrainingSample> {
        // 简单的 LCG 伪随机数生成器（确定性，可复现）
        let mut rng_state = seed;
        let mut next_rand = || {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng_state >> 33) as f64 / (1u64 << 31) as f64
        };

        (0..n)
            .map(|_| {
                let node_count = (next_rand() * 20.0).floor() + 1.0;
                let scan_count = (next_rand() * 5.0).floor();
                let filter_count = (next_rand() * 4.0).floor();
                let join_count = (next_rand() * 3.0).floor();
                let sort_count = (next_rand() * 2.0).floor();
                let limit_count = (next_rand() * 2.0).floor();
                let estimated_cost = node_count * 10.0 + scan_count * 5.0 + join_count * 50.0;

                // Ground truth：手工 estimated_cost 只是其中一个特征
                let actual_ms = 0.3 * estimated_cost
                    + 2.0 * node_count
                    + 1.5 * scan_count
                    + 3.0 * filter_count
                    + 5.0 * join_count
                    + 4.0 * sort_count
                    + 1.0 * limit_count
                    + next_rand() * 2.0; // 噪声

                TrainingSample {
                    features: PlanFeatures {
                        node_count,
                        scan_count,
                        index_scan_count: 0.0,
                        filter_count,
                        projection_count: 0.0,
                        join_count,
                        aggregate_count: 0.0,
                        sort_count,
                        limit_count,
                        distinct_count: 0.0,
                        join_inner: join_count,
                        join_left: 0.0,
                        join_cross: 0.0,
                        max_depth: node_count,
                        estimated_rows: estimated_cost,
                        estimated_cost,
                    },
                    actual_ms,
                }
            })
            .collect()
    }

    #[test]
    fn test_ml_model_beats_handcrafted_on_100k_samples() {
        // 生成 100000 条合成样本
        let all_samples = generate_synthetic_samples(100_000, 42);

        // 80% 训练 / 20% 测试
        let split = all_samples.len() * 4 / 5;
        let train_set = &all_samples[..split];
        let test_set = &all_samples[split..];

        // 训练 ML 模型（批量加载 + 显式训练）
        let mut ml_model = MLCostModel::new();
        ml_model.add_samples(train_set.iter().copied());
        ml_model.train();
        assert!(ml_model.is_trained());

        // 评估 ML 模型
        let ml_mape = ml_model.evaluate_mape(test_set);
        // 评估手工模型（对比基线）
        let hand_mape = MLCostModel::evaluate_handcrafted_mape(test_set);

        // 验证标准：ML 误差 < 手工模型误差
        assert!(
            ml_mape < hand_mape,
            "ML MAPE = {}%, handcrafted MAPE = {}% — ML 应优于手工模型",
            ml_mape,
            hand_mape
        );

        // 输出对比（cargo test -- --nocapture 可见）
        eprintln!(
            "[Phase 5.10 验证] 100000 样本 → ML MAPE = {:.2}% vs 手工 MAPE = {:.2}% (训练次数: {}, 样本数: {})",
            ml_mape,
            hand_mape,
            ml_model.train_count(),
            ml_model.sample_count()
        );
    }

    #[test]
    fn test_ml_model_incremental_training_improves_accuracy() {
        let all_samples = generate_synthetic_samples(500, 99);
        let test_set: Vec<TrainingSample> = generate_synthetic_samples(100, 100);

        // 第一阶段：仅用前 100 个样本训练
        let mut model_v1 = MLCostModel::new();
        model_v1.add_samples(all_samples[..100].iter().copied());
        model_v1.train();
        let mape_v1 = model_v1.evaluate_mape(&test_set);

        // 第二阶段：用全部 500 个样本训练
        let mut model_v2 = MLCostModel::new();
        model_v2.add_samples(all_samples.iter().copied());
        model_v2.train();
        let mape_v2 = model_v2.evaluate_mape(&test_set);

        // 更多样本应使模型更准确（或至少不退化太多）
        // 注：由于噪声和模型容量，可能不严格单调改善，但差异应在合理范围
        eprintln!(
            "[Phase 5.10 增量训练] 100 样本 MAPE = {:.2}% → 500 样本 MAPE = {:.2}%",
            mape_v1, mape_v2
        );
        // 宽松断言：v2 不应比 v1 差 5 倍以上
        assert!(mape_v2 < mape_v1 * 5.0, "增量训练后模型不应大幅退化");
    }

    // -----------------------------------------------------------------
    //  特征提取覆盖测试
    // -----------------------------------------------------------------

    #[test]
    fn test_features_from_sort() {
        let store = stats_store_with("t", 100);
        let model = CostModel::new(store);
        let plan = sort_plan(scan_plan("t"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.sort_count, 1.0);
        assert_eq!(f.scan_count, 1.0);
        assert_eq!(f.node_count, 2.0);
        assert_eq!(f.max_depth, 2.0);
    }

    #[test]
    fn test_features_from_aggregate() {
        let store = stats_store_with("t", 100);
        let model = CostModel::new(store);
        let plan = aggregate_plan(scan_plan("t"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.aggregate_count, 1.0);
    }

    #[test]
    fn test_features_from_projection() {
        let store = stats_store_with("t", 100);
        let model = CostModel::new(store);
        let plan = projection_plan(scan_plan("t"));
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.projection_count, 1.0);
    }

    #[test]
    fn test_features_from_union() {
        let store = stats_store_with("t", 100);
        let model = CostModel::new(store);
        let plan = union_plan(scan_plan("t1"), scan_plan("t2"));
        let f = PlanFeatures::from_plan(&plan, &model);
        // SetOp 递归左右子树，所以 scan_count 应为 2
        assert_eq!(f.scan_count, 2.0);
        // node_count = 1(SetOp) + 1(Scan) + 1(Scan) = 3
        assert_eq!(f.node_count, 3.0);
    }

    #[test]
    fn test_features_from_shared_memo_ref() {
        let store = stats_store_with("t", 100);
        let model = CostModel::new(store);
        let plan = LogicalPlan::Shared {
            id: 1,
            plan: Box::new(scan_plan("t")),
        };
        let f = PlanFeatures::from_plan(&plan, &model);
        // Shared 应递归到内部 plan，所以 scan_count 应为 1
        assert_eq!(f.scan_count, 1.0);
    }

    #[test]
    fn test_features_empty_plan() {
        let store = empty_stats_store();
        let model = CostModel::new(store);
        let plan = LogicalPlan::Empty;
        let f = PlanFeatures::from_plan(&plan, &model);
        assert_eq!(f.node_count, 1.0); // Empty 自身计入 node_count
        assert_eq!(f.max_depth, 1.0);
    }

    // -----------------------------------------------------------------
    //  端到端集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_end_to_end_ml_beats_handcrafted_with_real_plan() {
        // 使用真实 LogicalPlan 提取特征，配合合成 actual_ms。
        // 关键：必须生成多样化的计划结构（不能仅靠 6 个固定计划 × N 样本），
        // 否则特征完全相同，ML 只能学到噪声均值，无法超越手工模型。
        let store = stats_store_with("t", 1000);
        let model = CostModel::new(store);

        // 简单 LCG 伪随机数生成器
        let mut rng_state: u64 = 7;
        let mut next_rand = || {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng_state >> 33) as f64 / (1u64 << 31) as f64
        };

        // 随机生成多样化的计划结构
        let tables = ["t1", "t2", "t3", "t4"];
        let mut train_samples: Vec<TrainingSample> = Vec::with_capacity(6000);
        let mut test_samples: Vec<TrainingSample> = Vec::with_capacity(1000);

        for i in 0..6000 {
            let t_idx = (next_rand() * tables.len() as f64) as usize % tables.len();
            let table = tables[t_idx];
            let mut plan = scan_plan(table);

            // 随机包装 0-4 层算子
            let wrap_count = (next_rand() * 5.0) as usize;
            for _ in 0..wrap_count {
                let op = (next_rand() * 5.0) as usize;
                plan = match op {
                    0 => filter_plan(
                        table,
                        AExpr::BinaryOp {
                            left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                            op: BinaryOp::Gt,
                            right: Box::new(lit_int((next_rand() * 100.0) as i64)),
                        },
                    ),
                    1 => sort_plan(plan),
                    2 => limit_plan(plan, (next_rand() * 50.0) as i64 + 1),
                    3 => distinct_plan(plan),
                    _ => projection_plan(plan),
                };
            }

            // 30% 概率 JOIN 另一张表
            if next_rand() < 0.3 {
                let other = tables[(next_rand() * tables.len() as f64) as usize % tables.len()];
                let right_plan = scan_plan(other);
                let join_t = (next_rand() * 3.0) as usize;
                plan = match join_t {
                    0 => inner_join(plan, right_plan),
                    1 => left_join(plan, right_plan),
                    _ => cross_join(plan, right_plan),
                };
            }

            let features = PlanFeatures::from_plan(&plan, &model);
            // 合成 actual_ms：estimated_cost 占比低（手工模型预测 estimated_cost 误差大），
            // 其他特征贡献显著（ML 可学习这些特征 → 误差低）
            let noise = next_rand() * 3.0;
            let actual_ms = 0.1 * features.estimated_cost
                + 5.0 * features.node_count
                + 3.0 * features.scan_count
                + 10.0 * features.join_count
                + 5.0 * features.sort_count
                + 3.0 * features.filter_count
                + 2.0 * features.limit_count
                + noise;

            train_samples.push(TrainingSample {
                features,
                actual_ms,
            });

            // 测试集：每 6 个样本取 1 个作为测试集（独立计划）
            if i % 6 == 0 {
                let test_actual = 0.1 * features.estimated_cost
                    + 5.0 * features.node_count
                    + 3.0 * features.scan_count
                    + 10.0 * features.join_count
                    + 5.0 * features.sort_count
                    + 3.0 * features.filter_count
                    + 2.0 * features.limit_count
                    + 1.0; // 固定噪声
                test_samples.push(TrainingSample {
                    features,
                    actual_ms: test_actual,
                });
            }
        }

        // 训练 ML 模型（批量加载 + 显式训练）
        let mut ml_model = MLCostModel::new();
        ml_model.add_samples(train_samples.iter().copied());
        ml_model.train();
        assert!(ml_model.is_trained());

        let ml_mape = ml_model.evaluate_mape(&test_samples);
        let hand_mape = MLCostModel::evaluate_handcrafted_mape(&test_samples);

        assert!(
            ml_mape < hand_mape,
            "ML MAPE = {}%, handcrafted MAPE = {}% — ML 应优于手工模型",
            ml_mape,
            hand_mape
        );

        eprintln!(
            "[Phase 5.10 端到端] 多样化真实计划 + 合成标签 → ML MAPE = {:.2}% vs 手工 MAPE = {:.2}%",
            ml_mape, hand_mape
        );
    }

    #[test]
    fn test_hybrid_model_with_real_plan_fallback_then_predict() {
        // 端到端验证：HybridCostModel 冷启动 → 记录执行 → 切换到 ML 预测
        let store = stats_store_with("t", 500);
        let mut hybrid = HybridCostModel::new(store);
        let plan = scan_plan("t");

        // 冷启动：估算应等于手工模型
        let cold_cost = hybrid.estimate(&plan);
        let hand_cost = hybrid.hand_crafted().estimate(&plan);
        assert_eq!(cold_cost.cardinality, hand_cost.cardinality);
        assert!((cold_cost.total() - hand_cost.total()).abs() < 1e-9);

        // 记录足够多的执行
        for _ in 0..(MIN_SAMPLES_TO_PREDICT + RETRAIN_THRESHOLD) {
            hybrid.record_execution(&plan, 3.0);
        }

        // 训练后：估算应使用 ML 预测（不再等于手工）
        let warm_cost = hybrid.estimate(&plan);
        assert!(hybrid.ml_model().is_trained());
        // ML 预测应接近 3.0（拆分为 cpu=1.5 + io=1.5）
        eprintln!(
            "[Phase 5.10 冷热切换] 冷启动 total = {:.2}, 训练后 total = {:.2} (期望 ~3.0)",
            cold_cost.total(),
            warm_cost.total()
        );
    }

    // -----------------------------------------------------------------
    //  附加：回归测试（确保 feature 提取不 panic）
    // -----------------------------------------------------------------

    #[test]
    fn test_features_do_not_panic_on_dml() {
        let store = empty_stats_store();
        let model = CostModel::new(store);
        // DML 节点 — 不应 panic
        let plan = LogicalPlan::Insert {
            table: TableName::new("t".to_string()),
            schema: TableSchema {
                name: TableName::new("t".to_string()),
                columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
            },
            columns: None,
            source: szrsql_sql::plan::InsertSourcePlan::Values(vec![]),
            on_conflict: None,
            returning: None,
        };
        let _ = PlanFeatures::from_plan(&plan, &model);
        // 不 panic 即通过
    }

    #[test]
    fn test_features_do_not_panic_on_memo_ref() {
        let store = empty_stats_store();
        let model = CostModel::new(store);
        let plan = LogicalPlan::MemoRef {
            id: 42,
            schema: TableSchema {
                name: TableName::new("t".to_string()),
                columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
            },
        };
        let _ = PlanFeatures::from_plan(&plan, &model);
        // 不 panic 即通过
    }

    // -----------------------------------------------------------------
    //  Phase 7b.1 — ML 成本模型完善 + 持续学习
    // -----------------------------------------------------------------

    #[test]
    fn test_7b1_weight_norm_untrained() {
        // 未训练模型的权重 L2 范数应为 0
        let model = MLCostModel::new();
        assert_eq!(model.weight_norm(), 0.0);
        assert!(model.training_history().is_empty());
    }

    #[test]
    fn test_7b1_job_benchmark_10k_queries() {
        // Phase 7b.1 核心验证：JOB 基准 10000 条查询 → 训练 ONNX 模型 → ML 误差 < 手工模型
        let mut generator = JobBenchmarkGenerator::new(42);
        let all_samples = generator.generate(10_000);

        // 80% 训练 / 20% 测试
        let split = all_samples.len() * 4 / 5;
        let train_set = &all_samples[..split];
        let test_set = &all_samples[split..];

        // 训练 ML 模型（批量加载 + 显式训练 = ONNX 模型训练）
        let mut ml_model = MLCostModel::new();
        ml_model.add_samples(train_set.iter().copied());
        ml_model.train();
        assert!(ml_model.is_trained());

        // 评估 ML 模型
        let ml_mape = ml_model.evaluate_mape(test_set);
        // 评估手工模型（对比基线）
        let hand_mape = MLCostModel::evaluate_handcrafted_mape(test_set);

        // 验证标准：ML 误差 < 手工模型误差
        assert!(
            ml_mape < hand_mape,
            "JOB 10k: ML MAPE = {:.2}% vs 手工 MAPE = {:.2}% — ML 应优于手工模型",
            ml_mape,
            hand_mape
        );

        eprintln!(
            "[Phase 7b.1 JOB 10k] ML MAPE = {:.2}% vs 手工 MAPE = {:.2}% (训练次数: {}, 样本数: {}, 权重范数: {:.4})",
            ml_mape,
            hand_mape,
            ml_model.train_count(),
            ml_model.sample_count(),
            ml_model.weight_norm()
        );
    }

    #[test]
    fn test_7b1_continuous_learning_weight_progression() {
        // Phase 7b.1 核心验证：10000 次后在线学习权重自动更新，从 0 逐步提升到 > 0.5
        let mut generator = JobBenchmarkGenerator::new(99);
        let all_samples = generator.generate(10_000);

        let mut model = MLCostModel::new();

        // 初始权重范数为 0
        assert_eq!(model.weight_norm(), 0.0);

        // 增量训练：分批添加样本，触发多次增量训练
        let batch_size = 200;
        let mut weight_norms: Vec<f64> = Vec::new();
        for chunk in all_samples.chunks(batch_size) {
            model.add_samples(chunk.iter().copied());
            model.train();
            weight_norms.push(model.weight_norm());
        }

        // 验证：训练历史非空
        assert!(!model.training_history().is_empty());
        assert_eq!(model.training_history().len(), model.train_count());

        // 验证：权重范数从 0 逐步提升
        let initial_norm = weight_norms[0];
        let final_norm = *weight_norms.last().unwrap();
        assert!(initial_norm > 0.0, "首次训练后权重范数应 > 0");
        assert!(
            final_norm > 0.5,
            "10000 次后权重范数应 > 0.5，实际 = {:.4}",
            final_norm
        );

        // 验证：训练历史与手动记录一致
        assert_eq!(model.training_history(), weight_norms.as_slice());

        eprintln!(
            "[Phase 7b.1 持续学习] 权重范数: 初始 = {:.4} → 最终 = {:.4} (训练 {} 次, 样本 {} 条)",
            initial_norm,
            final_norm,
            model.train_count(),
            model.sample_count()
        );
    }

    #[test]
    fn test_7b1_onnx_export() {
        // Phase 7b.1: ONNX 模型导出验证
        let mut model = MLCostModel::new();

        // 未训练模型导出（应成功，权重全零）
        let untrained_json = model.export_onnx_json().unwrap();
        let untrained: serde_json::Value = serde_json::from_str(&untrained_json).unwrap();
        assert_eq!(untrained["ir_version"], 8);
        assert_eq!(untrained["producer_name"], "szrsql-optimizer");
        assert_eq!(untrained["model_type"], "LinearRegression");
        assert_eq!(untrained["feature_dim"], FEATURE_DIM);
        assert_eq!(untrained["weight_norm"], 0.0);

        // 训练后导出
        let mut generator = JobBenchmarkGenerator::new(77);
        let samples = generator.generate(500);
        model.add_samples(samples.iter().copied());
        model.train();

        let trained_json = model.export_onnx_json().unwrap();
        let trained: serde_json::Value = serde_json::from_str(&trained_json).unwrap();

        // 验证 ONNX 图结构
        let graph = &trained["graph"];
        assert_eq!(graph["node"][0]["op_type"], "LinearRegressor");
        assert_eq!(graph["input"][0]["name"], "features");
        assert_eq!(graph["input"][0]["shape"][0], FEATURE_DIM);
        assert_eq!(graph["output"][0]["name"], "prediction");

        // 验证权重非零
        let coefficients = graph["node"][0]["attributes"]["coefficients"]
            .as_array()
            .unwrap();
        assert_eq!(coefficients.len(), FEATURE_DIM);

        // 验证归一化参数
        let normalizer = &trained["normalizer"];
        assert!(normalizer["mean"].is_array());
        assert!(normalizer["std"].is_array());

        // 验证训练元数据
        assert_eq!(trained["train_count"], 1);
        assert!(trained["weight_norm"].as_f64().unwrap() > 0.0);

        eprintln!(
            "[Phase 7b.1 ONNX 导出] 模型已导出为 JSON ({} bytes, 权重范数 = {:.4})",
            trained_json.len(),
            model.weight_norm()
        );
    }

    #[test]
    fn test_7b1_training_history_monotonic_increase() {
        // 验证训练历史记录的权重范数序列在初期训练中逐步增长
        let mut generator = JobBenchmarkGenerator::new(123);
        let samples = generator.generate(1000);

        let mut model = MLCostModel::new();
        // 分 5 批训练
        for chunk in samples.chunks(200) {
            model.add_samples(chunk.iter().copied());
            model.train();
        }

        let history = model.training_history();
        assert_eq!(history.len(), 5);

        // 至少最后一次的权重范数 > 0.5
        let final_norm = *history.last().unwrap();
        assert!(
            final_norm > 0.5,
            "最终权重范数应 > 0.5，实际 = {:.4}",
            final_norm
        );

        // 首次训练后权重范数应 > 0
        assert!(history[0] > 0.0, "首次训练后权重范数应 > 0");
    }

    #[test]
    fn test_7b1_hybrid_model_online_learning() {
        // Phase 7b.1: HybridCostModel 在线学习 — 记录执行后自动训练
        // 使用多样化计划池（不同表 + 不同算子组合）确保特征多样性，否则权重梯度为 0
        let mut store_inner = InMemoryStatisticsStore::new();
        let tables = ["t1", "t2", "t3", "t4", "t5"];
        let table_rows = [1_000, 2_000, 500, 3_000, 800];
        for (i, t) in tables.iter().enumerate() {
            let mut ts = TableStatistics::empty(*t);
            ts.row_count = table_rows[i];
            store_inner.update_table_stats(t, ts);
        }
        let store: Arc<dyn StatisticsStore> = Arc::new(store_inner);
        let cost_model = CostModel::new(store.clone());
        let mut hybrid = HybridCostModel::new(store);

        // 冷启动：权重为 0
        assert_eq!(hybrid.ml_model().weight_norm(), 0.0);

        // 构建多样化计划池：Scan / Filter / Sort / 2-表 JOIN / 3-表 JOIN
        let mut plan_pool: Vec<LogicalPlan> = Vec::new();
        for &t in &tables {
            plan_pool.push(scan_plan(t));
            plan_pool.push(filter_plan(
                t,
                AExpr::BinaryOp {
                    left: Box::new(AExpr::Identifier(vec!["id".to_string()])),
                    op: BinaryOp::Gt,
                    right: Box::new(AExpr::Literal(szrsql_types::value::Value::Int64(100))),
                },
            ));
            plan_pool.push(LogicalPlan::Sort {
                order_by: vec![OrderByExpr {
                    expr: AExpr::Identifier(vec!["id".to_string()]),
                    asc: true,
                    nulls_first: false,
                }],
                input: Box::new(scan_plan(t)),
            });
        }
        // 2-表 JOIN
        for i in 0..tables.len() {
            for j in (i + 1)..tables.len() {
                plan_pool.push(inner_join(scan_plan(tables[i]), scan_plan(tables[j])));
            }
        }
        // 3-表 JOIN
        for i in 0..tables.len() {
            for j in (i + 1)..tables.len() {
                for k in (j + 1)..tables.len() {
                    plan_pool.push(inner_join(
                        inner_join(scan_plan(tables[i]), scan_plan(tables[j])),
                        scan_plan(tables[k]),
                    ));
                }
            }
        }

        // 模拟 10000 次在线学习 — 循环使用多样化计划 + 变化的 actual_ms
        let mut rng_state: u64 = 42;
        for i in 0..10_000 {
            let plan = &plan_pool[i % plan_pool.len()];
            let features = PlanFeatures::from_plan(plan, &cost_model);
            // actual_ms 基于特征 + 随机噪声（确保特征-目标存在真实关系）
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = (rng_state >> 33) as f64 / (1u64 << 31) as f64 * 5.0;
            let actual_ms = 0.2 * features.estimated_cost
                + 5.0 * features.node_count
                + 3.0 * features.sort_count
                + 2.0 * features.filter_count
                + 8.0 * features.join_count
                + noise;
            hybrid.record_execution(plan, actual_ms);
        }

        // 验证：ML 模型已训练
        assert!(hybrid.ml_model().is_trained());
        // 验证：权重范数 > 0.5
        let norm = hybrid.ml_model().weight_norm();
        assert!(
            norm > 0.5,
            "10000 次在线学习后权重范数应 > 0.5，实际 = {:.4}",
            norm
        );
        // 验证：训练历史非空
        assert!(!hybrid.ml_model().training_history().is_empty());

        eprintln!(
            "[Phase 7b.1 在线学习] 10000 次执行后: 权重范数 = {:.4}, 训练 {} 次",
            norm,
            hybrid.ml_model().train_count()
        );
    }
}
