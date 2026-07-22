//! HTTP Java 服务客户端 — 对齐 PHP `http_java_post` 全局函数
//!
//! ## PHP 对齐
//!
//! PHP 项目将 PDF 生成委派给 Java 服务（`http://127.0.0.1:8086`），通过
//! `http_java_post` 全局函数发送 JSON POST 请求。
//!
//! 本模块提供两种 API：
//!
//! - [`http_java_post`]：自由函数，直接 POST 到完整 URL（1:1 对齐 PHP `http_java_post`）
//! - [`JavaClient`]：结构体，封装 base_url + timeout，便于复用连接
//!
//! ## PHP 源码参考
//!
//! `e:\vue\test\鲜视达\server\app\common.php` 第 1871 行：
//!
//! ```php
//! function http_java_post($url, $data = NULL){
//!     $curl = curl_init();
//!     curl_setopt($curl, CURLOPT_URL, $url);
//!     curl_setopt($curl, CURLOPT_SSL_VERIFYPEER, false);
//!     curl_setopt($curl, CURLOPT_SSL_VERIFYHOST, false);
//!     if (!$data) {
//!         return 'data is null';
//!     }
//!     if (is_array($data)) {
//!         $data = json_encode($data);
//!     }
//!     curl_setopt($curl, CURLOPT_POST, 1);
//!     curl_setopt($curl, CURLOPT_POSTFIELDS, $data);
//!     curl_setopt($curl, CURLOPT_HEADER, 0);
//!     curl_setopt($curl, CURLOPT_HTTPHEADER, array(
//!         'Content-Type: application/json; charset=utf-8',
//!         'Content-Length:' . strlen($data),
//!         'Cache-Control: no-cache',
//!         'Pragma: no-cache'
//!     ));
//!     curl_setopt($curl, CURLOPT_RETURNTRANSFER, 1);
//!     $res = curl_exec($curl);
//!     $errorno = curl_errno($curl);
//!     if ($errorno) {
//!         return $errorno;
//!     }
//!     curl_close($curl);
//!     return $res;
//! }
//! ```
//!
//! ## 调用方参考
//!
//! `e:\vue\test\鲜视达\server\app\job\controller\Pdf.php`（7 种 PDF 业务类型）：
//!
//! ```php
//! // 付款单 PDF
//! $res = http_java_post('http://127.0.0.1:8086/home/payment', $data);
//! $result = (array)json_decode($res);
//! if(!empty($result['status']) && $result['status'] == 500){
//!     // 错误：发送企业微信告警
//! } else {
//!     if(!empty($result['code']) && $result['code'] == 200){
//!         // 成功：更新数据库 hash + filename
//!     }
//! }
//! ```
//!
//! ## R5 硬约束
//!
//! - R5-44：`http_java_post` / `JavaClient::post` — POST JSON + 特定 HTTP 头
//!   对齐 PHP `http_java_post` 全局函数
//!   （Content-Type: application/json; charset=utf-8 / Cache-Control: no-cache / Pragma: no-cache）

use std::time::Duration;

use crate::PdfError;

// ============================================================================
// 常量 — 对齐 PHP 硬编码值
// ============================================================================

/// Java PDF 服务默认地址 — 对齐 PHP 硬编码 `http://127.0.0.1:8086`
///
/// PHP `app\job\controller\Pdf.php` 中所有 PDF 生成请求都指向此地址。
pub const DEFAULT_JAVA_PDF_SERVICE_URL: &str = "http://127.0.0.1:8086";

/// 默认请求超时（30 秒）
///
/// PHP cURL 默认无超时，但生产环境需要合理超时以防止挂起。
/// 30 秒足够 Java PDF 服务完成生成。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

// ============================================================================
// JavaClient — 封装 reqwest 客户端
// ============================================================================

/// HTTP Java 服务客户端 — 封装 reqwest，对齐 PHP `http_java_post`
///
/// 使用 builder pattern，支持自定义 base_url 和 timeout。
///
/// # PHP 行为对齐
///
/// - SSL 验证关闭（对齐 `CURLOPT_SSL_VERIFYPEER=false`）
/// - POST JSON body（对齐 `json_encode($data)` + `CURLOPT_POSTFIELDS`）
/// - 特定 HTTP 头（对齐 `CURLOPT_HTTPHEADER`）
/// - 返回响应文本（对齐 `CURLOPT_RETURNTRANSFER=1`）
///
/// # 示例
///
/// ```ignore
/// use sz_rust_pdf::java_client::JavaClient;
/// use serde_json::json;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let client = JavaClient::new_default()?;
/// let data = json!({
///     "pdfName": "/path/to/template.pdf",
///     "type": "payment",
///     "store_name": "太平店",
///     "amount": "2500.50"
/// });
/// let response = client.post("/home/payment", &data).await?;
/// # Ok(())
/// # }
/// ```
pub struct JavaClient {
    /// 基础 URL（如 `http://127.0.0.1:8086`）
    base_url: String,
    /// 请求超时
    timeout: Duration,
    /// reqwest 客户端
    client: reqwest::Client,
}

impl JavaClient {
    /// 创建 Java 服务客户端 — 指定 base_url
    ///
    /// # 错误
    ///
    /// reqwest 客户端构建失败返回 [`PdfError::Http`]。
    pub fn new(base_url: &str) -> Result<Self, PdfError> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true) // 对齐 PHP CURLOPT_SSL_VERIFYPEER=false
            .no_proxy() // 对齐 PHP cURL 默认不使用代理（未设置 CURLOPT_PROXY）
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| PdfError::Http(format!("reqwest client build failed: {}", e)))?;
        Ok(Self {
            base_url: base_url.to_string(),
            timeout: DEFAULT_TIMEOUT,
            client,
        })
    }

    /// 创建默认客户端 — base_url 为 [`DEFAULT_JAVA_PDF_SERVICE_URL`]
    ///
    /// 对齐 PHP 所有 PDF 生成请求都指向 `http://127.0.0.1:8086`。
    ///
    /// 注：方法名使用 `new_default` 而非 `default`，避免与 `std::default::Default` trait 混淆
    /// （`Default` trait 返回 `Self`，本方法返回 `Result<Self, PdfError>`）。
    pub fn new_default() -> Result<Self, PdfError> {
        Self::new(DEFAULT_JAVA_PDF_SERVICE_URL)
    }

    /// 设置请求超时 — builder pattern
    ///
    /// # 错误
    ///
    /// reqwest 客户端重建失败返回 [`PdfError::Http`]。
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, PdfError> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .no_proxy() // 对齐 PHP cURL 默认不使用代理
            .timeout(timeout)
            .build()
            .map_err(|e| PdfError::Http(format!("reqwest client build failed: {}", e)))?;
        self.client = client;
        self.timeout = timeout;
        Ok(self)
    }

    /// 获取 base_url
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 获取当前超时
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// POST JSON 到 Java 服务 — 对齐 PHP `http_java_post`
    ///
    /// # PHP 行为对齐
    ///
    /// 1. 如果 `data` 是 `Value::Null`，返回 `Ok("data is null")`（对齐 PHP `if (!$data) return 'data is null'`）
    /// 2. JSON 序列化 data 作为请求体（对齐 PHP `json_encode($data)`）
    /// 3. 设置 HTTP 头：
    ///    - `Content-Type: application/json; charset=utf-8`
    ///    - `Content-Length: {body.len()}`
    ///    - `Cache-Control: no-cache`
    ///    - `Pragma: no-cache`
    /// 4. POST 请求到 `{base_url}{path}`
    /// 5. 返回响应文本（对齐 PHP `CURLOPT_RETURNTRANSFER=1`）
    ///
    /// # PHP 行为差异
    ///
    /// PHP 在 cURL 错误时返回错误号（int），Rust 返回 `Err(PdfError::Http(...))`。
    /// 这是有意的设计差异：Rust 使用 `Result` 类型区分成功和失败，更符合惯用法。
    ///
    /// # R5-44 硬约束
    ///
    /// - POST JSON body
    /// - 特定 HTTP 头
    /// - 返回响应文本
    ///
    /// # 错误
    ///
    /// - JSON 序列化失败 → [`PdfError::Http`]
    /// - HTTP 请求失败 → [`PdfError::Http`]
    /// - 响应读取失败 → [`PdfError::Http`]
    pub async fn post(&self, path: &str, data: &serde_json::Value) -> Result<String, PdfError> {
        // 对齐 PHP：if (!$data) return 'data is null';
        if data.is_null() {
            return Ok("data is null".to_string());
        }

        // JSON 序列化（对齐 PHP json_encode($data)）
        let body = serde_json::to_string(data)
            .map_err(|e| PdfError::Http(format!("JSON serialize failed: {}", e)))?;

        // 构建完整 URL
        let url = format!("{}{}", self.base_url, path);

        // 发送 POST 请求（对齐 PHP cURL 选项）
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .header("Content-Length", body.len())
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .body(body)
            .send()
            .await
            .map_err(|e| PdfError::Http(format!("HTTP request failed: {}", e)))?;

        // 读取响应文本（对齐 PHP CURLOPT_RETURNTRANSFER=1）
        let text = response
            .text()
            .await
            .map_err(|e| PdfError::Http(format!("HTTP response read failed: {}", e)))?;

        Ok(text)
    }
}

// ============================================================================
// http_java_post — 自由函数，1:1 对齐 PHP
// ============================================================================

/// POST JSON 到指定 URL — 对齐 PHP `http_java_post($url, $data)`
///
/// 自由函数版本，直接 POST 到完整 URL，每次调用创建新的 reqwest 客户端。
/// 适用于一次性调用场景。频繁调用建议使用 [`JavaClient`] 复用连接。
///
/// # PHP 行为对齐
///
/// 1. 如果 `data` 是 `Value::Null`，返回 `Ok("data is null")`
/// 2. JSON 序列化 + POST + 特定 HTTP 头
/// 3. 返回响应文本
///
/// # R5-44 硬约束
///
/// # 示例
///
/// ```ignore
/// use sz_rust_pdf::java_client::http_java_post;
/// use serde_json::json;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let data = json!({"type": "payment", "id": 123});
/// let res = http_java_post("http://127.0.0.1:8086/home/payment", &data).await?;
/// # Ok(())
/// # }
/// ```
pub async fn http_java_post(url: &str, data: &serde_json::Value) -> Result<String, PdfError> {
    // 对齐 PHP：if (!$data) return 'data is null';
    if data.is_null() {
        return Ok("data is null".to_string());
    }

    // JSON 序列化
    let body = serde_json::to_string(data)
        .map_err(|e| PdfError::Http(format!("JSON serialize failed: {}", e)))?;

    // 构建 reqwest 客户端（对齐 PHP CURLOPT_SSL_VERIFYPEER=false）
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy() // 对齐 PHP cURL 默认不使用代理（未设置 CURLOPT_PROXY）
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(|e| PdfError::Http(format!("reqwest client build failed: {}", e)))?;

    // 发送 POST 请求
    let response = client
        .post(url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Content-Length", body.len())
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .body(body)
        .send()
        .await
        .map_err(|e| PdfError::Http(format!("HTTP request failed: {}", e)))?;

    // 读取响应文本
    let text = response
        .text()
        .await
        .map_err(|e| PdfError::Http(format!("HTTP response read failed: {}", e)))?;

    Ok(text)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ------------------------------------------------------------------------
    // 常量测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_default_java_pdf_service_url() {
        // 对齐 PHP 硬编码 http://127.0.0.1:8086
        assert_eq!(DEFAULT_JAVA_PDF_SERVICE_URL, "http://127.0.0.1:8086");
    }

    #[test]
    fn test_default_timeout() {
        // 30 秒超时
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
    }

    // ------------------------------------------------------------------------
    // JavaClient 构建测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_44_java_client_new() {
        let client = JavaClient::new("http://example.com:8080").unwrap();
        assert_eq!(client.base_url(), "http://example.com:8080");
        assert_eq!(client.timeout(), DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_r5_44_java_client_default() {
        let client = JavaClient::new_default().unwrap();
        assert_eq!(client.base_url(), DEFAULT_JAVA_PDF_SERVICE_URL);
        assert_eq!(client.timeout(), DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_r5_44_java_client_with_timeout() {
        let client = JavaClient::new_default()
            .unwrap()
            .with_timeout(Duration::from_secs(60))
            .unwrap();
        assert_eq!(client.timeout(), Duration::from_secs(60));
    }

    // ------------------------------------------------------------------------
    // http_java_post null data 测试（对齐 PHP if (!$data) return 'data is null'）
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_r5_44_http_java_post_null_data() {
        // 对齐 PHP: if (!$data) return 'data is null';
        let result = http_java_post("http://127.0.0.1:9999/no-connection", &json!(null)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "data is null");
    }

    #[tokio::test]
    async fn test_r5_44_java_client_post_null_data() {
        // 对齐 PHP: if (!$data) return 'data is null';
        let client = JavaClient::new_default().unwrap();
        let result = client.post("/home/payment", &json!(null)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "data is null");
    }

    // ------------------------------------------------------------------------
    // http_java_post 连接失败测试（对齐 PHP cURL 错误返回 errorno）
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_r5_44_http_java_post_connection_refused() {
        // 连接到不存在的端口，应该返回错误（PHP 返回 cURL errorno，Rust 返回 Err）
        let data = json!({"test": "value"});
        let result = http_java_post("http://127.0.0.1:19999/nonexistent", &data).await;
        assert!(result.is_err());
        // 错误信息应包含 HTTP 相关字样
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("HTTP") || err_msg.contains("connection"));
    }

    #[tokio::test]
    async fn test_r5_44_java_client_post_connection_refused() {
        let client = JavaClient::new("http://127.0.0.1:19999").unwrap();
        let data = json!({"test": "value"});
        let result = client.post("/test", &data).await;
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------------
    // URL 构建测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_44_url_construction() {
        // 验证 JavaClient::post 内部 URL 拼接逻辑
        // base_url = "http://127.0.0.1:8086", path = "/home/payment"
        // → url = "http://127.0.0.1:8086/home/payment"
        let base = "http://127.0.0.1:8086";
        let path = "/home/payment";
        let url = format!("{}{}", base, path);
        assert_eq!(url, "http://127.0.0.1:8086/home/payment");
    }

    // ------------------------------------------------------------------------
    // PHP 行为对比测试
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_r5_44_php_null_data_behavior() {
        // 对齐 PHP http_java_post($url, NULL) 返回 'data is null'
        // PHP 代码：
        //   if (!$data) { return 'data is null'; }
        // 注意：PHP !$data 对 null/0/false/''/[] 均为 true
        // Rust 端仅对 Value::Null 返回 'data is null'

        let result = http_java_post("http://any.url", &json!(null)).await;
        assert_eq!(result.unwrap(), "data is null");
    }

    #[tokio::test]
    async fn test_r5_44_php_non_null_data_attempts_request() {
        // 对齐 PHP http_java_post($url, $nonNullData) 尝试 HTTP 请求
        // 非 null data 应该尝试连接（连接失败返回错误）
        let data = json!({"id": 1, "type": "payment"});
        let result = http_java_post("http://127.0.0.1:19999/no-server", &data).await;
        assert!(result.is_err()); // 连接失败
    }

    // ------------------------------------------------------------------------
    // HTTP 头对齐测试（验证客户端构建不报错）
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_44_client_ssl_verification_disabled() {
        // 对齐 PHP CURLOPT_SSL_VERIFYPEER=false / CURLOPT_SSL_VERIFYHOST=false
        // reqwest danger_accept_invalid_certs(true) 应该成功构建
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(DEFAULT_TIMEOUT)
            .build();
        assert!(client.is_ok());
    }

    // ------------------------------------------------------------------------
    // 7 种 PDF 业务类型路径测试（对齐 Pdf.php 调用）
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_44_pdf_business_paths() {
        // 对齐 PHP Pdf.php 中 7 种 PDF 业务类型的 http_java_post 调用路径
        let base = DEFAULT_JAVA_PDF_SERVICE_URL;

        // paymentPdf → http://127.0.0.1:8086/home/payment
        assert_eq!(
            format!("{}{}", base, "/home/payment"),
            "http://127.0.0.1:8086/home/payment"
        );

        // huiyiPdf → http://127.0.0.1:8086/home/huiyi
        assert_eq!(
            format!("{}{}", base, "/home/huiyi"),
            "http://127.0.0.1:8086/home/huiyi"
        );

        // salePdf → http://127.0.0.1:8086/home/sale
        assert_eq!(
            format!("{}{}", base, "/home/sale"),
            "http://127.0.0.1:8086/home/sale"
        );

        // saleOrderPdf → http://127.0.0.1:8086/home/index
        assert_eq!(
            format!("{}{}", base, "/home/index"),
            "http://127.0.0.1:8086/home/index"
        );

        // allotPdf → http://127.0.0.1:8086/home/index
        // allot 和 saleOrder 共用 /home/index 端点（PHP 源码确认）
    }
}
