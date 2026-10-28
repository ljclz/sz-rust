// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CSV 导出模块 — 对齐 PHP `export_excel` / `exportCsv` / `fputcsv`
//!
//! ## PHP 对齐
//!
//! 本模块以 PHP 项目实际使用的 CSV 导出 API 子集为对齐基准：
//!
//! - `export_excel($fileName, $tileArray, $dataArray)` → [`export_csv_to_writer`] / [`export_csv_to_file`] / [`export_csv_to_bytes`]
//! - `exportCsv($filename, $data)` → [`export_csv_no_bom_to_writer`] / [`export_csv_no_bom_to_bytes`]
//! - `fputcsv($fp, $fields)` → [`write_csv_row`]
//!
//! ## PHP 源码参考
//!
//! `e:\vue\test\鲜视达\server\app\common.php`：
//!
//! ```php
//! // 第 924 行：export_excel（带 UTF-8 BOM + 表头行 + 1000 行分批 flush）
//! function export_excel($fileName, $tileArray = [], $dataArray = [])
//! {
//!     ini_set('memory_limit', '512M');
//!     ini_set('max_execution_time', 0);
//!     ob_end_clean();
//!     ob_start();
//!     header("Content-Type: text/csv");
//!     header("Content-Disposition:filename=" . $fileName);
//!     $fp = fopen('php://output', 'w');
//!     fwrite($fp, chr(0xEF) . chr(0xBB) . chr(0xBF)); // UTF-8 BOM
//!     fputcsv($fp, $tileArray);
//!     $index = 0;
//!     foreach ($dataArray as $item) {
//!         if ($index == 1000) {
//!             $index = 0;
//!             ob_flush();
//!             flush();
//!         }
//!         $index++;
//!         fputcsv($fp, $item);
//!     }
//!     ob_flush();
//!     flush();
//!     ob_end_clean();
//! }
//!
//! // 第 958 行：exportCsv（不带 BOM，直接 fputcsv）
//! function exportCsv($filename, array $data)
//! {
//!     header("Cache-Control: public");
//!     header("Pragma: public");
//!     header("Content-Type: application/vnd.ms-excel");
//!     header("Content-Disposition: attachment; filename={$filename}.csv");
//!     $handle = fopen("php://output", "w");
//!     foreach ($data as $v) {
//!         if (is_array($v)) {
//!             fputcsv($handle, $v);
//!         }
//!     }
//!     exit;
//! }
//! ```
//!
//! ## fputcsv 行为说明
//!
//! PHP `fputcsv` 默认行为（PHP 8.1+）：
//! - 分隔符：`,`（逗号）
//! - 包裹符：`"`（双引号）
//! - 转义符：`""`（双引号加倍，RFC 4180 兼容）
//! - 行尾：`\n`
//!
//! 字段需要包裹的条件：包含分隔符、包裹符、`\n`、`\r` 或 ASCII < 32 的字符。
//!
//! **注**：PHP 8.0 默认转义符为 `\`（反斜杠），与 RFC 4180 不兼容。
//! 本模块采用 RFC 4180 行为（PHP 8.1+ 默认），项目 `composer.json` 要求 `php >= 8.0`，
//! 实际运行环境为 PHP 8.1+，行为一致。
//!
//! ## R5 硬约束
//!
//! - R5-42：`export_csv_to_*` 系列 — UTF-8 BOM + fputcsv 行为对齐 PHP `export_excel`
//! - R5-43：`export_csv_no_bom_to_*` 系列 — 不带 BOM 的 fputcsv 行为对齐 PHP `exportCsv`

use std::io::Write;
use std::path::Path;

use crate::PdfError;

// ============================================================================
// 常量 — 对齐 PHP 导出行为
// ============================================================================

/// UTF-8 BOM 字节序列 — 对齐 PHP `chr(0xEF) . chr(0xBB) . chr(0xBF)`
///
/// PHP `export_excel` 在 CSV 内容前写入 BOM，防止 Excel 打开时中文乱码
/// （如微信昵称等特殊字符）。
pub const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// CSV 分隔符 — 对齐 PHP `fputcsv` 默认 delimiter `,`
const CSV_DELIMITER: char = ',';

/// CSV 包裹符 — 对齐 PHP `fputcsv` 默认 enclosure `"`
const CSV_ENCLOSURE: char = '"';

/// CSV 行尾 — 对齐 PHP `fputcsv` 默认 eol `\n`
const CSV_EOL: &str = "\n";

/// `export_excel` 的 HTTP Content-Type — 对齐 PHP `header("Content-Type: text/csv")`
pub const HTTP_CONTENT_TYPE_CSV: &str = "text/csv";

/// `exportCsv` 的 HTTP Content-Type — 对齐 PHP `header("Content-Type: application/vnd.ms-excel")`
pub const HTTP_CONTENT_TYPE_DOWNLOAD: &str = "application/vnd.ms-excel";

// ============================================================================
// write_csv_row — 对齐 PHP `fputcsv`
// ============================================================================

/// 写入单行 CSV — 对齐 PHP `fputcsv($fp, $fields)`
///
/// # PHP 行为（PHP 8.1+ 默认）
///
/// 1. 对每个字段判断是否需要包裹：
///    - 包含分隔符 `,`
///    - 包含包裹符 `"`
///    - 包含换行符 `\n` 或 `\r`
///    - 包含 ASCII < 32 的控制字符
/// 2. 需要包裹的字段：用 `"` 包裹，内部 `"` 加倍为 `""`
/// 3. 不需要包裹的字段：原样写入
/// 4. 字段间用 `,` 连接
/// 5. 行尾追加 `\n`
///
/// # R5-42 / R5-43 硬约束
///
/// 此函数是 CSV 导出的基础，被 [`export_csv_to_writer`] 和 [`export_csv_no_bom_to_writer`] 调用。
///
/// # 示例
///
/// ```
/// use sz_rust_pdf::csv_export::write_csv_row;
///
/// let mut buf = Vec::new();
/// write_csv_row(&mut buf, &["hello".to_string(), "world".to_string()]).unwrap();
/// assert_eq!(buf, b"hello,world\n");
/// ```
pub fn write_csv_row<W: Write>(writer: &mut W, fields: &[String]) -> Result<(), PdfError> {
    let mut row = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            row.push(CSV_DELIMITER);
        }
        if needs_enclosure(field) {
            row.push(CSV_ENCLOSURE);
            for ch in field.chars() {
                if ch == CSV_ENCLOSURE {
                    // RFC 4180：双引号加倍
                    row.push(CSV_ENCLOSURE);
                    row.push(CSV_ENCLOSURE);
                } else {
                    row.push(ch);
                }
            }
            row.push(CSV_ENCLOSURE);
        } else {
            row.push_str(field);
        }
    }
    row.push_str(CSV_EOL);
    writer.write_all(row.as_bytes())?;
    Ok(())
}

/// 判断字段是否需要包裹 — 对齐 PHP `fputcsv` 内部逻辑
fn needs_enclosure(field: &str) -> bool {
    field.chars().any(|ch| {
        ch == CSV_DELIMITER || ch == CSV_ENCLOSURE || ch == '\n' || ch == '\r' || (ch as u32) < 32
    })
}

// ============================================================================
// export_csv_to_* — 对齐 PHP `export_excel`（带 UTF-8 BOM）
// ============================================================================

/// 将 CSV 数据写入 writer（带 UTF-8 BOM）— 对齐 PHP `export_excel`
///
/// # PHP 行为
///
/// 1. 写入 UTF-8 BOM（`0xEF 0xBB 0xBF`）
/// 2. 写入表头行 `tile_array`（通过 `fputcsv`）
/// 3. 逐行写入数据 `data`（通过 `fputcsv`）
///
/// **注**：PHP 每 1000 行执行 `ob_flush() + flush()`，用于 HTTP 流式输出。
/// Rust 端写入到 `writer`，由调用方决定 flush 策略（通常不需要手动 flush）。
///
/// # R5-42 硬约束
///
/// - UTF-8 BOM 必须写入
/// - 表头行和数据行通过 `fputcsv` 格式化
///
/// # 示例
///
/// ```
/// use sz_rust_pdf::csv_export::export_csv_to_writer;
///
/// let mut buf = Vec::new();
/// let tile = vec!["订单号".to_string(), "金额".to_string()];
/// let data = vec![
///     vec!["ORD001".to_string(), "100.50".to_string()],
///     vec!["ORD002".to_string(), "200.00".to_string()],
/// ];
/// export_csv_to_writer(&mut buf, &tile, &data).unwrap();
/// // buf 开头是 UTF-8 BOM
/// assert_eq!(&buf[..3], &[0xEF, 0xBB, 0xBF]);
/// ```
pub fn export_csv_to_writer<W: Write>(
    writer: &mut W,
    tile_array: &[String],
    data: &[Vec<String>],
) -> Result<(), PdfError> {
    // 1. 写入 UTF-8 BOM
    writer.write_all(&UTF8_BOM)?;

    // 2. 写入表头行（如果非空）
    if !tile_array.is_empty() {
        write_csv_row(writer, tile_array)?;
    }

    // 3. 逐行写入数据
    for row in data {
        write_csv_row(writer, row)?;
    }

    Ok(())
}

/// 将 CSV 数据写入文件（带 UTF-8 BOM）— 对齐 PHP `export_excel`
///
/// 便捷函数，等价于 `export_csv_to_writer(&mut File::create(path)?, tile_array, data)`。
///
/// # R5-42 硬约束
pub fn export_csv_to_file<P: AsRef<Path>>(
    path: P,
    tile_array: &[String],
    data: &[Vec<String>],
) -> Result<(), PdfError> {
    let mut file = std::fs::File::create(path)?;
    export_csv_to_writer(&mut file, tile_array, data)
}

/// 将 CSV 数据写入字节缓冲（带 UTF-8 BOM）— 对齐 PHP `export_excel`
///
/// 便捷函数，返回 `Vec<u8>`，由上层 web 框架设置 HTTP 头并返回响应体。
///
/// # R5-42 硬约束
pub fn export_csv_to_bytes(
    tile_array: &[String],
    data: &[Vec<String>],
) -> Result<Vec<u8>, PdfError> {
    let mut buf = Vec::new();
    export_csv_to_writer(&mut buf, tile_array, data)?;
    Ok(buf)
}

// ============================================================================
// export_csv_no_bom_to_* — 对齐 PHP `exportCsv`（不带 BOM）
// ============================================================================

/// 将 CSV 数据写入 writer（不带 BOM）— 对齐 PHP `exportCsv`
///
/// # PHP 行为
///
/// 1. 不写入 BOM
/// 2. 逐行写入数据 `data`（通过 `fputcsv`），跳过非数组项
///
/// **注**：PHP `exportCsv` 不写入表头行，数据的第一行即为表头（由调用方组装）。
///
/// # R5-43 硬约束
pub fn export_csv_no_bom_to_writer<W: Write>(
    writer: &mut W,
    data: &[Vec<String>],
) -> Result<(), PdfError> {
    for row in data {
        write_csv_row(writer, row)?;
    }
    Ok(())
}

/// 将 CSV 数据写入字节缓冲（不带 BOM）— 对齐 PHP `exportCsv`
///
/// # R5-43 硬约束
pub fn export_csv_no_bom_to_bytes(data: &[Vec<String>]) -> Result<Vec<u8>, PdfError> {
    let mut buf = Vec::new();
    export_csv_no_bom_to_writer(&mut buf, data)?;
    Ok(buf)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // needs_enclosure 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_needs_enclosure_plain_text() {
        assert!(!needs_enclosure("hello"));
        assert!(!needs_enclosure("订单001"));
        assert!(!needs_enclosure("100.50"));
    }

    #[test]
    fn test_needs_enclosure_with_comma() {
        assert!(needs_enclosure("hello,world"));
        assert!(needs_enclosure("太平店,南山区"));
    }

    #[test]
    fn test_needs_enclosure_with_quote() {
        assert!(needs_enclosure("hello\"world"));
        assert!(needs_enclosure("\"quoted\""));
    }

    #[test]
    fn test_needs_enclosure_with_newline() {
        assert!(needs_enclosure("hello\nworld"));
        assert!(needs_enclosure("hello\rworld"));
        assert!(needs_enclosure("hello\r\nworld"));
    }

    #[test]
    fn test_needs_enclosure_with_control_char() {
        assert!(needs_enclosure("hello\tworld")); // tab = ASCII 9
        assert!(needs_enclosure("hello\0world")); // null = ASCII 0
    }

    #[test]
    fn test_needs_enclosure_empty() {
        assert!(!needs_enclosure(""));
    }

    // ------------------------------------------------------------------------
    // write_csv_row 测试（对齐 PHP fputcsv）
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_42_write_csv_row_basic() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["hello".to_string(), "world".to_string()]).unwrap();
        assert_eq!(buf, b"hello,world\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_single_field() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["only".to_string()]).unwrap();
        assert_eq!(buf, b"only\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_empty_fields() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["".to_string(), "".to_string()]).unwrap();
        // 空字段不需要包裹，输出 `,\n`
        assert_eq!(buf, b",\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_with_comma() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["hello,world".to_string(), "ok".to_string()]).unwrap();
        assert_eq!(buf, b"\"hello,world\",ok\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_with_quote() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["hello\"world".to_string()]).unwrap();
        // RFC 4180：双引号加倍
        assert_eq!(buf, b"\"hello\"\"world\"\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_with_newline() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["line1\nline2".to_string()]).unwrap();
        assert_eq!(buf, b"\"line1\nline2\"\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_chinese() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["订单号".to_string(), "金额".to_string()]).unwrap();
        assert_eq!(buf, "订单号,金额\n".as_bytes());
    }

    #[test]
    fn test_r5_42_write_csv_row_empty_row() {
        let mut buf = Vec::new();
        let fields: Vec<String> = vec![];
        write_csv_row(&mut buf, &fields).unwrap();
        // 空行只有行尾 `\n`
        assert_eq!(buf, b"\n");
    }

    #[test]
    fn test_r5_42_write_csv_row_multiple_quotes() {
        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["\"a\"\"b\"".to_string()]).unwrap();
        // 字段值 "a""b" → 包裹后 """a""""b"""（每个 " 加倍）
        assert_eq!(buf, b"\"\"\"a\"\"\"\"b\"\"\"\n");
    }

    // ------------------------------------------------------------------------
    // export_csv_to_* 测试（对齐 PHP export_excel，带 BOM）
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_42_export_csv_to_bytes_with_bom() {
        let tile = vec!["订单号".to_string(), "金额".to_string()];
        let data = vec![
            vec!["ORD001".to_string(), "100.50".to_string()],
            vec!["ORD002".to_string(), "200.00".to_string()],
        ];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        // 开头是 UTF-8 BOM
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);

        // BOM 后是表头行 + 数据行
        let content = String::from_utf8_lossy(&bytes[3..]);
        assert_eq!(content, "订单号,金额\nORD001,100.50\nORD002,200.00\n");
    }

    #[test]
    fn test_r5_42_export_csv_to_bytes_empty_data() {
        let tile = vec!["A".to_string(), "B".to_string()];
        let data: Vec<Vec<String>> = vec![];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let content = String::from_utf8_lossy(&bytes[3..]);
        assert_eq!(content, "A,B\n");
    }

    #[test]
    fn test_r5_42_export_csv_to_bytes_empty_tile() {
        let tile: Vec<String> = vec![];
        let data = vec![vec!["row1".to_string()]];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let content = String::from_utf8_lossy(&bytes[3..]);
        assert_eq!(content, "row1\n");
    }

    #[test]
    fn test_r5_42_export_csv_to_file() {
        let tile = vec!["A".to_string(), "B".to_string()];
        let data = vec![vec!["1".to_string(), "2".to_string()]];

        let tmp = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        export_csv_to_file(tmp.path(), &tile, &data).unwrap();

        let bytes = std::fs::read(tmp.path()).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let content = String::from_utf8_lossy(&bytes[3..]);
        assert_eq!(content, "A,B\n1,2\n");
    }

    #[test]
    fn test_r5_42_export_csv_to_writer_special_chars() {
        let tile = vec!["名称".to_string(), "备注".to_string()];
        let data = vec![
            vec!["太平店,南山区".to_string(), "正常".to_string()],
            vec!["测试\"引用".to_string(), "换行\n内容".to_string()],
        ];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        let content = String::from_utf8_lossy(&bytes[3..]);
        assert_eq!(
            content,
            "名称,备注\n\"太平店,南山区\",正常\n\"测试\"\"引用\",\"换行\n内容\"\n"
        );
    }

    // ------------------------------------------------------------------------
    // export_csv_no_bom_to_* 测试（对齐 PHP exportCsv，不带 BOM）
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_43_export_csv_no_bom_to_bytes() {
        let data = vec![
            vec!["订单号".to_string(), "金额".to_string()],
            vec!["ORD001".to_string(), "100.50".to_string()],
        ];
        let bytes = export_csv_no_bom_to_bytes(&data).unwrap();

        // 不带 BOM
        assert_ne!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);

        let content = String::from_utf8_lossy(&bytes);
        assert_eq!(content, "订单号,金额\nORD001,100.50\n");
    }

    #[test]
    fn test_r5_43_export_csv_no_bom_empty_data() {
        let data: Vec<Vec<String>> = vec![];
        let bytes = export_csv_no_bom_to_bytes(&data).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_r5_43_export_csv_no_bom_to_writer() {
        let data = vec![vec!["A".to_string(), "B".to_string()]];
        let mut buf = Vec::new();
        export_csv_no_bom_to_writer(&mut buf, &data).unwrap();
        assert_eq!(buf, b"A,B\n");
    }

    // ------------------------------------------------------------------------
    // R5 对比测试：PHP vs Rust 行为对比
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_42_php_export_excel_comparison() {
        // 对齐 PHP export_excel('test.csv', ['A', 'B'], [['1', '2'], ['3', '4']])
        // PHP 输出（去掉 HTTP header 后）：
        //   \xEF\xBB\xBF (BOM)
        //   A,B\n
        //   1,2\n
        //   3,4\n

        let tile = vec!["A".to_string(), "B".to_string()];
        let data = vec![
            vec!["1".to_string(), "2".to_string()],
            vec!["3".to_string(), "4".to_string()],
        ];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        let expected: Vec<u8> = vec![
            0xEF, 0xBB, 0xBF, // BOM
            b'A', b',', b'B', b'\n', // 表头
            b'1', b',', b'2', b'\n', // 数据行 1
            b'3', b',', b'4', b'\n', // 数据行 2
        ];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_r5_43_php_exportcsv_comparison() {
        // 对齐 PHP exportCsv('test', [['A', 'B'], ['1', '2']])
        // PHP 输出（去掉 HTTP header 后）：
        //   A,B\n
        //   1,2\n

        let data = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let bytes = export_csv_no_bom_to_bytes(&data).unwrap();

        let expected: Vec<u8> = vec![b'A', b',', b'B', b'\n', b'1', b',', b'2', b'\n'];
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_r5_42_php_fputcsv_quote_escaping() {
        // 对齐 PHP fputcsv 对包含双引号字段的处理
        // PHP 8.1+: field "hello" → "hello"（包裹 + 双引号加倍）
        // 输入：hello"world
        // 输出："hello""world"\n

        let mut buf = Vec::new();
        write_csv_row(&mut buf, &["hello\"world".to_string()]).unwrap();
        assert_eq!(buf, b"\"hello\"\"world\"\n");
    }

    #[test]
    fn test_r5_42_wechat_nickname_bom_purpose() {
        // 对齐 PHP export_excel 注释："转码 防止乱码(比如微信昵称)"
        // 验证 BOM 存在，确保 Excel 正确识别 UTF-8 编码
        let tile = vec!["昵称".to_string()];
        let data = vec![vec!["微信用户🌙".to_string()]];
        let bytes = export_csv_to_bytes(&tile, &data).unwrap();

        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let content = String::from_utf8_lossy(&bytes[3..]);
        assert!(content.contains("微信用户🌙"));
    }

    // ------------------------------------------------------------------------
    // HTTP 常量测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_http_content_type_constants() {
        // 对齐 PHP export_excel 的 Content-Type
        assert_eq!(HTTP_CONTENT_TYPE_CSV, "text/csv");
        // 对齐 PHP exportCsv 的 Content-Type
        assert_eq!(HTTP_CONTENT_TYPE_DOWNLOAD, "application/vnd.ms-excel");
    }

    #[test]
    fn test_utf8_bom_constant() {
        // 对齐 PHP chr(0xEF) . chr(0xBB) . chr(0xBF)
        assert_eq!(UTF8_BOM, [0xEF, 0xBB, 0xBF]);
    }
}
