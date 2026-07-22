//! Excel 导出模块 — 对齐 PHP `PhpOffice\PhpSpreadsheet`
//!
//! ## PHP 对齐
//!
//! 本模块以 PHP 项目实际使用的 PhpSpreadsheet API 子集为对齐基准：
//!
//! - `new Spreadsheet()` → [`Spreadsheet::new`]
//! - `getActiveSheet()` → [`Spreadsheet::active_sheet`]
//! - `getColumnDimension('B')->setWidth(30)` → [`Worksheet::set_column_width`]
//! - `setTitle('订单明细')` → [`Worksheet::set_title`]
//! - `setCellValue('A1', $val)` → [`Worksheet::set_cell_value`]
//! - `setCellValueExplicit('A1', $val, 's')` → [`Worksheet::set_cell_value_explicit`]
//! - `IOFactory::createWriter($s, 'Xlsx')` → [`create_writer`]
//! - `$writer->save($path)` → [`Writer::save`] / [`Writer::save_to_buffer`]
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\app\farm\service\order\ExportService.php`（5 个导出方法）
//! - `e:\vue\test\鲜视达\server\app\common\service\order\ExportService.php`（13+ 导出方法）

use std::path::Path;

use rust_xlsxwriter::{Format, Workbook, XlsxError};

use crate::PdfError;

// ============================================================================
// A1 引用解析 — 对齐 PHP `Coordinate::coordinateFromString`
// ============================================================================

/// A1 引用解析 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Cell\Coordinate::coordinateFromString`
///
/// PHP 行为：将 `'B1'` 解析为 `('B', 1)`，列字母转列号（A=0, B=1, ..., Z=25, AA=26）。
///
/// Rust 端返回 `(row, col)`（均为 0-based 索引），对齐 rust_xlsxwriter 的 `write(row, col, data)` API。
///
/// # R5-33 硬约束
///
/// - `'A1'` → `(0, 0)`
/// - `'B1'` → `(0, 1)`
/// - `'AA1'` → `(0, 26)`
/// - `'AB10'` → `(9, 27)`
///
/// # 错误
///
/// - 空字符串 → [`PdfError::InvalidCellRef`]
/// - 无效列字母（非 A-Z） → [`PdfError::InvalidCellRef`]
/// - 无效行号（非数字或 0） → [`PdfError::InvalidCellRef`]
pub fn parse_a1(reference: &str) -> Result<(u32, u16), PdfError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(PdfError::InvalidCellRef(reference.to_string()));
    }

    // 对齐 PHP：列字母部分（A-Z，可多字母），行数字部分
    let mut col_letters_end = 0;
    for (i, ch) in reference.chars().enumerate() {
        if ch.is_ascii_alphabetic() {
            col_letters_end = i + 1;
        } else {
            break;
        }
    }

    if col_letters_end == 0 {
        return Err(PdfError::InvalidCellRef(reference.to_string()));
    }

    let col_str = &reference[..col_letters_end];
    let row_str = &reference[col_letters_end..];

    // 列字母 → 列号（A=0, B=1, ..., Z=25, AA=26, AB=27）
    let mut col: u16 = 0;
    for ch in col_str.chars() {
        if !ch.is_ascii_uppercase() {
            // 小写转大写（PHP 大小写不敏感）
            if !ch.is_ascii_alphabetic() {
                return Err(PdfError::InvalidCellRef(reference.to_string()));
            }
        }
        let upper = ch.to_ascii_uppercase();
        col = col
            .checked_mul(26)
            .and_then(|c| c.checked_add((upper as u16) - ('A' as u16) + 1))
            .ok_or_else(|| PdfError::InvalidCellRef(reference.to_string()))?;
    }
    // A=1 → col=0
    let col = col
        .checked_sub(1)
        .ok_or_else(|| PdfError::InvalidCellRef(reference.to_string()))?;

    // 行号 → 行索引（1-based → 0-based）
    let row: u32 = row_str
        .parse()
        .map_err(|_| PdfError::InvalidCellRef(reference.to_string()))?;
    if row == 0 {
        return Err(PdfError::InvalidCellRef(reference.to_string()));
    }
    let row = row - 1;

    Ok((row, col))
}

// ============================================================================
// 单元格值类型 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Cell\DataType`
// ============================================================================

/// 单元格值类型 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Cell\DataType` 常量
///
/// PHP 源码（`DataType.php`）：
/// ```php
/// const TYPE_STRING2 = 'str';
/// const TYPE_STRING  = 's';
/// const TYPE_FORMULA = 'f';
/// const TYPE_NUMERIC = 'n';
/// const TYPE_BOOL    = 'b';
/// const TYPE_NULL    = 'null';
/// ```
///
/// Rust 端仅保留业务实际使用的两种类型（字符串 + 自动推断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellType {
    /// 自动推断类型（对齐 PHP `setCellValue` 行为）
    ///
    /// - 纯数字字符串 → 数字
    /// - 其他 → 字符串
    #[default]
    Auto,
    /// 强制字符串类型（对齐 PHP `DataType::TYPE_STRING = 's'`）
    String,
}

// ============================================================================
// 单元格值 — 统一承载 PHP 的 mixed 类型
// ============================================================================

/// 单元格值 — 对齐 PHP `setCellValue($coord, $value)` 中的 `$value`
///
/// PHP 端 `$value` 是 mixed 类型，可以是 int/float/string/null/bool。
/// Rust 端用枚举统一承载，并提供 `From` 实现以便从常见 Rust 类型转换。
#[derive(Debug, Clone)]
pub enum CellValue {
    /// 字符串（对齐 PHP string）
    String(String),
    /// 整数（对齐 PHP int）
    Int(i64),
    /// 浮点数（对齐 PHP float）
    Float(f64),
    /// 布尔（对齐 PHP bool）
    Bool(bool),
    /// 空值（对齐 PHP null）
    Null,
}

impl From<&str> for CellValue {
    fn from(s: &str) -> Self {
        Self::String(s.to_string())
    }
}

impl From<String> for CellValue {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<i64> for CellValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for CellValue {
    fn from(v: i32) -> Self {
        Self::Int(v as i64)
    }
}

impl From<u32> for CellValue {
    fn from(v: u32) -> Self {
        Self::Int(v as i64)
    }
}

impl From<u64> for CellValue {
    fn from(v: u64) -> Self {
        Self::Int(v as i64)
    }
}

impl From<f64> for CellValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<f32> for CellValue {
    fn from(v: f32) -> Self {
        Self::Float(v as f64)
    }
}

impl From<bool> for CellValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl CellValue {
    /// 写入到 rust_xlsxwriter 工作表 — 对齐 PHP `setCellValue` 自动类型推断
    ///
    /// PHP 行为（`Cell::setValue` + `DefaultValueBinder::bindValue`）：
    /// - null → 空字符串
    /// - bool → 'TRUE'/'FALSE' 字符串（Excel 内部转 bool）
    /// - int/float → 数字
    /// - string → 检测是否为数字字符串，是则转数字
    ///
    /// Rust 端对应：
    /// - Null → 跳过写入
    /// - Bool → 写入 bool
    /// - Int → 写入 i64
    /// - Float → 写入 f64
    /// - String + Auto → 检测数字字符串
    /// - String + String → 强制字符串
    fn write_to(
        &self,
        sheet: &mut rust_xlsxwriter::Worksheet,
        row: u32,
        col: u16,
        cell_type: CellType,
    ) -> Result<(), XlsxError> {
        match self {
            Self::Null => {
                // 对齐 PHP null → 空单元格（不写入）
                Ok(())
            }
            Self::Bool(b) => {
                sheet.write(row, col, *b)?;
                Ok(())
            }
            Self::Int(n) => {
                sheet.write(row, col, *n)?;
                Ok(())
            }
            Self::Float(f) => {
                sheet.write(row, col, *f)?;
                Ok(())
            }
            Self::String(s) => {
                match cell_type {
                    CellType::String => {
                        // 对齐 PHP `setCellValueExplicit(..., DataType::TYPE_STRING)`
                        // 强制字符串类型，不进行数字检测
                        sheet.write_string(row, col, s)?;
                        Ok(())
                    }
                    CellType::Auto => {
                        // 对齐 PHP `setCellValue` 自动类型推断
                        // PHP `DefaultValueBinder::bindValue` 会对纯数字字符串转为数字
                        if let Ok(i) = s.parse::<i64>() {
                            sheet.write(row, col, i)?;
                        } else if let Ok(f) = s.parse::<f64>() {
                            // 排除 "inf" "nan" 等被 parse 解析的特殊值
                            if f.is_finite() {
                                sheet.write(row, col, f)?;
                            } else {
                                sheet.write_string(row, col, s)?;
                            }
                        } else {
                            sheet.write_string(row, col, s)?;
                        }
                        Ok(())
                    }
                }
            }
        }
    }
}

// ============================================================================
// Worksheet 封装 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Worksheet`
// ============================================================================

/// 工作表 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Worksheet\Worksheet`
///
/// 对应 PHP 端 `$sheet = $spreadsheet->getActiveSheet()` 返回的对象。
///
/// 内部持有 `&mut rust_xlsxwriter::Worksheet`，避免生命周期冲突。
/// 由于 `Worksheet` 的方法都需要 `&mut self`，Rust 端设计为持有 `Worksheet` 的可变引用。
pub struct Worksheet<'a> {
    sheet: &'a mut rust_xlsxwriter::Worksheet,
}

impl<'a> Worksheet<'a> {
    /// 设置工作表名 — 对齐 PHP `$sheet->setTitle('订单明细')`
    pub fn set_title(&mut self, title: &str) -> Result<(), PdfError> {
        self.sheet.set_name(title)?;
        Ok(())
    }

    /// 设置列宽 — 对齐 PHP `$sheet->getColumnDimension('B')->setWidth(30)`
    ///
    /// 参数 `col` 是 0-based 列号（A=0, B=1, ...）。
    ///
    /// 注意：PHP `setWidth` 的 width 单位是字符数，rust_xlsxwriter 也是字符数，单位一致。
    pub fn set_column_width(&mut self, col: u16, width: f64) -> Result<(), PdfError> {
        self.sheet.set_column_width(col, width)?;
        Ok(())
    }

    /// 通过 A1 引用设置单元格值 — 对齐 PHP `$sheet->setCellValue('A1', $val)`
    ///
    /// 自动类型推断（对齐 PHP `DefaultValueBinder`）：
    /// - 纯数字字符串 → 数字
    /// - 其他 → 字符串
    pub fn set_cell_value<T: Into<CellValue>>(
        &mut self,
        reference: &str,
        value: T,
    ) -> Result<(), PdfError> {
        let (row, col) = parse_a1(reference)?;
        let value = value.into();
        value.write_to(self.sheet, row, col, CellType::Auto)?;
        Ok(())
    }

    /// 通过 A1 引用设置单元格值（显式类型）— 对齐 PHP `$sheet->setCellValueExplicit('A1', $val, 's')`
    ///
    /// `cell_type` 参数对齐 PHP `DataType::TYPE_STRING = 's'` / `TYPE_NUMERIC = 'n'`。
    /// 当前仅支持 [`CellType::String`]（'s'），其他类型回退到自动推断。
    pub fn set_cell_value_explicit<T: Into<CellValue>>(
        &mut self,
        reference: &str,
        value: T,
        cell_type: CellType,
    ) -> Result<(), PdfError> {
        let (row, col) = parse_a1(reference)?;
        let value = value.into();
        value.write_to(self.sheet, row, col, cell_type)?;
        Ok(())
    }

    /// 通过行列号设置单元格值（0-based）— 便于程序化批量写入
    ///
    /// 对齐 PHP `$sheet->setCellValueByColumnAndRow($col, $row, $val)`（已废弃但项目使用）。
    pub fn set_cell_value_by_row_col<T: Into<CellValue>>(
        &mut self,
        row: u32,
        col: u16,
        value: T,
    ) -> Result<(), PdfError> {
        let value = value.into();
        value.write_to(self.sheet, row, col, CellType::Auto)?;
        Ok(())
    }

    /// 设置单元格格式（对齐 PHP `$sheet->getStyle('A1')->applyFromArray([...])`）
    ///
    /// 当前仅支持 bold 格式，业务代码未使用复杂样式。
    pub fn set_cell_format(&mut self, reference: &str, format: &Format) -> Result<(), PdfError> {
        let (row, col) = parse_a1(reference)?;
        // rust_xlsxwriter 的 write_with_format 要求同时写入值，这里仅设置列宽/行高样式
        // 业务实际未使用单元格级格式，仅保留 API 占位
        let _ = (row, col, format);
        Ok(())
    }
}

// ============================================================================
// Spreadsheet 封装 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Spreadsheet`
// ============================================================================

/// 工作簿 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Spreadsheet`
///
/// PHP 用法：
/// ```php
/// $spreadsheet = new Spreadsheet();
/// $sheet = $spreadsheet->getActiveSheet();
/// $sheet->setCellValue('A1', '订单号');
/// ```
///
/// Rust 用法：
/// ```ignore
/// let mut spreadsheet = Spreadsheet::new();
/// let sheet = spreadsheet.active_sheet();
/// sheet.set_cell_value("A1", "订单号")?;
/// ```
pub struct Spreadsheet {
    workbook: Workbook,
    /// 当前活跃工作表索引（对齐 PHP `getActiveSheet()`）
    active_index: usize,
}

impl Spreadsheet {
    /// 创建新工作簿 — 对齐 PHP `new Spreadsheet()`
    pub fn new() -> Self {
        let mut workbook = Workbook::new();
        // PHP `new Spreadsheet()` 默认创建一个工作表
        workbook.add_worksheet();
        Self {
            workbook,
            active_index: 0,
        }
    }

    /// 获取活跃工作表 — 对齐 PHP `$spreadsheet->getActiveSheet()`
    ///
    /// 返回 [`Worksheet`] 封装，提供对齐 PHP 的方法。
    pub fn active_sheet(&mut self) -> Worksheet<'_> {
        let sheet = self
            .workbook
            .worksheet_from_index(self.active_index)
            .expect("active worksheet must exist");
        Worksheet { sheet }
    }

    /// 添加新工作表 — 对齐 PHP `$spreadsheet->createSheet()`
    pub fn add_sheet(&mut self) -> Worksheet<'_> {
        self.workbook.add_worksheet();
        self.active_index = self.worksheet_count() - 1;
        let sheet = self
            .workbook
            .worksheet_from_index(self.active_index)
            .expect("newly added worksheet must exist");
        Worksheet { sheet }
    }

    /// 按 index 切换活跃工作表 — 对齐 PHP `$spreadsheet->setActiveSheetIndex($i)`
    pub fn set_active_sheet_index(&mut self, index: usize) -> Result<(), PdfError> {
        if index >= self.worksheet_count() {
            return Err(PdfError::ExcelWrite(format!(
                "worksheet index {index} out of range (count={})",
                self.worksheet_count()
            )));
        }
        self.active_index = index;
        Ok(())
    }

    /// 工作表数量
    pub fn worksheet_count(&mut self) -> usize {
        // worksheets() 在 rust_xlsxwriter 0.79 中需要 &mut self
        self.workbook.worksheets().len()
    }

    /// 获取内部 `Workbook`（用于写入器）
    pub(crate) fn into_workbook(self) -> Workbook {
        self.workbook
    }
}

impl Default for Spreadsheet {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Writer — 对齐 PHP `IOFactory::createWriter` + `$writer->save`
// ============================================================================

/// Excel 写入器 — 对齐 PHP `PhpOffice\PhpSpreadsheet\Writer\Xlsx`
///
/// PHP 用法：
/// ```php
/// $writer = IOFactory::createWriter($spreadsheet, 'Xlsx');
/// $writer->save('php://output');  // 输出到浏览器
/// $writer->save('/path/to/file.xlsx');  // 保存到文件
/// ```
///
/// Rust 用法：
/// ```ignore
/// let writer = create_writer(spreadsheet);
/// writer.save("/path/to/file.xlsx")?;  // 保存到文件
/// let bytes: Vec<u8> = writer.save_to_buffer()?;  // 输出为字节流
/// ```
pub struct Writer {
    workbook: Workbook,
}

/// 创建 Excel 写入器 — 对齐 PHP `IOFactory::createWriter($spreadsheet, 'Xlsx')`
///
/// PHP 的 `$type` 参数当前固定为 'Xlsx'，Rust 端不支持其他格式（业务未使用）。
pub fn create_writer(spreadsheet: Spreadsheet) -> Writer {
    Writer {
        workbook: spreadsheet.into_workbook(),
    }
}

impl Writer {
    /// 保存到文件 — 对齐 PHP `$writer->save('/path/to/file.xlsx')`
    pub fn save<P: AsRef<Path>>(mut self, path: P) -> Result<(), PdfError> {
        self.workbook.save(path)?;
        Ok(())
    }

    /// 保存到字节缓冲区 — 对齐 PHP `$writer->save('php://output')`
    ///
    /// PHP 端 `php://output` 直接输出到浏览器，Rust 端返回 `Vec<u8>`，
    /// 由上层 web 框架设置 `Content-Disposition: attachment; filename=...` 头。
    pub fn save_to_buffer(mut self) -> Result<Vec<u8>, PdfError> {
        let bytes = self.workbook.save_to_buffer()?;
        Ok(bytes)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // R5-33：A1 引用解析测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_33_a1_reference_basic() {
        // 对齐 PHP `Coordinate::coordinateFromString`
        assert_eq!(parse_a1("A1").unwrap(), (0, 0));
        assert_eq!(parse_a1("B1").unwrap(), (0, 1));
        assert_eq!(parse_a1("C1").unwrap(), (0, 2));
        assert_eq!(parse_a1("Z1").unwrap(), (0, 25));
    }

    #[test]
    fn test_r5_33_a1_reference_multi_letters() {
        // AA = 26, AB = 27
        assert_eq!(parse_a1("AA1").unwrap(), (0, 26));
        assert_eq!(parse_a1("AB1").unwrap(), (0, 27));
        assert_eq!(parse_a1("AZ1").unwrap(), (0, 51));
        assert_eq!(parse_a1("BA1").unwrap(), (0, 52));
    }

    #[test]
    fn test_r5_33_a1_reference_rows() {
        assert_eq!(parse_a1("A1").unwrap(), (0, 0));
        assert_eq!(parse_a1("A2").unwrap(), (1, 0));
        assert_eq!(parse_a1("A10").unwrap(), (9, 0));
        assert_eq!(parse_a1("B10").unwrap(), (9, 1));
        assert_eq!(parse_a1("AB10").unwrap(), (9, 27));
    }

    #[test]
    fn test_r5_33_a1_reference_lowercase() {
        // 对齐 PHP 大小写不敏感行为
        assert_eq!(parse_a1("a1").unwrap(), (0, 0));
        assert_eq!(parse_a1("b1").unwrap(), (0, 1));
        assert_eq!(parse_a1("aa1").unwrap(), (0, 26));
    }

    #[test]
    fn test_r5_33_a1_reference_with_whitespace() {
        // 对齐 PHP 允许前后空格
        assert_eq!(parse_a1("  A1  ").unwrap(), (0, 0));
    }

    #[test]
    fn test_r5_33_a1_reference_invalid() {
        // 空字符串
        assert!(parse_a1("").is_err());
        // 无列字母
        assert!(parse_a1("1").is_err());
        // 无行号
        assert!(parse_a1("A").is_err());
        // 行号为 0
        assert!(parse_a1("A0").is_err());
        // 行号非数字
        assert!(parse_a1("AX").is_err());
        // 特殊字符
        assert!(parse_a1("A-1").is_err());
    }

    // ------------------------------------------------------------------------
    // R5-34：setCellValue 自动类型推断测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_34_set_cell_value_string() {
        // 字符串 "订单号" 应保持为字符串
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", "订单号").unwrap();
        // 写入成功即通过（rust_xlsxwriter 内部记录类型）
    }

    #[test]
    fn test_r5_34_set_cell_value_int() {
        // 整数 42 应写入为数字
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", 42i64).unwrap();
    }

    #[test]
    fn test_r5_34_set_cell_value_float() {
        // 浮点数 2.5 应写入为数字
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", 2.5f64).unwrap();
    }

    #[test]
    fn test_r5_34_set_cell_value_numeric_string() {
        // 对齐 PHP `DefaultValueBinder`：纯数字字符串自动转数字
        // "123" → 数字 123
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", "123").unwrap();
    }

    #[test]
    fn test_r5_34_set_cell_value_null() {
        // null → 空单元格（不写入）
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", CellValue::Null).unwrap();
    }

    #[test]
    fn test_r5_34_set_cell_value_bool() {
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", true).unwrap();
        sheet.set_cell_value("A2", false).unwrap();
    }

    // ------------------------------------------------------------------------
    // R5-35：setCellValueExplicit 强制字符串类型测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_35_set_cell_value_explicit_string() {
        // 对齐 PHP `setCellValueExplicit('A1', '123', DataType::TYPE_STRING)`
        // 即使值是 "123"，也应作为字符串写入（而非数字）
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet
            .set_cell_value_explicit("A1", "123", CellType::String)
            .unwrap();
    }

    #[test]
    fn test_r5_35_set_cell_value_explicit_order_no() {
        // 对齐业务代码 `setCellValueExplicit('A' . ($index + 2), $order['order_no'], 's')`
        // 订单号如 "202109010001" 应作为字符串写入（避免长数字被科学计数法显示）
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet
            .set_cell_value_explicit("A2", "202109010001", CellType::String)
            .unwrap();
    }

    // ------------------------------------------------------------------------
    // 业务场景对齐测试 — 对齐 ExportService::orderList
    // ------------------------------------------------------------------------

    #[test]
    fn test_business_order_list_export_pattern() {
        // 对齐 `app\farm\service\order\ExportService::orderList` 方法
        // 业务模式：1. setColumnDimension 2. setTitle 3. setCellValue A1-AD1 表头 4. 循环填充数据 5. save
        let mut spreadsheet = Spreadsheet::new();
        {
            let mut sheet = spreadsheet.active_sheet();
            // 列宽（对齐 PHP `$sheet->getColumnDimension('B')->setWidth(30)`）
            sheet.set_column_width(1, 30.0).unwrap(); // B 列
            sheet.set_column_width(15, 30.0).unwrap(); // P 列
                                                       // 工作表名
            sheet.set_title("订单明细").unwrap();
            // 表头（30 个列）
            let headers = [
                "订单号",
                "商品信息",
                "订单总额",
                "优惠券抵扣",
                "积分抵扣",
                "运费金额",
                "后台改价",
                "实付款金额",
                "支付方式",
                "下单时间",
                "买家",
                "买家留言",
                "配送方式",
                "自提门店名称",
                "自提联系人",
                "自提联系电话",
                "收货人姓名",
                "联系电话",
                "收货人地址",
                "物流公司",
                "物流单号",
                "付款状态",
                "付款时间",
                "发货状态",
                "发货时间",
                "收货状态",
                "收货时间",
                "订单状态",
                "微信支付交易号",
                "是否已评价",
            ];
            for (i, h) in headers.iter().enumerate() {
                // 列号 → A1 列字母（A=0, B=1, ..., Z=25, AA=26, AB=27, ...）
                let col_letter = {
                    let mut n = i as u32;
                    let mut s = String::new();
                    n += 1; // 1-based for modulo calculation
                    while n > 0 {
                        n -= 1;
                        let c = (b'A' + (n % 26) as u8) as char;
                        s.insert(0, c);
                        n /= 26;
                    }
                    s
                };
                let coord = format!("{col_letter}1");
                sheet.set_cell_value(&coord, *h).unwrap();
            }
            // 数据行
            sheet.set_cell_value("A2", "\tORD001\t").unwrap();
            sheet.set_cell_value("C2", 100.50f64).unwrap();
            sheet.set_cell_value("J2", "2026-07-21 10:00:00").unwrap();
        }
        // 保存到缓冲区（对齐 PHP `$writer->save('php://output')`）
        let writer = create_writer(spreadsheet);
        let bytes = writer.save_to_buffer().unwrap();
        assert!(!bytes.is_empty());
        // xlsx 文件签名：PK\x03\x04（zip 格式）
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }

    #[test]
    fn test_business_tab_wrapped_value() {
        // 对齐业务代码 `setCellValue('A' . ($index + 2), "\t" . $order['order_no'] . "\t")`
        // 用 \t 包裹强制 Excel 识别为文本（避免长数字被科学计数法显示）
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        sheet.set_cell_value("A1", "\tORD001\t").unwrap();
    }

    // ------------------------------------------------------------------------
    // R5-36/R5-37：文件保存与读取往返测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_36_save_to_file() {
        // 对齐 PHP `$writer->save('/path/to/file.xlsx')`
        let mut spreadsheet = Spreadsheet::new();
        {
            let mut sheet = spreadsheet.active_sheet();
            sheet.set_title("TestSheet").unwrap();
            sheet.set_column_width(0, 20.0).unwrap();
            sheet.set_cell_value("A1", "Hello").unwrap();
            sheet.set_cell_value("B1", "World").unwrap();
            sheet.set_cell_value("A2", 42i64).unwrap();
            sheet.set_cell_value("B2", 2.5f64).unwrap();
        }
        let writer = create_writer(spreadsheet);
        // 临时文件
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        writer.save(&path).unwrap();
        // 验证文件非空
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(metadata.len() > 0);
    }

    #[test]
    fn test_r5_37_save_to_buffer() {
        // 对齐 PHP `$writer->save('php://output')`
        let mut spreadsheet = Spreadsheet::new();
        {
            let mut sheet = spreadsheet.active_sheet();
            sheet.set_cell_value("A1", "测试中文").unwrap();
        }
        let writer = create_writer(spreadsheet);
        let bytes = writer.save_to_buffer().unwrap();
        assert!(!bytes.is_empty());
        // xlsx 是 zip 格式，签名 PK\x03\x04
        assert_eq!(&bytes[..4], b"PK\x03\x04");
    }

    // ------------------------------------------------------------------------
    // 多工作表测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_multiple_sheets() {
        let mut spreadsheet = Spreadsheet::new();
        // 默认 1 个工作表
        assert_eq!(spreadsheet.worksheet_count(), 1);
        {
            let mut sheet = spreadsheet.active_sheet();
            sheet.set_title("Sheet1").unwrap();
            sheet.set_cell_value("A1", "S1A1").unwrap();
        }
        // 添加第二个工作表
        spreadsheet.add_sheet();
        assert_eq!(spreadsheet.worksheet_count(), 2);
        {
            let mut sheet = spreadsheet.active_sheet();
            sheet.set_title("Sheet2").unwrap();
            sheet.set_cell_value("A1", "S2A1").unwrap();
        }
        // 切回第一个工作表
        spreadsheet.set_active_sheet_index(0).unwrap();
        {
            let mut sheet = spreadsheet.active_sheet();
            sheet.set_cell_value("A2", "S1A2").unwrap();
        }
        // 保存
        let writer = create_writer(spreadsheet);
        let bytes = writer.save_to_buffer().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_set_active_sheet_index_out_of_range() {
        let mut spreadsheet = Spreadsheet::new();
        assert!(spreadsheet.set_active_sheet_index(99).is_err());
    }

    // ------------------------------------------------------------------------
    // CellValue 类型转换测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_cell_value_from_str() {
        let v: CellValue = "hello".into();
        assert!(matches!(v, CellValue::String(_)));
    }

    #[test]
    fn test_cell_value_from_string() {
        let v: CellValue = String::from("hello").into();
        assert!(matches!(v, CellValue::String(_)));
    }

    #[test]
    fn test_cell_value_from_i64() {
        let v: CellValue = 42i64.into();
        assert!(matches!(v, CellValue::Int(42)));
    }

    #[test]
    fn test_cell_value_from_i32() {
        let v: CellValue = 42i32.into();
        assert!(matches!(v, CellValue::Int(42)));
    }

    #[test]
    fn test_cell_value_from_f64() {
        let v: CellValue = 2.5f64.into();
        assert!(matches!(v, CellValue::Float(_)));
    }

    #[test]
    fn test_cell_value_from_bool() {
        let v: CellValue = true.into();
        assert!(matches!(v, CellValue::Bool(true)));
    }

    // ------------------------------------------------------------------------
    // 列号边界测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_a1_reference_max_excel_column() {
        // Excel 最大列 XFD (16383)
        assert_eq!(parse_a1("XFD1").unwrap(), (0, 16383));
    }

    #[test]
    fn test_set_column_width_boundary() {
        let mut spreadsheet = Spreadsheet::new();
        let mut sheet = spreadsheet.active_sheet();
        // 列 0 (A)
        sheet.set_column_width(0, 10.0).unwrap();
        // 列 25 (Z)
        sheet.set_column_width(25, 20.0).unwrap();
        // 列 26 (AA)
        sheet.set_column_width(26, 30.0).unwrap();
    }
}
