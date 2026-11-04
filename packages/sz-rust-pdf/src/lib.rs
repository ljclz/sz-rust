// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! # sz-rust-pdf — Excel/PDF 导出导入包
//!
//! 本包实现 Excel/PDF 处理功能，对齐 PHP 项目实际使用的
//! `phpoffice/phpspreadsheet` + `mikehaertl/php-pdftk` + `FPDM` API 子集。
//!
//! ## PHP 对齐说明
//!
//! 项目 `composer.json` 声明 `tecnickcom/tcpdf ^6.4.4`，但业务代码未直接使用
//! （僵尸依赖，PDF 生成委派给 Java 服务 `http://127.0.0.1:8086`）。
//! 实际使用的 PDF 库是 `mikehaertl/php-pdftk ^0.10.4` 和 `FPDM`（表单填充）。
//!
//! ## PHP 对齐
//!
//! ### Excel 导出（[`excel_export`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `new Spreadsheet()` | [`excel_export::Spreadsheet`] | 工作簿 |
//! | `getActiveSheet()` | [`excel_export::Spreadsheet::active_sheet`] | 活跃工作表 |
//! | `getColumnDimension('B')->setWidth(30)` | [`excel_export::Worksheet::set_column_width`] | 列宽 |
//! | `setTitle('订单明细')` | [`excel_export::Worksheet::set_title`] | 工作表名 |
//! | `setCellValue('A1', $val)` | [`excel_export::Worksheet::set_cell_value`] | A1 引用写入 |
//! | `setCellValueExplicit('A1', $val, 's')` | [`excel_export::Worksheet::set_cell_value_explicit`] | 显式类型写入 |
//! | `IOFactory::createWriter($s, 'Xlsx')` | [`excel_export::create_writer`] | 创建写入器 |
//! | `$writer->save($path)` | [`excel_export::Writer::save`] / [`excel_export::Writer::save_to_buffer`] | 保存到文件/字节流 |
//!
//! ### Excel 导入（[`excel_import`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `IOFactory::identify($path)` | [`excel_import::identify`] | 自动识别 Excel 类型 |
//! | `IOFactory::createReader($type)` | [`excel_import::create_reader`] | 创建读取器 |
//! | `$reader->load($path)` | [`excel_import::Reader::load`] | 加载文件 |
//! | `$phpExcel->getSheet(0)` | [`excel_import::Workbook::sheet_by_index`] | 取第 N 个工作表 |
//! | `$sheet->toArray()` | [`excel_import::Worksheet::to_array`] | 转二维数组 |
//!
//! ### PDF 表单填充（[`pdf_form`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `new Pdf($path)` | [`pdf_form::Pdf::load`] | 加载 PDF |
//! | `fillForm($data)` | [`pdf_form::Pdf::fill_form`] | 填充表单字段 |
//! | `flatten()` | [`pdf_form::Pdf::flatten`] | 扁平化表单 |
//! | `saveAs($url)` | [`pdf_form::Pdf::save_as`] | 保存到文件 |
//! | `send($filename)` | [`pdf_form::Pdf::to_bytes`] | 输出为字节流 |
//!
//! ### CSV 导出（[`csv_export`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `export_excel($fileName, $tileArray, $dataArray)` | [`csv_export::export_csv_to_writer`] / [`csv_export::export_csv_to_file`] / [`csv_export::export_csv_to_bytes`] | 带 UTF-8 BOM 的 CSV 导出 |
//! | `exportCsv($filename, $data)` | [`csv_export::export_csv_no_bom_to_writer`] / [`csv_export::export_csv_no_bom_to_bytes`] | 不带 BOM 的 CSV 导出 |
//! | `fputcsv($fp, $fields)` | [`csv_export::write_csv_row`] | 写入单行 CSV |
//!
//! ### HTTP Java 服务客户端（[`java_client`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `http_java_post($url, $data)` | [`java_client::http_java_post`] / [`java_client::JavaClient::post`] | POST JSON 到 Java PDF 服务 |
//!
//! ### 辅助函数（[`util`] 模块）
//!
//! | PHP API | Rust API | 说明 |
//! |---------|----------|------|
//! | `moneyToArray($num)` | [`util::money_to_array`] | 金额转大写汉字数组 |
//! | `data_path($path)` | [`util::data_path`] | 数据目录路径 |
//! | `date('YmdHis')` 文件名 | [`util::filename_with_timestamp`] | 带时间戳的文件名 |
//!
//! ## PHP 行为对齐（R5 硬约束）
//!
//! - **R5-33**：A1 引用样式解析（'A1' → (0,0)、'B1' → (0,1)、'AA1' → (0,26)）
//!   对齐 PHP `PhpOffice\PhpSpreadsheet\Cell\Coordinate::coordinateFromString`
//! - **R5-34**：`setCellValue` 自动类型推断（int/float/string）对齐 PHP `Cell::setValue`
//! - **R5-35**：`setCellValueExplicit(..., 's')` 强制字符串类型对齐 PHP `DataType::TYPE_STRING`
//! - **R5-36**：`IOFactory::identify` 自动识别 Xlsx/Xls/Ods 对齐 PHP `IOFactory::identify`
//! - **R5-37**：`toArray()` 空单元格返回空字符串对齐 PHP `toArray()` 默认行为
//! - **R5-38**：`fillForm` 按字段名（/T）匹配并设置值（/V）对齐 PHP `pdftk fillForm`
//! - **R5-39**：`flatten` 设置 `/NeedAppearances=true` 并清除 `/AP` 对齐 PHP `pdftk flatten`
//!   （注：PHP pdftk 实际将字段渲染到页面内容流；Rust 端采用 `/NeedAppearances` + `/AP`
//!   重建策略，行为对齐但实现略简化，PDF 1.7 规范允许此方式）
//! - **R5-40**：`saveAs` 输出 PDF 1.5+ 兼容格式对齐 PHP `pdftk saveAs`
//! - **R5-41**：`send` 通过 `save_to_buffer` 返回 `Vec<u8>`，由上层 web 框架
//!   设置 `Content-Disposition: attachment; filename=...` 头
//! - **R5-42**：`export_excel` UTF-8 BOM + fputcsv 行为对齐 PHP `export_excel` 全局函数
//!   （BOM = 0xEF 0xBB 0xBF；fputcsv 采用 RFC 4180 / PHP 8.1+ 默认行为）
//! - **R5-43**：`exportCsv` 不带 BOM 的 fputcsv 行为对齐 PHP `exportCsv` 全局函数
//! - **R5-44**：`http_java_post` POST JSON + 特定 HTTP 头对齐 PHP `http_java_post` 全局函数
//!   （Content-Type: application/json; charset=utf-8 / Cache-Control: no-cache / Pragma: no-cache）
//! - **R5-45**：`money_to_array` 金额按位拆分为大写汉字数组对齐 PHP `moneyToArray`
//!   （小数 2 位键 0-1，整数部分键 2+，低位在前）
//! - **R5-46**：`data_path` 数据目录路径对齐 PHP `data_path` 全局函数
//! - **R5-47**：`filename_with_timestamp` 带时间戳文件名对齐 PHP `date('YmdHis')` 命名规则
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\app\farm\service\order\ExportService.php`（农场端 Excel 导出，5 方法）
//! - `e:\vue\test\鲜视达\server\app\common\service\order\ExportService.php`（公共端 Excel 导出，13+ 方法）
//! - `e:\vue\test\鲜视达\server\app\farm\model\order\Order.php`（Excel 导入 `batchDelivery` 方法）
//! - `e:\vue\test\鲜视达\server\app\oapi\controller\Index.php`（PDF 表单填充 `outpdf`/`outpdf2`/`outpdf3`）
//! - `e:\vue\test\鲜视达\server\vendor\mikehaertl\php-pdftk\src\Pdf.php`（PHP pdftk 封装）
//! - `e:\vue\test\鲜视达\server\app\common.php`（`export_excel` 第 924 行 / `exportCsv` 第 958 行 /
//!   `http_java_post` 第 1871 行 / `moneyToArray` 第 1814 行 / `data_path` 第 38 行）
//! - `e:\vue\test\鲜视达\server\app\job\controller\Pdf.php`（PDF 生成队列任务，7 种业务类型）

#![forbid(unsafe_code)]
#![allow(missing_docs)]

pub mod csv_export;
pub mod excel_export;
pub mod excel_import;
pub mod java_client;
pub mod pdf_form;
pub mod util;

// pub use excel_export::{
//     create_writer, Spreadsheet, Writer, Worksheet, CellValue, CellType,
// };
// pub use excel_import::{identify, Reader, Workbook, Worksheet as ImportWorksheet, CellData, ExcelType};
// pub use pdf_form::Pdf;

// ============================================================================
// 错误类型
// ============================================================================

/// Excel/PDF 处理错误 — 对齐 PHP phpspreadsheet / pdftk 异常
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    /// Excel 写入错误（对齐 PHP `PhpOffice\PhpSpreadsheet\Writer\Exception`）
    #[error("Excel writer error: {0}")]
    ExcelWrite(String),

    /// Excel 读取错误（对齐 PHP `PhpOffice\PhpSpreadsheet\Reader\Exception`）
    #[error("Excel reader error: {0}")]
    ExcelRead(String),

    /// PDF 处理错误（对齐 PHP `mikehaertl\php-pdftk\Pdf::getError()`）
    #[error("PDF error: {0}")]
    Pdf(String),

    /// A1 引用解析错误（对齐 PHP `Cell\Coordinate::coordinateFromString` 异常）
    #[error("Invalid cell reference: {0}")]
    InvalidCellRef(String),

    /// IO 错误
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// rust_xlsxwriter 错误
    #[error(transparent)]
    Xlsx(#[from] rust_xlsxwriter::XlsxError),

    /// calamine 错误
    #[error(transparent)]
    Calamine(#[from] calamine::Error),

    /// lopdf 错误
    #[error(transparent)]
    Lopdf(#[from] lopdf::Error),

    /// HTTP 请求错误（对齐 PHP cURL 错误）
    #[error("HTTP error: {0}")]
    Http(String),
}

// ============================================================================
// Addon 接线：PdfState + register_routes
// ============================================================================

use axum::body::Body;
use axum::extract::Json as ExtractJson;
use axum::http::header;
use axum::response::{Json, Response};
use serde::Deserialize;
use serde_json::json;
use sz_rust_core::router::RouterBuilder;

/// PDF addon 状态
#[derive(Clone)]
pub struct PdfState {
    pub modules: Vec<&'static str>,
    pub version: &'static str,
}

impl Default for PdfState {
    fn default() -> Self {
        Self {
            modules: vec!["csv_export", "excel_export", "excel_import", "pdf_form"],
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// CSV 导出请求体
#[derive(Debug, Deserialize)]
pub struct CsvExportRequest {
    pub filename: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// 注册 PDF addon 路由到 sz300 RouterBuilder
pub fn register_routes<S>(builder: RouterBuilder<S>, state: PdfState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let builder = builder.post("/api/pdf/export/csv", {
        move |ExtractJson(req): ExtractJson<CsvExportRequest>| async move {
            let bytes =
                csv_export::export_csv_to_bytes(&req.headers, &req.rows).unwrap_or_default();
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "filename": req.filename,
                    "format": "csv",
                    "size": bytes.len(),
                    "content_base64": base64_encode(&bytes)
                }
            }))
        }
    });

    let builder = builder.post("/api/pdf/export/csv/download", {
        move |ExtractJson(req): ExtractJson<CsvExportRequest>| async move {
            let bytes =
                csv_export::export_csv_to_bytes(&req.headers, &req.rows).unwrap_or_default();
            let mut resp = Response::new(Body::from(bytes));
            // 静态字面量与受控 filename，解析失败时回退安全默认值（铁律 2：禁 unwrap）
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                "text/csv; charset=utf-8".parse().unwrap_or_else(|_| {
                    header::HeaderValue::from_static("application/octet-stream")
                }),
            );
            resp.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", req.filename)
                    .parse()
                    .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
            );
            resp
        }
    });

    builder.get("/api/pdf/health", {
        let s = state;
        move || async move {
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "plugin": "pdf",
                    "status": "active",
                    "modules": s.modules,
                    "version": s.version
                }
            }))
        }
    })
}

pub mod capability;
pub use capability::PdfPlugin;
