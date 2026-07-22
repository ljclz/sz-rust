//! 请求模块 — postData/getData/file/upload
//!
//! 对齐 PHP `$this->request->post()` / `$this->request->get()` / `$this->request->param()`。
//! 强制 POST，不使用 GET 分支（遵循项目规范）。
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | 行为 | Rust 等价 |
//! |---------|------|-----------|
//! | `$this->request->param()` | 合并 POST + GET + route 参数 | [`fetch_post_data`]（合并 body + query） |
//! | `$this->request->post()` | 仅 POST body | [`fetch_body_data`] |
//! | `$this->request->get()` | 仅 GET query | [`fetch_query_data`] |
//! | `postData($key)` | `param($key.'/a')`（强制数组） | [`fetch_post_data_by_key`] |
//! | `getData($key)` | `get($key)` | [`fetch_query_data_by_key`] |
//!
//! ## 注意
//!
//! - `param()` 在 PHP 中还会合并 route 参数（`{id}` 等），在 axum 中这些由 handler 参数捕获，
//!   所以本模块仅合并 body + query。
//! - `/a` 强制数组在 Rust 中不适用（强类型语言），调用方需自行处理类型转换。
//! - 本模块提供低层数据获取；上层控制器在 Phase 2 实现 `postData()` 方法时封装。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::request::fetch_post_data;
//! use axum::http::Request;
//! use axum::body::Body;
//!
//! async fn handler(req: Request<Body>) {
//!     // 合并 body + query
//!     let data = fetch_post_data(req).await.unwrap();
//!     println!("{}", data);
//! }
//! ```

use std::collections::HashMap;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{Map, Value};

/// 从请求中获取合并参数（body + query）
///
/// 对齐 PHP `$this->request->param()`：合并 POST body 和 GET query，
/// body 中的字段优先级高于 query。
///
/// ## 参数
///
/// - `req`：axum::http::Request<Body>
///
/// ## 返回
///
/// - `Ok(Value::Object)`：合并后的 JSON Object
/// - `Err(String)`：body 不可读或 JSON 解析失败
///
/// ## 行为
///
/// 1. 解析 query string 为 `Value::Object`（每个键值对均为字符串）
/// 2. 读取 body bytes
/// 3. 如果 Content-Type 为 `application/json`，解析 body 为 JSON 并合并到 query
/// 4. 如果 Content-Type 为 `application/x-www-form-urlencoded`，解析为表单并合并
/// 5. body 字段覆盖 query 字段
pub async fn fetch_post_data(req: Request<Body>) -> Result<Value, String> {
    // 1. 解析 query
    let query_map = parse_query(req.uri().query().unwrap_or(""));

    // 2. 读取 body
    let (parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .map_err(|e| format!("read body failed: {e}"))?
        .to_bytes();

    // 3. 根据 Content-Type 解析 body
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let mut result = Map::new();

    // query 先写入（低优先级）
    for (k, v) in query_map {
        result.insert(k, Value::String(v));
    }

    // body 后写入（高优先级，覆盖 query）
    if !bytes.is_empty() {
        if content_type.contains("application/json") {
            let body_value: Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON body: {e}"))?;
            if let Value::Object(body_map) = body_value {
                for (k, v) in body_map {
                    result.insert(k, v);
                }
            } else {
                // 非 object 的 body，整体作为 "data" 字段
                result.insert("data".to_string(), body_value);
            }
        } else if content_type.contains("application/x-www-form-urlencoded") {
            let body_str = String::from_utf8_lossy(&bytes);
            let body_map = parse_query(&body_str);
            for (k, v) in body_map {
                result.insert(k, Value::String(v));
            }
        } else {
            // 未知 Content-Type：尝试当 JSON 解析，失败则作为 raw 字符串
            if let Ok(body_value) = serde_json::from_slice::<Value>(&bytes) {
                if let Value::Object(body_map) = body_value {
                    for (k, v) in body_map {
                        result.insert(k, v);
                    }
                } else {
                    result.insert("data".to_string(), body_value);
                }
            } else {
                // 当作 raw 字符串
                let raw = String::from_utf8_lossy(&bytes).to_string();
                if !raw.is_empty() {
                    result.insert("data".to_string(), Value::String(raw));
                }
            }
        }
    }

    Ok(Value::Object(result))
}

/// 从请求中获取单个合并参数（body + query）
///
/// 对齐 PHP `postData($key)`：返回单个字段的值。
pub async fn fetch_post_data_by_key(
    req: Request<Body>,
    key: &str,
) -> Result<Option<Value>, String> {
    let data = fetch_post_data(req).await?;
    Ok(data.get(key).cloned())
}

/// 从请求中仅获取 body 参数（不含 query）
///
/// 对齐 PHP `$this->request->post()`。
pub async fn fetch_body_data(req: Request<Body>) -> Result<Value, String> {
    let (parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .map_err(|e| format!("read body failed: {e}"))?
        .to_bytes();

    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let mut result = Map::new();

    if bytes.is_empty() {
        return Ok(Value::Object(result));
    }

    if content_type.contains("application/json") {
        let body_value: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON body: {e}"))?;
        if let Value::Object(body_map) = body_value {
            for (k, v) in body_map {
                result.insert(k, v);
            }
        } else {
            result.insert("data".to_string(), body_value);
        }
    } else if content_type.contains("application/x-www-form-urlencoded") {
        let body_str = String::from_utf8_lossy(&bytes);
        let body_map = parse_query(&body_str);
        for (k, v) in body_map {
            result.insert(k, Value::String(v));
        }
    } else if let Ok(body_value) = serde_json::from_slice::<Value>(&bytes) {
        if let Value::Object(body_map) = body_value {
            for (k, v) in body_map {
                result.insert(k, v);
            }
        } else {
            result.insert("data".to_string(), body_value);
        }
    } else {
        let raw = String::from_utf8_lossy(&bytes).to_string();
        result.insert("data".to_string(), Value::String(raw));
    }

    Ok(Value::Object(result))
}

/// 从请求中仅获取 query 参数
///
/// 对齐 PHP `$this->request->get()` / `getData()`。
pub fn fetch_query_data(req: &Request<Body>) -> Value {
    let query_map = parse_query(req.uri().query().unwrap_or(""));
    let mut result = Map::new();
    for (k, v) in query_map {
        result.insert(k, Value::String(v));
    }
    Value::Object(result)
}

/// 从请求中仅获取 query 参数的单个字段
///
/// 对齐 PHP `getData($key)`。
pub fn fetch_query_data_by_key(req: &Request<Body>, key: &str) -> Option<Value> {
    let query_map = parse_query(req.uri().query().unwrap_or(""));
    query_map.get(key).map(|v| Value::String(v.clone()))
}

/// 解析 query string 为 `HashMap<String, String>`
///
/// 支持 `key=value&key2=value2` 格式，URL 解码。
pub fn parse_query(query: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if query.is_empty() {
        return result;
    }

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut split = pair.splitn(2, '=');
        let key = url_decode(split.next().unwrap_or(""));
        let value = url_decode(split.next().unwrap_or(""));
        result.insert(key, value);
    }

    result
}

/// 简单 URL 解码（支持 %XX 与 + → space）
pub fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let h1 = chars.next();
                let h2 = chars.next();
                if let (Some(a), Some(b)) = (h1, h2) {
                    if let Ok(byte) = u8::from_str_radix(&format!("{a}{b}"), 16) {
                        result.push(byte as char);
                    } else {
                        result.push('%');
                        result.push(a);
                        result.push(b);
                    }
                } else {
                    result.push('%');
                }
            }
            _ => result.push(c),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Method, Request, StatusCode};

    fn make_json_request(body: &str, query: Option<&str>) -> Request<Body> {
        let uri = match query {
            Some(q) => format!("/?{q}"),
            None => "/".to_string(),
        };
        Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn make_form_request(body: &str, query: Option<&str>) -> Request<Body> {
        let uri = match query {
            Some(q) => format!("/?{q}"),
            None => "/".to_string(),
        };
        Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // ====================================================================
    // parse_query / url_decode 单元测试
    // ====================================================================

    #[test]
    fn test_parse_query_empty() {
        let m = parse_query("");
        assert!(m.is_empty());
    }

    #[test]
    fn test_parse_query_single_pair() {
        let m = parse_query("key=value");
        assert_eq!(m.get("key"), Some(&"value".to_string()));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn test_parse_query_multiple_pairs() {
        let m = parse_query("a=1&b=2&c=3");
        assert_eq!(m.get("a"), Some(&"1".to_string()));
        assert_eq!(m.get("b"), Some(&"2".to_string()));
        assert_eq!(m.get("c"), Some(&"3".to_string()));
    }

    #[test]
    fn test_parse_query_no_value() {
        let m = parse_query("key");
        assert_eq!(m.get("key"), Some(&"".to_string()));
    }

    #[test]
    fn test_parse_query_url_encoded() {
        let m = parse_query("name=hello%20world&email=a%40b.com");
        assert_eq!(m.get("name"), Some(&"hello world".to_string()));
        assert_eq!(m.get("email"), Some(&"a@b.com".to_string()));
    }

    #[test]
    fn test_parse_query_plus_for_space() {
        let m = parse_query("q=hello+world");
        assert_eq!(m.get("q"), Some(&"hello world".to_string()));
    }

    #[test]
    fn test_parse_query_skip_empty_pairs() {
        let m = parse_query("a=1&&b=2&");
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("a"), Some(&"1".to_string()));
        assert_eq!(m.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn test_url_decode_basic() {
        assert_eq!(url_decode("hello"), "hello");
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("a%40b"), "a@b");
        assert_eq!(url_decode("a+b"), "a b");
    }

    #[test]
    fn test_url_decode_trailing_percent() {
        // 末尾单独的 % 应当保留
        assert_eq!(url_decode("100%"), "100%");
    }

    // ====================================================================
    // fetch_post_data 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_fetch_post_data_json_body_only() {
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["name"], "alice");
        assert_eq!(data["age"], 30);
    }

    #[tokio::test]
    async fn test_fetch_post_data_query_only() {
        let req = make_json_request("", Some("page=1&size=10"));
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["page"], "1");
        assert_eq!(data["size"], "10");
    }

    #[tokio::test]
    async fn test_fetch_post_data_body_overrides_query() {
        // body 与 query 同 key 时，body 优先
        let req = make_json_request(r#"{"page":99}"#, Some("page=1&size=10"));
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["page"], 99); // 来自 body
        assert_eq!(data["size"], "10"); // 来自 query
    }

    #[tokio::test]
    async fn test_fetch_post_data_form_urlencoded() {
        let req = make_form_request("name=bob&age=25", None);
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["name"], "bob");
        assert_eq!(data["age"], "25");
    }

    #[tokio::test]
    async fn test_fetch_post_data_empty_body() {
        let req = make_json_request("", None);
        let data = fetch_post_data(req).await.unwrap();
        assert!(data.as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_fetch_post_data_invalid_json() {
        let req = make_json_request("{invalid}", None);
        let result = fetch_post_data(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fetch_post_data_by_key() {
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let name = fetch_post_data_by_key(req, "name").await.unwrap();
        assert_eq!(name, Some(Value::String("alice".to_string())));
    }

    #[tokio::test]
    async fn test_fetch_post_data_by_key_missing() {
        let req = make_json_request(r#"{"name":"alice"}"#, None);
        let age = fetch_post_data_by_key(req, "age").await.unwrap();
        assert_eq!(age, None);
    }

    #[tokio::test]
    async fn test_fetch_post_data_array_value_in_body() {
        let req = make_json_request(r#"{"ids":[1,2,3]}"#, None);
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["ids"], serde_json::json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn test_fetch_post_data_nested_object_in_body() {
        let req = make_json_request(r#"{"user":{"name":"alice","age":30}}"#, None);
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["user"]["name"], "alice");
        assert_eq!(data["user"]["age"], 30);
    }

    // ====================================================================
    // fetch_body_data 单元测试
    // ====================================================================

    #[tokio::test]
    async fn test_fetch_body_data_json() {
        let req = make_json_request(r#"{"name":"alice"}"#, Some("ignored=1"));
        let data = fetch_body_data(req).await.unwrap();
        assert_eq!(data["name"], "alice");
        // query 应当被忽略
        assert!(data.get("ignored").is_none());
    }

    #[tokio::test]
    async fn test_fetch_body_data_empty() {
        let req = make_json_request("", None);
        let data = fetch_body_data(req).await.unwrap();
        assert!(data.as_object().unwrap().is_empty());
    }

    // ====================================================================
    // fetch_query_data 单元测试
    // ====================================================================

    #[test]
    fn test_fetch_query_data_basic() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/?page=1&size=10")
            .body(Body::empty())
            .unwrap();
        let data = fetch_query_data(&req);
        assert_eq!(data["page"], "1");
        assert_eq!(data["size"], "10");
    }

    #[test]
    fn test_fetch_query_data_no_query() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let data = fetch_query_data(&req);
        assert!(data.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_fetch_query_data_by_key_found() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/?page=1&size=10")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            fetch_query_data_by_key(&req, "page"),
            Some(Value::String("1".to_string()))
        );
        assert_eq!(
            fetch_query_data_by_key(&req, "size"),
            Some(Value::String("10".to_string()))
        );
    }

    #[test]
    fn test_fetch_query_data_by_key_not_found() {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/?page=1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(fetch_query_data_by_key(&req, "missing"), None);
    }

    // ====================================================================
    // 集成测试：完整请求流程
    // ====================================================================

    #[tokio::test]
    async fn test_post_data_via_axum_handler() {
        use axum::routing::post;
        use tower::ServiceExt;

        async fn handler(req: Request<Body>) -> (StatusCode, String) {
            let data = fetch_post_data(req).await.unwrap();
            let name = data["name"].as_str().unwrap_or("unknown");
            let age = data["age"].as_i64().unwrap_or(0);
            (StatusCode::OK, format!("{name} is {age}"))
        }

        let router = axum::Router::new().route("/", post(handler));

        let req = Request::builder()
            .method(Method::POST)
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"alice","age":30}"#))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        use http_body_util::BodyExt;
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"alice is 30");
    }

    // ====================================================================
    // PHP 一致性测试（R5: PHP/Rust 行为对比）
    //
    // 对齐 PHP `SzController::postData($key = null)` 与 `getData($key = null)`：
    // - `postData()` 调用 `$this->request->param('')`，合并 route + GET + POST，
    //   POST 优先级最高；ThinkPHP 8 自动解析 application/json body 到 POST 参数
    // - `postData($key)` 调用 `param($key.'/a')`，返回单字段值（强制数组类型，
    //   Rust 中用 `Option<Value>` 代替，调用方自行处理类型）
    // - `getData()` 调用 `$this->request->get('')`，仅返回 query 参数
    // - `getData($key)` 调用 `get($key)`，返回单字段值
    // ====================================================================

    #[tokio::test]
    async fn test_php_consistency_post_data_merges_body_and_query() {
        // PHP `postData()`：body 优先级 > query（对齐 ThinkPHP `param()` 合并顺序）
        // 场景：body 含 page=99，query 含 page=1&size=10
        // 预期：page 来自 body（99），size 来自 query（"10"）
        let req = make_json_request(r#"{"page":99}"#, Some("page=1&size=10"));
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["page"], 99, "body 应覆盖 query 同名字段");
        assert_eq!(data["size"], "10", "query 字段应保留");
    }

    #[tokio::test]
    async fn test_php_consistency_post_data_form_urlencoded_body() {
        // PHP `postData()`：application/x-www-form-urlencoded body 也应被解析
        // 场景：form-urlencoded body 含 name=bob&age=25
        // 预期：name="bob", age="25"（form 字段值类型为字符串）
        let req = make_form_request("name=bob&age=25", None);
        let data = fetch_post_data(req).await.unwrap();
        assert_eq!(data["name"], "bob");
        assert_eq!(data["age"], "25");
    }

    #[tokio::test]
    async fn test_php_consistency_post_data_by_key_returns_value() {
        // PHP `postData($key)`：返回单字段值（对齐 `param($key.'/a')`）
        // 场景：body 含 name=alice&age=30，取 name
        // 预期：Some("alice")
        let req = make_json_request(r#"{"name":"alice","age":30}"#, None);
        let name = fetch_post_data_by_key(req, "name").await.unwrap();
        assert_eq!(name, Some(Value::String("alice".to_string())));
    }

    #[test]
    fn test_php_consistency_get_data_returns_only_query() {
        // PHP `getData()`：仅返回 query 参数（对齐 `$this->request->get('')`）
        // 场景：GET 请求含 page=1&size=10
        // 预期：返回 {page:"1", size:"10"}，不包含 body
        let req = Request::builder()
            .method(Method::GET)
            .uri("/?page=1&size=10")
            .body(Body::empty())
            .unwrap();
        let data = fetch_query_data(&req);
        assert_eq!(data["page"], "1");
        assert_eq!(data["size"], "10");
        // 不应有 body 字段
        assert!(data.get("body").is_none());
    }

    #[test]
    fn test_php_consistency_get_data_by_key_returns_query_value() {
        // PHP `getData($key)`：返回 query 中单字段值（对齐 `get($key)`）
        // 场景：GET 请求含 page=1&size=10
        // 预期：page=Some("1"), missing=None
        let req = Request::builder()
            .method(Method::GET)
            .uri("/?page=1&size=10")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            fetch_query_data_by_key(&req, "page"),
            Some(Value::String("1".to_string()))
        );
        assert_eq!(fetch_query_data_by_key(&req, "missing"), None);
    }
}
