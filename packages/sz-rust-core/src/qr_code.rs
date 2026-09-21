// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 二维码生成模块 — 对齐 PHP `endroid/qr-code`
//!
//! 提供二维码生成功能，支持 PNG/SVG 输出和原始矩阵获取。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 类 | Rust 结构 | 说明 |
//! |---------|-----------|------|
//! | `Endroid\QrCode\QrCode` | [`QrCodeGenerator`] | 二维码生成器 |
//! | `Endroid\QrCode\QrCode::setSize()` | [`QrCodeConfig::with_size`] | 设置尺寸 |
//! | `Endroid\QrCode\QrCode::setMargin()` | [`QrCodeConfig::with_margin`] | 设置边距 |
//! | `Endroid\QrCode\QrCode::setForegroundColor()` | [`QrCodeConfig::with_foreground_color`] | 前景色 |
//! | `Endroid\QrCode\QrCode::setBackgroundColor()` | [`QrCodeConfig::with_background_color`] | 背景色 |
//! | `Endroid\QrCode\QrCode::setErrorCorrectionLevel()` | [`QrCodeConfig::with_error_correction_level`] | 容错级别 |
//! | `Endroid\QrCode\ErrorCorrectionLevel` | [`ErrorCorrectionLevel`] | 容错级别枚举 |
//! | `Endroid\QrCode\QrCode::writeString()` (PNG) | [`QrCodeGenerator::generate_png`] | 生成 PNG |
//! | `Endroid\QrCode\QrCode::writeString()` (SVG) | [`QrCodeGenerator::generate_svg`] | 生成 SVG |
//!
//! ### PHP 行为对齐
//!
//! - **容错级别**：PHP 支持 Low(7%)/Medium(15%)/Quartile(25%)/High(30%)，Rust 通过 [`ErrorCorrectionLevel`] 表达。
//! - **尺寸与边距**：PHP `setSize(size)` + `setMargin(margin)`，Rust 通过 [`QrCodeConfig`] 的 `size`/`margin` 字段表达。
//! - **前景/背景色**：PHP 使用 RGBA 数组，Rust 简化为 RGB `[u8; 3]`。
//! - **PNG 输出**：先从 `qrcode` crate 获取矩阵，再用 `image` crate 渲染为 PNG（像素级边距控制）。
//! - **SVG 输出**：直接使用 `qrcode` crate 的 SVG 渲染器（`qrcode::render::svg::Color`）。
//!
//! ## Rust 用法
//!
//! ```rust,ignore
//! use sz_rust_core::qr_code::{QrCodeGenerator, QrCodeConfig, ErrorCorrectionLevel};
//!
//! // 默认配置
//! let generator = QrCodeGenerator::new();
//! let png_bytes = generator.generate_png("https://example.com").unwrap();
//! let svg_string = generator.generate_svg("https://example.com").unwrap();
//!
//! // 自定义配置
//! let config = QrCodeConfig::new()
//!     .with_size(300)
//!     .with_margin(20)
//!     .with_foreground_color([0, 0, 255])
//!     .with_error_correction_level(ErrorCorrectionLevel::High);
//! let generator = QrCodeGenerator::with_config(config);
//! let png_bytes = generator.generate_png("Hello").unwrap();
//! ```

use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
use qrcode::render::svg;
use qrcode::types::Color as QrColor;
use qrcode::{EcLevel, QrCode};
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// 二维码生成错误 — 对齐 PHP `endroid\qr-code` 异常
#[derive(Debug, Error)]
pub enum QrCodeError {
    /// 二维码生成失败（数据过长、版本不兼容等）
    #[error("二维码生成失败: {0}")]
    Generation(String),

    /// 数据编码失败（空数据、无效字符等）
    #[error("数据编码失败: {0}")]
    Encoding(String),

    /// IO 错误（PNG 编码写入失败等）
    #[error("IO 错误: {0}")]
    Io(String),
}

impl From<std::io::Error> for QrCodeError {
    fn from(err: std::io::Error) -> Self {
        QrCodeError::Io(err.to_string())
    }
}

impl From<image::ImageError> for QrCodeError {
    fn from(err: image::ImageError) -> Self {
        QrCodeError::Io(err.to_string())
    }
}

// ============================================================================
// 容错级别枚举 — 对齐 PHP endroid\qr-code\ErrorCorrectionLevel
// ============================================================================

/// 二维码容错级别 — 对齐 PHP `Endroid\QrCode\ErrorCorrectionLevel`
///
/// 容错级别越高，二维码能容忍的损坏面积越大，但数据密度越低。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorCorrectionLevel {
    /// 低容错 — 允许 7% 损坏
    Low,
    /// 中等容错（默认）— 允许 15% 损坏
    #[default]
    Medium,
    /// 四分位容错 — 允许 25% 损坏
    Quartile,
    /// 高容错 — 允许 30% 损坏
    High,
}

impl ErrorCorrectionLevel {
    /// 转换为 `qrcode` crate 的 `EcLevel`
    fn to_ec_level(self) -> EcLevel {
        match self {
            Self::Low => EcLevel::L,
            Self::Medium => EcLevel::M,
            Self::Quartile => EcLevel::Q,
            Self::High => EcLevel::H,
        }
    }

    /// 转换为字符串标识（对齐 PHP `ErrorCorrectionLevel` 类名）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::Quartile => "quartile",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for ErrorCorrectionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ============================================================================
// 二维码配置 — 对齐 PHP endroid\qr-code\QrCode
// ============================================================================

/// 二维码配置 — 对齐 PHP `Endroid\QrCode\QrCode`
///
/// 使用 Builder 模式构建配置，通过 [`QrCodeGenerator::with_config`] 创建生成器。
#[derive(Debug, Clone)]
pub struct QrCodeConfig {
    /// 尺寸（像素，默认 200）
    pub size: u32,
    /// 边距（像素，默认 10）
    pub margin: u32,
    /// 前景色（RGB，默认黑色 `[0, 0, 0]`）
    pub foreground_color: [u8; 3],
    /// 背景色（RGB，默认白色 `[255, 255, 255]`）
    pub background_color: [u8; 3],
    /// 容错级别（默认 `Medium`）
    pub error_correction_level: ErrorCorrectionLevel,
}

impl Default for QrCodeConfig {
    fn default() -> Self {
        Self {
            size: 200,
            margin: 10,
            foreground_color: [0, 0, 0],
            background_color: [255, 255, 255],
            error_correction_level: ErrorCorrectionLevel::Medium,
        }
    }
}

impl QrCodeConfig {
    /// 创建默认配置
    ///
    /// - 尺寸：200px
    /// - 边距：10px
    /// - 前景色：黑色 `[0, 0, 0]`
    /// - 背景色：白色 `[255, 255, 255]`
    /// - 容错级别：`Medium`
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置尺寸（像素）
    pub fn with_size(mut self, size: u32) -> Self {
        self.size = size;
        self
    }

    /// 设置边距（像素）
    pub fn with_margin(mut self, margin: u32) -> Self {
        self.margin = margin;
        self
    }

    /// 设置前景色（RGB）
    pub fn with_foreground_color(mut self, color: [u8; 3]) -> Self {
        self.foreground_color = color;
        self
    }

    /// 设置背景色（RGB）
    pub fn with_background_color(mut self, color: [u8; 3]) -> Self {
        self.background_color = color;
        self
    }

    /// 设置容错级别
    pub fn with_error_correction_level(mut self, level: ErrorCorrectionLevel) -> Self {
        self.error_correction_level = level;
        self
    }

    /// 将 RGB 颜色转换为 CSS hex 字符串（用于 SVG 渲染）
    fn color_to_hex(color: [u8; 3]) -> String {
        format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
    }
}

// ============================================================================
// 二维码生成器 — 对齐 PHP endroid\qr-code\QrCode
// ============================================================================

/// 二维码生成器 — 对齐 PHP `Endroid\QrCode\QrCode`
///
/// 持有 [`QrCodeConfig`] 配置，提供 PNG/SVG/矩阵三种输出方式。
///
/// # PHP 对齐
///
/// ```php
/// // PHP endroid/qr-code
/// $qr = new QrCode('Hello');
/// $qr->setSize(200);
/// $qr->setMargin(10);
/// $qr->setErrorCorrectionLevel(new ErrorCorrectionLevel(ErrorCorrectionLevel::MEDIUM));
/// $png = $qr->writeString(); // 默认 PNG
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_core::qr_code::{QrCodeGenerator, QrCodeConfig, ErrorCorrectionLevel};
///
/// let config = QrCodeConfig::new()
///     .with_size(200)
///     .with_margin(10)
///     .with_error_correction_level(ErrorCorrectionLevel::Medium);
/// let generator = QrCodeGenerator::with_config(config);
/// let png = generator.generate_png("Hello").unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct QrCodeGenerator {
    /// 二维码配置
    config: QrCodeConfig,
}

impl QrCodeGenerator {
    /// 创建默认配置的生成器
    pub fn new() -> Self {
        Self {
            config: QrCodeConfig::default(),
        }
    }

    /// 使用指定配置创建生成器
    pub fn with_config(config: QrCodeConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &QrCodeConfig {
        &self.config
    }

    /// 生成原始矩阵 — 返回 `bool` 二维数组（`true` = 深色模块）
    ///
    /// 矩阵尺寸为 `N × N`（正方形），不包含边距。
    ///
    /// # 参数
    ///
    /// - `data`: 待编码的数据（不能为空）
    ///
    /// # 返回
    ///
    /// 成功返回 `Vec<Vec<bool>>`，失败返回 [`QrCodeError`]。
    pub fn generate_matrix(&self, data: &str) -> Result<Vec<Vec<bool>>, QrCodeError> {
        if data.is_empty() {
            return Err(QrCodeError::Encoding("数据不能为空".to_string()));
        }

        let code = QrCode::with_error_correction_level(
            data.as_bytes(),
            self.config.error_correction_level.to_ec_level(),
        )
        .map_err(|e| QrCodeError::Generation(format!("二维码编码失败: {e}")))?;

        let width = code.width();
        let matrix = (0..width)
            .map(|y| (0..width).map(|x| code[(x, y)] == QrColor::Dark).collect())
            .collect();

        Ok(matrix)
    }

    /// 生成 PNG 二进制
    ///
    /// 先生成原始矩阵，再用 `image` crate 渲染为 PNG。
    /// 像素级边距控制：`size` 为总尺寸（含边距），`margin` 为四周白边宽度。
    ///
    /// # 参数
    ///
    /// - `data`: 待编码的数据（不能为空）
    ///
    /// # 返回
    ///
    /// 成功返回 PNG 字节流 `Vec<u8>`，失败返回 [`QrCodeError`]。
    pub fn generate_png(&self, data: &str) -> Result<Vec<u8>, QrCodeError> {
        let matrix = self.generate_matrix(data)?;
        let png_bytes = self.render_matrix_to_png(&matrix)?;
        Ok(png_bytes)
    }

    /// 生成 SVG 字符串
    ///
    /// 使用 `qrcode` crate 内置的 SVG 渲染器（`qrcode::render::svg::Color`）。
    /// 边距通过 quiet zone 控制（`margin > 0` 时启用）。
    ///
    /// # 参数
    ///
    /// - `data`: 待编码的数据（不能为空）
    ///
    /// # 返回
    ///
    /// 成功返回 SVG 字符串，失败返回 [`QrCodeError`]。
    pub fn generate_svg(&self, data: &str) -> Result<String, QrCodeError> {
        if data.is_empty() {
            return Err(QrCodeError::Encoding("数据不能为空".to_string()));
        }

        let code = QrCode::with_error_correction_level(
            data.as_bytes(),
            self.config.error_correction_level.to_ec_level(),
        )
        .map_err(|e| QrCodeError::Generation(format!("二维码编码失败: {e}")))?;

        let fg_hex = QrCodeConfig::color_to_hex(self.config.foreground_color);
        let bg_hex = QrCodeConfig::color_to_hex(self.config.background_color);

        let svg_string = code
            .render::<svg::Color>()
            .dark_color(svg::Color(&fg_hex))
            .light_color(svg::Color(&bg_hex))
            .quiet_zone(self.config.margin > 0)
            .min_dimensions(self.config.size, self.config.size)
            .build();

        Ok(svg_string)
    }

    /// 将矩阵渲染为 PNG 字节流
    ///
    /// `size` = 总尺寸（含边距），`margin` = 四周白边宽度。
    /// QR 码区域 = `size - 2 * margin`，按整数分块映射每个模块。
    fn render_matrix_to_png(&self, matrix: &[Vec<bool>]) -> Result<Vec<u8>, QrCodeError> {
        let matrix_width = matrix.len();
        if matrix_width == 0 {
            return Err(QrCodeError::Generation("矩阵为空".to_string()));
        }

        let total_size = self.config.size;
        let margin = self.config.margin;

        // 计算二维码区域尺寸（总尺寸减去两侧边距）
        let qr_area = total_size
            .checked_sub(margin.saturating_mul(2))
            .filter(|&v| v > 0)
            .ok_or_else(|| {
                QrCodeError::Generation(format!(
                    "尺寸不足以容纳边距: size={total_size}, margin={margin}"
                ))
            })?;

        // 每个模块的像素大小（整数除法，至少 1px）
        let module_size = (qr_area / matrix_width as u32).max(1);

        let [fr, fg, fb] = self.config.foreground_color;
        let [br, bg, bb] = self.config.background_color;
        let fg_pixel = Rgba([fr, fg, fb, 255]);
        let bg_pixel = Rgba([br, bg, bb, 255]);

        // 创建背景色填充的画布
        let mut img: RgbaImage = ImageBuffer::from_pixel(total_size, total_size, bg_pixel);

        // 绘制深色模块
        for (y, row) in matrix.iter().enumerate() {
            for (x, &is_dark) in row.iter().enumerate() {
                if is_dark {
                    let start_x = margin + (x as u32) * module_size;
                    let start_y = margin + (y as u32) * module_size;
                    for dy in 0..module_size {
                        for dx in 0..module_size {
                            let px = start_x + dx;
                            let py = start_y + dy;
                            if px < total_size && py < total_size {
                                img.put_pixel(px, py, fg_pixel);
                            }
                        }
                    }
                }
            }
        }

        // 编码为 PNG
        let dynamic = DynamicImage::ImageRgba8(img);
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        dynamic.write_to(&mut cursor, image::ImageFormat::Png)?;
        Ok(bytes)
    }
}

impl Default for QrCodeGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // QrCodeConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 QrCodeConfig 默认值
    #[test]
    fn test_qr_code_config_default() {
        let config = QrCodeConfig::default();
        assert_eq!(config.size, 200);
        assert_eq!(config.margin, 10);
        assert_eq!(config.foreground_color, [0, 0, 0]);
        assert_eq!(config.background_color, [255, 255, 255]);
        assert_eq!(config.error_correction_level, ErrorCorrectionLevel::Medium);
    }

    /// 测试 QrCodeConfig builder 链式调用
    #[test]
    fn test_qr_code_config_builder() {
        let config = QrCodeConfig::new()
            .with_size(300)
            .with_margin(20)
            .with_foreground_color([255, 0, 0])
            .with_background_color([0, 0, 255])
            .with_error_correction_level(ErrorCorrectionLevel::High);

        assert_eq!(config.size, 300);
        assert_eq!(config.margin, 20);
        assert_eq!(config.foreground_color, [255, 0, 0]);
        assert_eq!(config.background_color, [0, 0, 255]);
        assert_eq!(config.error_correction_level, ErrorCorrectionLevel::High);
    }

    // ------------------------------------------------------------------------
    // ErrorCorrectionLevel 测试
    // ------------------------------------------------------------------------

    /// 测试 ErrorCorrectionLevel 默认值和转换
    #[test]
    fn test_error_correction_level() {
        // 默认值
        assert_eq!(
            ErrorCorrectionLevel::default(),
            ErrorCorrectionLevel::Medium
        );

        // as_str
        assert_eq!(ErrorCorrectionLevel::Low.as_str(), "low");
        assert_eq!(ErrorCorrectionLevel::Medium.as_str(), "medium");
        assert_eq!(ErrorCorrectionLevel::Quartile.as_str(), "quartile");
        assert_eq!(ErrorCorrectionLevel::High.as_str(), "high");

        // Display
        assert_eq!(format!("{}", ErrorCorrectionLevel::Low), "low");
        assert_eq!(format!("{}", ErrorCorrectionLevel::High), "high");

        // to_ec_level 映射
        assert_eq!(ErrorCorrectionLevel::Low.to_ec_level(), EcLevel::L);
        assert_eq!(ErrorCorrectionLevel::Medium.to_ec_level(), EcLevel::M);
        assert_eq!(ErrorCorrectionLevel::Quartile.to_ec_level(), EcLevel::Q);
        assert_eq!(ErrorCorrectionLevel::High.to_ec_level(), EcLevel::H);
    }

    // ------------------------------------------------------------------------
    // QrCodeGenerator 测试
    // ------------------------------------------------------------------------

    /// 测试 QrCodeGenerator 默认配置
    #[test]
    fn test_qr_code_generator_default() {
        let generator = QrCodeGenerator::new();
        assert_eq!(generator.config().size, 200);
        assert_eq!(generator.config().margin, 10);
        assert_eq!(
            generator.config().error_correction_level,
            ErrorCorrectionLevel::Medium
        );
    }

    /// 测试 QrCodeGenerator 自定义配置
    #[test]
    fn test_qr_code_generator_with_config() {
        let config = QrCodeConfig::new()
            .with_size(400)
            .with_margin(15)
            .with_error_correction_level(ErrorCorrectionLevel::Quartile);
        let generator = QrCodeGenerator::with_config(config);
        assert_eq!(generator.config().size, 400);
        assert_eq!(generator.config().margin, 15);
        assert_eq!(
            generator.config().error_correction_level,
            ErrorCorrectionLevel::Quartile
        );
    }

    // ------------------------------------------------------------------------
    // generate_matrix 测试
    // ------------------------------------------------------------------------

    /// 测试矩阵生成基本功能（非空且正方形）
    #[test]
    fn test_generate_matrix_basic() {
        let generator = QrCodeGenerator::new();
        let matrix = generator.generate_matrix("Hello, World!").unwrap();

        assert!(!matrix.is_empty(), "矩阵不能为空");
        let width = matrix.len();
        for row in &matrix {
            assert_eq!(row.len(), width, "矩阵必须是正方形");
        }
    }

    /// 测试空数据返回错误
    #[test]
    fn test_generate_matrix_empty_data() {
        let generator = QrCodeGenerator::new();
        let result = generator.generate_matrix("");
        assert!(result.is_err(), "空数据应返回错误");
        match result {
            Err(QrCodeError::Encoding(_)) => {}
            other => panic!("期望 Encoding 错误，得到: {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // generate_png 测试
    // ------------------------------------------------------------------------

    /// 测试 PNG 生成基本功能（验证 PNG 头部 magic bytes）
    #[test]
    fn test_generate_png_basic() {
        let generator = QrCodeGenerator::new();
        let png = generator.generate_png("https://example.com").unwrap();
        assert!(!png.is_empty(), "PNG 字节流不能为空");

        // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
        assert_eq!(png[0], 0x89, "PNG magic byte 0");
        assert_eq!(png[1], 0x50, "PNG magic byte 1 ('P')");
        assert_eq!(png[2], 0x4E, "PNG magic byte 2 ('N')");
        assert_eq!(png[3], 0x47, "PNG magic byte 3 ('G')");
        assert_eq!(png[4], 0x0D, "PNG magic byte 4 (CR)");
        assert_eq!(png[5], 0x0A, "PNG magic byte 5 (LF)");
        assert_eq!(png[6], 0x1A, "PNG magic byte 6");
        assert_eq!(png[7], 0x0A, "PNG magic byte 7 (LF)");
    }

    /// 测试 PNG 输出有效头部
    #[test]
    fn test_generate_png_valid_output() {
        let generator = QrCodeGenerator::new();
        let png = generator.generate_png("test data 12345").unwrap();
        // 验证 PNG 签名（8 字节）
        let png_signature: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(&png[..8], &png_signature, "PNG 头部签名必须匹配");
        // 至少有 IHDR chunk
        assert!(png.len() > 24, "PNG 数据长度应超过头部+IHDR");
    }

    // ------------------------------------------------------------------------
    // generate_svg 测试
    // ------------------------------------------------------------------------

    /// 测试 SVG 生成基本功能（含 <svg> 标签）
    #[test]
    fn test_generate_svg_basic() {
        let generator = QrCodeGenerator::new();
        let svg = generator.generate_svg("Hello SVG").unwrap();
        assert!(!svg.is_empty(), "SVG 字符串不能为空");
        assert!(svg.contains("<svg"), "SVG 必须包含 <svg> 标签");
        assert!(svg.contains("</svg>"), "SVG 必须包含 </svg> 闭合标签");
    }

    /// 测试 SVG 包含二维码数据（rect/path 元素）
    #[test]
    fn test_generate_svg_contains_data() {
        let generator = QrCodeGenerator::new();
        let svg = generator.generate_svg("Data content test 12345").unwrap();
        // SVG 应包含路径或矩形元素来绘制二维码模块
        assert!(
            svg.contains("<rect") || svg.contains("<path"),
            "SVG 必须包含 rect 或 path 元素来表示二维码模块"
        );
        // 验证 SVG 中包含前景色 hex（默认黑色 #000000）
        assert!(svg.contains("#000000"), "SVG 应包含默认前景色 #000000");
    }

    // ------------------------------------------------------------------------
    // 对比测试
    // ------------------------------------------------------------------------

    /// 测试不同数据生成不同矩阵
    #[test]
    fn test_generate_different_data_different_matrix() {
        let generator = QrCodeGenerator::new();
        let matrix1 = generator.generate_matrix("data one").unwrap();
        let matrix2 = generator.generate_matrix("data two").unwrap();

        // 两个矩阵不应完全相同
        assert_ne!(matrix1, matrix2, "不同数据应生成不同的二维码矩阵");
    }

    /// 测试高容错级别生成
    #[test]
    fn test_generate_high_error_correction() {
        let config = QrCodeConfig::new().with_error_correction_level(ErrorCorrectionLevel::High);
        let generator = QrCodeGenerator::with_config(config);

        // 高容错级别应能正常生成 PNG 和 SVG
        let png = generator.generate_png("High EC test").unwrap();
        assert!(!png.is_empty(), "高容错 PNG 不应为空");

        let svg = generator.generate_svg("High EC test").unwrap();
        assert!(svg.contains("<svg"), "高容错 SVG 应包含 <svg> 标签");

        // 矩阵应能正常生成
        let matrix = generator.generate_matrix("High EC test").unwrap();
        assert!(!matrix.is_empty(), "高容错矩阵不应为空");

        // 验证配置确实为 High
        assert_eq!(
            generator.config().error_correction_level,
            ErrorCorrectionLevel::High
        );
    }
}
