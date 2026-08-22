//! 银行支付服务真实 HTTP 实现 — C-4 修复
//!
//! 提供 CcbService/IcbcService/FuiouService 的真实 HTTP 客户端实现，
//! 通过 reqwest 调用银行 API endpoint，使用 MD5 签名。
//!
//! # 配置
//!
//! 通过环境变量配置各银行的 API endpoint 和凭证：
//!
//! | 银行 | 环境变量 | 说明 |
//! |------|---------|------|
//! | CCB | `CCB_API_URL` | 建行支付 API URL |
//! | CCB | `CCB_MERCHANT_ID` | 商户号 |
//! | CCB | `CCB_POS_ID` | 终端号 |
//! | ICBC | `ICBC_API_URL` | 工行支付 API URL |
//! | ICBC | `ICBC_MER_ID` | 商户号 |
//! | ICBC | `ICBC_APP_ID` | 应用 ID |
//! | FUIOU | `FUIOU_API_URL` | 富友支付 API URL |
//! | FUIOU | `FUIOU_INS_CD` | 机构号 |
//! | FUIOU | `FUIOU_MCHNT_CD` | 商户号 |
//! | FUIOU | `FUIOU_KEY` | 签名密钥 |
//! | QYWX | `QYWX_WEBHOOK_URL` | 企微 webhook URL |
//!
//! # 设计
//!
//! - 真实实现通过 reqwest 发送 HTTP 请求
//! - MD5 签名对齐 PHP SDK 的签名算法（参数排序 + 拼接 + MD5）
//! - 环境变量未配置时返回明确错误（而非 Mock 数据）
//! - 保留 `Mock*Service` 用于单元测试

use md5::{Digest, Md5};
use serde_json::Value;
use std::time::Duration;

/// HTTP 客户端配置
#[derive(Debug, Clone)]
pub struct HttpBankConfig {
    /// API endpoint URL
    pub api_url: String,
    /// 商户号
    pub merchant_id: String,
    /// 签名密钥
    pub sign_key: String,
    /// 请求超时（秒）
    pub timeout_secs: u64,
}

impl HttpBankConfig {
    /// 从环境变量读取 CCB 配置
    ///
    /// # 返回
    ///
    /// - `Some(config)`：环境变量已配置
    /// - `None`：环境变量未配置，调用方应返回错误
    pub fn from_env_ccb() -> Option<Self> {
        let api_url = std::env::var("CCB_API_URL").ok()?;
        let merchant_id = std::env::var("CCB_MERCHANT_ID").ok()?;
        let pos_id = std::env::var("CCB_POS_ID").ok()?;
        Some(Self {
            api_url,
            merchant_id: format!("{merchant_id}/{pos_id}"),
            sign_key: std::env::var("CCB_SIGN_KEY").unwrap_or_default(),
            timeout_secs: 30,
        })
    }

    /// 从环境变量读取 ICBC 配置
    pub fn from_env_icbc() -> Option<Self> {
        let api_url = std::env::var("ICBC_API_URL").ok()?;
        let merchant_id = std::env::var("ICBC_MER_ID").ok()?;
        Some(Self {
            api_url,
            merchant_id,
            sign_key: std::env::var("ICBC_SIGN_KEY").unwrap_or_default(),
            timeout_secs: 30,
        })
    }

    /// 从环境变量读取 Fuiou 配置
    pub fn from_env_fuiou() -> Option<Self> {
        let api_url = std::env::var("FUIOU_API_URL").ok()?;
        let mchnt_cd = std::env::var("FUIOU_MCHNT_CD").ok()?;
        Some(Self {
            api_url,
            merchant_id: mchnt_cd,
            sign_key: std::env::var("FUIOU_KEY").ok()?,
            timeout_secs: 30,
        })
    }
}

/// MD5 签名 — 对齐 PHP `Signature::generateSign`
///
/// # 算法
///
/// 1. 按 key 字典序排序
/// 2. 拼接为 `key1=value1&key2=value2...`（跳过空值和 `sign` 字段）
/// 3. 追加 `&key={sign_key}`
/// 4. 计算 MD5，转大写
pub fn md5_sign(params: &serde_json::Map<String, Value>, sign_key: &str) -> String {
    let mut keys: Vec<&String> = params
        .keys()
        .filter(|k| {
            k.as_str() != "sign" && params.get(*k).map(|v| !is_empty_value(v)).unwrap_or(false)
        })
        .collect();
    keys.sort();

    let mut sb = String::new();
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            sb.push('&');
        }
        sb.push_str(k);
        sb.push('=');
        sb.push_str(&value_to_string(&params[*k]));
    }
    if !sign_key.is_empty() {
        sb.push_str("&key=");
        sb.push_str(sign_key);
    }

    let mut hasher = Md5::new();
    hasher.update(sb.as_bytes());
    hex::encode_upper(hasher.finalize())
}

/// 判断 Value 是否为空
fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// Value 转字符串（对齐 PHP 的字符串拼接）
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        _ => v.to_string(),
    }
}

/// 通用 HTTP POST 客户端
pub struct HttpBankClient {
    config: HttpBankConfig,
    client: reqwest::Client,
}

impl HttpBankClient {
    /// 创建 HTTP 客户端
    pub fn new(config: HttpBankConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build reqwest client");
        Self { config, client }
    }

    /// 获取配置引用
    pub fn config(&self) -> &HttpBankConfig {
        &self.config
    }

    /// 获取 HTTP client 引用
    pub fn http(&self) -> &reqwest::Client {
        &self.client
    }

    /// POST JSON 请求
    ///
    /// # 参数
    ///
    /// - `path`：API 路径（拼接在 api_url 后）
    /// - `body`：JSON 请求体
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：响应 JSON
    /// - `Err(String)`：网络错误或解析失败
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        let url = if path.is_empty() {
            self.config.api_url.clone()
        } else if self.config.api_url.ends_with('/') {
            format!("{}{}", self.config.api_url, path.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.config.api_url, path.trim_start_matches('/'))
        };
        tracing::info!(url = %url, "POST JSON to bank API");

        // P1-SEC-06: tokio 级超时保护（防御性兜底，reqwest 已配置同值超时）
        let timeout_dur = Duration::from_secs(self.config.timeout_secs.max(5));
        let send_future = self.client.post(&url).json(body).send();
        let resp = match tokio::time::timeout(timeout_dur, send_future).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(format!("HTTP 请求失败: {e}")),
            Err(_) => return Err(format!("银行 API 超时（>{timeout_dur:?}）")),
        };

        if !resp.status().is_success() {
            return Err(format!("HTTP 状态码: {}", resp.status()));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| format!("响应 JSON 解析失败: {e}"))
    }

    /// POST XML 请求（富友使用 XML 格式）
    ///
    /// # 参数
    ///
    /// - `path`：API 路径
    /// - `xml_body`：XML 请求体字符串
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：响应 XML 解析为 JSON（简易解析，仅支持扁平结构）
    /// - `Err(String)`：网络错误或解析失败
    pub async fn post_xml(&self, path: &str, xml_body: &str) -> Result<Value, String> {
        let url = if path.is_empty() {
            self.config.api_url.clone()
        } else if self.config.api_url.ends_with('/') {
            format!("{}{}", self.config.api_url, path.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.config.api_url, path.trim_start_matches('/'))
        };
        tracing::info!(url = %url, "POST XML to bank API");

        // P1-SEC-06: tokio 级超时保护（防御性兜底）
        let timeout_dur = Duration::from_secs(self.config.timeout_secs.max(5));
        let send_future = self
            .client
            .post(&url)
            .header("Content-Type", "application/xml")
            .body(xml_body.to_string())
            .send();
        let resp = match tokio::time::timeout(timeout_dur, send_future).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(format!("HTTP 请求失败: {e}")),
            Err(_) => return Err(format!("银行 API 超时（>{timeout_dur:?}）")),
        };

        if !resp.status().is_success() {
            return Err(format!("HTTP 状态码: {}", resp.status()));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| format!("响应体读取失败: {e}"))?;
        parse_simple_xml(&text)
    }
}

/// 简易 XML 解析（仅支持 `<root><key>value</key>...</root>` 扁平结构）
///
/// 富友 API 响应通常是扁平 XML，此函数将其解析为 JSON 对象。
fn parse_simple_xml(xml: &str) -> Result<Value, String> {
    let mut map = serde_json::Map::new();
    let mut depth = 0;
    let mut current_key = String::new();
    let mut current_value = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut in_content = false;

    for ch in xml.chars() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            if in_content {
                in_content = false;
            }
            continue;
        }
        if ch == '>' {
            in_tag = false;
            let tag = tag_buf.trim();
            if let Some(key) = tag.strip_prefix('/') {
                if depth > 0 && key == current_key {
                    if !current_value.is_empty() {
                        map.insert(current_key.clone(), Value::String(current_value.clone()));
                    }
                    current_value.clear();
                    current_key.clear();
                    depth -= 1;
                }
            } else if tag.starts_with("?xml") || tag.is_empty() {
                continue;
            } else {
                if !current_key.is_empty() && depth > 0 {
                    // 上一个 key 未正常关闭，跳过
                }
                current_key = tag.to_string();
                depth += 1;
                in_content = true;
            }
            continue;
        }
        if in_tag {
            tag_buf.push(ch);
        } else if in_content && depth > 0 {
            current_value.push(ch);
        }
    }

    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_md5_sign_basic() {
        let mut params = serde_json::Map::new();
        params.insert("b".to_string(), Value::String("2".to_string()));
        params.insert("a".to_string(), Value::String("1".to_string()));
        params.insert("sign".to_string(), Value::String("should_skip".to_string()));
        params.insert("empty".to_string(), Value::String(String::new()));

        let sign = md5_sign(&params, "secret");
        // 验证签名是 32 位大写十六进制
        assert_eq!(sign.len(), 32);
        assert!(sign
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn test_parse_simple_xml() {
        let xml = r#"<?xml version="1.0"?><root><result_code>000000</result_code><result_msg>成功</result_msg></root>"#;
        let result = parse_simple_xml(xml).unwrap();
        assert_eq!(result["result_code"], "000000");
        assert_eq!(result["result_msg"], "成功");
    }

    #[test]
    fn test_value_to_string() {
        assert_eq!(value_to_string(&Value::Null), "");
        assert_eq!(value_to_string(&Value::String("abc".to_string())), "abc");
        assert_eq!(value_to_string(&json!(42)), "42");
        assert_eq!(value_to_string(&json!(true)), "true");
    }

    #[test]
    fn test_from_env_all_with_env_vars() {
        std::env::set_var("CCB_API_URL", "https://api.ccb.com");
        std::env::set_var("CCB_MERCHANT_ID", "merchant123");
        std::env::set_var("CCB_POS_ID", "pos456");
        std::env::set_var("CCB_SIGN_KEY", "signkey");
        let ccb = HttpBankConfig::from_env_ccb().unwrap();
        assert_eq!(ccb.api_url, "https://api.ccb.com");
        assert_eq!(ccb.merchant_id, "merchant123/pos456");
        assert_eq!(ccb.sign_key, "signkey");
        assert_eq!(ccb.timeout_secs, 30);
        std::env::remove_var("CCB_API_URL");
        std::env::remove_var("CCB_MERCHANT_ID");
        std::env::remove_var("CCB_POS_ID");
        std::env::remove_var("CCB_SIGN_KEY");

        std::env::set_var("ICBC_API_URL", "https://api.icbc.com");
        std::env::set_var("ICBC_MER_ID", "mer123");
        std::env::set_var("ICBC_SIGN_KEY", "icbc_key");
        let icbc = HttpBankConfig::from_env_icbc().unwrap();
        assert_eq!(icbc.api_url, "https://api.icbc.com");
        assert_eq!(icbc.merchant_id, "mer123");
        assert_eq!(icbc.sign_key, "icbc_key");
        assert_eq!(icbc.timeout_secs, 30);
        std::env::remove_var("ICBC_API_URL");
        std::env::remove_var("ICBC_MER_ID");
        std::env::remove_var("ICBC_SIGN_KEY");

        std::env::set_var("FUIOU_API_URL", "https://api.fuiou.com");
        std::env::set_var("FUIOU_MCHNT_CD", "mchnt123");
        std::env::set_var("FUIOU_KEY", "fuiou_key");
        let fuiou = HttpBankConfig::from_env_fuiou().unwrap();
        assert_eq!(fuiou.api_url, "https://api.fuiou.com");
        assert_eq!(fuiou.merchant_id, "mchnt123");
        assert_eq!(fuiou.sign_key, "fuiou_key");
        assert_eq!(fuiou.timeout_secs, 30);
        std::env::remove_var("FUIOU_API_URL");
        std::env::remove_var("FUIOU_MCHNT_CD");
        std::env::remove_var("FUIOU_KEY");

        std::env::set_var("FUIOU_API_URL", "https://api.fuiou.com");
        std::env::set_var("FUIOU_MCHNT_CD", "mchnt123");
        std::env::remove_var("FUIOU_KEY");
        assert!(HttpBankConfig::from_env_fuiou().is_none());
        std::env::remove_var("FUIOU_API_URL");
        std::env::remove_var("FUIOU_MCHNT_CD");
    }

    #[test]
    fn test_http_bank_client_new_and_accessors() {
        let config = HttpBankConfig {
            api_url: "https://api.example.com".to_string(),
            merchant_id: "m1".to_string(),
            sign_key: "k1".to_string(),
            timeout_secs: 10,
        };
        let client = HttpBankClient::new(config);
        assert_eq!(client.config().api_url, "https://api.example.com");
        assert_eq!(client.config().merchant_id, "m1");
        assert_eq!(client.config().timeout_secs, 10);
        let _ = client.http();
    }

    #[test]
    fn test_md5_sign_empty_sign_key() {
        let mut params = serde_json::Map::new();
        params.insert("a".to_string(), Value::String("1".to_string()));
        let sign = md5_sign(&params, "");
        assert_eq!(sign.len(), 32);
    }

    #[test]
    fn test_is_empty_value_branches_via_md5_sign() {
        let mut params = serde_json::Map::new();
        params.insert("null_val".to_string(), Value::Null);
        params.insert("empty_arr".to_string(), Value::Array(vec![]));
        params.insert(
            "empty_obj".to_string(),
            Value::Object(serde_json::Map::new()),
        );
        params.insert("num".to_string(), json!(42));
        params.insert("bool".to_string(), json!(true));
        let sign = md5_sign(&params, "key");
        assert_eq!(sign.len(), 32);
    }

    #[test]
    fn test_value_to_string_all_branches() {
        assert_eq!(value_to_string(&json!(false)), "false");
        assert_eq!(value_to_string(&json!([1, 2])), "[1,2]");
        assert_eq!(value_to_string(&json!({"a": 1})), "{\"a\":1}");
    }

    #[tokio::test]
    async fn test_post_json_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let body = r#"{"result":"ok"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        let config = HttpBankConfig {
            api_url: format!("http://127.0.0.1:{port}"),
            merchant_id: "m".to_string(),
            sign_key: "k".to_string(),
            timeout_secs: 5,
        };
        let client = HttpBankClient::new(config);
        let result = client.post_json("", &json!({"data": "test"})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["result"], "ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_post_json_http_error_status() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        let config = HttpBankConfig {
            api_url: format!("http://127.0.0.1:{port}"),
            merchant_id: "m".to_string(),
            sign_key: "k".to_string(),
            timeout_secs: 5,
        };
        let client = HttpBankClient::new(config);
        let result = client.post_json("", &json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP 状态码"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_post_json_with_path_and_trailing_slash() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let body = r#"{"ok":true}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        let config = HttpBankConfig {
            api_url: format!("http://127.0.0.1:{port}/"),
            merchant_id: "m".to_string(),
            sign_key: "k".to_string(),
            timeout_secs: 5,
        };
        let client = HttpBankClient::new(config);
        let result = client.post_json("pay", &json!({})).await;
        assert!(result.is_ok());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_post_xml_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let body = r#"<?xml version="1.0"?><root><code>000000</root></root>"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        let config = HttpBankConfig {
            api_url: format!("http://127.0.0.1:{port}"),
            merchant_id: "m".to_string(),
            sign_key: "k".to_string(),
            timeout_secs: 5,
        };
        let client = HttpBankClient::new(config);
        let result = client.post_xml("", "<request/>").await;
        assert!(result.is_ok());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_post_xml_http_error_status() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        let config = HttpBankConfig {
            api_url: format!("http://127.0.0.1:{port}"),
            merchant_id: "m".to_string(),
            sign_key: "k".to_string(),
            timeout_secs: 5,
        };
        let client = HttpBankClient::new(config);
        let result = client.post_xml("", "<request/>").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("HTTP 状态码"));
        server.await.unwrap();
    }

    #[test]
    fn test_parse_simple_xml_empty_and_no_match() {
        let result = parse_simple_xml("").unwrap();
        assert!(result.as_object().unwrap().is_empty());
        let result2 = parse_simple_xml("<?xml version=\"1.0\"?>").unwrap();
        assert!(result2.as_object().unwrap().is_empty());
    }
}
