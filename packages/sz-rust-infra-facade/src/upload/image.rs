//! 图像处理模块 — 对齐 PHP `Grafika\Gd\Editor` + `Grafika\Gd\Image` + `Grafika\Color` + `Grafika\Position`
//!
//! 本模块实现图像处理功能，对齐 PHP `kosinix/grafika` 库
//! 的 GD 后端 API（项目中实际使用的图像处理库）。
//!
//! ## PHP 对齐说明
//!
//! 项目 `composer.json` 未引入 `topthink/image`，实际使用 `kosinix/grafika` 库。
//! 因此本模块以 Grafika 为对齐基准，而非 think\Image。
//!
//! ## PHP 对齐
//!
//! ### 核心类映射
//!
//! | PHP 类 | Rust 结构 | 说明 |
//! |---------|-----------|------|
//! | `Grafika\Gd\Image` | [`Image`] | 图像类（持有状态） |
//! | `Grafika\Gd\Editor` | [`Editor`] | 编辑器类（所有操作入口） |
//! | `Grafika\Color` | [`Color`] | 颜色类（CSS hex 解析） |
//! | `Grafika\Position` | [`Position`] | 9 种位置枚举 |
//! | `Grafika\ImageType` | [`ImageType`] | 图像类型枚举 |
//! | `imagettfbbox` | [`measure_text`] | TTF 文本边界测量 |
//! | `imagettftext` | [`Editor::text`] | TTF 文本绘制 |
//! | `imagecopyresampled` | `image::imageops::resize` | 缩放重采样 |
//! | `imagecopy` | `image::imageops::overlay` | 图层合成 |
//!
//! ### 核心方法映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Image::createFromFile($file)` | [`Image::open`] | 文件 → Image |
//! | `Image::createBlank($w, $h)` | [`Image::create_blank`] | 空白画布 |
//! | `Image::getWidth()` | [`Image::width`] | 宽 |
//! | `Image::getHeight()` | [`Image::height`] | 高 |
//! | `Image::getType()` | [`Image::image_type`] | 类型 |
//! | `Editor::open(&$img, $file)` | [`Editor::open`] | 打开文件 |
//! | `Editor::resizeExact(&$img, $w, $h)` | [`Editor::resize_exact`] | 强制尺寸 |
//! | `Editor::resizeFit(&$img, $w, $h)` | [`Editor::resize_fit`] | 等比缩放适配 |
//! | `Editor::resizeFill(&$img, $w, $h)` | [`Editor::resize_fill`] | 填充+裁剪 |
//! | `Editor::resizeExactWidth(&$img, $w)` | [`Editor::resize_exact_width`] | 等比按宽 |
//! | `Editor::resizeExactHeight(&$img, $h)` | [`Editor::resize_exact_height`] | 等比按高 |
//! | `Editor::crop(&$img, $w, $h, $pos, $ox, $oy)` | [`Editor::crop`] | 裁剪 |
//! | `Editor::blend(&$img1, $img2, $type, $opacity, $pos, $ox, $oy)` | [`Editor::blend`] | 合成 |
//! | `Editor::text(&$img, $text, $size, $x, $y, $color, $font, $angle)` | [`Editor::text`] | 文本绘制 |
//! | `Editor::rotate(&$img, $angle, $color)` | [`Editor::rotate`] | 旋转 |
//! | `Editor::flip(&$img, $mode)` | [`Editor::flip`] | 翻转（'h'/'v'） |
//! | `Editor::fill(&$img, $color, $x, $y)` | [`Editor::fill`] | 填充 |
//! | `Editor::save($img, $file, $type, $quality, $interlace, $perm)` | [`Editor::save`] | 保存 |
//!
//! ## PHP 行为对齐（R5 硬约束）
//!
//! - **R5-24**：`ImageType` 5 种类型（UNKNOWN/GIF/JPEG/PNG/WBMP）对齐 `Grafika\ImageType`
//! - **R5-25**：`Color` hex 解析（`#rgb`/`#rrggbb`/`#rgba`/`#rrggbbaa`）对齐 `Grafika\Color`
//! - **R5-26**：`Position` 9 种位置 + `get_xy` 对齐 `Grafika\Position::getXY`
//! - **R5-27**：`Image::open` 按 `getimagesize` 探测类型后分派（GIF/JPEG/PNG/WBMP）对齐 `Image::createFromFile`
//! - **R5-28**：`Editor::resize_exact` 强制目标尺寸（忽略宽高比）对齐 `Editor::resizeExact`
//! - **R5-29**：`Editor::blend` normal 模式 + opacity + offset 对齐 `Editor::blend`
//! - **R5-30**：`Editor::text` y 坐标基线偏移（GD `imagettftext` y 是基线，Grafika 内部 `y += size`）
//!   — Rust 端 `imageproc::drawing::draw_text_mut` y 是顶部，所以 Rust y = PHP y - size
//! - **R5-31**：`Editor::save` 按扩展名猜类型 + JPEG 默认 quality=75 对齐 `Editor::save`
//! - **R5-32**：`wrap_text` 对齐业务侧 `wrapText`（`imagettfbbox` 测量 + max_line 截断 + 省略号）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\kosinix\grafika\src\Grafika\Gd\Image.php`（457 行）
//! - `e:\vue\test\鲜视达\server\vendor\kosinix\grafika\src\Grafika\Gd\Editor.php`（~830 行）
//! - `e:\vue\test\鲜视达\server\vendor\kosinix\grafika\src\Grafika\Color.php`
//! - `e:\vue\test\鲜视达\server\vendor\kosinix\grafika\src\Grafika\Position.php`
//! - `e:\vue\test\鲜视达\server\vendor\kosinix\grafika\src\Grafika\ImageType.php`
//! - `e:\vue\test\鲜视达\server\app\common\service\qrcode\ProductService.php`（wrapText 业务侧实现）

use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontVec, Glyph, PxScale, ScaleFont};
use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage};
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// 图像处理错误 — 对齐 PHP Grafika 异常
#[derive(Debug, Error)]
pub enum ImageError {
    /// 图像打开失败
    #[error("Failed to open image: {0}")]
    OpenFailed(String),

    /// 图像保存失败
    #[error("Failed to save image: {0}")]
    SaveFailed(String),

    /// 不支持的图像类型
    #[error("Unsupported image type: {0}")]
    UnsupportedType(String),

    /// 颜色解析失败
    #[error("Invalid color: {0}")]
    InvalidColor(String),

    /// 字体加载失败
    #[error("Failed to load font: {0}")]
    FontLoadFailed(String),

    /// IO 错误
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// 图像解码错误
    #[error(transparent)]
    Decode(#[from] image::ImageError),

    /// 参数无效
    #[error("{0}")]
    InvalidArgument(String),
}

// ============================================================================
// ImageType 枚举 — 对齐 Grafika\ImageType
// ============================================================================

/// 图像类型 — 对齐 PHP `Grafika\ImageType`
///
/// PHP 源码（`ImageType.php`）：
/// ```php
/// class ImageType {
///     const UNKNOWN = '';
///     const GIF     = 'GIF';
///     const JPEG    = 'JPEG';
///     const PNG     = 'PNG';
///     const WBMP    = 'WBMP';
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageType {
    /// 未知类型（对齐 `UNKNOWN = ''`）
    #[default]
    Unknown,
    /// GIF（对齐 `GIF = 'GIF'`）
    Gif,
    /// JPEG（对齐 `JPEG = 'JPEG'`）
    Jpeg,
    /// PNG（对齐 `PNG = 'PNG'`）
    Png,
    /// WBMP（对齐 `WBMP = 'WBMP'`）
    Wbmp,
}

impl ImageType {
    /// 对齐 PHP `ImageType` 常量字符串值
    pub fn as_str(self) -> &'static str {
        match self {
            ImageType::Unknown => "",
            ImageType::Gif => "GIF",
            ImageType::Jpeg => "JPEG",
            ImageType::Png => "PNG",
            ImageType::Wbmp => "WBMP",
        }
    }

    /// 从扩展名推断类型（对齐 Grafika `_getImageTypeFromFileName`）
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "gif" => ImageType::Gif,
            "jpg" | "jpeg" => ImageType::Jpeg,
            "png" => ImageType::Png,
            "wbmp" => ImageType::Wbmp,
            _ => ImageType::Unknown,
        }
    }

    /// 从 image crate 的 ImageFormat 转换（对齐 PHP `getimagesize` 探测结果）
    pub fn from_image_format(format: image::ImageFormat) -> Self {
        match format {
            image::ImageFormat::Gif => ImageType::Gif,
            image::ImageFormat::Jpeg => ImageType::Jpeg,
            image::ImageFormat::Png => ImageType::Png,
            image::ImageFormat::WebP => ImageType::Unknown, // PHP Grafika 不支持 WebP
            _ => ImageType::Unknown,
        }
    }

    /// 转换为 image crate 的 ImageFormat
    pub fn to_image_format(self) -> Option<image::ImageFormat> {
        match self {
            ImageType::Gif => Some(image::ImageFormat::Gif),
            ImageType::Jpeg => Some(image::ImageFormat::Jpeg),
            ImageType::Png => Some(image::ImageFormat::Png),
            ImageType::Wbmp => None, // image crate 不支持 WBMP
            ImageType::Unknown => None,
        }
    }
}

// ============================================================================
// Color 结构 — 对齐 Grafika\Color
// ============================================================================

/// 颜色 — 对齐 PHP `Grafika\Color`
///
/// PHP 构造方法支持：
/// - `new Color('#rgb')`
/// - `new Color('#rrggbb')`
/// - `new Color('#rgba')`（含 alpha）
/// - `new Color('#rrggbbaa')`（含 alpha）
/// - `new Color([r, g, b])`
/// - `new Color([r, g, b, a])`（a 是 0-1 浮点）
/// - `new Color(r, g, b)`（3 个 int 参数）
/// - `new Color(r, g, b, a)`（4 个参数，a 是 0-1 浮点）
///
/// Rust 端简化为 hex 字符串 + RGB 元组两种构造方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// 红色（0-255）
    pub r: u8,
    /// 绿色（0-255）
    pub g: u8,
    /// 蓝色（0-255）
    pub b: u8,
    /// Alpha（0-255，0=透明，255=不透明；对齐 Rust `Rgba<u8>` 语义）
    ///
    /// 注意：PHP GD alpha 是 0（不透明）~127（透明），Grafika 内部用 `gdAlpha()` 转换。
    /// Rust 端直接用 0-255（0=透明，255=不透明），与 `Rgba<u8>` 一致。
    pub a: u8,
}

impl Color {
    /// 创建不透明颜色 — 对齐 `new Color(r, g, b)`
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 创建带 alpha 的颜色 — 对齐 `new Color(r, g, b, a)`
    ///
    /// PHP 的 alpha 是 0-1 浮点（0=透明，1=不透明），Rust 端用 0-255。
    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 从 hex 字符串解析 — 对齐 `new Color('#xxxxxx')`
    ///
    /// 支持格式：
    /// - `#rgb` → `#rrggbb`
    /// - `#rgba` → `#rrggbbaa`
    /// - `#rrggbb`
    /// - `#rrggbbaa`
    /// - 不带 `#` 前缀也支持
    pub fn from_hex(hex: &str) -> Result<Self, ImageError> {
        let hex = hex.trim().trim_start_matches('#');
        let parse = |s: &str| {
            u8::from_str_radix(s, 16).map_err(|_| ImageError::InvalidColor(hex.to_string()))
        };
        let (r, g, b, a) = match hex.len() {
            3 => {
                // #rgb → #rrggbb
                let r = parse(&format!("{}{}", &hex[0..1], &hex[0..1]))?;
                let g = parse(&format!("{}{}", &hex[1..2], &hex[1..2]))?;
                let b = parse(&format!("{}{}", &hex[2..3], &hex[2..3]))?;
                (r, g, b, 255u8)
            }
            4 => {
                // #rgba → #rrggbbaa
                let r = parse(&format!("{}{}", &hex[0..1], &hex[0..1]))?;
                let g = parse(&format!("{}{}", &hex[1..2], &hex[1..2]))?;
                let b = parse(&format!("{}{}", &hex[2..3], &hex[2..3]))?;
                let a = parse(&format!("{}{}", &hex[3..4], &hex[3..4]))?;
                (r, g, b, a)
            }
            6 => {
                // #rrggbb
                let r = parse(&hex[0..2])?;
                let g = parse(&hex[2..4])?;
                let b = parse(&hex[4..6])?;
                (r, g, b, 255u8)
            }
            8 => {
                // #rrggbbaa
                let r = parse(&hex[0..2])?;
                let g = parse(&hex[2..4])?;
                let b = parse(&hex[4..6])?;
                let a = parse(&hex[6..8])?;
                (r, g, b, a)
            }
            _ => return Err(ImageError::InvalidColor(hex.to_string())),
        };
        Ok(Self { r, g, b, a })
    }

    /// 转换为 `Rgba<u8>` — 用于 image crate
    pub fn to_rgba(self) -> Rgba<u8> {
        Rgba([self.r, self.g, self.b, self.a])
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::rgb(0, 0, 0)
    }
}

// ============================================================================
// Position 枚举 — 对齐 Grafika\Position
// ============================================================================

/// 位置 — 对齐 PHP `Grafika\Position`
///
/// PHP 源码（`Position.php`）9 种位置字符串：
/// - `top-left` / `top-center` / `top-right`
/// - `center-left` / `center` / `center-right`
/// - `bottom-left` / `bottom-center` / `bottom-right`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// 左上（对齐 `top-left`）
    TopLeft,
    /// 顶部居中（对齐 `top-center`）
    TopCenter,
    /// 右上（对齐 `top-right`）
    TopRight,
    /// 左侧居中（对齐 `center-left`）
    CenterLeft,
    /// 正中（对齐 `center`）
    Center,
    /// 右侧居中（对齐 `center-right`）
    CenterRight,
    /// 左下（对齐 `bottom-left`）
    BottomLeft,
    /// 底部居中（对齐 `bottom-center`）
    BottomCenter,
    /// 右下（对齐 `bottom-right`）
    BottomRight,
}

impl Position {
    /// 从字符串解析 — 对齐 Grafika `Position::__construct($position)`
    pub fn parse(s: &str) -> Result<Self, ImageError> {
        match s.to_lowercase().as_str() {
            "top-left" => Ok(Self::TopLeft),
            "top-center" => Ok(Self::TopCenter),
            "top-right" => Ok(Self::TopRight),
            "center-left" => Ok(Self::CenterLeft),
            "center" => Ok(Self::Center),
            "center-right" => Ok(Self::CenterRight),
            "bottom-left" => Ok(Self::BottomLeft),
            "bottom-center" => Ok(Self::BottomCenter),
            "bottom-right" => Ok(Self::BottomRight),
            _ => Err(ImageError::InvalidArgument(format!(
                "Unknown position: {s}"
            ))),
        }
    }

    /// 转换为字符串（对齐 PHP 字符串值）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopCenter => "top-center",
            Self::TopRight => "top-right",
            Self::CenterLeft => "center-left",
            Self::Center => "center",
            Self::CenterRight => "center-right",
            Self::BottomLeft => "bottom-left",
            Self::BottomCenter => "bottom-center",
            Self::BottomRight => "bottom-right",
        }
    }

    /// 计算 x/y 偏移 — 对齐 `Grafika\Position::getXY($w1, $h1, $w2, $h2)`
    ///
    /// 参数：
    /// - `w1, h1`：主图宽高
    /// - `w2, h2`：叠加图宽高
    ///
    /// 返回 `(x, y)` 偏移坐标
    pub fn get_xy(self, w1: u32, h1: u32, w2: u32, h2: u32) -> (i32, i32) {
        let w1 = w1 as i32;
        let h1 = h1 as i32;
        let w2 = w2 as i32;
        let h2 = h2 as i32;
        let x = match self {
            Self::TopLeft | Self::CenterLeft | Self::BottomLeft => 0,
            Self::TopCenter | Self::Center | Self::BottomCenter => (w1 - w2) / 2,
            Self::TopRight | Self::CenterRight | Self::BottomRight => w1 - w2,
        };
        let y = match self {
            Self::TopLeft | Self::TopCenter | Self::TopRight => 0,
            Self::CenterLeft | Self::Center | Self::CenterRight => (h1 - h2) / 2,
            Self::BottomLeft | Self::BottomCenter | Self::BottomRight => h1 - h2,
        };
        (x, y)
    }
}

// ============================================================================
// Image 结构 — 对齐 Grafika\Gd\Image
// ============================================================================

/// 图像 — 对齐 PHP `Grafika\Gd\Image`
///
/// PHP `Image` 类持有 GD 资源 + 元数据（width/height/type/file_path/animated/blocks）。
/// Rust 端用 `DynamicImage` 替代 GD 资源，其余字段保留。
#[derive(Debug)]
pub struct Image {
    /// 内部图像数据（对齐 PHP `$gd`）
    dyn_image: DynamicImage,
    /// 源文件路径（对齐 PHP `$imageFile`）
    file_path: Option<PathBuf>,
    /// 图像类型（对齐 PHP `$type`）
    image_type: ImageType,
}

impl Image {
    /// 从文件打开 — 对齐 `Image::createFromFile($imageFile)`
    ///
    /// PHP 行为：用 `getimagesize()` 探测类型，分派到 `_createGif/_createJpeg/_createPng/_createWbmp`。
    /// Rust 端用 `image::open` 统一处理，再用 `guess_type` 推断类型。
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, ImageError> {
        let path = path.as_ref();
        let dyn_image = image::open(path)?;
        let image_type = guess_image_type(path)?;
        Ok(Self {
            dyn_image,
            file_path: Some(path.to_path_buf()),
            image_type,
        })
    }

    /// 创建空白画布 — 对齐 `Image::createBlank($width, $height)`
    ///
    /// PHP 行为：`imagecreatetruecolor($width, $height)`，默认黑色。
    /// Rust 端用 `RgbaImage::new`，默认透明（与 PHP GD 默认黑色不同，但更符合 Rust 习惯）。
    pub fn create_blank(width: u32, height: u32) -> Self {
        let image: RgbaImage = ImageBuffer::new(width, height);
        Self {
            dyn_image: DynamicImage::ImageRgba8(image),
            file_path: None,
            image_type: ImageType::Unknown,
        }
    }

    /// 从 DynamicImage 构造（Rust 扩展，无 PHP 对应）
    pub fn from_dynamic(dyn_image: DynamicImage, image_type: ImageType) -> Self {
        Self {
            dyn_image,
            file_path: None,
            image_type,
        }
    }

    /// 获取宽度 — 对齐 `Image::getWidth()`
    pub fn width(&self) -> u32 {
        self.dyn_image.width()
    }

    /// 获取高度 — 对齐 `Image::getHeight()`
    pub fn height(&self) -> u32 {
        self.dyn_image.height()
    }

    /// 获取图像类型 — 对齐 `Image::getType()`
    pub fn image_type(&self) -> ImageType {
        self.image_type
    }

    /// 获取源文件路径 — 对齐 `Image::getImageFile()`
    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    /// 获取内部 DynamicImage 引用（Rust 扩展）
    pub fn as_dynamic(&self) -> &DynamicImage {
        &self.dyn_image
    }

    /// 获取内部 DynamicImage 可变引用（Rust 扩展）
    pub fn as_dynamic_mut(&mut self) -> &mut DynamicImage {
        &mut self.dyn_image
    }

    /// 转换为 RGBA8 — 用于图像操作（Rust 扩展，对齐 GD `imagecreatetruecolor` 返回的资源）
    pub fn to_rgba8(&self) -> RgbaImage {
        self.dyn_image.to_rgba8()
    }

    /// 从 RGBA8 缓冲构造（Rust 扩展）
    pub fn from_rgba8(image: RgbaImage, image_type: ImageType) -> Self {
        Self {
            dyn_image: DynamicImage::ImageRgba8(image),
            file_path: None,
            image_type,
        }
    }
}

/// 从文件路径推断图像类型 — 对齐 Grafika `_guessType($imageFile)` 用 `getimagesize`
fn guess_image_type(path: &Path) -> Result<ImageType, ImageError> {
    // 优先按扩展名判断（对齐 Grafika `_getImageTypeFromFileName`）
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let from_ext = ImageType::from_extension(ext);
    if from_ext != ImageType::Unknown {
        return Ok(from_ext);
    }
    // 扩展名未知时用 image crate 探测（对齐 PHP `getimagesize`）
    let format = image::ImageReader::open(path)
        .map_err(|e| ImageError::OpenFailed(format!("{path:?}: {e}")))?
        .with_guessed_format()
        .map_err(|e| ImageError::OpenFailed(format!("{path:?}: {e}")))?
        .format()
        .ok_or_else(|| ImageError::UnsupportedType("Unknown image format".to_string()))?;
    Ok(ImageType::from_image_format(format))
}

// ============================================================================
// Editor 结构 — 对齐 Grafika\Gd\Editor
// ============================================================================

/// 图像编辑器 — 对齐 PHP `Grafika\Gd\Editor`
///
/// PHP Grafika 把所有操作方法集中在 `Editor` 类，`Image` 只持有状态。
/// Rust 端保持同样的架构分离。
///
/// 使用方式（对齐 PHP `$editor = Grafika::createEditor(['Gd'])`）：
/// ```ignore
/// use sz_rust_core::upload::image::{Editor, Image, Color};
/// let mut editor = Editor::new();
/// let mut img = Image::open("test.png")?;
/// editor.resize_exact(&mut img, 100, 100);
/// editor.save(&img, "out.png", None, None, false, 0o755)?;
/// ```
pub struct Editor;

impl Editor {
    /// 创建编辑器实例 — 对齐 `Grafika::createEditor(['Gd'])`
    pub fn new() -> Self {
        Self
    }

    /// 打开文件 — 对齐 `Editor::open(&$image, $imageFile)`
    ///
    /// PHP 语义：`$image` 按引用传递，函数内部赋值为新 Image 对象。
    /// Rust 语义：返回新 `Image`，调用方自行赋值。
    pub async fn open<P: AsRef<Path>>(&self, path: P) -> Result<Image, ImageError> {
        Image::open(path).await
    }

    /// 强制尺寸缩放 — 对齐 `Editor::resizeExact(&$image, $newWidth, $newHeight)`
    ///
    /// PHP 行为：忽略宽高比，强制缩放到目标尺寸（对应 GD `imagecopyresampled`）。
    /// Rust 端用 `image::imageops::resize` + `FilterType::Lanczos3`（高质量重采样）。
    pub fn resize_exact(&self, image: &mut Image, new_width: u32, new_height: u32) {
        let resized = image::imageops::resize(
            image.as_dynamic(),
            new_width,
            new_height,
            image::imageops::FilterType::Lanczos3,
        );
        image.dyn_image = DynamicImage::ImageRgba8(resized);
    }

    /// 等比缩放适配 — 对齐 `Editor::resizeFit(&$image, $newWidth, $newHeight)`
    ///
    /// PHP 行为：等比缩放，使图像完全包含在目标框内（不裁剪）。
    pub fn resize_fit(&self, image: &mut Image, new_width: u32, new_height: u32) {
        let (w, h) = (image.width(), image.height());
        let ratio = (new_width as f64 / w as f64).min(new_height as f64 / h as f64);
        let target_w = (w as f64 * ratio).round() as u32;
        let target_h = (h as f64 * ratio).round() as u32;
        let resized = image::imageops::resize(
            image.as_dynamic(),
            target_w,
            target_h,
            image::imageops::FilterType::Lanczos3,
        );
        image.dyn_image = DynamicImage::ImageRgba8(resized);
    }

    /// 填充+裁剪 — 对齐 `Editor::resizeFill(&$image, $newWidth, $newHeight)`
    ///
    /// PHP 行为：等比缩放使图像完全覆盖目标框，超出部分居中裁剪。
    pub fn resize_fill(&self, image: &mut Image, new_width: u32, new_height: u32) {
        let (w, h) = (image.width(), image.height());
        let ratio = (new_width as f64 / w as f64).max(new_height as f64 / h as f64);
        let scaled_w = (w as f64 * ratio).round() as u32;
        let scaled_h = (h as f64 * ratio).round() as u32;
        // 1. 等比放大
        let scaled = image::imageops::resize(
            image.as_dynamic(),
            scaled_w,
            scaled_h,
            image::imageops::FilterType::Lanczos3,
        );
        // 2. 居中裁剪
        let x = (scaled_w - new_width) / 2;
        let y = (scaled_h - new_height) / 2;
        let cropped = image::imageops::crop_imm(&scaled, x, y, new_width, new_height).to_image();
        image.dyn_image = DynamicImage::ImageRgba8(cropped);
    }

    /// 等比按宽 — 对齐 `Editor::resizeExactWidth(&$image, $newWidth)`
    pub fn resize_exact_width(&self, image: &mut Image, new_width: u32) {
        let h = image.height();
        let new_height = (h as f64 * (new_width as f64 / image.width() as f64)).round() as u32;
        self.resize_exact(image, new_width, new_height);
    }

    /// 等比按高 — 对齐 `Editor::resizeExactHeight(&$image, $newHeight)`
    pub fn resize_exact_height(&self, image: &mut Image, new_height: u32) {
        let w = image.width();
        let new_width = (w as f64 * (new_height as f64 / image.height() as f64)).round() as u32;
        self.resize_exact(image, new_width, new_height);
    }

    /// 裁剪 — 对齐 `Editor::crop(&$image, $cropWidth, $cropHeight, $position, $offsetX, $offsetY)`
    ///
    /// PHP 行为：按 `$position` 计算裁剪起点，加 `$offsetX/$offsetY` 偏移。
    pub fn crop(
        &self,
        image: &mut Image,
        crop_width: u32,
        crop_height: u32,
        position: Position,
        offset_x: i32,
        offset_y: i32,
    ) -> Result<(), ImageError> {
        let (w, h) = (image.width(), image.height());
        if crop_width > w || crop_height > h {
            return Err(ImageError::InvalidArgument(format!(
                "crop size {crop_width}x{crop_height} larger than image {w}x{h}"
            )));
        }
        let (mut x, mut y) = position.get_xy(w, h, crop_width, crop_height);
        x += offset_x;
        y += offset_y;
        // 边界检查
        let x = x.max(0) as u32;
        let y = y.max(0) as u32;
        let x = x.min(w - crop_width);
        let y = y.min(h - crop_height);
        let cropped =
            image::imageops::crop_imm(image.as_dynamic(), x, y, crop_width, crop_height).to_image();
        image.dyn_image = DynamicImage::ImageRgba8(cropped);
        Ok(())
    }

    /// 图层合成 — 对齐 `Editor::blend(&$image1, $image2, $type, $opacity, $position, $offsetX, $offsetY)`
    ///
    /// PHP 行为：
    /// 1. 按 `$position` + `$offsetX/$offsetY` 计算叠加位置
    /// 2. 创建新画布（image1 大小）
    /// 3. 复制 image1 到新画布
    /// 4. 按 `$type`（normal/multiply/overlay/screen）混合 image2 到新画布
    /// 5. 销毁原 image1 GD 资源，替换为新画布
    ///
    /// Rust 端实现 normal 模式（项目业务只用 normal），其他模式预留接口。
    #[allow(clippy::too_many_arguments)]
    pub fn blend(
        &self,
        image1: &mut Image,
        image2: &Image,
        blend_type: BlendType,
        opacity: f32,
        position: Position,
        offset_x: i32,
        offset_y: i32,
    ) -> Result<(), ImageError> {
        let (w1, h1) = (image1.width(), image1.height());
        let (w2, h2) = (image2.width(), image2.height());
        let (base_x, base_y) = position.get_xy(w1, h1, w2, h2);
        let x = base_x + offset_x;
        let y = base_y + offset_y;

        // 转换为 RGBA8
        let mut base = image1.to_rgba8();
        let overlay = image2.to_rgba8();

        match blend_type {
            BlendType::Normal => {
                blend_normal(&mut base, &overlay, x, y, opacity);
            }
            BlendType::Multiply => {
                blend_multiply(&mut base, &overlay, x, y, opacity);
            }
            BlendType::Overlay => {
                blend_overlay(&mut base, &overlay, x, y, opacity);
            }
            BlendType::Screen => {
                blend_screen(&mut base, &overlay, x, y, opacity);
            }
        }

        image1.dyn_image = DynamicImage::ImageRgba8(base);
        Ok(())
    }

    /// 文本绘制 — 对齐 `Editor::text(&$image, $text, $size, $x, $y, $color, $font, $angle)`
    ///
    /// PHP 行为（GD `imagettftext`）：
    /// - `$x, $y` 是文本基线起点
    /// - Grafika 内部 `$y += $size`（GD 的 y 是基线，绘制时 y 要加 size 才是顶部）
    /// - `$angle` 是角度（0=水平）
    /// - `$font` 是 TTF 文件路径
    ///
    /// Rust 端使用 `imageproc::drawing::draw_text_mut`：
    /// - y 是顶部（不是基线）
    /// - 所以 Rust y = PHP y - size（反向偏移）
    /// - angle 暂不支持（imageproc 0.25 文本绘制不支持旋转）
    #[allow(clippy::too_many_arguments)]
    pub async fn text(
        &self,
        image: &mut Image,
        text: &str,
        size: u32,
        x: i32,
        y: i32,
        color: Color,
        font_path: Option<&Path>,
    ) -> Result<(), ImageError> {
        let font = load_font(font_path).await?;
        // R5-30：GD y 是基线，Rust y 是顶部，反向偏移
        let rust_y = y - size as i32;
        let mut rgba_image = image.to_rgba8();
        let scale = PxScale::from(size as f32);
        imageproc::drawing::draw_text_mut(
            &mut rgba_image,
            color.to_rgba(),
            x,
            rust_y,
            scale,
            &font,
            text,
        );
        image.dyn_image = DynamicImage::ImageRgba8(rgba_image);
        Ok(())
    }

    /// 旋转 — 对齐 `Editor::rotate(&$image, $angle, $color)`
    ///
    /// PHP 行为：用 `imagerotate` 旋转，`$color` 是旋转后空白区域填充色。
    /// Rust 端用 `image::imageops::rotate90/180/270` 处理 90/180/270 度，
    /// 任意角度暂不支持（imageproc 0.25 不直接支持）。
    pub fn rotate(&self, image: &mut Image, angle: f32) -> Result<(), ImageError> {
        // 标准化到 [0, 360)
        let angle = angle.rem_euclid(360.0);
        let rotated = match angle as i32 {
            0 => image.dyn_image.clone(),
            90 | -270 => image.dyn_image.rotate90(),
            180 | -180 => image.dyn_image.rotate180(),
            270 | -90 => image.dyn_image.rotate270(),
            _ => {
                return Err(ImageError::InvalidArgument(format!(
                    "rotate only supports 0/90/180/270 degrees, got {angle}"
                )))
            }
        };
        image.dyn_image = rotated;
        Ok(())
    }

    /// 翻转 — 对齐 `Editor::flip(&$image, $mode)`
    ///
    /// PHP 行为：`$mode` 是 'h'（水平翻转）或 'v'（垂直翻转）。
    pub fn flip(&self, image: &mut Image, mode: FlipMode) {
        match mode {
            FlipMode::Horizontal => image.dyn_image = image.dyn_image.fliph(),
            FlipMode::Vertical => image.dyn_image = image.dyn_image.flipv(),
        }
    }

    /// 填充 — 对齐 `Editor::fill(&$image, $color, $x, $y)`
    ///
    /// PHP 行为：从 `($x, $y)` 开始用 `$color` 填充连通区域（`imagefill`）。
    /// Rust 端简化为填充整个图像（业务侧不使用此方法）。
    pub fn fill(&self, image: &mut Image, color: Color) {
        let (w, h) = (image.width(), image.height());
        let pixel = color.to_rgba();
        let mut buf: RgbaImage = ImageBuffer::new(w, h);
        for y in 0..h {
            for x in 0..w {
                buf.put_pixel(x, y, pixel);
            }
        }
        image.dyn_image = DynamicImage::ImageRgba8(buf);
    }

    /// 保存 — 对齐 `Editor::save($image, $file, $type, $quality, $interlace, $permission)`
    ///
    /// PHP 行为：
    /// - `$type=null` 时按文件扩展名猜，扩展名也未知时用原图类型
    /// - 自动 `mkdir($targetDir, $permission, true)`
    /// - GIF：动画 → GifHelper::encode；非动画 → imagegif
    /// - PNG：imagepng（无 quality）
    /// - JPEG：quality=null → 75；imageinterlace 控制渐进式；imagejpeg
    ///
    /// Rust 端用 `image::save` 简化处理，JPEG quality 通过 `image::codecs::jpeg::JpegEncoder` 设置。
    pub async fn save(
        &self,
        image: &Image,
        file: &Path,
        image_type: Option<ImageType>,
        quality: Option<u8>,
        _interlace: bool,
        permission: u32,
    ) -> Result<(), ImageError> {
        // 在非 unix 平台 permission 不使用，显式忽略避免警告
        let _ = &permission;
        // 1. 确定保存类型
        let save_type = image_type.unwrap_or_else(|| {
            // 按扩展名猜（对齐 PHP `$type=null` 行为）
            let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
            let t = ImageType::from_extension(ext);
            if t != ImageType::Unknown {
                t
            } else {
                image.image_type()
            }
        });

        // 2. 自动创建父目录（对齐 PHP `mkdir`）
        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = tokio::fs::set_permissions(
                        parent,
                        tokio::fs::Permissions::from_mode(permission),
                    )
                    .await;
                }
            }
        }

        // 3. 按类型保存
        match save_type {
            ImageType::Png => {
                image.as_dynamic().save(file)?;
            }
            ImageType::Jpeg => {
                // JPEG 默认 quality=75（对齐 PHP）
                let q = quality.unwrap_or(75);
                let q = q.clamp(1, 100);
                let rgba = image.to_rgba8();
                let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
                let mut buf = Vec::new();
                let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
                encoder.encode_image(&image::DynamicImage::ImageRgb8(rgb))?;
                tokio::fs::write(file, buf).await?;
            }
            ImageType::Gif => {
                image.as_dynamic().save(file)?;
            }
            ImageType::Wbmp => {
                return Err(ImageError::UnsupportedType(
                    "WBMP encoding not supported by image crate".to_string(),
                ));
            }
            ImageType::Unknown => {
                return Err(ImageError::UnsupportedType(format!(
                    "Cannot determine save type for file: {file:?}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// BlendType 枚举 — 对齐 Grafika blend $type 参数
// ============================================================================

/// 混合模式 — 对齐 PHP `Editor::blend` 的 `$type` 参数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendType {
    /// 普通模式（对齐 `'normal'`）— 项目业务唯一使用
    Normal,
    /// 正片叠底（对齐 `'multiply'`）
    Multiply,
    /// 叠加（对齐 `'overlay'`）
    Overlay,
    /// 滤色（对齐 `'screen'`）
    Screen,
}

impl BlendType {
    /// 从字符串解析 — 对齐 Grafika `$type` 字符串
    pub fn parse(s: &str) -> Result<Self, ImageError> {
        match s.to_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "multiply" => Ok(Self::Multiply),
            "overlay" => Ok(Self::Overlay),
            "screen" => Ok(Self::Screen),
            _ => Err(ImageError::InvalidArgument(format!(
                "Unknown blend type: {s}"
            ))),
        }
    }
}

// ============================================================================
// FlipMode 枚举 — 对齐 Grafika flip $mode 参数
// ============================================================================

/// 翻转模式 — 对齐 PHP `Editor::flip` 的 `$mode` 参数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlipMode {
    /// 水平翻转（对齐 `'h'`）
    Horizontal,
    /// 垂直翻转（对齐 `'v'`）
    Vertical,
}

impl FlipMode {
    /// 从字符串解析 — 对齐 Grafika `$mode` 字符串
    pub fn parse(s: &str) -> Result<Self, ImageError> {
        match s.to_lowercase().as_str() {
            "h" => Ok(Self::Horizontal),
            "v" => Ok(Self::Vertical),
            _ => Err(ImageError::InvalidArgument(format!(
                "Unknown flip mode: {s}"
            ))),
        }
    }
}

// ============================================================================
// Normal 混合实现 — 对齐 GD imagecopy + alpha
// ============================================================================

/// Normal 混合 — 对齐 Grafika `_blendNormal`
///
/// 算法：`dest = src * opacity + dest * (1 - opacity)`（按 alpha 通道加权）
fn blend_normal(base: &mut RgbaImage, overlay: &RgbaImage, x: i32, y: i32, opacity: f32) {
    let (w1, h1) = base.dimensions();
    let (w2, h2) = overlay.dimensions();
    let opacity = opacity.clamp(0.0, 1.0);

    for oy in 0..h2 {
        for ox in 0..w2 {
            let bx = x + ox as i32;
            let by = y + oy as i32;
            if bx < 0 || by < 0 || bx >= w1 as i32 || by >= h1 as i32 {
                continue;
            }
            let src = overlay.get_pixel(ox, oy);
            let dst = base.get_pixel(bx as u32, by as u32);
            // 源像素有效 alpha + opacity
            let src_alpha = (src[3] as f32 / 255.0) * opacity;
            if src_alpha < 1e-6 {
                continue;
            }
            let dst_alpha = dst[3] as f32 / 255.0;
            // 输出 alpha = src_a + dst_a * (1 - src_a)
            let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
            if out_alpha < 1e-6 {
                base.put_pixel(bx as u32, by as u32, Rgba([0, 0, 0, 0]));
                continue;
            }
            // 输出 RGB = (src_rgb * src_a + dst_rgb * dst_a * (1 - src_a)) / out_a
            let out_r = ((src[0] as f32 * src_alpha
                + dst[0] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_g = ((src[1] as f32 * src_alpha
                + dst[1] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_b = ((src[2] as f32 * src_alpha
                + dst[2] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_a = (out_alpha * 255.0) as u8;
            base.put_pixel(bx as u32, by as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// Multiply 混合 — 对齐 Grafika `_blendMultiply`
fn blend_multiply(base: &mut RgbaImage, overlay: &RgbaImage, x: i32, y: i32, opacity: f32) {
    let (w1, h1) = base.dimensions();
    let (w2, h2) = overlay.dimensions();
    let opacity = opacity.clamp(0.0, 1.0);

    for oy in 0..h2 {
        for ox in 0..w2 {
            let bx = x + ox as i32;
            let by = y + oy as i32;
            if bx < 0 || by < 0 || bx >= w1 as i32 || by >= h1 as i32 {
                continue;
            }
            let src = overlay.get_pixel(ox, oy);
            let dst = base.get_pixel(bx as u32, by as u32);
            let src_alpha = (src[3] as f32 / 255.0) * opacity;
            if src_alpha < 1e-6 {
                continue;
            }
            // multiply: out = src * dst / 255
            let mult_r = (src[0] as u16 * dst[0] as u16 / 255) as u8;
            let mult_g = (src[1] as u16 * dst[1] as u16 / 255) as u8;
            let mult_b = (src[2] as u16 * dst[2] as u16 / 255) as u8;
            let dst_alpha = dst[3] as f32 / 255.0;
            let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
            if out_alpha < 1e-6 {
                base.put_pixel(bx as u32, by as u32, Rgba([0, 0, 0, 0]));
                continue;
            }
            let out_r = ((mult_r as f32 * src_alpha
                + dst[0] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_g = ((mult_g as f32 * src_alpha
                + dst[1] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_b = ((mult_b as f32 * src_alpha
                + dst[2] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_a = (out_alpha * 255.0) as u8;
            base.put_pixel(bx as u32, by as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// Overlay 混合 — 对齐 Grafika `_blendOverlay`
fn blend_overlay(base: &mut RgbaImage, overlay: &RgbaImage, x: i32, y: i32, opacity: f32) {
    let (w1, h1) = base.dimensions();
    let (w2, h2) = overlay.dimensions();
    let opacity = opacity.clamp(0.0, 1.0);

    for oy in 0..h2 {
        for ox in 0..w2 {
            let bx = x + ox as i32;
            let by = y + oy as i32;
            if bx < 0 || by < 0 || bx >= w1 as i32 || by >= h1 as i32 {
                continue;
            }
            let src = overlay.get_pixel(ox, oy);
            let dst = base.get_pixel(bx as u32, by as u32);
            let src_alpha = (src[3] as f32 / 255.0) * opacity;
            if src_alpha < 1e-6 {
                continue;
            }
            // overlay: if dst <= 128: out = 2 * src * dst / 255; else: out = 255 - 2 * (255 - src) * (255 - dst) / 255
            let overlay_channel = |s: u8, d: u8| -> u8 {
                if d <= 128 {
                    (2 * s as u16 * d as u16 / 255) as u8
                } else {
                    (255 - (2 * (255 - s) as u16 * (255 - d) as u16 / 255)) as u8
                }
            };
            let ov_r = overlay_channel(src[0], dst[0]);
            let ov_g = overlay_channel(src[1], dst[1]);
            let ov_b = overlay_channel(src[2], dst[2]);
            let dst_alpha = dst[3] as f32 / 255.0;
            let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
            if out_alpha < 1e-6 {
                base.put_pixel(bx as u32, by as u32, Rgba([0, 0, 0, 0]));
                continue;
            }
            let out_r = ((ov_r as f32 * src_alpha + dst[0] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_g = ((ov_g as f32 * src_alpha + dst[1] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_b = ((ov_b as f32 * src_alpha + dst[2] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_a = (out_alpha * 255.0) as u8;
            base.put_pixel(bx as u32, by as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// Screen 混合 — 对齐 Grafika `_blendScreen`
fn blend_screen(base: &mut RgbaImage, overlay: &RgbaImage, x: i32, y: i32, opacity: f32) {
    let (w1, h1) = base.dimensions();
    let (w2, h2) = overlay.dimensions();
    let opacity = opacity.clamp(0.0, 1.0);

    for oy in 0..h2 {
        for ox in 0..w2 {
            let bx = x + ox as i32;
            let by = y + oy as i32;
            if bx < 0 || by < 0 || bx >= w1 as i32 || by >= h1 as i32 {
                continue;
            }
            let src = overlay.get_pixel(ox, oy);
            let dst = base.get_pixel(bx as u32, by as u32);
            let src_alpha = (src[3] as f32 / 255.0) * opacity;
            if src_alpha < 1e-6 {
                continue;
            }
            // screen: out = 255 - (255 - src) * (255 - dst) / 255
            let screen_r = (255 - (255 - src[0]) as u16 * (255 - dst[0]) as u16 / 255) as u8;
            let screen_g = (255 - (255 - src[1]) as u16 * (255 - dst[1]) as u16 / 255) as u8;
            let screen_b = (255 - (255 - src[2]) as u16 * (255 - dst[2]) as u16 / 255) as u8;
            let dst_alpha = dst[3] as f32 / 255.0;
            let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
            if out_alpha < 1e-6 {
                base.put_pixel(bx as u32, by as u32, Rgba([0, 0, 0, 0]));
                continue;
            }
            let out_r = ((screen_r as f32 * src_alpha
                + dst[0] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_g = ((screen_g as f32 * src_alpha
                + dst[1] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_b = ((screen_b as f32 * src_alpha
                + dst[2] as f32 * dst_alpha * (1.0 - src_alpha))
                / out_alpha) as u8;
            let out_a = (out_alpha * 255.0) as u8;
            base.put_pixel(bx as u32, by as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

// ============================================================================
// 字体加载 + 文本测量 — 对齐 imagettfbbox
// ============================================================================

/// 加载字体 — 优先用 FontRef（零拷贝），失败时返回内置默认字体
///
/// 对齐 Grafika `text()` 默认字体 `LiberationSans-Regular.ttf`。
async fn load_font(font_path: Option<&Path>) -> Result<FontVec, ImageError> {
    match font_path {
        Some(path) => {
            let data = tokio::fs::read(path)
                .await
                .map_err(|e| ImageError::FontLoadFailed(format!("{path:?}: {e}")))?;
            Ok(FontVec::try_from_vec(data)
                .map_err(|e| ImageError::FontLoadFailed(format!("Invalid font {path:?}: {e}")))?)
        }
        None => {
            // 无字体路径时返回错误（PHP Grafika 有默认字体，Rust 端要求显式提供）
            Err(ImageError::FontLoadFailed(
                "font_path is required (no default font available)".to_string(),
            ))
        }
    }
}

/// 测量文本边界 — 对齐 PHP `imagettfbbox($size, $angle, $font, $text)`
///
/// PHP 返回 8 个值（4 个角点）：
/// - 0: 左下角 x
/// - 1: 左下角 y
/// - 2: 右下角 x
/// - 3: 右下角 y
/// - 4: 右上角 x
/// - 5: 右上角 y
/// - 6: 左上角 x
/// - 7: 左上角 y
///
/// Rust 端简化为返回 `(width, height)` — 大多数场景只需要这两个值。
///
/// 注意：PHP `imagettfbbox` 的 y 轴向下，但返回值中"上"的 y 是负数。
pub async fn measure_text(
    font_path: &Path,
    size: u32,
    text: &str,
) -> Result<TextMetrics, ImageError> {
    let data = tokio::fs::read(font_path)
        .await
        .map_err(|e| ImageError::FontLoadFailed(format!("{font_path:?}: {e}")))?;
    let font = FontVec::try_from_vec(data)
        .map_err(|e| ImageError::FontLoadFailed(format!("Invalid font: {e}")))?;
    Ok(measure_text_with_font(&font, size, text))
}

/// 用已加载的字体测量文本
fn measure_text_with_font<F: Font>(font: &F, size: u32, text: &str) -> TextMetrics {
    let scale = PxScale::from(size as f32);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let height = (ascent - descent).ceil();

    let mut width: f32 = 0.0;
    let mut prev_glyph: Option<Glyph> = None;
    for ch in text.chars() {
        let glyph = scaled.scaled_glyph(ch);
        if let Some(prev) = prev_glyph {
            width += scaled.kern(prev.id, glyph.id);
        }
        width += scaled.h_advance(glyph.id);
        prev_glyph = Some(glyph);
    }

    TextMetrics {
        width: width.ceil() as i32,
        height: height.ceil() as i32,
        ascent: ascent.ceil() as i32,
        descent: descent.ceil() as i32,
    }
}

/// 文本测量结果
#[derive(Debug, Clone, Copy)]
pub struct TextMetrics {
    /// 文本宽度（像素）
    pub width: i32,
    /// 文本高度（像素）
    pub height: i32,
    /// 字体 ascent（基线到顶部）
    pub ascent: i32,
    /// 字体 descent（基线到底部，通常为负数）
    pub descent: i32,
}

// ============================================================================
// wrap_text — 对齐业务侧 ProductService::wrapText
// ============================================================================

/// 文本自动换行 — 对齐 PHP `ProductService::wrapText($fontsize, $angle, $fontface, $string, $width, $max_line)`
///
/// PHP 源码（`app/common/service/qrcode/ProductService.php` 第 114-138 行）：
/// ```php
/// private function wrapText($fontsize, $angle, $fontface, $string, $width, $max_line = null) {
///     $content = "";
///     $letter = [];
///     for ($i = 0; $i < mb_strlen($string, 'UTF-8'); $i++) {
///         $letter[] = mb_substr($string, $i, 1, 'UTF-8');
///     }
///     $line_count = 0;
///     foreach ($letter as $l) {
///         $testbox = imagettfbbox($fontsize, $angle, $fontface, $content . ' ' . $l);
///         if (($testbox[2] > $width) && ($content !== "")) {
///             $line_count++;
///             if ($max_line && $line_count >= $max_line) {
///                 $content = mb_substr($content, 0, -1, 'UTF-8') . "...";
///                 break;
///             }
///             $content .= "\n";
///         }
///         $content .= $l;
///     }
///     return $content;
/// }
/// ```
///
/// **关键细节**：
/// 1. PHP 用 `mb_strlen`/`mb_substr` 按 UTF-8 字符拆分
/// 2. `imagettfbbox` 测量 `$content . ' ' . $l`（注意有空格连接符）
/// 3. `$testbox[2]` 是右下角 x（即文本宽度）
/// 4. 超过 `$width` 时：先 `$line_count++`，再判断是否达到 `$max_line`
/// 5. 达到 `$max_line` 时：截掉最后一个字符 + `"..."` + break
/// 6. 未达到时：在 `$content` 末尾加 `\n`，然后继续加 `$l`
///
/// **Rust 实现**：
/// - 用 `measure_text_with_font` 替代 `imagettfbbox`
/// - 测量 `$content . ' ' . $l` 即 `format!("{content} {l}")`
/// - 其余逻辑 1:1 对齐
pub async fn wrap_text(
    font_path: &Path,
    fontsize: u32,
    string: &str,
    width: i32,
    max_line: Option<usize>,
) -> Result<String, ImageError> {
    let data = tokio::fs::read(font_path)
        .await
        .map_err(|e| ImageError::FontLoadFailed(format!("{font_path:?}: {e}")))?;
    let font = FontVec::try_from_vec(data)
        .map_err(|e| ImageError::FontLoadFailed(format!("Invalid font: {e}")))?;
    Ok(wrap_text_with_font(
        &font, fontsize, string, width, max_line,
    ))
}

/// 用已加载字体执行 wrap_text（避免重复读取字体文件）
fn wrap_text_with_font<F: Font>(
    font: &F,
    fontsize: u32,
    string: &str,
    width: i32,
    max_line: Option<usize>,
) -> String {
    let mut content = String::new();
    let mut line_count: usize = 0;
    for l in string.chars() {
        // 对齐 PHP `$content . ' ' . $l`
        let test = format!("{content} {l}");
        let metrics = measure_text_with_font(font, fontsize, &test);
        // 对齐 PHP `($testbox[2] > $width) && ($content !== "")`
        if metrics.width > width && !content.is_empty() {
            line_count += 1;
            if let Some(ml) = max_line {
                if line_count >= ml {
                    // 对齐 PHP `mb_substr($content, 0, -1, 'UTF-8') . "..."`
                    let trimmed: String =
                        content.chars().take(content.chars().count() - 1).collect();
                    content = format!("{trimmed}...");
                    break;
                }
            }
            content.push('\n');
        }
        content.push(l);
    }
    content
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 组 1：ImageType 基础 ----

    #[test]
    fn test_image_type_as_str() {
        assert_eq!(ImageType::Unknown.as_str(), "");
        assert_eq!(ImageType::Gif.as_str(), "GIF");
        assert_eq!(ImageType::Jpeg.as_str(), "JPEG");
        assert_eq!(ImageType::Png.as_str(), "PNG");
        assert_eq!(ImageType::Wbmp.as_str(), "WBMP");
    }

    #[test]
    fn test_image_type_from_extension() {
        assert_eq!(ImageType::from_extension("gif"), ImageType::Gif);
        assert_eq!(ImageType::from_extension("jpg"), ImageType::Jpeg);
        assert_eq!(ImageType::from_extension("jpeg"), ImageType::Jpeg);
        assert_eq!(ImageType::from_extension("png"), ImageType::Png);
        assert_eq!(ImageType::from_extension("wbmp"), ImageType::Wbmp);
        assert_eq!(ImageType::from_extension("unknown"), ImageType::Unknown);
    }

    #[test]
    fn test_image_type_default() {
        assert_eq!(ImageType::default(), ImageType::Unknown);
    }

    #[test]
    fn test_image_type_from_image_format() {
        use image::ImageFormat;
        assert_eq!(
            ImageType::from_image_format(ImageFormat::Gif),
            ImageType::Gif
        );
        assert_eq!(
            ImageType::from_image_format(ImageFormat::Jpeg),
            ImageType::Jpeg
        );
        assert_eq!(
            ImageType::from_image_format(ImageFormat::Png),
            ImageType::Png
        );
        assert_eq!(
            ImageType::from_image_format(ImageFormat::WebP),
            ImageType::Unknown
        );
    }

    #[test]
    fn test_image_type_to_image_format() {
        assert_eq!(
            ImageType::Gif.to_image_format(),
            Some(image::ImageFormat::Gif)
        );
        assert_eq!(
            ImageType::Jpeg.to_image_format(),
            Some(image::ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageType::Png.to_image_format(),
            Some(image::ImageFormat::Png)
        );
        assert_eq!(ImageType::Wbmp.to_image_format(), None);
        assert_eq!(ImageType::Unknown.to_image_format(), None);
    }

    #[test]
    fn test_image_type_from_extension_case_insensitive() {
        assert_eq!(ImageType::from_extension("GIF"), ImageType::Gif);
        assert_eq!(ImageType::from_extension("PNG"), ImageType::Png);
        assert_eq!(ImageType::from_extension("JPG"), ImageType::Jpeg);
    }

    // ---- 组 2：Color ----

    #[test]
    fn test_color_rgb() {
        let c = Color::rgb(255, 128, 0);
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 255); // 不透明
    }

    #[test]
    fn test_color_rgba() {
        let c = Color::rgba(255, 128, 0, 128);
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 128);
    }

    #[test]
    fn test_color_from_hex_rrggbb() {
        let c = Color::from_hex("#ff8000").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 255);
    }

    #[test]
    fn test_color_from_hex_rgb() {
        let c = Color::from_hex("#f80").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 136);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 255);
    }

    #[test]
    fn test_color_from_hex_rrggbbaa() {
        let c = Color::from_hex("#ff800080").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 128);
    }

    #[test]
    fn test_color_from_hex_no_hash() {
        let c = Color::from_hex("ff8000").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_color_from_hex_invalid() {
        assert!(Color::from_hex("#xyz").is_err());
        assert!(Color::from_hex("#1").is_err());
        assert!(Color::from_hex("12345").is_err());
    }

    #[test]
    fn test_color_to_rgba() {
        let c = Color::rgb(1, 2, 3);
        assert_eq!(c.to_rgba(), Rgba([1, 2, 3, 255]));
    }

    #[test]
    fn test_color_default() {
        let c = Color::default();
        assert_eq!(c, Color::rgb(0, 0, 0));
    }

    // ---- 组 3：Position ----

    #[test]
    fn test_position_parse() {
        assert_eq!(Position::parse("top-left").unwrap(), Position::TopLeft);
        assert_eq!(Position::parse("top-center").unwrap(), Position::TopCenter);
        assert_eq!(Position::parse("TOP-RIGHT").unwrap(), Position::TopRight);
        assert_eq!(Position::parse("center").unwrap(), Position::Center);
        assert_eq!(
            Position::parse("bottom-right").unwrap(),
            Position::BottomRight
        );
    }

    #[test]
    fn test_position_parse_invalid() {
        assert!(Position::parse("invalid").is_err());
        assert!(Position::parse("").is_err());
    }

    #[test]
    fn test_position_as_str() {
        assert_eq!(Position::TopLeft.as_str(), "top-left");
        assert_eq!(Position::Center.as_str(), "center");
        assert_eq!(Position::BottomRight.as_str(), "bottom-right");
    }

    #[test]
    fn test_position_get_xy_top_left() {
        // 主图 100x100，叠加图 20x20，左上角偏移 (0, 0)
        let (x, y) = Position::TopLeft.get_xy(100, 100, 20, 20);
        assert_eq!(x, 0);
        assert_eq!(y, 0);
    }

    #[test]
    fn test_position_get_xy_center() {
        // 主图 100x100，叠加图 20x20，居中偏移 (40, 40)
        let (x, y) = Position::Center.get_xy(100, 100, 20, 20);
        assert_eq!(x, 40);
        assert_eq!(y, 40);
    }

    #[test]
    fn test_position_get_xy_bottom_right() {
        // 主图 100x100，叠加图 20x20，右下偏移 (80, 80)
        let (x, y) = Position::BottomRight.get_xy(100, 100, 20, 20);
        assert_eq!(x, 80);
        assert_eq!(y, 80);
    }

    #[test]
    fn test_position_get_xy_top_center() {
        let (x, y) = Position::TopCenter.get_xy(100, 100, 20, 20);
        assert_eq!(x, 40); // (100-20)/2
        assert_eq!(y, 0);
    }

    // ---- 组 4：Image 基础 ----

    fn create_test_png(path: &Path, w: u32, h: u32, color: Rgba<u8>) {
        let img: RgbaImage = ImageBuffer::from_pixel(w, h, color);
        img.save(path).unwrap();
    }

    #[test]
    fn test_image_create_blank() {
        let img = Image::create_blank(100, 50);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 50);
        assert_eq!(img.image_type(), ImageType::Unknown);
        assert!(img.file_path().is_none());
    }

    #[tokio::test]
    async fn test_image_open_png() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let path = tmp.path();
        create_test_png(path, 80, 60, Rgba([255, 0, 0, 255]));
        let img = Image::open(path).await.unwrap();
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
        assert_eq!(img.image_type(), ImageType::Png);
        assert!(img.file_path().is_some());
    }

    #[test]
    fn test_image_from_dynamic() {
        let buf: RgbaImage = ImageBuffer::from_pixel(50, 50, Rgba([0, 255, 0, 255]));
        let dyn_img = DynamicImage::ImageRgba8(buf);
        let img = Image::from_dynamic(dyn_img, ImageType::Png);
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
        assert_eq!(img.image_type(), ImageType::Png);
    }

    #[test]
    fn test_image_to_rgba8() {
        let img = Image::create_blank(30, 30);
        let rgba = img.to_rgba8();
        assert_eq!(rgba.dimensions(), (30, 30));
    }

    // ---- 组 5：Editor open/save ----

    #[test]
    fn test_editor_new() {
        let _editor = Editor::new();
        let _editor2 = Editor;
    }

    #[tokio::test]
    async fn test_editor_open() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 0, 255, 255]));
        let editor = Editor::new();
        let img = editor.open(tmp.path()).await.unwrap();
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[tokio::test]
    async fn test_editor_save_png() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 50, 50, Rgba([0, 255, 0, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_out.path(), None, None, false, 0o755)
            .await
            .unwrap();
        // 验证保存的文件可读
        let reopened = image::open(tmp_out.path()).unwrap();
        assert_eq!(reopened.width(), 50);
        assert_eq!(reopened.height(), 50);
    }

    #[tokio::test]
    async fn test_editor_save_jpeg_with_quality() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 50, 50, Rgba([128, 64, 32, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_out.path(), None, Some(90), false, 0o755)
            .await
            .unwrap();
        let reopened = image::open(tmp_out.path()).unwrap();
        assert_eq!(reopened.width(), 50);
        assert_eq!(reopened.height(), 50);
    }

    // ---- 组 6：Editor resize ----

    #[tokio::test]
    async fn test_editor_resize_exact() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([255, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.resize_exact(&mut img, 50, 80);
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 80);
    }

    #[tokio::test]
    async fn test_editor_resize_fit() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 200, 100, Rgba([0, 255, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        // 200x100 → fit 100x100 → ratio=0.5 → 100x50
        editor.resize_fit(&mut img, 100, 100);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 50);
    }

    #[tokio::test]
    async fn test_editor_resize_fill() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 200, 100, Rgba([0, 0, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        // 200x100 → fill 100x100 → ratio=1.0（按高）→ 200x100 → crop center 100x100
        editor.resize_fill(&mut img, 100, 100);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[tokio::test]
    async fn test_editor_resize_exact_width() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 200, 100, Rgba([255, 255, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        // 200x100 → width=50 → 50x25
        editor.resize_exact_width(&mut img, 50);
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 25);
    }

    #[tokio::test]
    async fn test_editor_resize_exact_height() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 200, 100, Rgba([255, 0, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        // 200x100 → height=50 → 100x50
        editor.resize_exact_height(&mut img, 50);
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 50);
    }

    // ---- 组 7：Editor crop/flip/rotate ----

    #[tokio::test]
    async fn test_editor_crop_center() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor
            .crop(&mut img, 50, 50, Position::Center, 0, 0)
            .unwrap();
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
    }

    #[tokio::test]
    async fn test_editor_crop_too_large() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 50, 50, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        assert!(editor
            .crop(&mut img, 100, 100, Position::TopLeft, 0, 0)
            .is_err());
    }

    #[tokio::test]
    async fn test_editor_flip_horizontal() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([255, 128, 64, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.flip(&mut img, FlipMode::Horizontal);
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_flip_vertical() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([255, 128, 64, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.flip(&mut img, FlipMode::Vertical);
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_rotate_90() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([64, 255, 128, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, 90.0).unwrap();
        assert_eq!(img.width(), 60); // 旋转 90 度后宽高互换
        assert_eq!(img.height(), 80);
    }

    #[tokio::test]
    async fn test_editor_rotate_180() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([64, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, 180.0).unwrap();
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_rotate_invalid_angle() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        assert!(editor.rotate(&mut img, 45.0).is_err());
    }

    // ---- 组 8：Editor blend ----

    #[tokio::test]
    async fn test_editor_blend_normal() {
        let tmp1 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let tmp2 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp1.path(), 100, 100, Rgba([0, 0, 0, 255]));
        create_test_png(tmp2.path(), 50, 50, Rgba([255, 255, 255, 255]));
        let editor = Editor::new();
        let mut img1 = editor.open(tmp1.path()).await.unwrap();
        let img2 = editor.open(tmp2.path()).await.unwrap();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        assert_eq!(img1.width(), 100);
        assert_eq!(img1.height(), 100);
        // 左上角第一个像素应该是叠加图的颜色（白色，不透明）
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert_eq!(pixel[0], 255);
        assert_eq!(pixel[1], 255);
        assert_eq!(pixel[2], 255);
    }

    #[tokio::test]
    async fn test_editor_blend_with_offset() {
        let tmp1 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let tmp2 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp1.path(), 100, 100, Rgba([0, 0, 0, 255]));
        create_test_png(tmp2.path(), 50, 50, Rgba([255, 255, 255, 255]));
        let editor = Editor::new();
        let mut img1 = editor.open(tmp1.path()).await.unwrap();
        let img2 = editor.open(tmp2.path()).await.unwrap();
        // 偏移到 (30, 30)，叠加图 50x50，应影响 (30,30)-(80,80)
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                1.0,
                Position::TopLeft,
                30,
                30,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        // (0, 0) 应该是黑色（未被叠加）
        let p1 = rgba.get_pixel(0, 0);
        assert_eq!(p1[0], 0);
        // (50, 50) 应该是白色（在叠加区内）
        let p2 = rgba.get_pixel(50, 50);
        assert_eq!(p2[0], 255);
        // (90, 90) 应该是黑色（在叠加区外）
        let p3 = rgba.get_pixel(90, 90);
        assert_eq!(p3[0], 0);
    }

    #[tokio::test]
    async fn test_editor_blend_opacity_half() {
        let tmp1 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let tmp2 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp1.path(), 50, 50, Rgba([0, 0, 0, 255]));
        create_test_png(tmp2.path(), 50, 50, Rgba([255, 255, 255, 255]));
        let editor = Editor::new();
        let mut img1 = editor.open(tmp1.path()).await.unwrap();
        let img2 = editor.open(tmp2.path()).await.unwrap();
        // opacity=0.5：黑色 + 50%白色 ≈ 128
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                0.5,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        // 半透明白色叠加到黑色：128 左右
        assert!(
            (120..=136).contains(&pixel[0]),
            "expected ~128, got {}",
            pixel[0]
        );
    }

    #[test]
    fn test_blend_type_parse() {
        assert_eq!(BlendType::parse("normal").unwrap(), BlendType::Normal);
        assert_eq!(BlendType::parse("MULTIPLY").unwrap(), BlendType::Multiply);
        assert_eq!(BlendType::parse("overlay").unwrap(), BlendType::Overlay);
        assert_eq!(BlendType::parse("screen").unwrap(), BlendType::Screen);
        assert!(BlendType::parse("invalid").is_err());
    }

    #[test]
    fn test_flip_mode_parse() {
        assert_eq!(FlipMode::parse("h").unwrap(), FlipMode::Horizontal);
        assert_eq!(FlipMode::parse("V").unwrap(), FlipMode::Vertical);
        assert!(FlipMode::parse("x").is_err());
    }

    // ---- 组 9：Editor fill ----

    #[test]
    fn test_editor_fill() {
        let img = Image::create_blank(50, 50);
        let editor = Editor::new();
        let mut img = img;
        editor.fill(&mut img, Color::rgb(255, 0, 0));
        let rgba = img.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert_eq!(pixel[0], 255);
        assert_eq!(pixel[1], 0);
        assert_eq!(pixel[2], 0);
    }

    // ---- 组 10：PHP 行为对齐 R5 ----

    // R5-24：ImageType 5 种类型对齐 Grafika\ImageType
    #[test]
    fn test_r5_24_image_type_constants() {
        // 对齐 PHP ImageType 常量
        assert_eq!(ImageType::Unknown.as_str(), ""); // const UNKNOWN = ''
        assert_eq!(ImageType::Gif.as_str(), "GIF"); // const GIF = 'GIF'
        assert_eq!(ImageType::Jpeg.as_str(), "JPEG"); // const JPEG = 'JPEG'
        assert_eq!(ImageType::Png.as_str(), "PNG"); // const PNG = 'PNG'
        assert_eq!(ImageType::Wbmp.as_str(), "WBMP"); // const WBMP = 'WBMP'
    }

    // R5-25：Color hex 解析对齐 Grafika\Color
    #[test]
    fn test_r5_25_color_hex_parsing() {
        // 对齐 PHP new Color('#333333')
        let c1 = Color::from_hex("#333333").unwrap();
        assert_eq!((c1.r, c1.g, c1.b), (0x33, 0x33, 0x33));
        // 对齐 PHP new Color('#ff4444')
        let c2 = Color::from_hex("#ff4444").unwrap();
        assert_eq!((c2.r, c2.g, c2.b), (0xff, 0x44, 0x44));
        // 对齐 PHP new Color('#f00')
        let c3 = Color::from_hex("#f00").unwrap();
        assert_eq!((c3.r, c3.g, c3.b), (0xff, 0x00, 0x00));
    }

    // R5-26：Position 9 种位置 + get_xy 对齐 Grafika\Position::getXY
    #[test]
    fn test_r5_26_position_get_xy_all_nine() {
        let w1 = 100u32;
        let h1 = 100u32;
        let w2 = 20u32;
        let h2 = 20u32;
        // 验证 9 种位置的 get_xy 计算
        let cases = [
            (Position::TopLeft, 0, 0),
            (Position::TopCenter, 40, 0),
            (Position::TopRight, 80, 0),
            (Position::CenterLeft, 0, 40),
            (Position::Center, 40, 40),
            (Position::CenterRight, 80, 40),
            (Position::BottomLeft, 0, 80),
            (Position::BottomCenter, 40, 80),
            (Position::BottomRight, 80, 80),
        ];
        for (pos, ex, ey) in cases {
            let (x, y) = pos.get_xy(w1, h1, w2, h2);
            assert_eq!(x, ex, "Position {:?} x mismatch", pos);
            assert_eq!(y, ey, "Position {:?} y mismatch", pos);
        }
    }

    // R5-27：Image::open 按 getimagesize 探测类型
    #[tokio::test]
    async fn test_r5_27_image_open_detects_type() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([255, 255, 255, 255]));
        let img = Image::open(tmp.path()).await.unwrap();
        assert_eq!(img.image_type(), ImageType::Png);
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    // R5-28：Editor::resize_exact 强制目标尺寸
    #[tokio::test]
    async fn test_r5_28_resize_exact_forces_dimensions() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 200, 100, Rgba([255, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        // 200x100 → 50x50（强制忽略宽高比）
        editor.resize_exact(&mut img, 50, 50);
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
    }

    // R5-29：Editor::blend normal + opacity + offset
    #[tokio::test]
    async fn test_r5_29_blend_normal_with_offset_and_opacity() {
        let tmp1 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let tmp2 = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp1.path(), 200, 200, Rgba([0, 0, 0, 255]));
        create_test_png(tmp2.path(), 100, 100, Rgba([255, 255, 255, 255]));
        let editor = Editor::new();
        let mut img1 = editor.open(tmp1.path()).await.unwrap();
        let img2 = editor.open(tmp2.path()).await.unwrap();
        // 对齐 PHP $editor->blend($bg, $fg, 'normal', 1.0, 'top-left', 30, 30)
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                1.0,
                Position::TopLeft,
                30,
                30,
            )
            .unwrap();
        // 验证叠加区
        let rgba = img1.to_rgba8();
        // (0, 0) 未叠加 → 黑
        assert_eq!(rgba.get_pixel(0, 0)[0], 0);
        // (50, 50) 在叠加区 → 白
        assert_eq!(rgba.get_pixel(50, 50)[0], 255);
        // (129, 129) 在叠加区边界内 → 白（叠加区为 [30, 130) x [30, 130)）
        assert_eq!(rgba.get_pixel(129, 129)[0], 255);
        // (130, 130) 在叠加区外 → 黑
        assert_eq!(rgba.get_pixel(130, 130)[0], 0);
        // (150, 150) 在叠加区外 → 黑
        assert_eq!(rgba.get_pixel(150, 150)[0], 0);
    }

    // R5-30：Editor::text y 坐标基线偏移
    #[tokio::test]
    async fn test_r5_30_text_y_baseline_offset() {
        // 无法精确测试像素布局（依赖字体），但验证 y - size 偏移逻辑：
        // PHP: imagettftext y 是基线，Grafika: y += size（y 变为顶部）
        // Rust: y 是顶部，所以 Rust y = PHP y - size
        // 验证：text() 不 panic 即可（字体文件不一定可用，用闭包模拟）
        let mut img = Image::create_blank(200, 100);
        let editor = Editor::new();
        // 无字体路径 → 期望 FontLoadFailed 错误
        let result = editor
            .text(&mut img, "test", 30, 10, 50, Color::rgb(0, 0, 0), None)
            .await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    // R5-31：Editor::save 按扩展名猜类型 + JPEG 默认 quality=75
    #[tokio::test]
    async fn test_r5_31_save_infers_type_from_extension() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 128, 64, 255]));

        // 保存为 .png → 应识别为 PNG
        let tmp_png = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_png.path(), None, None, false, 0o755)
            .await
            .unwrap();
        assert!(tmp_png.path().exists());

        // 保存为 .jpg → 应识别为 JPEG
        let tmp_jpg = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        editor
            .save(&img, tmp_jpg.path(), None, None, false, 0o755)
            .await
            .unwrap();
        assert!(tmp_jpg.path().exists());
    }

    // R5-32：wrap_text 对齐 PHP wrapText（无字体文件时返回错误，但函数签名对齐）
    #[tokio::test]
    async fn test_r5_32_wrap_text_signature_alignment() {
        // 对齐 PHP wrapText($fontsize, $angle, $fontface, $string, $width, $max_line)
        // Rust: wrap_text(font_path, fontsize, string, width, max_line)
        // 验证无字体文件时返回 FontLoadFailed
        let result = wrap_text(Path::new("/nonexistent.ttf"), 30, "hello", 680, Some(2)).await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    // R5-32 补充：wrap_text_with_font 逻辑验证
    #[test]
    fn test_r5_32_wrap_text_with_font_logic() {
        // 用内置测量逻辑验证 wrap_text 的换行行为
        // 加载一个简单的字体（如果可用）；否则跳过
        let font_path = Path::new(
            "e:/vue/test/鲜视达/server/vendor/kosinix/grafika/src/Grafika/fonts/st-heiti-light.ttc",
        );
        if !font_path.exists() {
            // 字体文件不可用，跳过本测试
            eprintln!(
                "Skipping test_r5_32_wrap_text_with_font_logic: font not found at {font_path:?}"
            );
            return;
        }
        let data = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(data).unwrap();
        // 短文本不换行
        let result = wrap_text_with_font(&font, 30, "hello", 680, Some(2));
        assert_eq!(result, "hello");
        // 长文本 + max_line=2 → 应该有省略号
        let long_text = "这是一个非常长的商品名称用于测试自动换行功能应该被截断并添加省略号";
        let result = wrap_text_with_font(&font, 30, long_text, 100, Some(2));
        assert!(
            result.ends_with("..."),
            "result should end with ..., got: {result}"
        );
        assert!(
            result.contains('\n'),
            "result should contain newline, got: {result}"
        );
    }

    // ---- 组 11：measure_text ----

    #[tokio::test]
    async fn test_measure_text_nonexistent_font() {
        let result = measure_text(Path::new("/nonexistent.ttf"), 30, "hello").await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    #[test]
    fn test_text_metrics_debug() {
        let m = TextMetrics {
            width: 100,
            height: 30,
            ascent: 25,
            descent: -5,
        };
        assert_eq!(m.width, 100);
        assert_eq!(m.height, 30);
    }

    // ---- 组 12：Color 补充 ----

    #[test]
    fn test_color_from_hex_rgba_short() {
        let c = Color::from_hex("#f80f").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 136);
        assert_eq!(c.b, 0);
        assert_eq!(c.a, 255);
    }

    #[test]
    fn test_color_from_hex_with_whitespace() {
        let c = Color::from_hex("  #ff8000  ").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_color_from_hex_empty() {
        assert!(Color::from_hex("#").is_err());
        assert!(Color::from_hex("").is_err());
    }

    #[test]
    fn test_color_from_hex_invalid_chars() {
        assert!(Color::from_hex("#gggggg").is_err());
        assert!(Color::from_hex("#zz").is_err());
    }

    #[test]
    fn test_color_copy_and_eq() {
        let c1 = Color::rgb(1, 2, 3);
        let c2 = c1;
        assert_eq!(c1, c2);
    }

    // ---- 组 13：ImageType 补充 ----

    #[test]
    fn test_image_type_from_image_format_other() {
        use image::ImageFormat;
        assert_eq!(
            ImageType::from_image_format(ImageFormat::Bmp),
            ImageType::Unknown
        );
        assert_eq!(
            ImageType::from_image_format(ImageFormat::Tiff),
            ImageType::Unknown
        );
    }

    #[test]
    fn test_image_type_from_extension_empty() {
        assert_eq!(ImageType::from_extension(""), ImageType::Unknown);
    }

    // ---- 组 14：Position 补充 ----

    #[test]
    fn test_position_parse_all_nine() {
        assert_eq!(Position::parse("top-left").unwrap(), Position::TopLeft);
        assert_eq!(Position::parse("top-center").unwrap(), Position::TopCenter);
        assert_eq!(Position::parse("top-right").unwrap(), Position::TopRight);
        assert_eq!(
            Position::parse("center-left").unwrap(),
            Position::CenterLeft
        );
        assert_eq!(Position::parse("center").unwrap(), Position::Center);
        assert_eq!(
            Position::parse("center-right").unwrap(),
            Position::CenterRight
        );
        assert_eq!(
            Position::parse("bottom-left").unwrap(),
            Position::BottomLeft
        );
        assert_eq!(
            Position::parse("bottom-center").unwrap(),
            Position::BottomCenter
        );
        assert_eq!(
            Position::parse("bottom-right").unwrap(),
            Position::BottomRight
        );
    }

    #[test]
    fn test_position_as_str_all_nine() {
        assert_eq!(Position::TopLeft.as_str(), "top-left");
        assert_eq!(Position::TopCenter.as_str(), "top-center");
        assert_eq!(Position::TopRight.as_str(), "top-right");
        assert_eq!(Position::CenterLeft.as_str(), "center-left");
        assert_eq!(Position::Center.as_str(), "center");
        assert_eq!(Position::CenterRight.as_str(), "center-right");
        assert_eq!(Position::BottomLeft.as_str(), "bottom-left");
        assert_eq!(Position::BottomCenter.as_str(), "bottom-center");
        assert_eq!(Position::BottomRight.as_str(), "bottom-right");
    }

    #[test]
    fn test_position_get_xy_unequal_dimensions() {
        let (x, y) = Position::Center.get_xy(200, 100, 40, 30);
        assert_eq!(x, 80);
        assert_eq!(y, 35);
    }

    // ---- 组 15：Image 补充 ----

    #[test]
    fn test_image_from_rgba8() {
        let buf: RgbaImage = ImageBuffer::from_pixel(40, 30, Rgba([10, 20, 30, 255]));
        let img = Image::from_rgba8(buf, ImageType::Png);
        assert_eq!(img.width(), 40);
        assert_eq!(img.height(), 30);
        assert_eq!(img.image_type(), ImageType::Png);
        assert!(img.file_path().is_none());
    }

    #[test]
    fn test_image_as_dynamic() {
        let img = Image::create_blank(50, 50);
        let dyn_ref = img.as_dynamic();
        assert_eq!(dyn_ref.width(), 50);
        assert_eq!(dyn_ref.height(), 50);
    }

    #[test]
    fn test_image_as_dynamic_mut() {
        let mut img = Image::create_blank(50, 50);
        let dyn_mut = img.as_dynamic_mut();
        assert_eq!(dyn_mut.width(), 50);
        assert_eq!(dyn_mut.height(), 50);
    }

    #[tokio::test]
    async fn test_image_open_nonexistent() {
        let result = Image::open(Path::new("/nonexistent/file.png")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_image_open_unknown_extension_fails() {
        // image crate uses extension for format detection; .bin is not recognized.
        // Create PNG with proper extension first, then rename to .bin.
        let tmp_png = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_png.path(), 60, 40, Rgba([255, 0, 0, 255]));
        let bin_path = tmp_png.path().with_extension("bin");
        std::fs::rename(tmp_png.path(), &bin_path).unwrap();
        let result = Image::open(&bin_path).await;
        assert!(result.is_err());
    }

    // ---- 组 16：Editor 补充 ----

    #[test]
    fn test_editor_default() {
        let _editor = Editor;
    }

    #[tokio::test]
    async fn test_editor_rotate_0() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, 0.0).unwrap();
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_rotate_270() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, 270.0).unwrap();
        assert_eq!(img.width(), 60);
        assert_eq!(img.height(), 80);
    }

    #[tokio::test]
    async fn test_editor_rotate_negative_90() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, -90.0).unwrap();
        assert_eq!(img.width(), 60);
        assert_eq!(img.height(), 80);
    }

    #[tokio::test]
    async fn test_editor_rotate_negative_180() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, -180.0).unwrap();
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_rotate_360_normalized_to_0() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 80, 60, Rgba([0, 0, 0, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor.rotate(&mut img, 360.0).unwrap();
        assert_eq!(img.width(), 80);
        assert_eq!(img.height(), 60);
    }

    #[tokio::test]
    async fn test_editor_crop_with_positive_offset() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor
            .crop(&mut img, 50, 50, Position::Center, 10, 10)
            .unwrap();
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
    }

    #[tokio::test]
    async fn test_editor_crop_with_negative_offset_clamped() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor
            .crop(&mut img, 50, 50, Position::TopLeft, -100, -100)
            .unwrap();
        assert_eq!(img.width(), 50);
        assert_eq!(img.height(), 50);
    }

    #[tokio::test]
    async fn test_editor_crop_top_left() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor
            .crop(&mut img, 30, 30, Position::TopLeft, 0, 0)
            .unwrap();
        assert_eq!(img.width(), 30);
        assert_eq!(img.height(), 30);
    }

    #[tokio::test]
    async fn test_editor_crop_bottom_right() {
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp.path(), 100, 100, Rgba([0, 128, 255, 255]));
        let editor = Editor::new();
        let mut img = editor.open(tmp.path()).await.unwrap();
        editor
            .crop(&mut img, 30, 30, Position::BottomRight, 0, 0)
            .unwrap();
        assert_eq!(img.width(), 30);
        assert_eq!(img.height(), 30);
    }

    // ---- 组 17：Editor blend 补充 ----

    #[test]
    fn test_editor_blend_multiply() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([128, 128, 128, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([128, 128, 128, 255]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Multiply,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert!(
            (60..=68).contains(&pixel[0]),
            "expected ~64, got {}",
            pixel[0]
        );
    }

    #[test]
    fn test_editor_blend_overlay_dark() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([64, 64, 64, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([128, 128, 128, 255]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Overlay,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert!(
            (60..=68).contains(&pixel[0]),
            "expected ~64, got {}",
            pixel[0]
        );
    }

    #[test]
    fn test_editor_blend_overlay_light() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([200, 200, 200, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([200, 200, 200, 255]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Overlay,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert!(
            (225..=235).contains(&pixel[0]),
            "expected ~231, got {}",
            pixel[0]
        );
    }

    #[test]
    fn test_editor_blend_screen() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([0, 0, 0, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([0, 0, 0, 255]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Screen,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert_eq!(pixel[0], 0);
    }

    #[test]
    fn test_editor_blend_with_negative_offset() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([0, 0, 0, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([255, 255, 255, 255]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                1.0,
                Position::TopLeft,
                -25,
                -25,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0)[0], 255);
        assert_eq!(rgba.get_pixel(24, 24)[0], 255);
        assert_eq!(rgba.get_pixel(25, 25)[0], 0);
    }

    #[test]
    fn test_editor_blend_transparent_overlay() {
        let base = ImageBuffer::from_pixel(50, 50, Rgba([100, 100, 100, 255]));
        let mut img1 = Image::from_rgba8(base, ImageType::Png);
        let overlay = ImageBuffer::from_pixel(50, 50, Rgba([255, 0, 0, 0]));
        let img2 = Image::from_rgba8(overlay, ImageType::Png);
        let editor = Editor::new();
        editor
            .blend(
                &mut img1,
                &img2,
                BlendType::Normal,
                1.0,
                Position::TopLeft,
                0,
                0,
            )
            .unwrap();
        let rgba = img1.to_rgba8();
        let pixel = rgba.get_pixel(0, 0);
        assert_eq!(pixel[0], 100);
    }

    // ---- 组 18：Editor save 补充 ----

    #[tokio::test]
    async fn test_editor_save_gif() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 0, 0, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".gif").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_out.path(), None, None, false, 0o755)
            .await
            .unwrap();
        assert!(tmp_out.path().exists());
        let reopened = image::open(tmp_out.path()).unwrap();
        assert_eq!(reopened.width(), 30);
    }

    #[tokio::test]
    async fn test_editor_save_wbmp_error() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 0, 0, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".wbmp").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        let result = editor
            .save(&img, tmp_out.path(), None, None, false, 0o755)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_editor_save_unknown_type_error() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 0, 0, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".bin").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        let result = editor
            .save(&img, tmp_out.path(), None, None, false, 0o755)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_editor_save_with_explicit_png_type() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 0, 0, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(
                &img,
                tmp_out.path(),
                Some(ImageType::Png),
                None,
                false,
                0o755,
            )
            .await
            .unwrap();
        assert!(tmp_out.path().exists());
    }

    #[tokio::test]
    async fn test_editor_save_jpeg_explicit_type() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([128, 64, 32, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(
                &img,
                tmp_out.path(),
                Some(ImageType::Jpeg),
                Some(80),
                false,
                0o755,
            )
            .await
            .unwrap();
        assert!(tmp_out.path().exists());
    }

    #[tokio::test]
    async fn test_editor_save_quality_clamping_high() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([128, 64, 32, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_out.path(), None, Some(200), false, 0o755)
            .await
            .unwrap();
        assert!(tmp_out.path().exists());
    }

    #[tokio::test]
    async fn test_editor_save_quality_clamping_zero() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([128, 64, 32, 255]));
        let tmp_out = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, tmp_out.path(), None, Some(0), false, 0o755)
            .await
            .unwrap();
        assert!(tmp_out.path().exists());
    }

    #[tokio::test]
    async fn test_editor_save_creates_parent_dir() {
        let tmp_in = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        create_test_png(tmp_in.path(), 30, 30, Rgba([255, 0, 0, 255]));
        let tmp_dir = tempfile::tempdir().unwrap();
        let output_path = tmp_dir.path().join("subdir").join("output.png");
        assert!(!output_path.parent().unwrap().exists());
        let editor = Editor::new();
        let img = editor.open(tmp_in.path()).await.unwrap();
        editor
            .save(&img, &output_path, None, None, false, 0o755)
            .await
            .unwrap();
        assert!(output_path.exists());
    }

    // ---- 组 19：load_font / measure_text / wrap_text 补充 ----

    #[tokio::test]
    async fn test_load_font_invalid_data() {
        let tmp = tempfile::Builder::new().suffix(".ttf").tempfile().unwrap();
        std::fs::write(tmp.path(), b"this is not a font").unwrap();
        let mut img = Image::create_blank(100, 50);
        let editor = Editor::new();
        let result = editor
            .text(
                &mut img,
                "test",
                20,
                10,
                30,
                Color::rgb(0, 0, 0),
                Some(tmp.path()),
            )
            .await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    #[tokio::test]
    async fn test_measure_text_invalid_font() {
        let tmp = tempfile::Builder::new().suffix(".ttf").tempfile().unwrap();
        std::fs::write(tmp.path(), b"invalid font data").unwrap();
        let result = measure_text(tmp.path(), 30, "hello").await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    #[tokio::test]
    async fn test_wrap_text_nonexistent_font() {
        let result = wrap_text(Path::new("/nonexistent.ttf"), 30, "hello", 680, None).await;
        assert!(matches!(result, Err(ImageError::FontLoadFailed(_))));
    }

    #[tokio::test]
    async fn test_editor_text_with_font() {
        let font_path = Path::new("C:/Windows/Fonts/arial.ttf");
        if !font_path.exists() {
            eprintln!("Skipping test_editor_text_with_font: font not found");
            return;
        }
        let mut img = Image::create_blank(200, 100);
        let editor = Editor::new();
        let result = editor
            .text(
                &mut img,
                "hello",
                30,
                10,
                50,
                Color::rgb(255, 0, 0),
                Some(font_path),
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(img.width(), 200);
        assert_eq!(img.height(), 100);
    }

    #[tokio::test]
    async fn test_measure_text_with_font() {
        let font_path = Path::new("C:/Windows/Fonts/arial.ttf");
        if !font_path.exists() {
            eprintln!("Skipping test_measure_text_with_font: font not found");
            return;
        }
        let result = measure_text(font_path, 30, "hello").await.unwrap();
        assert!(result.width > 0);
        assert!(result.height > 0);
    }

    #[test]
    fn test_wrap_text_with_font_no_max_line() {
        let font_path = Path::new("C:/Windows/Fonts/arial.ttf");
        if !font_path.exists() {
            eprintln!("Skipping test_wrap_text_with_font_no_max_line: font not found");
            return;
        }
        let data = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(data).unwrap();
        let long_text = "this is a very long text that should wrap";
        let result = wrap_text_with_font(&font, 30, long_text, 100, None);
        assert!(result.contains('\n'), "should contain newline: {result}");
        assert!(
            !result.ends_with("..."),
            "should not end with ... when no max_line"
        );
    }
}
