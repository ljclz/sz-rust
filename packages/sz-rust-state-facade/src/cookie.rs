// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Cookie 模块 — 对齐 PHP `think\Cookie`
//!
//! 本模块实现 Cookie 管理，对齐 PHP `think\Cookie` 的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Cookie::set($name, $value, $option)` | [`CookieJar::set`] | 设置 cookie（暂存到 jar，等 save 时统一发送） |
//! | `Cookie::get($name, $default)` | [`CookieJar::get`] / [`CookieJar::get_with_default`] | 从 Request 读取 cookie |
//! | `Cookie::has($name)` | [`CookieJar::has`] | 检查 Request 中是否存在 cookie |
//! | `Cookie::delete($name, $options)` | [`CookieJar::delete`] | 删除 cookie（设置过期时间为过去） |
//! | `Cookie::forever($name, $value, $option)` | [`CookieJar::forever`] | 永久保存（10 年） |
//! | `Cookie::save()` | [`CookieJar::apply_to_response`] | 将所有暂存的 cookie 写入 Response 的 Set-Cookie 头 |
//!
//! ### PHP 行为对齐
//!
//! - **延迟发送**：PHP `set()` 仅暂存到 `$this->cookie` 数组，`save()` 时统一发送
//!   `Set-Cookie` 头。Rust 通过 [`CookieJar::set`] 暂存到内部 Vec，
//!   [`CookieJar::apply_to_response`] 时统一写入 Response。
//! - **配置覆盖**：PHP `set()` 的 `$option` 参数会覆盖默认配置。Rust 通过
//!   [`CookieOptions`] 实现相同行为。
//! - **永久 Cookie**：PHP `forever()` 设置 315360000 秒（10 年）过期。
//!   Rust 使用相同常量。
//! - **删除 Cookie**：PHP `delete()` 通过设置过期时间为 `time() - 3600` 实现。
//!   Rust 使用相同策略（expire 设为 -3600）。
//!
//! ## 架构说明
//!
//! 本模块不依赖外部 `cookie` crate，直接基于 `axum::http::HeaderMap` 操作
//! `Cookie` 和 `Set-Cookie` 头，保持依赖最小化。
//!
//! ### Cookie 头格式（RFC 6265）
//!
//! - **请求头**：`Cookie: name1=value1; name2=value2`
//! - **响应头**：`Set-Cookie: name=value; Expires=...; Path=/; Domain=...; Secure; HttpOnly; SameSite=Lax`

use axum::http::{HeaderName, HeaderValue, Request, Response};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// ============================================================================
// 默认配置常量（对齐 PHP `think\Cookie::$config` 默认值）
// ============================================================================

/// 永久 Cookie 过期时间（秒）— 对齐 PHP `Cookie::forever()` 的 315360000
const FOREVER_EXPIRE_SECONDS: i64 = 315360000;

/// 删除 Cookie 时使用的过期时间偏移（秒）— 对齐 PHP `delete()` 的 `time() - 3600`
const DELETE_EXPIRE_OFFSET_SECONDS: i64 = -3600;

// ============================================================================
// Cookie 选项（对齐 PHP `think\Cookie::$config`）
// ============================================================================

/// Cookie 配置选项（对齐 PHP `think\Cookie::$config`）
///
/// 对齐 PHP 默认配置：
/// ```php
/// protected $config = [
///     'expire'   => 0,
///     'path'     => '/',
///     'domain'   => '',
///     'secure'   => false,
///     'httponly' => false,
///     'samesite' => '',
/// ];
/// ```
#[derive(Debug, Clone)]
pub struct CookieOptions {
    /// 过期时间（秒）。0 表示会话 cookie（浏览器关闭时删除）。
    /// 对齐 PHP `'expire' => 0`。
    pub expire: i64,
    /// Cookie 有效路径。对齐 PHP `'path' => '/'`。
    pub path: String,
    /// Cookie 有效域名。对齐 PHP `'domain' => ''`。
    pub domain: String,
    /// 是否仅通过 HTTPS 传输。对齐 PHP `'secure' => false`。
    pub secure: bool,
    /// 是否仅通过 HTTP 访问（JS 不可读）。对齐 PHP `'httponly' => false`。
    pub httponly: bool,
    /// SameSite 策略（`Strict` / `Lax` / `None` / 空）。对齐 PHP `'samesite' => ''`。
    pub samesite: String,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            expire: 0,
            path: "/".to_string(),
            domain: String::new(),
            secure: false,
            httponly: false,
            samesite: String::new(),
        }
    }
}

impl CookieOptions {
    /// 创建一个指定过期时间的选项（其他字段使用默认值）
    ///
    /// 对齐 PHP `set($name, $value, $option)` 中 `$option` 为数字时
    /// 转换为 `['expire' => $option]` 的行为。
    pub fn with_expire(expire: i64) -> Self {
        Self {
            expire,
            ..Default::default()
        }
    }
}

// ============================================================================
// Cookie 条目（暂存的 Set-Cookie 数据）
// ============================================================================

/// 单个 Cookie 条目（暂存在 [`CookieJar`] 中，等待 `apply_to_response` 发送）
///
/// 对齐 PHP `think\Cookie::setCookie()` 暂存的 `[$value, $expire, $option]` 元组。
#[derive(Debug, Clone)]
pub struct CookieEntry {
    /// Cookie 名称
    pub name: String,
    /// Cookie 值
    pub value: String,
    /// 过期时间戳（Unix 秒）。0 表示会话 cookie。
    pub expire: i64,
    /// 其他配置选项
    pub options: CookieOptions,
}

impl CookieEntry {
    /// 将 Cookie 条目转换为 `Set-Cookie` 头值字符串
    ///
    /// 格式（RFC 6265）：
    /// ```text
    /// name=value; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Path=/; Domain=example.com; Secure; HttpOnly; SameSite=Lax
    /// ```
    ///
    /// 对齐 PHP `think\Cookie::saveCookie()` 的 `setcookie()` 调用。
    pub fn to_header_string(&self) -> String {
        let mut parts = vec![format!("{}={}", self.name, self.value)];

        // Expires 头（仅当 expire > 0 时添加，对齐 PHP 仅在 expire 非零时发送）
        if self.expire > 0 {
            let expire_dt =
                DateTime::<Utc>::from_timestamp(self.expire, 0).unwrap_or_else(Utc::now);
            // 格式：Wed, 21 Oct 2026 07:28:00 GMT（RFC 7231 IMF-fixdate）
            parts.push(format!(
                "Expires={}",
                expire_dt.format("%a, %d %b %Y %H:%M:%S GMT")
            ));
        }

        // Path（对齐 PHP `'path' => '/'`）
        if !self.options.path.is_empty() {
            parts.push(format!("Path={}", self.options.path));
        }

        // Domain（对齐 PHP `'domain' => ''`）
        if !self.options.domain.is_empty() {
            parts.push(format!("Domain={}", self.options.domain));
        }

        // Secure（对齐 PHP `'secure' => false`）
        if self.options.secure {
            parts.push("Secure".to_string());
        }

        // HttpOnly（对齐 PHP `'httponly' => false`）
        if self.options.httponly {
            parts.push("HttpOnly".to_string());
        }

        // SameSite（对齐 PHP `'samesite' => ''`）
        if !self.options.samesite.is_empty() {
            parts.push(format!("SameSite={}", self.options.samesite));
        }

        parts.join("; ")
    }
}

// ============================================================================
// CookieJar — Cookie 管理器（对齐 PHP `think\Cookie` 类）
// ============================================================================

/// Cookie 管理器（对齐 PHP `think\Cookie`）
///
/// 同时管理两类数据：
/// 1. **请求 Cookie**（从 `Request` 的 `Cookie` 头解析得到，只读）
/// 2. **响应 Cookie**（通过 `set()` 暂存，`apply_to_response()` 时发送）
///
/// # 用法
///
/// ```ignore
/// use sz_rust_state_facade::cookie::{CookieJar, CookieOptions};
/// use axum::http::Request;
/// use axum::body::Body;
///
/// // 1. 从 Request 创建 CookieJar
/// let req = Request::<Body>::default();
/// let jar = CookieJar::from_request(&req);
///
/// // 2. 读取请求 Cookie
/// if let Some(value) = jar.get("session_id") {
///     println!("session_id = {}", value);
/// }
///
/// // 3. 设置响应 Cookie
/// let jar = CookieJar::from_request(&req)
///     .set("token", "abc123", CookieOptions::default());
///
/// // 4. 应用到 Response
/// let mut resp = Response::new(Body::empty());
/// jar.apply_to_response(&mut resp);
/// ```
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    /// 请求 Cookie（从 Request 解析，只读）
    request_cookies: HashMap<String, String>,
    /// 待发送的响应 Cookie（通过 `set()` 暂存）
    response_cookies: Vec<CookieEntry>,
    /// 默认配置（对齐 PHP `think\Cookie::$config`）
    config: CookieOptions,
}

impl CookieJar {
    /// 创建空的 CookieJar（使用默认配置）
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建指定默认配置的 CookieJar
    ///
    /// 对齐 PHP `new Cookie($request, $config)` 构造函数。
    pub fn with_config(config: CookieOptions) -> Self {
        Self {
            request_cookies: HashMap::new(),
            response_cookies: Vec::new(),
            config,
        }
    }

    /// 从 Request 的 Cookie 头解析创建 CookieJar
    ///
    /// 对齐 PHP `Cookie::__construct(Request $request)` 通过 `$this->request->cookie()` 读取。
    ///
    /// # 解析格式
    ///
    /// `Cookie: name1=value1; name2=value2`
    pub fn from_request<B>(req: &Request<B>) -> Self {
        let mut jar = Self::default();
        if let Some(cookie_header) = req.headers().get(axum::http::header::COOKIE) {
            if let Ok(header_str) = cookie_header.to_str() {
                jar.request_cookies = parse_cookie_header(header_str);
            }
        }
        jar
    }

    /// 获取请求 Cookie 值（对齐 PHP `Cookie::get($name, $default)`）
    ///
    /// # 返回
    ///
    /// - `Some(value)`：Cookie 存在
    /// - `None`：Cookie 不存在
    pub fn get(&self, name: &str) -> Option<String> {
        self.request_cookies.get(name).cloned()
    }

    /// 获取请求 Cookie 值，不存在时返回默认值
    ///
    /// 对齐 PHP `Cookie::get($name, $default)` 的 `$default` 参数。
    pub fn get_with_default(&self, name: &str, default: &str) -> String {
        self.request_cookies
            .get(name)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// 检查请求 Cookie 是否存在（对齐 PHP `Cookie::has($name)`）
    pub fn has(&self, name: &str) -> bool {
        self.request_cookies.contains_key(name)
    }

    /// 设置响应 Cookie（对齐 PHP `Cookie::set($name, $value, $option)`）
    ///
    /// 仅暂存到内部 Vec，需要调用 [`CookieJar::apply_to_response`] 才会真正发送。
    ///
    /// # 参数
    ///
    /// - `name`：Cookie 名称
    /// - `value`：Cookie 值
    /// - `options`：Cookie 选项（覆盖默认配置）
    ///
    /// # 返回
    ///
    /// 返回 `self` 以支持链式调用（Builder 风格）。
    pub fn set(mut self, name: &str, value: &str, options: CookieOptions) -> Self {
        // 计算过期时间戳（对齐 PHP `time() + intval($config['expire'])`）
        let expire = if options.expire > 0 {
            Utc::now().timestamp() + options.expire
        } else {
            0
        };

        self.response_cookies.push(CookieEntry {
            name: name.to_string(),
            value: value.to_string(),
            expire,
            options,
        });
        self
    }

    /// 永久保存 Cookie（对齐 PHP `Cookie::forever($name, $value, $option)`）
    ///
    /// 设置过期时间为 10 年后（315360000 秒）。
    pub fn forever(self, name: &str, value: &str, mut options: CookieOptions) -> Self {
        options.expire = FOREVER_EXPIRE_SECONDS;
        self.set(name, value, options)
    }

    /// 删除 Cookie（对齐 PHP `Cookie::delete($name, $options)`）
    ///
    /// 通过设置过期时间为过去（当前时间 - 3600 秒）实现删除。
    pub fn delete(mut self, name: &str, options: CookieOptions) -> Self {
        let expire = Utc::now().timestamp() + DELETE_EXPIRE_OFFSET_SECONDS;
        self.response_cookies.push(CookieEntry {
            name: name.to_string(),
            value: String::new(),
            expire,
            options,
        });
        self
    }

    /// 将所有暂存的 Cookie 写入 Response 的 Set-Cookie 头
    ///
    /// 对齐 PHP `Cookie::save()` 的批量发送行为。
    pub fn apply_to_response<B>(self, resp: &mut Response<B>) {
        if self.response_cookies.is_empty() {
            return;
        }

        let headers = resp.headers_mut();
        for entry in &self.response_cookies {
            // 每个 Cookie 一个独立的 Set-Cookie 头（RFC 6265 要求）
            if let Ok(value) = HeaderValue::from_str(&entry.to_header_string()) {
                headers.append(HeaderName::from_static("set-cookie"), value);
            }
        }
    }

    /// 获取所有暂存的响应 Cookie（对齐 PHP `Cookie::getCookie()`）
    pub fn get_response_cookies(&self) -> &[CookieEntry] {
        &self.response_cookies
    }

    /// 获取默认配置（不可变引用）
    pub fn config(&self) -> &CookieOptions {
        &self.config
    }
}

// ============================================================================
// Cookie 头解析工具函数
// ============================================================================

/// 解析 Request 的 Cookie 头字符串为 HashMap
///
/// 格式：`name1=value1; name2=value2`
///
/// 对齐 PHP `think\Request::cookie()` 的解析行为：
/// - 按 `;` 分割多个 cookie
/// - 每个条目按第一个 `=` 分割 name 和 value
/// - 自动 trim 空白字符
fn parse_cookie_header(header: &str) -> HashMap<String, String> {
    let mut cookies = HashMap::new();
    for pair in header.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some(eq_pos) = pair.find('=') {
            let name = pair[..eq_pos].trim().to_string();
            let value = pair[eq_pos + 1..].trim().to_string();
            if !name.is_empty() {
                cookies.insert(name, value);
            }
        }
    }
    cookies
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, Response};

    // ------------------------------------------------------------------------
    // CookieOptions 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_cookie_options_default() {
        let opts = CookieOptions::default();
        assert_eq!(opts.expire, 0);
        assert_eq!(opts.path, "/");
        assert_eq!(opts.domain, "");
        assert!(!opts.secure);
        assert!(!opts.httponly);
        assert_eq!(opts.samesite, "");
    }

    #[test]
    fn test_cookie_options_with_expire() {
        let opts = CookieOptions::with_expire(3600);
        assert_eq!(opts.expire, 3600);
        assert_eq!(opts.path, "/"); // 其他字段保持默认
    }

    // ------------------------------------------------------------------------
    // CookieEntry::to_header_string 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_cookie_entry_minimal_header() {
        // 会话 cookie（expire=0），仅 name=value
        let entry = CookieEntry {
            name: "token".to_string(),
            value: "abc123".to_string(),
            expire: 0,
            options: CookieOptions {
                path: String::new(), // 空 path 不输出
                domain: String::new(),
                samesite: String::new(),
                ..Default::default()
            },
        };
        let header = entry.to_header_string();
        assert_eq!(header, "token=abc123");
    }

    #[test]
    fn test_cookie_entry_with_path() {
        let entry = CookieEntry {
            name: "token".to_string(),
            value: "abc".to_string(),
            expire: 0,
            options: CookieOptions {
                path: "/api".to_string(),
                ..Default::default()
            },
        };
        let header = entry.to_header_string();
        assert!(header.contains("token=abc"));
        assert!(header.contains("Path=/api"));
    }

    #[test]
    fn test_cookie_entry_with_all_attributes() {
        let entry = CookieEntry {
            name: "session".to_string(),
            value: "xyz".to_string(),
            expire: 1893456000, // 2030-01-01
            options: CookieOptions {
                path: "/".to_string(),
                domain: "example.com".to_string(),
                secure: true,
                httponly: true,
                samesite: "Lax".to_string(),
                ..Default::default()
            },
        };
        let header = entry.to_header_string();
        assert!(header.contains("session=xyz"));
        assert!(header.contains("Expires="));
        assert!(header.contains("Path=/"));
        assert!(header.contains("Domain=example.com"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
    }

    #[test]
    fn test_cookie_entry_expire_zero_no_expires_header() {
        // expire=0 表示会话 cookie，不应输出 Expires 头
        let entry = CookieEntry {
            name: "session".to_string(),
            value: "v".to_string(),
            expire: 0,
            options: CookieOptions::default(),
        };
        let header = entry.to_header_string();
        assert!(!header.contains("Expires="));
    }

    // ------------------------------------------------------------------------
    // CookieJar 基本 API 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_cookie_jar_default_empty() {
        let jar = CookieJar::new();
        assert!(jar.get("any").is_none());
        assert!(!jar.has("any"));
        assert!(jar.get_response_cookies().is_empty());
    }

    #[test]
    fn test_cookie_jar_with_config() {
        let config = CookieOptions {
            path: "/app".to_string(),
            ..Default::default()
        };
        let jar = CookieJar::with_config(config);
        assert_eq!(jar.config().path, "/app");
    }

    #[test]
    fn test_cookie_jar_set_adds_to_response_cookies() {
        let jar = CookieJar::new().set("token", "abc", CookieOptions::default());
        assert_eq!(jar.get_response_cookies().len(), 1);
        assert_eq!(jar.get_response_cookies()[0].name, "token");
        assert_eq!(jar.get_response_cookies()[0].value, "abc");
    }

    #[test]
    fn test_cookie_jar_set_chain() {
        // Builder 风格链式调用
        let jar = CookieJar::new()
            .set("a", "1", CookieOptions::default())
            .set("b", "2", CookieOptions::default())
            .set("c", "3", CookieOptions::default());
        assert_eq!(jar.get_response_cookies().len(), 3);
    }

    #[test]
    fn test_cookie_jar_set_with_expire_calculates_timestamp() {
        let before = Utc::now().timestamp();
        let jar = CookieJar::new().set("token", "abc", CookieOptions::with_expire(3600));
        let after = Utc::now().timestamp();

        let entry = &jar.get_response_cookies()[0];
        // expire 应该在 [before + 3600, after + 3600] 范围内
        assert!(entry.expire >= before + 3600);
        assert!(entry.expire <= after + 3600);
    }

    #[test]
    fn test_cookie_jar_set_expire_zero_keeps_zero() {
        let jar = CookieJar::new().set("token", "abc", CookieOptions::default());
        let entry = &jar.get_response_cookies()[0];
        assert_eq!(entry.expire, 0);
    }

    #[test]
    fn test_cookie_jar_forever_sets_10_year_expire() {
        let before = Utc::now().timestamp();
        let jar = CookieJar::new().forever("token", "abc", CookieOptions::default());

        let entry = &jar.get_response_cookies()[0];
        // 检查 expire ≈ now + 10 years
        let expected_min = before + FOREVER_EXPIRE_SECONDS;
        assert!(entry.expire >= expected_min);
    }

    #[test]
    fn test_cookie_jar_delete_sets_past_expire() {
        let before = Utc::now().timestamp();
        let jar = CookieJar::new().delete("token", CookieOptions::default());

        let entry = &jar.get_response_cookies()[0];
        assert_eq!(entry.value, ""); // 删除 cookie 值为空
                                     // expire 应该是过去时间（before - 3600 附近）
        assert!(entry.expire < before);
    }

    // ------------------------------------------------------------------------
    // CookieJar::from_request 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_from_request_no_cookie_header() {
        let req = Request::<Body>::default();
        let jar = CookieJar::from_request(&req);
        assert!(jar.get("any").is_none());
    }

    #[test]
    fn test_from_request_single_cookie() {
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("token=abc123"),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get("token"), Some("abc123".to_string()));
        assert!(jar.has("token"));
    }

    #[test]
    fn test_from_request_multiple_cookies() {
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("token=abc; user=42; theme=dark"),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get("token"), Some("abc".to_string()));
        assert_eq!(jar.get("user"), Some("42".to_string()));
        assert_eq!(jar.get("theme"), Some("dark".to_string()));
    }

    #[test]
    fn test_from_request_cookie_with_whitespace() {
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("  token = abc  ;  user = 42  "),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get("token"), Some("abc".to_string()));
        assert_eq!(jar.get("user"), Some("42".to_string()));
    }

    #[test]
    fn test_from_request_cookie_value_with_equals() {
        // 值中包含 = 字符（仅按第一个 = 分割）
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("data=a=b=c"),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get("data"), Some("a=b=c".to_string()));
    }

    #[test]
    fn test_from_request_empty_cookie_header() {
        let mut req = Request::<Body>::default();
        req.headers_mut()
            .insert(axum::http::header::COOKIE, HeaderValue::from_static(""));
        let jar = CookieJar::from_request(&req);
        assert!(jar.get("any").is_none());
    }

    #[test]
    fn test_from_request_malformed_pairs_ignored() {
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("token=abc; malformed; =empty_name; valid=ok"),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get("token"), Some("abc".to_string()));
        assert!(jar.get("malformed").is_none()); // 无 = 分隔
        assert!(jar.get("").is_none()); // 空名被忽略
        assert_eq!(jar.get("valid"), Some("ok".to_string()));
    }

    #[test]
    fn test_get_with_default_returns_value_when_exists() {
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("name=alice"),
        );
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get_with_default("name", "guest"), "alice");
    }

    #[test]
    fn test_get_with_default_returns_default_when_missing() {
        let req = Request::<Body>::default();
        let jar = CookieJar::from_request(&req);
        assert_eq!(jar.get_with_default("name", "guest"), "guest");
    }

    // ------------------------------------------------------------------------
    // CookieJar::apply_to_response 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_apply_to_response_no_cookies() {
        let jar = CookieJar::new();
        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);
        assert!(resp.headers().get("set-cookie").is_none());
    }

    #[test]
    fn test_apply_to_response_single_cookie() {
        let jar = CookieJar::new().set("token", "abc", CookieOptions::default());
        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let set_cookies: Vec<_> = resp.headers().get_all("set-cookie").iter().collect();
        assert_eq!(set_cookies.len(), 1);
        assert_eq!(set_cookies[0].to_str().unwrap(), "token=abc; Path=/");
    }

    #[test]
    fn test_apply_to_response_multiple_cookies() {
        let jar = CookieJar::new()
            .set("a", "1", CookieOptions::default())
            .set("b", "2", CookieOptions::default())
            .set("c", "3", CookieOptions::default());
        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let set_cookies: Vec<_> = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(set_cookies.len(), 3);
        assert!(set_cookies.contains(&"a=1; Path=/".to_string()));
        assert!(set_cookies.contains(&"b=2; Path=/".to_string()));
        assert!(set_cookies.contains(&"c=3; Path=/".to_string()));
    }

    #[test]
    fn test_apply_to_response_with_all_attributes() {
        let jar = CookieJar::new().set(
            "session",
            "xyz",
            CookieOptions {
                expire: 1893456000, // 2030-01-01
                path: "/".to_string(),
                domain: "example.com".to_string(),
                secure: true,
                httponly: true,
                samesite: "Strict".to_string(),
            },
        );
        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let header = resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(header.contains("session=xyz"));
        assert!(header.contains("Expires="));
        assert!(header.contains("Path=/"));
        assert!(header.contains("Domain=example.com"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Strict"));
    }

    // ------------------------------------------------------------------------
    // parse_cookie_header 工具函数测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_parse_empty_header() {
        let cookies = parse_cookie_header("");
        assert!(cookies.is_empty());
    }

    #[test]
    fn test_parse_single_pair() {
        let cookies = parse_cookie_header("name=value");
        assert_eq!(cookies.get("name"), Some(&"value".to_string()));
    }

    #[test]
    fn test_parse_multiple_pairs() {
        let cookies = parse_cookie_header("a=1; b=2; c=3");
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies.get("a"), Some(&"1".to_string()));
        assert_eq!(cookies.get("b"), Some(&"2".to_string()));
        assert_eq!(cookies.get("c"), Some(&"3".to_string()));
    }

    #[test]
    fn test_parse_trims_whitespace() {
        let cookies = parse_cookie_header("  a = 1  ;  b = 2  ");
        assert_eq!(cookies.get("a"), Some(&"1".to_string()));
        assert_eq!(cookies.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn test_parse_skips_empty_pairs() {
        let cookies = parse_cookie_header("a=1;; ;b=2");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies.get("a"), Some(&"1".to_string()));
        assert_eq!(cookies.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn test_parse_skips_no_equals() {
        let cookies = parse_cookie_header("a=1; invalid; b=2");
        assert_eq!(cookies.len(), 2);
        assert!(!cookies.contains_key("invalid"));
    }

    #[test]
    fn test_parse_skips_empty_name() {
        let cookies = parse_cookie_header("a=1; =empty; b=2");
        assert_eq!(cookies.len(), 2);
        assert!(!cookies.contains_key(""));
    }

    // ------------------------------------------------------------------------
    // PHP 一致性综合流程测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_php_consistency_set_and_save_flow() {
        // 模拟 PHP 流程：set() 暂存 → save() 发送
        let req = Request::<Body>::default();
        let jar =
            CookieJar::from_request(&req).set("token", "abc123", CookieOptions::with_expire(3600));

        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let header = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(header.starts_with("token=abc123"));
        assert!(header.contains("Expires="));
        assert!(header.contains("Path=/"));
    }

    #[test]
    fn test_php_consistency_delete_flow() {
        // 模拟 PHP delete() 流程
        let req = Request::<Body>::default();
        let jar = CookieJar::from_request(&req).delete("token", CookieOptions::default());

        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let header = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        // 删除 cookie：值空 + Expires 为过去时间
        assert!(header.starts_with("token="));
        assert!(header.contains("Expires="));
    }

    #[test]
    fn test_php_consistency_forever_flow() {
        let req = Request::<Body>::default();
        let jar = CookieJar::from_request(&req).forever("pref", "dark", CookieOptions::default());

        let mut resp = Response::new(Body::empty());
        jar.apply_to_response(&mut resp);

        let header = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(header.contains("pref=dark"));
        assert!(header.contains("Expires="));
    }

    #[test]
    fn test_php_consistency_request_response_isolation() {
        // 请求 cookie 和响应 cookie 是隔离的
        let mut req = Request::<Body>::default();
        req.headers_mut().insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("old=value"),
        );
        let jar = CookieJar::from_request(&req).set("new", "value", CookieOptions::default());

        // 请求 cookie 可读
        assert_eq!(jar.get("old"), Some("value".to_string()));
        // 响应 cookie 不会出现在请求 cookie 中
        assert!(jar.get("new").is_none());
        // 响应 cookie 在 response_cookies 中
        assert_eq!(jar.get_response_cookies().len(), 1);
        assert_eq!(jar.get_response_cookies()[0].name, "new");
    }
}
