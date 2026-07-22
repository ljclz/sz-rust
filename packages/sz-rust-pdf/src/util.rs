//! 辅助函数模块 — 对齐 PHP `moneyToArray` / `data_path` / `date('YmdHis')`
//!
//! ## PHP 对齐
//!
//! 本模块以 PHP 项目实际使用的辅助函数为对齐基准：
//!
//! - `moneyToArray($num)` → [`money_to_array`]
//! - `data_path($path)` → [`data_path`]
//! - `date('YmdHis')` 文件命名 → [`filename_with_timestamp`]
//!
//! ## PHP 源码参考
//!
//! `e:\vue\test\鲜视达\server\app\common.php`：
//!
//! ```php
//! // 第 38 行：data_path（数据目录路径）
//! function data_path($path = ''): string
//! {
//!     return '/www/wwwroot/sz-api.ljclz.com/' . ($path ? $path . DIRECTORY_SEPARATOR : $path);
//! }
//!
//! // 第 1814 行：moneyToArray（金额转大写汉字数组）
//! function moneyToArray($num){
//!     $arr = [];
//!     $digits = ['零', '壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];
//!     $num_arr = explode('.', $num);
//!     $int_str = $num_arr[0] ?? '';
//!     $float_str = $num_arr[1] ?? '';
//!     // 处理小数部分（固定键0,1）
//!     $float_str = substr($float_str, 0, 2); // 最多取2位小数
//!     for ($i = 0; $i < 2; $i++) {
//!         $d = $float_str[$i] ?? '0'; // 不足补零
//!         $arr[$i] = $digits[$d];
//!     }
//!     // 处理整数部分（键从2开始）
//!     $int_str = strrev($int_str); // 反转字符串（从低位到高位）
//!     for ($i = 0; $i < strlen($int_str); $i++) {
//!         $d = $int_str[$i];
//!         $arr[$i + 2] = $digits[$d]; // 键从2开始递增
//!     }
//!     return $arr;
//! }
//! ```
//!
//! ## R5 硬约束
//!
//! - R5-45：`money_to_array` 金额按位拆分为大写汉字数组对齐 PHP `moneyToArray`
//!   （小数 2 位键 0-1，整数部分键 2+，低位在前）
//! - R5-46：`data_path` 数据目录路径对齐 PHP `data_path` 全局函数
//! - R5-47：`filename_with_timestamp` 带时间戳文件名对齐 PHP `date('YmdHis')` 命名规则

use chrono::{FixedOffset, Utc};

use crate::PdfError;

// ============================================================================
// 常量 — 对齐 PHP 全局常量
// ============================================================================

/// 数据目录基础路径 — 对齐 PHP `data_path` 函数硬编码的路径前缀
///
/// PHP 源码（`common.php` 第 38 行）：
/// ```php
/// function data_path($path = ''): string
/// {
///     return '/www/wwwroot/sz-api.ljclz.com/' . ($path ? $path . DIRECTORY_SEPARATOR : $path);
/// }
/// ```
///
/// 注意：基础路径末尾自带 `/`，对齐 PHP 字符串拼接行为。
pub const DATA_PATH_BASE: &str = "/www/wwwroot/sz-api.ljclz.com/";

/// 目录分隔符 — 对齐 PHP `DIRECTORY_SEPARATOR`（Linux 服务器为 `/`）
///
/// PHP 服务器部署在 Linux 环境，`DIRECTORY_SEPARATOR` 常量值为 `/`。
/// Rust 端硬编码 `/` 以对齐 Linux 服务器行为。
pub const DIRECTORY_SEPARATOR: &str = "/";

/// 大写汉字数字表 — 对齐 PHP `moneyToArray` 中的 `$digits` 数组
///
/// PHP 源码：`$digits = ['零', '壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];`
///
/// 索引 0-9 对应阿拉伯数字 0-9。
const DIGITS: [&str; 10] = ["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

/// Asia/Shanghai 时区偏移（UTC+8）— 对齐 PHP 服务器时区
///
/// PHP `date('YmdHis')` 默认使用 `date.timezone` 配置，中国服务器通常配置为 `Asia/Shanghai`（UTC+8）。
/// Rust 端使用 `FixedOffset` 创建 UTC+8 偏移，避免依赖 `chrono-tz` 包。
const SHANGHAI_OFFSET_SECS: i32 = 8 * 3600;

// ============================================================================
// money_to_array — 对齐 PHP `moneyToArray`
// ============================================================================

/// 金额转大写汉字数组 — 对齐 PHP `moneyToArray($num)` 全局函数
///
/// ## PHP 行为
///
/// PHP `moneyToArray` 将金额字符串按位拆分为大写汉字数组：
///
/// 1. 按 `.` 分割为整数部分和小数部分
/// 2. 小数部分：最多取 2 位，键 0（角位）和键 1（分位），不足补零
/// 3. 整数部分：反转后从低位到高位遍历，键从 2 开始递增
///
/// 数组结构（以 `"123.45"` 为例）：
///
/// | 键 | 含义 | 值 |
/// |----|------|----|
/// | 0  | 小数第 1 位（角） | `'肆'` |
/// | 1  | 小数第 2 位（分） | `'伍'` |
/// | 2  | 整数个位          | `'叁'` |
/// | 3  | 整数十位          | `'贰'` |
/// | 4  | 整数百位          | `'壹'` |
///
/// ## R5-45 硬约束
///
/// - 小数 2 位键 0-1，整数部分键 2+，低位在前
/// - 大写汉字：零壹贰叁肆伍陆柒捌玖
///
/// # 参数
///
/// - `num`：金额字符串（如 `"123.45"`、`"100"`、`"0.05"`）
///
/// # 返回
///
/// 大写汉字数组，索引对应 PHP 数组键。
///
/// # 错误
///
/// - 输入包含非数字字符（除小数点）→ [`PdfError::Http`]
/// - 小数点超过 1 个 → [`PdfError::Http`]
///
/// # 示例
///
/// ```
/// use sz_rust_pdf::util::money_to_array;
///
/// // 123.45 → ['肆', '伍', '叁', '贰', '壹']
/// let arr = money_to_array("123.45").unwrap();
/// assert_eq!(arr[0], "肆"); // 小数第 1 位
/// assert_eq!(arr[1], "伍"); // 小数第 2 位
/// assert_eq!(arr[2], "叁"); // 整数个位
/// assert_eq!(arr[3], "贰"); // 整数十位
/// assert_eq!(arr[4], "壹"); // 整数百位
///
/// // 100 → ['零', '零', '零', '零', '壹']
/// let arr = money_to_array("100").unwrap();
/// assert_eq!(arr, vec!["零", "零", "零", "零", "壹"]);
/// ```
pub fn money_to_array(num: &str) -> Result<Vec<String>, PdfError> {
    // 对齐 PHP：$num_arr = explode('.', $num);
    let parts: Vec<&str> = num.split('.').collect();

    // PHP explode('.', $num) 永远返回至少 1 个元素，不报错
    // 但 Rust split('.') 对空字符串返回 [""]，对 "123" 返回 ["123"]
    // 对齐 PHP：$int_str = $num_arr[0] ?? '';
    let int_str = parts.first().copied().unwrap_or("");
    // 对齐 PHP：$float_str = $num_arr[1] ?? '';
    let float_str = parts.get(1).copied().unwrap_or("");

    // 验证：小数点超过 1 个 → 错误（PHP explode 会产生 3+ 段，但 PHP 后续只取 [1]）
    // 实际 PHP 不会报错，但行为不明确。Rust 端选择报错以避免歧义。
    if parts.len() > 2 {
        return Err(PdfError::Http(format!(
            "Invalid money format (multiple decimal points): {}",
            num
        )));
    }

    // 验证：整数和小数部分只允许数字字符或空字符串（对齐 PHP ".05" 场景）
    // PHP `$digits[$d]` 对非数字字符产生 warning + null，Rust 端选择报错
    if int_str.chars().any(|c| !c.is_ascii_digit()) {
        return Err(PdfError::Http(format!(
            "Invalid money format (non-digit in integer part): {}",
            num
        )));
    }
    if float_str.chars().any(|c| !c.is_ascii_digit()) {
        return Err(PdfError::Http(format!(
            "Invalid money format (non-digit in decimal part): {}",
            num
        )));
    }

    let mut arr: Vec<String> = Vec::new();

    // 对齐 PHP：$float_str = substr($float_str, 0, 2); // 最多取 2 位小数
    let float_chars: Vec<char> = float_str.chars().take(2).collect();

    // 对齐 PHP：for ($i = 0; $i < 2; $i++) { $d = $float_str[$i] ?? '0'; $arr[$i] = $digits[$d]; }
    for i in 0..2 {
        let d = float_chars.get(i).copied().unwrap_or('0');
        let digit_index = (d as usize) - ('0' as usize);
        arr.push(DIGITS[digit_index].to_string());
    }

    // 对齐 PHP：$int_str = strrev($int_str); // 反转字符串（从低位到高位）
    let int_reversed: String = int_str.chars().rev().collect();

    // 对齐 PHP：for ($i = 0; $i < strlen($int_str); $i++) { $d = $int_str[$i]; $arr[$i + 2] = $digits[$d]; }
    // 注：Rust 端 arr.push 自动递增索引，无需显式使用 $i + 2
    for d in int_reversed.chars() {
        let digit_index = (d as usize) - ('0' as usize);
        arr.push(DIGITS[digit_index].to_string());
    }

    Ok(arr)
}

// ============================================================================
// data_path — 对齐 PHP `data_path`
// ============================================================================

/// 数据目录路径 — 对齐 PHP `data_path($path)` 全局函数
///
/// ## PHP 行为
///
/// PHP `data_path` 拼接固定基础路径和可选子路径：
///
/// ```php
/// function data_path($path = ''): string
/// {
///     return '/www/wwwroot/sz-api.ljclz.com/' . ($path ? $path . DIRECTORY_SEPARATOR : $path);
/// }
/// ```
///
/// - 基础路径末尾自带 `/`
/// - 如果 `path` 非空：追加 `path` + `DIRECTORY_SEPARATOR`（Linux 下为 `/`）
/// - 如果 `path` 为空：追加空字符串
///
/// ## R5-46 硬约束
///
/// - 基础路径：`/www/wwwroot/sz-api.ljclz.com/`
/// - 目录分隔符：`/`（Linux `DIRECTORY_SEPARATOR`）
///
/// # 参数
///
/// - `path`：子路径（如 `"data"`、`"data/sign"`、`""`）
///
/// # 返回
///
/// 拼接后的完整路径。
///
/// # 示例
///
/// ```
/// use sz_rust_pdf::util::data_path;
///
/// // 空路径 → 基础路径
/// assert_eq!(data_path(""), "/www/wwwroot/sz-api.ljclz.com/");
///
/// // 非空路径 → 基础路径 + path + /
/// assert_eq!(data_path("data"), "/www/wwwroot/sz-api.ljclz.com/data/");
/// assert_eq!(data_path("data/sign"), "/www/wwwroot/sz-api.ljclz.com/data/sign/");
/// ```
pub fn data_path(path: &str) -> String {
    // 对齐 PHP：return '/www/wwwroot/sz-api.ljclz.com/' . ($path ? $path . DIRECTORY_SEPARATOR : $path);
    if path.is_empty() {
        DATA_PATH_BASE.to_string()
    } else {
        format!("{}{}{}", DATA_PATH_BASE, path, DIRECTORY_SEPARATOR)
    }
}

// ============================================================================
// filename_with_timestamp — 对齐 PHP `date('YmdHis')` 文件命名规则
// ============================================================================

/// 带时间戳的文件名 — 对齐 PHP `date('YmdHis')` 命名规则
///
/// ## PHP 行为
///
/// PHP 业务代码常用 `date('YmdHis')` 生成时间戳文件名：
///
/// ```php
/// $filename = '订单导出_' . date('YmdHis') . '.csv';
/// // 结果：订单导出_20260722120000.csv
/// ```
///
/// PHP `date('YmdHis')` 格式：
/// - `Y`：4 位年（如 2026）
/// - `m`：2 位月（01-12）
/// - `d`：2 位日（01-31）
/// - `H`：2 位时（00-23）
/// - `i`：2 位分（00-59）
/// - `s`：2 位秒（00-59）
///
/// 时区：Asia/Shanghai（UTC+8），对齐 PHP 服务器 `date.timezone` 配置。
///
/// ## R5-47 硬约束
///
/// - 时间戳格式：`YmdHis`（如 `20260722120000`）
/// - 时区：Asia/Shanghai（UTC+8）
/// - 文件名格式：`{prefix}_{timestamp}.{ext}`
///
/// # 参数
///
/// - `prefix`：文件名前缀（如 `"订单导出"`、`"payment"`）
/// - `ext`：扩展名（如 `"csv"`、`"pdf"`，不含 `.`）
///
/// # 返回
///
/// 带时间戳的文件名，格式为 `{prefix}_{timestamp}.{ext}`。
///
/// # 示例
///
/// ```
/// use sz_rust_pdf::util::filename_with_timestamp;
///
/// let filename = filename_with_timestamp("订单导出", "csv");
/// // 结果：订单导出_20260722120000.csv（时间戳为当前 Asia/Shanghai 时间）
/// assert!(filename.starts_with("订单导出_"));
/// assert!(filename.ends_with(".csv"));
/// ```
pub fn filename_with_timestamp(prefix: &str, ext: &str) -> String {
    // 对齐 PHP：Asia/Shanghai 时区（UTC+8）
    let offset = FixedOffset::east_opt(SHANGHAI_OFFSET_SECS).expect("UTC+8 offset is always valid");
    let now = Utc::now().with_timezone(&offset);

    // 对齐 PHP：date('YmdHis') → chrono 格式 %Y%m%d%H%M%S
    let timestamp = now.format("%Y%m%d%H%M%S").to_string();

    // 对齐 PHP：$filename = 'prefix_' . date('YmdHis') . '.ext';
    format!("{}_{}.{}", prefix, timestamp, ext)
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // money_to_array 测试 — R5-45 硬约束
    // ------------------------------------------------------------------------

    #[test]
    fn test_money_to_array_basic_with_decimal() {
        // PHP moneyToArray("123.45") → ['肆', '伍', '叁', '贰', '壹']
        let arr = money_to_array("123.45").unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], "肆"); // 小数第 1 位（角）
        assert_eq!(arr[1], "伍"); // 小数第 2 位（分）
        assert_eq!(arr[2], "叁"); // 整数个位
        assert_eq!(arr[3], "贰"); // 整数十位
        assert_eq!(arr[4], "壹"); // 整数百位
    }

    #[test]
    fn test_money_to_array_integer() {
        // PHP moneyToArray("100") → ['零', '零', '零', '零', '壹']
        // explode('.', "100") → ["100"]
        // int_str = "100", float_str = ""
        // float_str = substr("", 0, 2) = ""
        // i=0: d=""[0] ?? '0' = '0' → '零'
        // i=1: d=""[1] ?? '0' = '0' → '零'
        // int_str = strrev("100") = "001"
        // i=0: d='0' → '零' (arr[2])
        // i=1: d='0' → '零' (arr[3])
        // i=2: d='1' → '壹' (arr[4])
        let arr = money_to_array("100").unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], "零");
        assert_eq!(arr[1], "零");
        assert_eq!(arr[2], "零");
        assert_eq!(arr[3], "零");
        assert_eq!(arr[4], "壹");
    }

    #[test]
    fn test_money_to_array_one_decimal_digit() {
        // PHP moneyToArray("123.4") → ['肆', '零', '叁', '贰', '壹']
        // float_str = substr("4", 0, 2) = "4"
        // i=0: d='4' → '肆'
        // i=1: d='4'[1] ?? '0' = '0' → '零'
        let arr = money_to_array("123.4").unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], "肆");
        assert_eq!(arr[1], "零"); // 不足补零
        assert_eq!(arr[2], "叁");
        assert_eq!(arr[3], "贰");
        assert_eq!(arr[4], "壹");
    }

    #[test]
    fn test_money_to_array_zero_amount() {
        // PHP moneyToArray("0.05") → ['零', '伍', '零']
        let arr = money_to_array("0.05").unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], "零"); // 小数第 1 位
        assert_eq!(arr[1], "伍"); // 小数第 2 位
        assert_eq!(arr[2], "零"); // 整数个位
    }

    #[test]
    fn test_money_to_array_three_decimal_digits_truncated() {
        // PHP moneyToArray("123.456") → ['肆', '伍', '叁', '贰', '壹']
        // float_str = substr("456", 0, 2) = "45"（最多取 2 位）
        let arr = money_to_array("123.456").unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], "肆");
        assert_eq!(arr[1], "伍");
        assert_eq!(arr[2], "叁");
        assert_eq!(arr[3], "贰");
        assert_eq!(arr[4], "壹");
    }

    #[test]
    fn test_money_to_array_trailing_dot() {
        // PHP moneyToArray("100.") → ['零', '零', '零', '零', '壹']
        // explode('.', "100.") → ["100", ""]
        // int_str = "100", float_str = ""
        let arr = money_to_array("100.").unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0], "零");
        assert_eq!(arr[1], "零");
        assert_eq!(arr[2], "零");
        assert_eq!(arr[3], "零");
        assert_eq!(arr[4], "壹");
    }

    #[test]
    fn test_money_to_array_leading_dot() {
        // PHP moneyToArray(".05") → ['零', '伍']
        // explode('.', ".05") → ["", "05"]
        // int_str = "", float_str = "05"
        // int_str = strrev("") = ""
        // strlen("") = 0，整数循环不执行
        let arr = money_to_array(".05").unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], "零");
        assert_eq!(arr[1], "伍");
    }

    #[test]
    fn test_money_to_array_large_amount() {
        // PHP moneyToArray("1234567890.12") → ['壹', '贰', '零', '玖', '捌', '柒', '陆', '伍', '肆', '叁', '贰', '壹']
        let arr = money_to_array("1234567890.12").unwrap();
        assert_eq!(arr.len(), 12);
        assert_eq!(arr[0], "壹"); // 小数第 1 位
        assert_eq!(arr[1], "贰"); // 小数第 2 位
        assert_eq!(arr[2], "零"); // 整数个位
        assert_eq!(arr[3], "玖"); // 整数十位
        assert_eq!(arr[4], "捌"); // 整数百位
        assert_eq!(arr[5], "柒"); // 整数千位
        assert_eq!(arr[6], "陆"); // 整数万位
        assert_eq!(arr[7], "伍"); // 整数十万位
        assert_eq!(arr[8], "肆"); // 整数百万位
        assert_eq!(arr[9], "叁"); // 整数千万位
        assert_eq!(arr[10], "贰"); // 整数亿位
        assert_eq!(arr[11], "壹"); // 整数十亿位
    }

    #[test]
    fn test_money_to_array_all_digits() {
        // 验证 0-9 所有数字的大写汉字映射
        let arr = money_to_array("9876543210.00").unwrap();
        assert_eq!(arr[0], "零"); // 小数第 1 位
        assert_eq!(arr[1], "零"); // 小数第 2 位
        assert_eq!(arr[2], "零"); // 整数个位
        assert_eq!(arr[3], "壹"); // 整数十位
        assert_eq!(arr[4], "贰"); // 整数百位
        assert_eq!(arr[5], "叁"); // 整数千位
        assert_eq!(arr[6], "肆"); // 整数万位
        assert_eq!(arr[7], "伍"); // 整数十万位
        assert_eq!(arr[8], "陆"); // 整数百万位
        assert_eq!(arr[9], "柒"); // 整数千万位
        assert_eq!(arr[10], "捌"); // 整数亿位
        assert_eq!(arr[11], "玖"); // 整数十亿位
    }

    #[test]
    fn test_money_to_array_error_multiple_decimal_points() {
        // 多个小数点 → 错误
        let result = money_to_array("123.45.67");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            PdfError::Http(msg) => {
                assert!(msg.contains("multiple decimal points"), "msg = {}", msg);
            }
            other => panic!("Expected PdfError::Http, got {:?}", other),
        }
    }

    #[test]
    fn test_money_to_array_error_non_digit_in_integer() {
        // 整数部分包含非数字字符 → 错误
        let result = money_to_array("12a.45");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            PdfError::Http(msg) => {
                assert!(msg.contains("non-digit in integer part"), "msg = {}", msg);
            }
            other => panic!("Expected PdfError::Http, got {:?}", other),
        }
    }

    #[test]
    fn test_money_to_array_error_non_digit_in_decimal() {
        // 小数部分包含非数字字符 → 错误
        let result = money_to_array("123.4a");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            PdfError::Http(msg) => {
                assert!(msg.contains("non-digit in decimal part"), "msg = {}", msg);
            }
            other => panic!("Expected PdfError::Http, got {:?}", other),
        }
    }

    #[test]
    fn test_money_to_array_error_negative_number() {
        // 负数（整数部分含 '-'）→ 错误
        // PHP 行为：$digits['-'] 产生 warning + null
        // Rust 端：返回错误（更安全）
        let result = money_to_array("-123.45");
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------------
    // data_path 测试 — R5-46 硬约束
    // ------------------------------------------------------------------------

    #[test]
    fn test_data_path_empty() {
        // PHP data_path('') → '/www/wwwroot/sz-api.ljclz.com/'
        assert_eq!(data_path(""), "/www/wwwroot/sz-api.ljclz.com/");
    }

    #[test]
    fn test_data_path_single_segment() {
        // PHP data_path('data') → '/www/wwwroot/sz-api.ljclz.com/data/'
        // PHP: 'base' . ('data' . DIRECTORY_SEPARATOR) = 'base' . 'data/' = 'basedata/'
        assert_eq!(data_path("data"), "/www/wwwroot/sz-api.ljclz.com/data/");
    }

    #[test]
    fn test_data_path_multi_segment() {
        // PHP data_path('data/sign') → '/www/wwwroot/sz-api.ljclz.com/data/sign/'
        assert_eq!(
            data_path("data/sign"),
            "/www/wwwroot/sz-api.ljclz.com/data/sign/"
        );
    }

    #[test]
    fn test_data_path_with_filename() {
        // PHP 业务代码：data_path('data/payment_from.pdf')
        // → '/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf/'
        // 注意：PHP 会追加尾部 DIRECTORY_SEPARATOR，即使传入的是文件名
        assert_eq!(
            data_path("data/payment_from.pdf"),
            "/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf/"
        );
    }

    #[test]
    fn test_data_path_base_constant() {
        // 验证基础路径常量
        assert_eq!(DATA_PATH_BASE, "/www/wwwroot/sz-api.ljclz.com/");
        assert_eq!(DIRECTORY_SEPARATOR, "/");
    }

    #[test]
    fn test_data_path_business_scenarios() {
        // 对齐 PHP Pdf.php 业务场景（第 71 行）：
        // file_exists(data_path()."data/sign/sign_".$detail['uid'].".png")
        // 这里 data_path() 不带参数，返回基础路径，然后字符串拼接
        let base = data_path("");
        let sign_path = format!("{}data/sign/sign_123.png", base);
        assert_eq!(
            sign_path,
            "/www/wwwroot/sz-api.ljclz.com/data/sign/sign_123.png"
        );

        // 对齐 PHP Pdf.php（第 108 行）：
        // data_path().'data/payment_from.pdf'
        let pdf_path = format!("{}data/payment_from.pdf", base);
        assert_eq!(
            pdf_path,
            "/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf"
        );
    }

    // ------------------------------------------------------------------------
    // filename_with_timestamp 测试 — R5-47 硬约束
    // ------------------------------------------------------------------------

    #[test]
    fn test_filename_with_timestamp_format() {
        let filename = filename_with_timestamp("订单导出", "csv");
        // 格式：订单导出_YYYYMMDDHHMMSS.csv
        assert!(filename.starts_with("订单导出_"), "filename = {}", filename);
        assert!(filename.ends_with(".csv"), "filename = {}", filename);

        // 验证时间戳部分为 14 位数字（YYYYMMDDHHMMSS）
        let timestamp_part = &filename["订单导出_".len()..filename.len() - ".csv".len()];
        assert_eq!(
            timestamp_part.len(),
            14,
            "timestamp should be 14 digits, got {}: {}",
            timestamp_part,
            filename
        );
        assert!(
            timestamp_part.chars().all(|c| c.is_ascii_digit()),
            "timestamp should be all digits, got {}: {}",
            timestamp_part,
            filename
        );
    }

    #[test]
    fn test_filename_with_timestamp_year_range() {
        // 验证年份部分为 2026 左右（测试运行时间在 2025-2030 范围内）
        let filename = filename_with_timestamp("test", "pdf");
        let timestamp_part = &filename["test_".len()..filename.len() - ".pdf".len()];
        let year: u32 = timestamp_part[..4].parse().unwrap();
        assert!(
            (2025..=2030).contains(&year),
            "year should be 2025-2030, got {}",
            year
        );
    }

    #[test]
    fn test_filename_with_timestamp_month_range() {
        let filename = filename_with_timestamp("test", "csv");
        let timestamp_part = &filename["test_".len()..filename.len() - ".csv".len()];
        let month: u32 = timestamp_part[4..6].parse().unwrap();
        assert!(
            (1..=12).contains(&month),
            "month should be 01-12, got {}",
            month
        );
    }

    #[test]
    fn test_filename_with_timestamp_day_range() {
        let filename = filename_with_timestamp("test", "csv");
        let timestamp_part = &filename["test_".len()..filename.len() - ".csv".len()];
        let day: u32 = timestamp_part[6..8].parse().unwrap();
        assert!((1..=31).contains(&day), "day should be 01-31, got {}", day);
    }

    #[test]
    fn test_filename_with_timestamp_hour_range() {
        let filename = filename_with_timestamp("test", "csv");
        let timestamp_part = &filename["test_".len()..filename.len() - ".csv".len()];
        let hour: u32 = timestamp_part[8..10].parse().unwrap();
        assert!(hour <= 23, "hour should be 00-23, got {}", hour);
    }

    #[test]
    fn test_filename_with_timestamp_minute_range() {
        let filename = filename_with_timestamp("test", "csv");
        let timestamp_part = &filename["test_".len()..filename.len() - ".csv".len()];
        let minute: u32 = timestamp_part[10..12].parse().unwrap();
        assert!(minute <= 59, "minute should be 00-59, got {}", minute);
    }

    #[test]
    fn test_filename_with_timestamp_second_range() {
        let filename = filename_with_timestamp("test", "csv");
        let timestamp_part = &filename["test_".len()..filename.len() - ".csv".len()];
        let second: u32 = timestamp_part[12..14].parse().unwrap();
        assert!(second <= 59, "second should be 00-59, got {}", second);
    }

    #[test]
    fn test_filename_with_timestamp_different_extensions() {
        let csv = filename_with_timestamp("export", "csv");
        let pdf = filename_with_timestamp("payment", "pdf");
        let xlsx = filename_with_timestamp("report", "xlsx");

        assert!(csv.ends_with(".csv"));
        assert!(pdf.ends_with(".pdf"));
        assert!(xlsx.ends_with(".xlsx"));
    }

    #[test]
    fn test_filename_with_timestamp_chinese_prefix() {
        // 对齐 PHP 中文文件名场景
        let filename = filename_with_timestamp("收款明细", "csv");
        assert!(filename.starts_with("收款明细_"));
        assert!(filename.ends_with(".csv"));
    }

    // ------------------------------------------------------------------------
    // R5 PHP/Rust 行为对比测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_45_money_to_array_php_behavior_alignment() {
        // R5-45 硬约束：金额按位拆分为大写汉字数组对齐 PHP moneyToArray
        // 小数 2 位键 0-1，整数部分键 2+，低位在前

        // 测试用例 1：123.45
        // PHP: ['肆', '伍', '叁', '贰', '壹']
        let arr1 = money_to_array("123.45").unwrap();
        assert_eq!(arr1, vec!["肆", "伍", "叁", "贰", "壹"]);

        // 测试用例 2：100（整数）
        // PHP: ['零', '零', '零', '零', '壹']
        let arr2 = money_to_array("100").unwrap();
        assert_eq!(arr2, vec!["零", "零", "零", "零", "壹"]);

        // 测试用例 3：0.05
        // PHP: ['零', '伍', '零']
        let arr3 = money_to_array("0.05").unwrap();
        assert_eq!(arr3, vec!["零", "伍", "零"]);

        // 测试用例 4：.05（无整数部分）
        // PHP: ['零', '伍']
        let arr4 = money_to_array(".05").unwrap();
        assert_eq!(arr4, vec!["零", "伍"]);

        // 测试用例 5：123.4（一位小数）
        // PHP: ['肆', '零', '叁', '贰', '壹']
        let arr5 = money_to_array("123.4").unwrap();
        assert_eq!(arr5, vec!["肆", "零", "叁", "贰", "壹"]);

        // 测试用例 6：123.456（三位小数，截断为两位）
        // PHP: ['肆', '伍', '叁', '贰', '壹']
        let arr6 = money_to_array("123.456").unwrap();
        assert_eq!(arr6, vec!["肆", "伍", "叁", "贰", "壹"]);

        // 测试用例 7：100.（尾部小数点）
        // PHP: ['零', '零', '零', '零', '壹']
        let arr7 = money_to_array("100.").unwrap();
        assert_eq!(arr7, vec!["零", "零", "零", "零", "壹"]);

        // 验证键顺序：小数键 0-1，整数键 2+，低位在前
        // 以 123.45 为例：
        // 键 0 = 小数第 1 位 = 4 → '肆'
        // 键 1 = 小数第 2 位 = 5 → '伍'
        // 键 2 = 整数个位 = 3 → '叁'
        // 键 3 = 整数十位 = 2 → '贰'
        // 键 4 = 整数百位 = 1 → '壹'
        assert_eq!(arr1[0], "肆"); // 小数第 1 位（角）
        assert_eq!(arr1[1], "伍"); // 小数第 2 位（分）
        assert_eq!(arr1[2], "叁"); // 整数个位（低位在前）
        assert_eq!(arr1[3], "贰"); // 整数十位
        assert_eq!(arr1[4], "壹"); // 整数百位（高位在后）
    }

    #[test]
    fn test_r5_46_data_path_php_behavior_alignment() {
        // R5-46 硬约束：数据目录路径对齐 PHP data_path 全局函数

        // PHP data_path('') → '/www/wwwroot/sz-api.ljclz.com/'
        assert_eq!(data_path(""), "/www/wwwroot/sz-api.ljclz.com/");

        // PHP data_path('data') → '/www/wwwroot/sz-api.ljclz.com/data/'
        assert_eq!(data_path("data"), "/www/wwwroot/sz-api.ljclz.com/data/");

        // PHP data_path('data/sign') → '/www/wwwroot/sz-api.ljclz.com/data/sign/'
        assert_eq!(
            data_path("data/sign"),
            "/www/wwwroot/sz-api.ljclz.com/data/sign/"
        );

        // PHP data_path('data/payment_from.pdf') → '/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf/'
        // 注意：PHP 会追加尾部 /，即使传入的是文件名
        assert_eq!(
            data_path("data/payment_from.pdf"),
            "/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf/"
        );

        // PHP 业务代码常用模式：data_path() . 'data/xxx'
        // 即 data_path('') 返回基础路径，然后字符串拼接
        let base = data_path("");
        assert_eq!(base, "/www/wwwroot/sz-api.ljclz.com/");
        assert_eq!(
            format!("{}data/payment_from.pdf", base),
            "/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf"
        );
    }

    #[test]
    fn test_r5_47_filename_with_timestamp_php_behavior_alignment() {
        // R5-47 硬约束：带时间戳文件名对齐 PHP date('YmdHis') 命名规则

        // PHP: $filename = 'prefix_' . date('YmdHis') . '.ext';
        // Rust: filename_with_timestamp('prefix', 'ext')

        let filename = filename_with_timestamp("payment", "pdf");
        assert!(filename.starts_with("payment_"));
        assert!(filename.ends_with(".pdf"));

        // 验证时间戳格式对齐 PHP date('YmdHis')
        let timestamp = &filename["payment_".len()..filename.len() - ".pdf".len()];
        assert_eq!(timestamp.len(), 14, "YmdHis should be 14 digits");

        // 验证各字段范围对齐 PHP date() 格式
        let year: u32 = timestamp[..4].parse().unwrap();
        let month: u32 = timestamp[4..6].parse().unwrap();
        let day: u32 = timestamp[6..8].parse().unwrap();
        let hour: u32 = timestamp[8..10].parse().unwrap();
        let minute: u32 = timestamp[10..12].parse().unwrap();
        let second: u32 = timestamp[12..14].parse().unwrap();

        assert!(year >= 2025, "year >= 2025, got {}", year);
        assert!((1..=12).contains(&month), "month 01-12, got {}", month);
        assert!((1..=31).contains(&day), "day 01-31, got {}", day);
        assert!(hour <= 23, "hour 00-23, got {}", hour);
        assert!(minute <= 59, "minute 00-59, got {}", minute);
        assert!(second <= 59, "second 00-59, got {}", second);
    }

    #[test]
    fn test_php_pdf_business_scenario_simulation() {
        // 模拟 PHP Pdf.php 业务场景（paymentPdf 方法）
        // 对齐 PHP 代码片段：
        //   $data['pdfName'] = data_path().'data/payment_from.pdf';
        //   $mArr = moneyToArray($detail['amount']);
        //   foreach ($mArr as $k=>$v){ $data['fa'.$k] = $v; }
        //   $t = count($mArr);
        //   for ($i = $t; $i < 10; $i++){ $data['fa'.$i] = "X"; }

        let amount = "12345.67";
        let m_arr = money_to_array(amount).unwrap();

        // 模拟 PHP fa0-fa9 字段填充
        let mut fa_fields: Vec<String> = Vec::with_capacity(10);
        for v in &m_arr {
            fa_fields.push(v.clone());
        }
        // 不足 10 位补 "X"
        let t = m_arr.len();
        for _ in t..10 {
            fa_fields.push("X".to_string());
        }

        // 验证：12345.67 → ['陆', '柒', '伍', '肆', '叁', '贰', '壹'] + ['X', 'X', 'X']
        assert_eq!(m_arr.len(), 7);
        assert_eq!(m_arr[0], "陆"); // 小数第 1 位（角）
        assert_eq!(m_arr[1], "柒"); // 小数第 2 位（分）
        assert_eq!(m_arr[2], "伍"); // 整数个位
        assert_eq!(m_arr[3], "肆"); // 整数十位
        assert_eq!(m_arr[4], "叁"); // 整数百位
        assert_eq!(m_arr[5], "贰"); // 整数千位
        assert_eq!(m_arr[6], "壹"); // 整数万位

        // 验证 fa 字段填充
        assert_eq!(fa_fields.len(), 10);
        assert_eq!(fa_fields[0], "陆"); // fa0
        assert_eq!(fa_fields[1], "柒"); // fa1
        assert_eq!(fa_fields[2], "伍"); // fa2
        assert_eq!(fa_fields[3], "肆"); // fa3
        assert_eq!(fa_fields[4], "叁"); // fa4
        assert_eq!(fa_fields[5], "贰"); // fa5
        assert_eq!(fa_fields[6], "壹"); // fa6
        assert_eq!(fa_fields[7], "X"); // fa7（补位）
        assert_eq!(fa_fields[8], "X"); // fa8（补位）
        assert_eq!(fa_fields[9], "X"); // fa9（补位）

        // 验证 pdfName 路径拼接
        let pdf_name = format!("{}data/payment_from.pdf", data_path(""));
        assert_eq!(
            pdf_name,
            "/www/wwwroot/sz-api.ljclz.com/data/payment_from.pdf"
        );
    }
}
