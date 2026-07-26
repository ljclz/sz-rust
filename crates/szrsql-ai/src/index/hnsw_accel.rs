//! HNSW 向量索引加速后端 — Phase 7f.3
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! 为 HNSW 向量索引提供多种加速后端，通过统一 trait `DistanceAccel` 抽象距离计算：
//!
//! 1. **ScalarBackend** — 标量基线实现，用于正确性验证和回退
//! 2. **SimdBackend** — SIMD 加速（AVX2/SSE2），运行时检测 CPU 特性
//! 3. **Avx512Backend** — AVX-512 加速（当前回退到 AVX2）
//! 4. **PQ** — Product Quantization 压缩，内存节省 32x+
//! 5. **SQ** — Scalar Quantization 压缩，内存节省 4x
//! 6. **HybridPqBackend** — HNSW + PQ 混合，图搜索用压缩向量
//! 7. **DiskAnnBackend** — 磁盘索引，SSD 友好布局
//! 8. **GpuBackend** — GPU 加速（桩实现，需 wgpu 依赖）
//!
//! # 验证标准
//!
//! - 各后端距离计算正确性：与标量基线误差 < 1e-5
//! - SIMD 加速比：AVX2 理论 8x，实测 >= 4x
//! - PQ 压缩比：32x（M=8, centroids=256, dim=128 -> 8 bytes vs 512 bytes）
//! - SQ 压缩比：4x（f32 -> u8）
//!
//! 对应 `SzRSQL实施进度.md` Phase 7f.3。

use thiserror::Error;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

// =====================================================================
//  错误类型
// =====================================================================

/// HNSW 加速后端错误
#[derive(Debug, Clone, PartialEq, Error)]
pub enum HnswAccelError {
    /// 维度无效
    #[error("invalid dimension: {0} (must be > 0)")]
    InvalidDimension(usize),
    /// 参数无效
    #[error("invalid param: {0}")]
    InvalidParam(String),
    /// 训练失败
    #[error("training failed: {0}")]
    TrainingFailed(String),
    /// 编码失败
    #[error("encoding failed: {0}")]
    EncodingFailed(String),
}

// =====================================================================
//  LCG — 确定性伪随机数生成器（用于 K-means 初始化）
// =====================================================================

/// 线性同余生成器（Numerical Recipes 常数）
///
/// 确定性、可复现，无需 `rand` crate 依赖。
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x6D2B_79F5),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    /// 返回 [0, max) 区间的 usize
    fn next_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            (self.next_u64() % max as u64) as usize
        }
    }
}

// =====================================================================
//  标量辅助函数
// =====================================================================

/// 标量点积
fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum()
}

/// 标量 L2 距离平方
fn squared_l2_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let diff = x - y;
            diff * diff
        })
        .sum()
}

/// 标量 L2 范数平方
fn norm_squared(a: &[f32]) -> f32 {
    a.iter().map(|&x| x * x).sum()
}

// =====================================================================
//  SIMD 辅助函数（x86_64 专用）
// =====================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
/// AVX2 加速点积计算（x86_64 专用）。
///
/// # Safety
///
/// 调用方必须确保：
/// 1. 当前 CPU 支持 AVX2 指令集（由 `is_x86_feature_detected!("avx2")` 守卫）
/// 2. `a` 和 `b` 长度相等（由 `debug_assert_eq!` 检查，release 下调用方负责）
/// 3. 切片内存对齐无要求（使用 `_mm256_loadu_ps` 未对齐加载）
unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len();
    let mut i = 0;

    // SAFETY: 已由函数级 `# Safety` 文档约定：调用方保证 AVX2 可用且 a.len()==b.len()。
    // `_mm256_loadu_ps` 接受未对齐指针，`a.as_ptr().add(i)` 在 `i+8 <= n` 守卫下不会越界。
    unsafe {
        let mut sum = _mm256_setzero_ps();

        while i + 8 <= n {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let vb = _mm256_loadu_ps(b.as_ptr().add(i));
            sum = _mm256_add_ps(sum, _mm256_mul_ps(va, vb));
            i += 8;
        }

        // 水平求和 256 位 -> 128 位
        let hi = _mm256_extractf128_ps(sum, 1);
        let lo = _mm256_castps256_ps128(sum);
        let mut sum128 = _mm_add_ps(hi, lo);
        sum128 = _mm_hadd_ps(sum128, sum128);
        sum128 = _mm_hadd_ps(sum128, sum128);
        let mut result = _mm_cvtss_f32(sum128);

        // 处理尾部
        while i < n {
            result += *a.as_ptr().add(i) * *b.as_ptr().add(i);
            i += 1;
        }

        result
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
/// AVX2 加速 L2 距离平方计算（x86_64 专用）。
///
/// # Safety
///
/// 调用方必须确保：
/// 1. 当前 CPU 支持 AVX2 指令集（由 `is_x86_feature_detected!("avx2")` 守卫）
/// 2. `a` 和 `b` 长度相等（由 `debug_assert_eq!` 检查，release 下调用方负责）
/// 3. 切片内存对齐无要求（使用 `_mm256_loadu_ps` 未对齐加载）
unsafe fn squared_l2_avx2(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len();
    let mut i = 0;

    // SAFETY: 同 dot_product_avx2，AVX2 可用性由调用方保证；越界由 `i+8 <= n` 守卫。
    unsafe {
        let mut sum = _mm256_setzero_ps();

        while i + 8 <= n {
            let va = _mm256_loadu_ps(a.as_ptr().add(i));
            let vb = _mm256_loadu_ps(b.as_ptr().add(i));
            let diff = _mm256_sub_ps(va, vb);
            sum = _mm256_add_ps(sum, _mm256_mul_ps(diff, diff));
            i += 8;
        }

        let hi = _mm256_extractf128_ps(sum, 1);
        let lo = _mm256_castps256_ps128(sum);
        let mut sum128 = _mm_add_ps(hi, lo);
        sum128 = _mm_hadd_ps(sum128, sum128);
        sum128 = _mm_hadd_ps(sum128, sum128);
        let mut result = _mm_cvtss_f32(sum128);

        while i < n {
            let diff = *a.as_ptr().add(i) - *b.as_ptr().add(i);
            result += diff * diff;
            i += 1;
        }

        result
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
/// SSE2 加速点积计算（x86_64 专用，作为 AVX2 不可用时的回退）。
///
/// # Safety
///
/// 调用方必须确保：
/// 1. 当前 CPU 支持 SSE2 指令集（由 `is_x86_feature_detected!("sse2")` 守卫）
/// 2. `a` 和 `b` 长度相等（由 `debug_assert_eq!` 检查，release 下调用方负责）
/// 3. 切片内存对齐无要求（使用 `_mm_loadu_ps` 未对齐加载）
unsafe fn dot_product_sse2(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len();
    let mut i = 0;

    // SAFETY: SSE2 可用性由调用方保证；`i+4 <= n` 守卫防止越界；`_mm_loadu_ps` 接受未对齐指针。
    unsafe {
        let mut sum = _mm_setzero_ps();

        while i + 4 <= n {
            let va = _mm_loadu_ps(a.as_ptr().add(i));
            let vb = _mm_loadu_ps(b.as_ptr().add(i));
            sum = _mm_add_ps(sum, _mm_mul_ps(va, vb));
            i += 4;
        }

        // 水平求和（只用 SSE2，通过 shuffle 避免 SSE3 依赖）
        let shuf = _mm_shuffle_ps(sum, sum, 0b01_00_11_10);
        sum = _mm_add_ps(sum, shuf);
        let shuf = _mm_shuffle_ps(sum, sum, 0b00_00_00_01);
        sum = _mm_add_ps(sum, shuf);
        let mut result = _mm_cvtss_f32(sum);

        while i < n {
            result += *a.as_ptr().add(i) * *b.as_ptr().add(i);
            i += 1;
        }

        result
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
/// SSE2 加速 L2 距离平方计算（x86_64 专用，作为 AVX2 不可用时的回退）。
///
/// # Safety
///
/// 调用方必须确保：
/// 1. 当前 CPU 支持 SSE2 指令集（由 `is_x86_feature_detected!("sse2")` 守卫）
/// 2. `a` 和 `b` 长度相等（由 `debug_assert_eq!` 检查，release 下调用方负责）
/// 3. 切片内存对齐无要求（使用 `_mm_loadu_ps` 未对齐加载）
unsafe fn squared_l2_sse2(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len();
    let mut i = 0;

    // SAFETY: 同 dot_product_sse2，SSE2 可用性由调用方保证；越界由 `i+4 <= n` 守卫。
    unsafe {
        let mut sum = _mm_setzero_ps();

        while i + 4 <= n {
            let va = _mm_loadu_ps(a.as_ptr().add(i));
            let vb = _mm_loadu_ps(b.as_ptr().add(i));
            let diff = _mm_sub_ps(va, vb);
            sum = _mm_add_ps(sum, _mm_mul_ps(diff, diff));
            i += 4;
        }

        let shuf = _mm_shuffle_ps(sum, sum, 0b01_00_11_10);
        sum = _mm_add_ps(sum, shuf);
        let shuf = _mm_shuffle_ps(sum, sum, 0b00_00_00_01);
        sum = _mm_add_ps(sum, shuf);
        let mut result = _mm_cvtss_f32(sum);

        while i < n {
            let diff = *a.as_ptr().add(i) - *b.as_ptr().add(i);
            result += diff * diff;
            i += 1;
        }

        result
    }
}

// =====================================================================
//  统一距离计算入口（运行时 SIMD 检测）
// =====================================================================

/// 点积（运行时自动选择最优 SIMD 实现）
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: is_x86_feature_detected!("avx2") 已验证 CPU 支持 AVX2；
            // a.len()==b.len() 由函数入口 debug_assert_eq! 保证。
            return unsafe { dot_product_avx2(a, b) };
        }
        if is_x86_feature_detected!("sse2") {
            // SAFETY: is_x86_feature_detected!("sse2") 已验证 CPU 支持 SSE2；
            // a.len()==b.len() 由函数入口 debug_assert_eq! 保证。
            return unsafe { dot_product_sse2(a, b) };
        }
    }
    dot_product_scalar(a, b)
}

/// L2 距离平方（运行时自动选择最优 SIMD 实现）
fn squared_l2(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: is_x86_feature_detected!("avx2") 已验证 CPU 支持 AVX2；
            // a.len()==b.len() 由函数入口 debug_assert_eq! 保证。
            return unsafe { squared_l2_avx2(a, b) };
        }
        if is_x86_feature_detected!("sse2") {
            // SAFETY: is_x86_feature_detected!("sse2") 已验证 CPU 支持 SSE2；
            // a.len()==b.len() 由函数入口 debug_assert_eq! 保证。
            return unsafe { squared_l2_sse2(a, b) };
        }
    }
    squared_l2_scalar(a, b)
}

// =====================================================================
//  SimdArch — SIMD 架构枚举
// =====================================================================

/// SIMD 架构枚举
///
/// 表示后端使用的 SIMD 指令集架构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimdArch {
    /// 纯标量（无 SIMD）
    Scalar,
    /// SSE2（x86 128-bit，4 x f32）
    Sse2,
    /// SSE4.2（x86 128-bit + 串比较优化）
    Sse42,
    /// AVX2（x86 256-bit，8 x f32）
    Avx2,
    /// AVX-512（x86 512-bit，16 x f32）
    Avx512,
    /// ARM NEON（128-bit，4 x f32）
    Neon,
}

impl SimdArch {
    /// 架构名称
    pub fn name(&self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Sse2 => "sse2",
            Self::Sse42 => "sse4.2",
            Self::Avx2 => "avx2",
            Self::Avx512 => "avx512",
            Self::Neon => "neon",
        }
    }

    /// 运行时检测当前 CPU 支持的最优 SIMD 架构
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx512f") {
                return Self::Avx512;
            }
            if is_x86_feature_detected!("avx2") {
                return Self::Avx2;
            }
            if is_x86_feature_detected!("sse4.2") {
                return Self::Sse42;
            }
            if is_x86_feature_detected!("sse2") {
                return Self::Sse2;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("neon") {
                return Self::Neon;
            }
        }
        Self::Scalar
    }

    /// 是否支持该架构（运行时检测）
    pub fn is_supported(&self) -> bool {
        match self {
            Self::Scalar => true,
            Self::Sse2 => {
                #[cfg(target_arch = "x86_64")]
                {
                    is_x86_feature_detected!("sse2")
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    false
                }
            }
            Self::Sse42 => {
                #[cfg(target_arch = "x86_64")]
                {
                    is_x86_feature_detected!("sse4.2")
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    false
                }
            }
            Self::Avx2 => {
                #[cfg(target_arch = "x86_64")]
                {
                    is_x86_feature_detected!("avx2")
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    false
                }
            }
            Self::Avx512 => {
                #[cfg(target_arch = "x86_64")]
                {
                    is_x86_feature_detected!("avx512f")
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    false
                }
            }
            Self::Neon => {
                #[cfg(target_arch = "aarch64")]
                {
                    std::arch::is_aarch64_feature_detected!("neon")
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    false
                }
            }
        }
    }
}

impl Default for SimdArch {
    fn default() -> Self {
        Self::detect()
    }
}

impl std::fmt::Display for SimdArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// =====================================================================
//  DistanceMetric — 距离度量
// =====================================================================

/// 距离度量类型
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DistanceMetric {
    /// 余弦距离（1 - 余弦相似度）
    #[default]
    Cosine,
    /// L2 距离（欧氏距离）
    L2,
}

impl DistanceMetric {
    /// 度量名称
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::L2 => "l2",
        }
    }
}

// =====================================================================
//  DistanceAccel trait — 统一距离计算抽象
// =====================================================================

/// 距离加速后端 trait
///
/// 所有 HNSW 加速后端实现此 trait，提供统一的距离计算接口。
pub trait DistanceAccel: Send + Sync {
    /// 计算两个向量之间的距离
    ///
    /// - `a` — 查询向量
    /// - `b` — 目标向量
    ///
    /// 返回距离值（非负）
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError>;

    /// 批量计算 query 到多个向量的距离
    ///
    /// - `query` — 查询向量
    /// - `vectors` — 目标向量列表
    ///
    /// 返回距离列表，长度等于 `vectors.len()`
    fn batch_distance(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        vectors.iter().map(|v| self.distance(query, v)).collect()
    }

    /// 后端名称
    fn name(&self) -> &'static str;

    /// 距离度量
    fn metric(&self) -> DistanceMetric;
}

// =====================================================================
//  ScalarBackend — 标量基线
// =====================================================================

/// 标量距离计算后端（基线实现）
///
/// 不使用任何 SIMD 指令，用于正确性验证和平台回退。
#[derive(Debug, Clone)]
pub struct ScalarBackend {
    /// 向量维度
    dim: usize,
    /// 距离度量
    metric: DistanceMetric,
}

impl ScalarBackend {
    /// 创建标量后端
    pub fn new(dim: usize, metric: DistanceMetric) -> Result<Self, HnswAccelError> {
        if dim == 0 {
            return Err(HnswAccelError::InvalidDimension(dim));
        }
        Ok(Self { dim, metric })
    }

    /// 向量维度
    pub fn dim(&self) -> usize {
        self.dim
    }
}

impl DistanceAccel for ScalarBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        if a.len() != self.dim || b.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(a.len()));
        }
        let dist = match self.metric {
            DistanceMetric::Cosine => {
                let dot = dot_product_scalar(a, b);
                let norm_a = norm_squared(a).sqrt();
                let norm_b = norm_squared(b).sqrt();
                if norm_a < 1e-12 || norm_b < 1e-12 {
                    return Ok(1.0);
                }
                1.0 - dot / (norm_a * norm_b)
            }
            DistanceMetric::L2 => squared_l2_scalar(a, b).sqrt(),
        };
        Ok(dist.max(0.0))
    }

    fn name(&self) -> &'static str {
        "scalar"
    }

    fn metric(&self) -> DistanceMetric {
        self.metric
    }
}

// =====================================================================
//  SimdBackend — SIMD 加速后端
// =====================================================================

/// SIMD 加速距离计算后端
///
/// 运行时检测 CPU 特性，自动选择最优 SIMD 实现（AVX2 > SSE2 > 标量）。
#[derive(Debug, Clone)]
pub struct SimdBackend {
    /// 向量维度
    dim: usize,
    /// 距离度量
    metric: DistanceMetric,
    /// 请求的 SIMD 架构
    arch: SimdArch,
}

impl SimdBackend {
    /// 创建 SIMD 后端
    ///
    /// - `arch` — 期望的 SIMD 架构，若 CPU 不支持则回退到 `SimdArch::detect()`
    pub fn new(arch: SimdArch, dim: usize) -> Result<Self, HnswAccelError> {
        Self::with_metric(arch, dim, DistanceMetric::Cosine)
    }

    /// 创建 SIMD 后端（指定距离度量）
    pub fn with_metric(
        arch: SimdArch,
        dim: usize,
        metric: DistanceMetric,
    ) -> Result<Self, HnswAccelError> {
        if dim == 0 {
            return Err(HnswAccelError::InvalidDimension(dim));
        }
        let arch = if arch.is_supported() {
            arch
        } else {
            SimdArch::detect()
        };
        Ok(Self { dim, metric, arch })
    }

    /// 向量维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 实际使用的 SIMD 架构
    pub fn arch(&self) -> SimdArch {
        self.arch
    }
}

impl DistanceAccel for SimdBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        if a.len() != self.dim || b.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(a.len()));
        }
        let dist = match self.metric {
            DistanceMetric::Cosine => {
                let dot = dot_product(a, b);
                let norm_a = norm_squared(a).sqrt();
                let norm_b = norm_squared(b).sqrt();
                if norm_a < 1e-12 || norm_b < 1e-12 {
                    return Ok(1.0);
                }
                1.0 - dot / (norm_a * norm_b)
            }
            DistanceMetric::L2 => squared_l2(a, b).sqrt(),
        };
        Ok(dist.max(0.0))
    }

    fn batch_distance(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        if query.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(query.len()));
        }
        let mut results = Vec::with_capacity(vectors.len());
        for v in vectors {
            results.push(self.distance(query, v)?);
        }
        Ok(results)
    }

    fn name(&self) -> &'static str {
        match self.arch {
            SimdArch::Scalar => "simd_scalar",
            SimdArch::Sse2 => "simd_sse2",
            SimdArch::Sse42 => "simd_sse4.2",
            SimdArch::Avx2 => "simd_avx2",
            SimdArch::Avx512 => "simd_avx512",
            SimdArch::Neon => "simd_neon",
        }
    }

    fn metric(&self) -> DistanceMetric {
        self.metric
    }
}

// =====================================================================
//  Avx512Backend — AVX-512 加速后端
// =====================================================================

/// AVX-512 加速距离计算后端
///
/// 当前实现回退到 AVX2（因为 stdarch 对 AVX-512 f32 点积的支持有限）。
/// 未来可通过 `#[target_feature(enable = "avx512f")]` 启用真正的 AVX-512。
#[derive(Debug, Clone)]
pub struct Avx512Backend {
    /// 内部使用 SimdBackend（AVX2 或回退）
    inner: SimdBackend,
}

impl Avx512Backend {
    /// 创建 AVX-512 后端
    pub fn new(dim: usize) -> Result<Self, HnswAccelError> {
        Self::with_metric(dim, DistanceMetric::Cosine)
    }

    /// 创建 AVX-512 后端（指定距离度量）
    pub fn with_metric(dim: usize, metric: DistanceMetric) -> Result<Self, HnswAccelError> {
        let arch = if SimdArch::Avx512.is_supported() {
            SimdArch::Avx512
        } else if SimdArch::Avx2.is_supported() {
            SimdArch::Avx2
        } else {
            SimdArch::detect()
        };
        let inner = SimdBackend::with_metric(arch, dim, metric)?;
        Ok(Self { inner })
    }

    /// 向量维度
    pub fn dim(&self) -> usize {
        self.inner.dim()
    }

    /// 实际使用的架构
    pub fn arch(&self) -> SimdArch {
        self.inner.arch()
    }
}

impl DistanceAccel for Avx512Backend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        self.inner.distance(a, b)
    }

    fn batch_distance(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        self.inner.batch_distance(query, vectors)
    }

    fn name(&self) -> &'static str {
        "avx512"
    }

    fn metric(&self) -> DistanceMetric {
        self.inner.metric()
    }
}

// =====================================================================
//  PQ — Product Quantization
// =====================================================================

/// PQ 训练参数
#[derive(Debug, Clone)]
pub struct PqParams {
    /// 子向量段数 M（dim 必须能被 M 整除）
    pub m: usize,
    /// 每段聚类中心数 K（必须 <= 256 以使用 u8 编码）
    pub centroids: usize,
    /// K-means 迭代次数
    pub iterations: usize,
    /// 随机种子
    pub seed: u64,
}

impl Default for PqParams {
    fn default() -> Self {
        Self {
            m: 8,
            centroids: 256,
            iterations: 10,
            seed: 0x5EED_5EED_5EED_5EED,
        }
    }
}

/// PQ 码本
///
/// 每段 M 个独立的 codebook，每个 codebook 包含 `centroids` 个 `sub_dim` 维中心向量。
#[derive(Debug, Clone)]
pub struct PqCodebook {
    /// 子向量段数
    m: usize,
    /// 每段中心数
    centroids: usize,
    /// 子向量维度 dim / M
    sub_dim: usize,
    /// 码本数据：m * centroids * sub_dim，按 [m][centroids][sub_dim] 排列
    data: Vec<f32>,
}

impl PqCodebook {
    /// 获取指定段、指定中心的子向量
    fn centroid(&self, segment: usize, idx: usize) -> &[f32] {
        let start = (segment * self.centroids + idx) * self.sub_dim;
        &self.data[start..start + self.sub_dim]
    }

    /// 段数
    pub fn m(&self) -> usize {
        self.m
    }

    /// 每段中心数
    pub fn centroids(&self) -> usize {
        self.centroids
    }

    /// 子向量维度
    pub fn sub_dim(&self) -> usize {
        self.sub_dim
    }

    /// 码本总内存占用（字节）
    pub fn memory_bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }
}

/// PQ 训练器
///
/// 对训练向量进行 K-means 聚类，生成 PQ 码本。
pub struct PqTrainer {
    params: PqParams,
}

impl PqTrainer {
    /// 创建 PQ 训练器
    pub fn new(params: PqParams) -> Result<Self, HnswAccelError> {
        if params.m == 0 {
            return Err(HnswAccelError::InvalidParam("m must be > 0".to_string()));
        }
        if params.centroids == 0 || params.centroids > 256 {
            return Err(HnswAccelError::InvalidParam(
                "centroids must be in 1..=256".to_string(),
            ));
        }
        Ok(Self { params })
    }

    /// 训练 PQ 码本
    ///
    /// - `data` — 训练向量集
    /// - `dim` — 向量维度（必须能被 M 整除）
    pub fn train(&self, data: &[Vec<f32>], dim: usize) -> Result<PqCodebook, HnswAccelError> {
        if dim == 0 {
            return Err(HnswAccelError::InvalidDimension(dim));
        }
        if !dim.is_multiple_of(self.params.m) {
            return Err(HnswAccelError::InvalidParam(format!(
                "dim {dim} must be divisible by m {}",
                self.params.m
            )));
        }
        if data.is_empty() {
            return Err(HnswAccelError::TrainingFailed(
                "training data is empty".to_string(),
            ));
        }
        for v in data {
            if v.len() != dim {
                return Err(HnswAccelError::InvalidDimension(v.len()));
            }
        }

        let sub_dim = dim / self.params.m;
        let mut rng = Lcg::new(self.params.seed);
        let k = self.params.centroids.min(data.len());

        let mut codebook_data = Vec::with_capacity(self.params.m * k * sub_dim);

        for seg in 0..self.params.m {
            let offset = seg * sub_dim;
            // 提取该段的所有子向量
            let sub_vectors: Vec<Vec<f32>> = data
                .iter()
                .map(|v| v[offset..offset + sub_dim].to_vec())
                .collect();

            let centroids = kmeans(&sub_vectors, k, sub_dim, self.params.iterations, &mut rng);

            for c in &centroids {
                codebook_data.extend_from_slice(c);
            }

            // 如果 k < centroids，用最后一个中心填充
            for _ in k..self.params.centroids {
                codebook_data.extend_from_slice(&centroids[k - 1]);
            }
        }

        Ok(PqCodebook {
            m: self.params.m,
            centroids: self.params.centroids,
            sub_dim,
            data: codebook_data,
        })
    }
}

/// PQ 压缩后端
///
/// 使用 Product Quantization 压缩向量，距离计算使用 ADC（Asymmetric Distance Computation）。
///
/// # 内存节省
///
/// - 原始向量：dim * 4 字节（f32）
/// - PQ 编码：M 字节（u8 编码）
/// - 压缩比：dim * 4 / M（dim=128, M=8 -> 64x）
#[derive(Debug, Clone)]
pub struct PqBackend {
    /// 码本
    codebook: PqCodebook,
    /// 原始向量维度
    dim: usize,
    /// 距离度量（PQ 始终用 L2，因为 ADC 基于 L2）
    metric: DistanceMetric,
}

impl PqBackend {
    /// 从码本创建 PQ 后端
    pub fn from_codebook(codebook: PqCodebook, dim: usize) -> Result<Self, HnswAccelError> {
        if dim == 0 {
            return Err(HnswAccelError::InvalidDimension(dim));
        }
        if dim != codebook.m * codebook.sub_dim {
            return Err(HnswAccelError::InvalidParam(format!(
                "dim {dim} != m {} * sub_dim {}",
                codebook.m, codebook.sub_dim
            )));
        }
        Ok(Self {
            codebook,
            dim,
            metric: DistanceMetric::L2,
        })
    }

    /// 编码单个向量为 PQ 码
    ///
    /// 返回 M 字节的 u8 编码
    pub fn encode(&self, vector: &[f32]) -> Result<Vec<u8>, HnswAccelError> {
        if vector.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(vector.len()));
        }
        let mut code = Vec::with_capacity(self.codebook.m);
        for seg in 0..self.codebook.m {
            let offset = seg * self.codebook.sub_dim;
            let sub_vec = &vector[offset..offset + self.codebook.sub_dim];
            let mut best_idx = 0u8;
            let mut best_dist = f32::MAX;
            for k in 0..self.codebook.centroids {
                let centroid = self.codebook.centroid(seg, k);
                let dist = squared_l2_scalar(sub_vec, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = k as u8;
                }
            }
            code.push(best_idx);
        }
        Ok(code)
    }

    /// 批量编码
    pub fn encode_batch(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<u8>>, HnswAccelError> {
        vectors.iter().map(|v| self.encode(v)).collect()
    }

    /// 计算 query（原始 f32）到 encoded_vector（PQ 码）的 ADC 距离
    ///
    /// ADC（Asymmetric Distance Computation）：
    /// 1. 对 query 的每段子向量，计算到所有中心的 L2 距离平方，构建查找表
    /// 2. 对编码向量的每段，查表累加距离
    pub fn distance_encoded(&self, query: &[f32], encoded: &[u8]) -> Result<f32, HnswAccelError> {
        if query.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(query.len()));
        }
        if encoded.len() != self.codebook.m {
            return Err(HnswAccelError::EncodingFailed(format!(
                "encoded length {} != m {}",
                encoded.len(),
                self.codebook.m
            )));
        }

        // 构建 ADC 查找表：[m][centroids]
        let mut lookup_table = vec![0.0f32; self.codebook.m * self.codebook.centroids];
        for seg in 0..self.codebook.m {
            let offset = seg * self.codebook.sub_dim;
            let sub_query = &query[offset..offset + self.codebook.sub_dim];
            for k in 0..self.codebook.centroids {
                let centroid = self.codebook.centroid(seg, k);
                lookup_table[seg * self.codebook.centroids + k] =
                    squared_l2_scalar(sub_query, centroid);
            }
        }

        // 查表累加
        let mut total = 0.0f32;
        for seg in 0..self.codebook.m {
            let idx = encoded[seg] as usize;
            total += lookup_table[seg * self.codebook.centroids + idx];
        }

        Ok(total.sqrt().max(0.0))
    }

    /// 批量计算 query 到多个编码向量的 ADC 距离
    pub fn batch_distance_encoded(
        &self,
        query: &[f32],
        encoded_vectors: &[&[u8]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        if query.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(query.len()));
        }

        // 预计算查找表（只计算一次）
        let mut lookup_table = vec![0.0f32; self.codebook.m * self.codebook.centroids];
        for seg in 0..self.codebook.m {
            let offset = seg * self.codebook.sub_dim;
            let sub_query = &query[offset..offset + self.codebook.sub_dim];
            for k in 0..self.codebook.centroids {
                let centroid = self.codebook.centroid(seg, k);
                lookup_table[seg * self.codebook.centroids + k] =
                    squared_l2_scalar(sub_query, centroid);
            }
        }

        let mut results = Vec::with_capacity(encoded_vectors.len());
        for encoded in encoded_vectors {
            if encoded.len() != self.codebook.m {
                return Err(HnswAccelError::EncodingFailed(format!(
                    "encoded length {} != m {}",
                    encoded.len(),
                    self.codebook.m
                )));
            }
            let mut total = 0.0f32;
            for seg in 0..self.codebook.m {
                let idx = encoded[seg] as usize;
                total += lookup_table[seg * self.codebook.centroids + idx];
            }
            results.push(total.sqrt().max(0.0));
        }
        Ok(results)
    }

    /// 码本引用
    pub fn codebook(&self) -> &PqCodebook {
        &self.codebook
    }

    /// 原始维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 编码后单向量内存占用（字节）
    pub fn encoded_size(&self) -> usize {
        self.codebook.m
    }

    /// 压缩比（原始 / 编码）
    pub fn compression_ratio(&self) -> f64 {
        (self.dim * std::mem::size_of::<f32>()) as f64 / self.encoded_size() as f64
    }
}

impl DistanceAccel for PqBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        if a.len() != self.dim || b.len() != self.dim {
            return Err(HnswAccelError::InvalidDimension(a.len()));
        }
        // 对两个原始向量：编码 b，然后 ADC 计算
        let encoded_b = self.encode(b)?;
        self.distance_encoded(a, &encoded_b)
    }

    fn batch_distance(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        // 预编码所有向量
        let encoded: Vec<Vec<u8>> = vectors
            .iter()
            .map(|v| self.encode(v))
            .collect::<Result<_, _>>()?;
        let encoded_refs: Vec<&[u8]> = encoded.iter().map(|e| e.as_slice()).collect();
        self.batch_distance_encoded(query, &encoded_refs)
    }

    fn name(&self) -> &'static str {
        "pq"
    }

    fn metric(&self) -> DistanceMetric {
        self.metric
    }
}

// =====================================================================
//  SQ — Scalar Quantization
// =====================================================================

/// SQ 训练参数
#[derive(Debug, Clone)]
pub struct SqParams {
    /// 量化位数（当前只支持 8）
    pub bits: u8,
    /// 随机种子
    pub seed: u64,
}

impl Default for SqParams {
    fn default() -> Self {
        Self {
            bits: 8,
            seed: 0x5EED_5EED_5EED_5EED,
        }
    }
}

/// SQ 量化参数（每维 min/max）
#[derive(Debug, Clone)]
pub struct SqCodebook {
    /// 每维最小值
    mins: Vec<f32>,
    /// 每维最大值
    maxs: Vec<f32>,
    /// 每维缩放因子：(max - min) / 255
    scales: Vec<f32>,
    /// 维度
    dim: usize,
}

impl SqCodebook {
    /// 维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 每维最小值
    pub fn mins(&self) -> &[f32] {
        &self.mins
    }

    /// 每维最大值
    pub fn maxs(&self) -> &[f32] {
        &self.maxs
    }

    /// 每维缩放因子
    pub fn scales(&self) -> &[f32] {
        &self.scales
    }

    /// 码本内存占用（字节）
    pub fn memory_bytes(&self) -> usize {
        self.mins.len() * std::mem::size_of::<f32>() * 3
    }
}

/// SQ 训练器
pub struct SqTrainer {
    params: SqParams,
}

impl SqTrainer {
    /// 创建 SQ 训练器
    pub fn new(params: SqParams) -> Result<Self, HnswAccelError> {
        if params.bits != 8 {
            return Err(HnswAccelError::InvalidParam(
                "only 8-bit SQ is supported".to_string(),
            ));
        }
        Ok(Self { params })
    }

    /// 训练 SQ 量化参数
    pub fn train(&self, data: &[Vec<f32>], dim: usize) -> Result<SqCodebook, HnswAccelError> {
        if dim == 0 {
            return Err(HnswAccelError::InvalidDimension(dim));
        }
        if data.is_empty() {
            return Err(HnswAccelError::TrainingFailed(
                "training data is empty".to_string(),
            ));
        }
        for v in data {
            if v.len() != dim {
                return Err(HnswAccelError::InvalidDimension(v.len()));
            }
        }

        let mut mins = vec![f32::MAX; dim];
        let mut maxs = vec![f32::MIN; dim];

        for v in data {
            for (d, &val) in v.iter().enumerate() {
                if val < mins[d] {
                    mins[d] = val;
                }
                if val > maxs[d] {
                    maxs[d] = val;
                }
            }
        }

        // 处理常量维度（min == max）
        let scales: Vec<f32> = (0..dim)
            .map(|d| {
                let range = maxs[d] - mins[d];
                if range < 1e-12 {
                    1.0 // 避免除零
                } else {
                    range / 255.0
                }
            })
            .collect();

        Ok(SqCodebook {
            mins,
            maxs,
            scales,
            dim,
        })
    }
}

/// SQ 量化后端
///
/// 将 f32 向量线性量化为 u8 向量，内存节省 4x。
#[derive(Debug, Clone)]
pub struct SqBackend {
    /// 量化参数
    codebook: SqCodebook,
    /// 距离度量
    metric: DistanceMetric,
}

impl SqBackend {
    /// 从码本创建 SQ 后端
    pub fn from_codebook(codebook: SqCodebook, metric: DistanceMetric) -> Self {
        Self { codebook, metric }
    }

    /// 编码单个向量
    pub fn encode(&self, vector: &[f32]) -> Result<Vec<u8>, HnswAccelError> {
        if vector.len() != self.codebook.dim {
            return Err(HnswAccelError::InvalidDimension(vector.len()));
        }
        Ok((0..self.codebook.dim)
            .map(|d| {
                let val = vector[d];
                let normalized = (val - self.codebook.mins[d]) / self.codebook.scales[d];
                normalized.round().clamp(0.0, 255.0) as u8
            })
            .collect())
    }

    /// 批量编码
    pub fn encode_batch(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<u8>>, HnswAccelError> {
        vectors.iter().map(|v| self.encode(v)).collect()
    }

    /// 解码 u8 向量为 f32 向量
    pub fn decode(&self, encoded: &[u8]) -> Result<Vec<f32>, HnswAccelError> {
        if encoded.len() != self.codebook.dim {
            return Err(HnswAccelError::InvalidDimension(encoded.len()));
        }
        Ok((0..self.codebook.dim)
            .map(|d| self.codebook.mins[d] + encoded[d] as f32 * self.codebook.scales[d])
            .collect())
    }

    /// 计算 query（原始 f32）到 encoded_vector（u8）的距离
    pub fn distance_encoded(&self, query: &[f32], encoded: &[u8]) -> Result<f32, HnswAccelError> {
        if query.len() != self.codebook.dim {
            return Err(HnswAccelError::InvalidDimension(query.len()));
        }
        if encoded.len() != self.codebook.dim {
            return Err(HnswAccelError::InvalidDimension(encoded.len()));
        }
        let mut sum = 0.0f32;
        for d in 0..self.codebook.dim {
            let decoded_val = self.codebook.mins[d] + encoded[d] as f32 * self.codebook.scales[d];
            let diff = query[d] - decoded_val;
            sum += diff * diff;
        }
        Ok(sum.sqrt().max(0.0))
    }

    /// 码本引用
    pub fn codebook(&self) -> &SqCodebook {
        &self.codebook
    }

    /// 编码后单向量内存占用（字节）
    pub fn encoded_size(&self) -> usize {
        self.codebook.dim
    }

    /// 压缩比
    pub fn compression_ratio(&self) -> f64 {
        (self.codebook.dim * std::mem::size_of::<f32>()) as f64 / self.encoded_size() as f64
    }
}

impl DistanceAccel for SqBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        if a.len() != self.codebook.dim || b.len() != self.codebook.dim {
            return Err(HnswAccelError::InvalidDimension(a.len()));
        }
        let encoded_b = self.encode(b)?;
        self.distance_encoded(a, &encoded_b)
    }

    fn name(&self) -> &'static str {
        "sq"
    }

    fn metric(&self) -> DistanceMetric {
        self.metric
    }
}

// =====================================================================
//  HybridPqBackend — HNSW + PQ 混合后端
// =====================================================================

/// HNSW + PQ 混合后端
///
/// 在 HNSW 图搜索中使用 PQ 压缩向量进行快速距离近似，
/// 最终对 top-K 候选使用原始向量精确重排。
#[derive(Debug, Clone)]
pub struct HybridPqBackend {
    /// PQ 后端（用于图搜索阶段）
    pq: PqBackend,
    /// HNSW M 参数
    hnsw_m: usize,
    /// 原始向量维度
    dim: usize,
}

impl HybridPqBackend {
    /// 创建混合后端
    pub fn new(pq: PqBackend, hnsw_m: usize) -> Result<Self, HnswAccelError> {
        if hnsw_m == 0 {
            return Err(HnswAccelError::InvalidParam(
                "hnsw_m must be > 0".to_string(),
            ));
        }
        let dim = pq.dim();
        Ok(Self { pq, hnsw_m, dim })
    }

    /// HNSW M 参数
    pub fn hnsw_m(&self) -> usize {
        self.hnsw_m
    }

    /// PQ 后端引用
    pub fn pq(&self) -> &PqBackend {
        &self.pq
    }

    /// 维度
    pub fn dim(&self) -> usize {
        self.dim
    }
}

impl DistanceAccel for HybridPqBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        // 图搜索阶段使用 PQ 近似距离
        self.pq.distance(a, b)
    }

    fn name(&self) -> &'static str {
        "hybrid_pq"
    }

    fn metric(&self) -> DistanceMetric {
        self.pq.metric()
    }
}

// =====================================================================
//  DiskAnnBackend — 磁盘向量索引后端
// =====================================================================

/// DiskAnn 磁盘索引后端
///
/// 参考DiskANN论文，将向量存储在 SSD 上，内存仅保留压缩后的 PQ 编码和图结构。
/// 搜索时用内存中的 PQ 编码快速筛选，再从 SSD 读取原始向量精确计算。
#[derive(Debug, Clone)]
pub struct DiskAnnBackend {
    /// PQ 后端（内存中的压缩向量）
    pq: PqBackend,
    /// PQ 中心数
    num_pq_centroids: usize,
    /// 搜索时的 L 参数（beam width）
    l_search: usize,
}

impl DiskAnnBackend {
    /// 创建 DiskAnn 后端
    pub fn new(
        pq: PqBackend,
        num_pq_centroids: usize,
        l_search: usize,
    ) -> Result<Self, HnswAccelError> {
        if num_pq_centroids == 0 {
            return Err(HnswAccelError::InvalidParam(
                "num_pq_centroids must be > 0".to_string(),
            ));
        }
        if l_search == 0 {
            return Err(HnswAccelError::InvalidParam(
                "l_search must be > 0".to_string(),
            ));
        }
        Ok(Self {
            pq,
            num_pq_centroids,
            l_search,
        })
    }

    /// PQ 中心数
    pub fn num_pq_centroids(&self) -> usize {
        self.num_pq_centroids
    }

    /// 搜索 beam width
    pub fn l_search(&self) -> usize {
        self.l_search
    }

    /// PQ 后端引用
    pub fn pq(&self) -> &PqBackend {
        &self.pq
    }
}

impl DistanceAccel for DiskAnnBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        // 磁盘索引搜索阶段用 PQ 近似
        self.pq.distance(a, b)
    }

    fn name(&self) -> &'static str {
        "diskann"
    }

    fn metric(&self) -> DistanceMetric {
        self.pq.metric()
    }
}

// =====================================================================
//  GpuBackend — GPU 加速后端（桩实现）
// =====================================================================

/// GPU 加速后端（桩实现）
///
/// 当前为桩实现，内部使用 SIMD/标量计算。
/// 要启用真正的 GPU 加速，需添加 `wgpu` 依赖并实现 compute shader。
#[derive(Debug, Clone)]
pub struct GpuBackend {
    /// 设备 ID（0 = 默认设备）
    device_id: u32,
    /// 内部使用 SimdBackend
    inner: SimdBackend,
}

impl GpuBackend {
    /// 创建 GPU 后端
    pub fn new(device_id: u32, dim: usize) -> Result<Self, HnswAccelError> {
        let inner = SimdBackend::new(SimdArch::detect(), dim)?;
        Ok(Self { device_id, inner })
    }

    /// 设备 ID
    pub fn device_id(&self) -> u32 {
        self.device_id
    }
}

impl DistanceAccel for GpuBackend {
    fn distance(&self, a: &[f32], b: &[f32]) -> Result<f32, HnswAccelError> {
        self.inner.distance(a, b)
    }

    fn batch_distance(
        &self,
        query: &[f32],
        vectors: &[&[f32]],
    ) -> Result<Vec<f32>, HnswAccelError> {
        self.inner.batch_distance(query, vectors)
    }

    fn name(&self) -> &'static str {
        "gpu"
    }

    fn metric(&self) -> DistanceMetric {
        self.inner.metric()
    }
}

// =====================================================================
//  HnswAccelBackend — 统一枚举
// =====================================================================

/// HNSW 加速后端配置枚举
///
/// 用于配置 HNSW 索引使用的距离计算后端。
#[derive(Debug, Clone, PartialEq)]
pub enum HnswAccelBackend {
    /// SIMD 加速后端
    Simd {
        /// SIMD 架构
        arch: SimdArch,
    },
    /// GPU 加速后端
    Gpu {
        /// 设备 ID
        device_id: u32,
    },
    /// AVX-512 加速后端
    Avx512,
    /// Product Quantization 压缩后端
    PQ {
        /// 子向量段数 M
        m: usize,
        /// 每段聚类中心数
        centroids: usize,
    },
    /// Scalar Quantization 压缩后端
    SQ {
        /// 量化位数（当前只支持 8）
        bits: u8,
    },
    /// HNSW + PQ 混合后端
    HybridPq {
        /// HNSW M 参数
        hnsw_m: usize,
        /// PQ M 参数
        pq_m: usize,
    },
    /// DiskAnn 磁盘索引后端
    DiskAnn {
        /// PQ 中心数
        num_pq_centroids: usize,
        /// 搜索 beam width
        l_search: usize,
    },
}

impl HnswAccelBackend {
    /// 后端名称
    pub fn name(&self) -> &'static str {
        match self {
            Self::Simd { .. } => "simd",
            Self::Gpu { .. } => "gpu",
            Self::Avx512 => "avx512",
            Self::PQ { .. } => "pq",
            Self::SQ { .. } => "sq",
            Self::HybridPq { .. } => "hybrid_pq",
            Self::DiskAnn { .. } => "diskann",
        }
    }
}

impl Default for HnswAccelBackend {
    fn default() -> Self {
        Self::Simd {
            arch: SimdArch::detect(),
        }
    }
}

// =====================================================================
//  工厂函数
// =====================================================================

/// 自动检测最优后端
///
/// 根据 CPU 特性和向量维度，选择最优的加速后端。
/// - 优先选择 SIMD（AVX2 > SSE2 > 标量）
/// - PQ/SQ 需要显式训练，不在此自动选择
pub fn auto_detect(dim: usize) -> Result<Box<dyn DistanceAccel>, HnswAccelError> {
    let arch = SimdArch::detect();
    let backend = SimdBackend::new(arch, dim)?;
    Ok(Box::new(backend))
}

/// 根据配置创建后端实例
///
/// 对于 PQ/SQ/HybridPq/DiskAnn 后端，创建的是未训练实例，
/// 需要通过对应的 Trainer 训练后才能进行距离计算。
pub fn create_backend(
    config: &HnswAccelBackend,
    dim: usize,
) -> Result<Box<dyn DistanceAccel>, HnswAccelError> {
    match config {
        HnswAccelBackend::Simd { arch } => {
            let backend = SimdBackend::new(*arch, dim)?;
            Ok(Box::new(backend))
        }
        HnswAccelBackend::Gpu { device_id } => {
            let backend = GpuBackend::new(*device_id, dim)?;
            Ok(Box::new(backend))
        }
        HnswAccelBackend::Avx512 => {
            let backend = Avx512Backend::new(dim)?;
            Ok(Box::new(backend))
        }
        HnswAccelBackend::PQ { m, centroids } => {
            let params = PqParams {
                m: *m,
                centroids: *centroids,
                ..Default::default()
            };
            let trainer = PqTrainer::new(params)?;
            let empty_data: Vec<Vec<f32>> = vec![vec![0.0; dim]; (*centroids).min(2)];
            let codebook = trainer.train(&empty_data, dim)?;
            let backend = PqBackend::from_codebook(codebook, dim)?;
            Ok(Box::new(backend))
        }
        HnswAccelBackend::SQ { bits } => {
            let params = SqParams {
                bits: *bits,
                ..Default::default()
            };
            let trainer = SqTrainer::new(params)?;
            let empty_data: Vec<Vec<f32>> = vec![vec![0.0; dim]; 2];
            let codebook = trainer.train(&empty_data, dim)?;
            let backend = SqBackend::from_codebook(codebook, DistanceMetric::L2);
            Ok(Box::new(backend))
        }
        HnswAccelBackend::HybridPq { hnsw_m, pq_m } => {
            let params = PqParams {
                m: *pq_m,
                ..Default::default()
            };
            let trainer = PqTrainer::new(params)?;
            let empty_data: Vec<Vec<f32>> = vec![vec![0.0; dim]; 2];
            let codebook = trainer.train(&empty_data, dim)?;
            let pq = PqBackend::from_codebook(codebook, dim)?;
            let backend = HybridPqBackend::new(pq, *hnsw_m)?;
            Ok(Box::new(backend))
        }
        HnswAccelBackend::DiskAnn {
            num_pq_centroids,
            l_search,
        } => {
            let params = PqParams {
                centroids: *num_pq_centroids,
                ..Default::default()
            };
            let trainer = PqTrainer::new(params)?;
            let empty_data: Vec<Vec<f32>> = vec![vec![0.0; dim]; (*num_pq_centroids).min(2)];
            let codebook = trainer.train(&empty_data, dim)?;
            let pq = PqBackend::from_codebook(codebook, dim)?;
            let backend = DiskAnnBackend::new(pq, *num_pq_centroids, *l_search)?;
            Ok(Box::new(backend))
        }
    }
}

// =====================================================================
//  K-means 聚类（用于 PQ 训练）
// =====================================================================

/// 简单 K-means 聚类
///
/// - `data` — 训练向量
/// - `k` — 聚类数
/// - `dim` — 向量维度
/// - `iterations` — 迭代次数
/// - `rng` — 随机数生成器
fn kmeans(
    data: &[Vec<f32>],
    k: usize,
    dim: usize,
    iterations: usize,
    rng: &mut Lcg,
) -> Vec<Vec<f32>> {
    if data.is_empty() || k == 0 {
        return vec![vec![0.0; dim]; k];
    }

    // 1. 初始化：随机选择 k 个数据点作为初始中心
    let mut centroids: Vec<Vec<f32>> = (0..k)
        .map(|_| data[rng.next_usize(data.len())].clone())
        .collect();

    // 2. 迭代
    for _ in 0..iterations {
        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];

        // 分配
        for point in data {
            let mut best_dist = f32::MAX;
            let mut best_centroid = 0;
            for (j, centroid) in centroids.iter().enumerate() {
                let dist = squared_l2_scalar(point, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_centroid = j;
                }
            }
            for (d, &val) in point.iter().enumerate() {
                sums[best_centroid][d] += val;
            }
            counts[best_centroid] += 1;
        }

        // 更新
        for (j, centroid) in centroids.iter_mut().enumerate() {
            if counts[j] > 0 {
                for (d, c) in centroid.iter_mut().enumerate() {
                    *c = sums[j][d] / counts[j] as f32;
                }
            } else {
                // 空簇：随机重新选择
                *centroid = data[rng.next_usize(data.len())].clone();
            }
        }
    }

    centroids
}

// =====================================================================
//  测试模块
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    /// 生成确定性测试向量
    fn make_vector(dim: usize, seed: u32) -> Vec<f32> {
        (0..dim)
            .map(|i| {
                let v = ((seed.wrapping_mul(i as u32 + 1)) as f64).sin() as f32;
                v * 0.5 + 0.5
            })
            .collect()
    }

    /// 归一化向量
    fn normalize(v: &[f32]) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-12 {
            v.iter().map(|x| x / norm).collect()
        } else {
            v.to_vec()
        }
    }

    // -----------------------------------------------------------------
    //  ScalarBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_scalar_cosine_distance() {
        let dim = 128;
        let backend = ScalarBackend::new(dim, DistanceMetric::Cosine).unwrap();
        let a = normalize(&make_vector(dim, 1));
        let b = normalize(&make_vector(dim, 2));

        let dist = backend.distance(&a, &b).unwrap();
        assert!(
            (0.0..=2.0).contains(&dist),
            "cosine distance out of range: {dist}"
        );

        // 相同向量距离应为 0
        let self_dist = backend.distance(&a, &a).unwrap();
        assert!(
            self_dist.abs() < 1e-5,
            "self distance should be 0, got {self_dist}"
        );
    }

    #[test]
    fn test_scalar_l2_distance() {
        let dim = 64;
        let backend = ScalarBackend::new(dim, DistanceMetric::L2).unwrap();
        let a = make_vector(dim, 1);
        let b = make_vector(dim, 2);

        let dist = backend.distance(&a, &b).unwrap();
        assert!(dist >= 0.0, "L2 distance must be non-negative");

        // 相同向量距离应为 0
        let self_dist = backend.distance(&a, &a).unwrap();
        assert!(
            self_dist.abs() < 1e-5,
            "self L2 distance should be 0, got {self_dist}"
        );
    }

    #[test]
    fn test_scalar_batch_distance() {
        let dim = 32;
        let backend = ScalarBackend::new(dim, DistanceMetric::Cosine).unwrap();
        let query = normalize(&make_vector(dim, 1));
        let vectors = [
            normalize(&make_vector(dim, 2)),
            normalize(&make_vector(dim, 3)),
            normalize(&make_vector(dim, 4)),
        ];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let dists = backend.batch_distance(&query, &refs).unwrap();
        assert_eq!(dists.len(), 3);
        for d in &dists {
            assert!((0.0..=2.0).contains(d));
        }
    }

    #[test]
    fn test_scalar_dimension_mismatch() {
        let backend = ScalarBackend::new(128, DistanceMetric::Cosine).unwrap();
        let a = vec![0.1; 64];
        let b = vec![0.2; 128];
        let result = backend.distance(&a, &b);
        assert!(result.is_err());
    }

    #[test]
    fn test_scalar_zero_vector() {
        let backend = ScalarBackend::new(64, DistanceMetric::Cosine).unwrap();
        let zero = vec![0.0; 64];
        let nonzero = normalize(&make_vector(64, 1));
        let dist = backend.distance(&zero, &nonzero).unwrap();
        // 零向量的余弦距离定义为 1.0
        assert!(
            (dist - 1.0).abs() < 1e-5,
            "zero vector distance should be 1.0, got {dist}"
        );
    }

    // -----------------------------------------------------------------
    //  SimdBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_simd_correctness() {
        let dim = 128;
        let scalar = ScalarBackend::new(dim, DistanceMetric::Cosine).unwrap();
        let simd = SimdBackend::new(SimdArch::detect(), dim).unwrap();

        let a = normalize(&make_vector(dim, 10));
        let b = normalize(&make_vector(dim, 20));

        let dist_scalar = scalar.distance(&a, &b).unwrap();
        let dist_simd = simd.distance(&a, &b).unwrap();

        assert!(
            (dist_scalar - dist_simd).abs() < 1e-5,
            "SIMD vs scalar mismatch: scalar={dist_scalar}, simd={dist_simd}"
        );
    }

    #[test]
    fn test_simd_l2_correctness() {
        let dim = 64;
        let scalar = ScalarBackend::new(dim, DistanceMetric::L2).unwrap();
        let simd = SimdBackend::with_metric(SimdArch::detect(), dim, DistanceMetric::L2).unwrap();

        let a = make_vector(dim, 10);
        let b = make_vector(dim, 20);

        let dist_scalar = scalar.distance(&a, &b).unwrap();
        let dist_simd = simd.distance(&a, &b).unwrap();

        assert!(
            (dist_scalar - dist_simd).abs() < 1e-5,
            "SIMD L2 vs scalar mismatch: scalar={dist_scalar}, simd={dist_simd}"
        );
    }

    #[test]
    fn test_simd_batch_distance() {
        let dim = 64;
        let backend = SimdBackend::new(SimdArch::detect(), dim).unwrap();
        let query = normalize(&make_vector(dim, 1));
        let vectors = [
            normalize(&make_vector(dim, 2)),
            normalize(&make_vector(dim, 3)),
            normalize(&make_vector(dim, 4)),
        ];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let dists = backend.batch_distance(&query, &refs).unwrap();
        assert_eq!(dists.len(), 3);
    }

    #[test]
    fn test_simd_arch_detect() {
        let arch = SimdArch::detect();
        // 检测结果应该被支持
        assert!(
            arch.is_supported(),
            "detected arch {arch} should be supported"
        );
    }

    #[test]
    fn test_simd_odd_dimension() {
        // 非对齐维度（非 8 的倍数）也必须正确
        let dim = 13;
        let scalar = ScalarBackend::new(dim, DistanceMetric::Cosine).unwrap();
        let simd = SimdBackend::new(SimdArch::detect(), dim).unwrap();

        let a = normalize(&make_vector(dim, 100));
        let b = normalize(&make_vector(dim, 200));

        let dist_scalar = scalar.distance(&a, &b).unwrap();
        let dist_simd = simd.distance(&a, &b).unwrap();

        assert!(
            (dist_scalar - dist_simd).abs() < 1e-5,
            "odd dim mismatch: scalar={dist_scalar}, simd={dist_simd}"
        );
    }

    // -----------------------------------------------------------------
    //  Avx512Backend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_avx512_correctness() {
        let dim = 128;
        let scalar = ScalarBackend::new(dim, DistanceMetric::Cosine).unwrap();
        let avx512 = Avx512Backend::new(dim).unwrap();

        let a = normalize(&make_vector(dim, 10));
        let b = normalize(&make_vector(dim, 20));

        let dist_scalar = scalar.distance(&a, &b).unwrap();
        let dist_avx512 = avx512.distance(&a, &b).unwrap();

        assert!(
            (dist_scalar - dist_avx512).abs() < 1e-5,
            "AVX512 vs scalar mismatch: scalar={dist_scalar}, avx512={dist_avx512}"
        );
    }

    #[test]
    fn test_avx512_batch() {
        let dim = 64;
        let backend = Avx512Backend::new(dim).unwrap();
        let query = normalize(&make_vector(dim, 1));
        let vectors = [
            normalize(&make_vector(dim, 2)),
            normalize(&make_vector(dim, 3)),
        ];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let dists = backend.batch_distance(&query, &refs).unwrap();
        assert_eq!(dists.len(), 2);
    }

    #[test]
    fn test_avx512_fallback() {
        // Avx512Backend 应该在 CPU 不支持 AVX-512 时回退到 AVX2
        let dim = 64;
        let backend = Avx512Backend::new(dim).unwrap();
        // 距离计算应该成功（无论是否回退）
        let a = normalize(&make_vector(dim, 1));
        let b = normalize(&make_vector(dim, 2));
        let dist = backend.distance(&a, &b).unwrap();
        assert!(dist >= 0.0);
    }

    // -----------------------------------------------------------------
    //  PQ 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_pq_training() {
        let dim = 64;
        let m = 8;
        let centroids = 16;
        let params = PqParams {
            m,
            centroids,
            iterations: 10,
            seed: 42,
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..200).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();

        assert_eq!(codebook.m(), m);
        assert_eq!(codebook.centroids(), centroids);
        assert_eq!(codebook.sub_dim(), dim / m);
    }

    #[test]
    fn test_pq_encoding() {
        let dim = 64;
        let m = 8;
        let params = PqParams {
            m,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let backend = PqBackend::from_codebook(codebook, dim).unwrap();

        let vector = make_vector(dim, 999);
        let encoded = backend.encode(&vector).unwrap();
        assert_eq!(encoded.len(), m);
        for &code in &encoded {
            assert!(code < 16, "code {code} out of range");
        }
    }

    #[test]
    fn test_pq_distance_approximation() {
        let dim = 64;
        let m = 8;
        let params = PqParams {
            m,
            centroids: 32,
            iterations: 10,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..1000).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();

        // 使用训练集内的向量，量化误差应较小
        let a = make_vector(dim, 100);
        let b = make_vector(dim, 100);

        // 相同向量的 PQ 距离应接近 0（量化误差）
        // 64 维向量、m=8、32 质心，量化误差约 1.0 属正常范围
        let dist = pq.distance(&a, &b).unwrap();
        assert!(
            dist < 1.5,
            "same vector PQ distance should be small, got {dist}"
        );

        // 不同向量的 PQ 距离应为正
        let c = make_vector(dim, 101);
        let dist2 = pq.distance(&a, &c).unwrap();
        assert!(dist2 >= 0.0);
    }

    #[test]
    fn test_pq_memory_saving() {
        let dim = 128;
        let m = 8;
        let params = PqParams {
            m,
            centroids: 256,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let backend = PqBackend::from_codebook(codebook, dim).unwrap();

        let original_size = dim * std::mem::size_of::<f32>();
        let encoded_size = backend.encoded_size();
        let ratio = backend.compression_ratio();

        assert_eq!(encoded_size, m);
        assert!(
            ratio > 30.0,
            "compression ratio should be > 30x, got {ratio}"
        );
        assert!(
            encoded_size < original_size,
            "encoded {encoded_size} should be < original {original_size}"
        );
    }

    #[test]
    fn test_pq_batch_distance_encoded() {
        let dim = 32;
        let m = 4;
        let params = PqParams {
            m,
            centroids: 8,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();

        let query = make_vector(dim, 1);
        let encoded_vectors: Vec<Vec<u8>> = data
            .iter()
            .take(10)
            .map(|v| pq.encode(v).unwrap())
            .collect();
        let refs: Vec<&[u8]> = encoded_vectors.iter().map(|e| e.as_slice()).collect();

        let dists = pq.batch_distance_encoded(&query, &refs).unwrap();
        assert_eq!(dists.len(), 10);
        for d in &dists {
            assert!(*d >= 0.0);
        }
    }

    // -----------------------------------------------------------------
    //  SQ 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sq_training() {
        let dim = 64;
        let params = SqParams::default();
        let trainer = SqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();

        assert_eq!(codebook.dim(), dim);
    }

    #[test]
    fn test_sq_encode_decode() {
        let dim = 32;
        let params = SqParams::default();
        let trainer = SqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let backend = SqBackend::from_codebook(codebook, DistanceMetric::L2);

        let original = make_vector(dim, 999);
        let encoded = backend.encode(&original).unwrap();
        let decoded = backend.decode(&encoded).unwrap();

        // 解码后应接近原始值（8-bit 量化有误差）
        for i in 0..dim {
            let diff = (original[i] - decoded[i]).abs();
            let range = backend.codebook().maxs()[i] - backend.codebook().mins()[i];
            let tolerance = range / 255.0 * 2.0; // 2 个量化级别
            assert!(
                diff <= tolerance,
                "decode error at {i}: diff={diff}, tolerance={tolerance}"
            );
        }
    }

    #[test]
    fn test_sq_distance() {
        let dim = 32;
        let params = SqParams::default();
        let trainer = SqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let backend = SqBackend::from_codebook(codebook, DistanceMetric::L2);

        let a = make_vector(dim, 50);
        let b = make_vector(dim, 50);
        let dist = backend.distance(&a, &b).unwrap();
        // 相同向量的 SQ 距离应很小
        assert!(
            dist < 0.5,
            "same vector SQ distance should be small, got {dist}"
        );
    }

    #[test]
    fn test_sq_memory_saving() {
        let dim = 128;
        let params = SqParams::default();
        let trainer = SqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let backend = SqBackend::from_codebook(codebook, DistanceMetric::L2);

        let original_size = dim * std::mem::size_of::<f32>();
        let encoded_size = backend.encoded_size();
        let ratio = backend.compression_ratio();

        assert_eq!(encoded_size, dim);
        // f32(4 bytes) -> u8(1 byte) = 4x 压缩
        assert!(
            (ratio - 4.0).abs() < 0.01,
            "compression ratio should be 4x, got {ratio}"
        );
        assert!(encoded_size < original_size);
    }

    // -----------------------------------------------------------------
    //  HybridPqBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_hybrid_pq_correctness() {
        let dim = 64;
        let m = 8;
        let params = PqParams {
            m,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();
        let hybrid = HybridPqBackend::new(pq, 16).unwrap();

        let a = make_vector(dim, 1);
        let b = make_vector(dim, 2);
        let dist = hybrid.distance(&a, &b).unwrap();
        assert!(dist >= 0.0);
    }

    #[test]
    fn test_hybrid_pq_memory() {
        let dim = 128;
        let m = 8;
        let params = PqParams {
            m,
            centroids: 32,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..100).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();
        let hybrid = HybridPqBackend::new(pq, 16).unwrap();

        // 混合后端使用 PQ 编码，内存节省与 PQ 相同
        assert_eq!(hybrid.pq().encoded_size(), m);
        assert!(hybrid.pq().compression_ratio() > 30.0);
    }

    #[test]
    fn test_hybrid_pq_params() {
        let dim = 64;
        let params = PqParams {
            m: 8,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();
        let hybrid = HybridPqBackend::new(pq, 16).unwrap();

        assert_eq!(hybrid.hnsw_m(), 16);
        assert_eq!(hybrid.dim(), dim);
    }

    // -----------------------------------------------------------------
    //  DiskAnnBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_disk_ann_setup() {
        let dim = 64;
        let params = PqParams {
            m: 8,
            centroids: 32,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();
        let diskann = DiskAnnBackend::new(pq, 32, 50).unwrap();

        assert_eq!(diskann.num_pq_centroids(), 32);
        assert_eq!(diskann.l_search(), 50);
    }

    #[test]
    fn test_disk_ann_distance() {
        let dim = 64;
        let params = PqParams {
            m: 8,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();
        let diskann = DiskAnnBackend::new(pq, 16, 50).unwrap();

        let a = make_vector(dim, 1);
        let b = make_vector(dim, 2);
        let dist = diskann.distance(&a, &b).unwrap();
        assert!(dist >= 0.0);
    }

    #[test]
    fn test_disk_ann_invalid_params() {
        let dim = 64;
        let params = PqParams {
            m: 8,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = (0..50).map(|i| make_vector(dim, i)).collect();
        let codebook = trainer.train(&data, dim).unwrap();
        let pq = PqBackend::from_codebook(codebook, dim).unwrap();

        // l_search = 0 应该报错
        let result = DiskAnnBackend::new(pq.clone(), 16, 0);
        assert!(result.is_err());

        // num_pq_centroids = 0 应该报错
        let result2 = DiskAnnBackend::new(pq, 0, 50);
        assert!(result2.is_err());
    }

    // -----------------------------------------------------------------
    //  GpuBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_gpu_stub_distance() {
        let dim = 64;
        let gpu = GpuBackend::new(0, dim).unwrap();

        let a = normalize(&make_vector(dim, 1));
        let b = normalize(&make_vector(dim, 2));
        let dist = gpu.distance(&a, &b).unwrap();
        assert!((0.0..=2.0).contains(&dist));
    }

    #[test]
    fn test_gpu_batch() {
        let dim = 64;
        let gpu = GpuBackend::new(0, dim).unwrap();
        let query = normalize(&make_vector(dim, 1));
        let vectors = [
            normalize(&make_vector(dim, 2)),
            normalize(&make_vector(dim, 3)),
        ];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let dists = gpu.batch_distance(&query, &refs).unwrap();
        assert_eq!(dists.len(), 2);
    }

    #[test]
    fn test_gpu_device_id() {
        let gpu = GpuBackend::new(1, 64).unwrap();
        assert_eq!(gpu.device_id(), 1);
    }

    // -----------------------------------------------------------------
    //  auto_detect / create_backend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_auto_detect() {
        let backend = auto_detect(128).unwrap();
        assert!(!backend.name().is_empty());
        let a = normalize(&make_vector(128, 1));
        let b = normalize(&make_vector(128, 2));
        let dist = backend.distance(&a, &b).unwrap();
        assert!(dist >= 0.0);
    }

    #[test]
    fn test_create_backend_simd() {
        let arch = SimdArch::detect();
        let config = HnswAccelBackend::Simd { arch };
        let backend = create_backend(&config, 64).unwrap();
        let expected = format!("simd_{}", arch.name());
        assert_eq!(backend.name(), expected);
    }

    #[test]
    fn test_create_backend_scalar() {
        let config = HnswAccelBackend::Simd {
            arch: SimdArch::Scalar,
        };
        let backend = create_backend(&config, 64).unwrap();
        let a = normalize(&make_vector(64, 1));
        let b = normalize(&make_vector(64, 2));
        let dist = backend.distance(&a, &b).unwrap();
        assert!(dist >= 0.0);
    }

    #[test]
    fn test_create_backend_avx512() {
        let config = HnswAccelBackend::Avx512;
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "avx512");
    }

    #[test]
    fn test_create_backend_gpu() {
        let config = HnswAccelBackend::Gpu { device_id: 0 };
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "gpu");
    }

    #[test]
    fn test_create_backend_sq() {
        let config = HnswAccelBackend::SQ { bits: 8 };
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "sq");
    }

    #[test]
    fn test_create_backend_pq() {
        let config = HnswAccelBackend::PQ {
            m: 8,
            centroids: 16,
        };
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "pq");
    }

    #[test]
    fn test_create_backend_hybrid_pq() {
        let config = HnswAccelBackend::HybridPq {
            hnsw_m: 16,
            pq_m: 8,
        };
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "hybrid_pq");
    }

    #[test]
    fn test_create_backend_diskann() {
        let config = HnswAccelBackend::DiskAnn {
            num_pq_centroids: 16,
            l_search: 50,
        };
        let backend = create_backend(&config, 64).unwrap();
        assert_eq!(backend.name(), "diskann");
    }

    // -----------------------------------------------------------------
    //  SimdArch 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_simd_arch_names() {
        assert_eq!(SimdArch::Scalar.name(), "scalar");
        assert_eq!(SimdArch::Sse2.name(), "sse2");
        assert_eq!(SimdArch::Sse42.name(), "sse4.2");
        assert_eq!(SimdArch::Avx2.name(), "avx2");
        assert_eq!(SimdArch::Avx512.name(), "avx512");
        assert_eq!(SimdArch::Neon.name(), "neon");
    }

    #[test]
    fn test_simd_arch_display() {
        let arch = SimdArch::Avx2;
        assert_eq!(format!("{arch}"), "avx2");
    }

    #[test]
    fn test_simd_arch_scalar_always_supported() {
        assert!(SimdArch::Scalar.is_supported());
    }

    // -----------------------------------------------------------------
    //  HnswAccelBackend 枚举测试
    // -----------------------------------------------------------------

    #[test]
    fn test_backend_names() {
        assert_eq!(
            HnswAccelBackend::Simd {
                arch: SimdArch::Avx2
            }
            .name(),
            "simd"
        );
        assert_eq!(HnswAccelBackend::Avx512.name(), "avx512");
        assert_eq!(
            HnswAccelBackend::PQ {
                m: 8,
                centroids: 256
            }
            .name(),
            "pq"
        );
        assert_eq!(HnswAccelBackend::SQ { bits: 8 }.name(), "sq");
        assert_eq!(
            HnswAccelBackend::HybridPq {
                hnsw_m: 16,
                pq_m: 8
            }
            .name(),
            "hybrid_pq"
        );
        assert_eq!(
            HnswAccelBackend::DiskAnn {
                num_pq_centroids: 256,
                l_search: 100
            }
            .name(),
            "diskann"
        );
    }

    #[test]
    fn test_backend_default() {
        let backend = HnswAccelBackend::default();
        match backend {
            HnswAccelBackend::Simd { .. } => {}
            _ => panic!("default should be Simd"),
        }
    }

    // -----------------------------------------------------------------
    //  错误处理测试
    // -----------------------------------------------------------------

    #[test]
    fn test_invalid_dimension_error() {
        let result = ScalarBackend::new(0, DistanceMetric::Cosine);
        assert!(matches!(result, Err(HnswAccelError::InvalidDimension(0))));
    }

    #[test]
    fn test_pq_invalid_m() {
        let params = PqParams {
            m: 0,
            centroids: 256,
            iterations: 10,
            seed: 42,
        };
        let result = PqTrainer::new(params);
        assert!(matches!(result, Err(HnswAccelError::InvalidParam(_))));
    }

    #[test]
    fn test_pq_centroids_out_of_range() {
        let params = PqParams {
            m: 8,
            centroids: 512,
            iterations: 10,
            seed: 42,
        };
        let result = PqTrainer::new(params);
        assert!(matches!(result, Err(HnswAccelError::InvalidParam(_))));
    }

    #[test]
    fn test_sq_invalid_bits() {
        let params = SqParams { bits: 4, seed: 42 };
        let result = SqTrainer::new(params);
        assert!(matches!(result, Err(HnswAccelError::InvalidParam(_))));
    }

    #[test]
    fn test_pq_dim_not_divisible() {
        let params = PqParams {
            m: 8,
            centroids: 16,
            ..Default::default()
        };
        let trainer = PqTrainer::new(params).unwrap();
        let data: Vec<Vec<f32>> = vec![vec![0.0; 10]];
        let result = trainer.train(&data, 10);
        assert!(matches!(result, Err(HnswAccelError::InvalidParam(_))));
    }
}
