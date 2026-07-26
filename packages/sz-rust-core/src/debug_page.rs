//! Whoops-style HTML 调试页面 — 对齐 PHP `whoops` 异常展示
//!
//! ## PHP 对齐
//!
//! ThinkPHP 6 默认集成 `whoops` 库，在开发环境渲染交互式错误页面：
//! - 顶部红色标题栏（错误类型 + 消息）
//! - 文件:行号（灰色路径，可点击打开 IDE）
//! - 堆栈列表（可折叠，每帧显示文件:行号 + 函数名 + 源码片段）
//! - 请求信息（method / uri / headers / query / body）
//! - 环境信息（Rust 版本 / 进程 PID / 工作目录）
//!
//! 生产环境关闭调试页，返回简洁 JSON 或静态 HTML，避免泄露堆栈。
//!
//! ## 安全约束
//!
//! - 所有用户输入（错误消息、文件路径、请求头值）必须 HTML 转义，防止 XSS
//! - 源码片段从文件读取，限制最多 21 行（前后 10 行 + 错误行），避免读取大文件
//! - 调试页只在 `debug_mode = true` 时渲染，生产环境强制返回简洁页

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

// ============================================================================
// 调试错误信息
// ============================================================================

/// 单个堆栈帧
///
/// 对齐 PHP `whoops\Frame`：每帧包含文件、行号、函数名、源码片段。
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// 文件路径（绝对路径）
    pub file: String,
    /// 行号（1-based）
    pub line: usize,
    /// 函数名（如 `app::handler::create_user`）
    pub function: String,
    /// 源码片段（行号 → 源码行），由 [`DebugError::with_source_snippet`] 填充
    pub source_lines: Vec<(usize, String)>,
}

impl StackFrame {
    /// 创建新的堆栈帧（不含源码片段）
    ///
    /// # 参数
    ///
    /// - `file`：文件路径
    /// - `line`：行号
    /// - `function`：函数名
    pub fn new(file: impl Into<String>, line: usize, function: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            function: function.into(),
            source_lines: Vec::new(),
        }
    }

    /// 从文件读取源码片段（错误行前后各 `context` 行，最多 `context * 2 + 1` 行）
    ///
    /// 读取失败（文件不存在/IO 错误）时静默忽略，`source_lines` 保持为空。
    ///
    /// # 参数
    ///
    /// - `context`：错误行前后的上下文行数（建议 10）
    pub fn load_source_snippet(&mut self, context: usize) {
        if self.line == 0 || self.file.is_empty() {
            return;
        }

        let content = match std::fs::read_to_string(&self.file) {
            Ok(c) => c,
            Err(_) => return, // 文件不可读（如内置函数、动态生成代码）
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = self.line.saturating_sub(context).max(1);
        let end = (self.line + context).min(lines.len());

        for (idx, line_content) in lines.iter().enumerate() {
            let line_num = idx + 1;
            if line_num >= start && line_num <= end {
                self.source_lines
                    .push((line_num, line_content.to_string()));
            }
        }
    }
}

/// 调试错误信息
///
/// 包含完整的错误上下文：消息、类型、堆栈、请求信息。
/// 由 [`render_debug_html`] 渲染为 Whoops-style HTML。
#[derive(Debug, Clone)]
pub struct DebugError {
    /// 错误类型名（如 `"panic"` / `"IoError"` / `"SqlError"`）
    pub error_type: String,
    /// 错误消息
    pub message: String,
    /// 错误发生的文件
    pub file: String,
    /// 错误发生的行号（1-based）
    pub line: usize,
    /// 堆栈帧列表（按调用顺序：最新帧在前）
    pub stack: Vec<StackFrame>,
    /// 请求方法
    pub method: String,
    /// 请求 URI
    pub uri: String,
    /// 请求头（已脱敏）
    pub headers: HashMap<String, String>,
    /// 请求查询参数
    pub query_params: HashMap<String, String>,
}

impl DebugError {
    /// 创建新的调试错误
    pub fn new(
        error_type: impl Into<String>,
        message: impl Into<String>,
        file: impl Into<String>,
        line: usize,
    ) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
            file: file.into(),
            line,
            stack: Vec::new(),
            method: String::new(),
            uri: String::new(),
            headers: HashMap::new(),
            query_params: HashMap::new(),
        }
    }

    /// 添加堆栈帧
    pub fn with_frame(mut self, frame: StackFrame) -> Self {
        self.stack.push(frame);
        self
    }

    /// 设置请求信息
    pub fn with_request(
        mut self,
        method: impl Into<String>,
        uri: impl Into<String>,
        headers: HeaderMap,
        query_params: HashMap<String, String>,
    ) -> Self {
        self.method = method.into();
        self.uri = uri.into();
        self.headers = sanitize_headers(&headers);
        self.query_params = query_params;
        self
    }

    /// 为所有堆栈帧加载源码片段（包含错误位置本身）
    ///
    /// # 参数
    ///
    /// - `context`：错误行前后的上下文行数（建议 10）
    pub fn with_source_snippet(mut self, context: usize) -> Self {
        // 为错误位置加载源码
        if !self.file.is_empty() && self.line > 0 {
            let mut main_frame = StackFrame::new(self.file.clone(), self.line, "<main>");
            main_frame.load_source_snippet(context);
            // 主错误信息也存为虚拟帧
            if !main_frame.source_lines.is_empty() {
                self.stack.insert(0, main_frame);
            }
        }

        // 为所有堆栈帧加载源码
        for frame in &mut self.stack {
            if frame.source_lines.is_empty() {
                frame.load_source_snippet(context);
            }
        }
        self
    }
}

// ============================================================================
// 调试页配置
// ============================================================================

/// 调试页配置
///
/// 控制调试页的渲染行为：是否启用、源码上下文行数、是否显示堆栈。
#[derive(Debug, Clone)]
pub struct DebugPageConfig {
    /// 是否启用调试模式（true 渲染 Whoops-style HTML，false 返回简洁错误页）
    pub debug_mode: bool,
    /// 源码上下文行数（错误行前后各 N 行）
    pub source_context: usize,
    /// 是否显示堆栈
    pub show_stack: bool,
    /// 是否显示请求信息
    pub show_request: bool,
    /// 是否显示环境信息
    pub show_environment: bool,
}

impl Default for DebugPageConfig {
    fn default() -> Self {
        Self {
            debug_mode: false,
            source_context: 10,
            show_stack: true,
            show_request: true,
            show_environment: true,
        }
    }
}

impl DebugPageConfig {
    /// 创建开发环境配置（启用所有调试信息）
    pub fn development() -> Self {
        Self {
            debug_mode: true,
            source_context: 10,
            show_stack: true,
            show_request: true,
            show_environment: true,
        }
    }

    /// 创建生产环境配置（关闭所有调试信息，仅显示简洁错误页）
    pub fn production() -> Self {
        Self {
            debug_mode: false,
            source_context: 0,
            show_stack: false,
            show_request: false,
            show_environment: false,
        }
    }
}

// ============================================================================
// HTML 渲染
// ============================================================================

/// 渲染 Whoops-style HTML 调试页
///
/// # 安全约束
///
/// - 所有用户输入经 `html_escape` 转义，防 XSS
/// - HTML 内联 CSS（无外部依赖）
/// - 源码片段限制最多 `context * 2 + 1` 行，避免读取大文件
pub fn render_debug_html(error: &DebugError, config: &DebugPageConfig) -> String {
    let title = html_escape(&format!("{}: {}", error.error_type, error.message));
    let file_display = html_escape(&error.file);
    let method_display = html_escape(&error.method);
    let uri_display = html_escape(&error.uri);
    // 显式绑定避免与内置 `line!` 宏冲突
    let error_line = error.line;

    let mut html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<title>{title}</title>
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #fafafa; color: #333; }}
.header {{ background: #d23f31; color: #fff; padding: 24px 32px; }}
.header h1 {{ font-size: 22px; margin-bottom: 8px; word-break: break-all; }}
.header .location {{ color: rgba(255,255,255,0.85); font-size: 13px; font-family: "Fira Code", monospace; }}
.container {{ max-width: 1200px; margin: 24px auto; padding: 0 24px; }}
.section {{ background: #fff; border-radius: 6px; box-shadow: 0 1px 3px rgba(0,0,0,0.08); margin-bottom: 16px; overflow: hidden; }}
.section-title {{ background: #f5f5f5; padding: 12px 20px; border-bottom: 1px solid #e0e0e0; font-size: 14px; font-weight: 600; color: #555; }}
.section-body {{ padding: 16px 20px; }}
.stack-frame {{ border-bottom: 1px solid #eee; padding: 12px 0; }}
.stack-frame:last-child {{ border-bottom: none; }}
.frame-header {{ display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }}
.frame-function {{ color: #d23f31; font-family: "Fira Code", monospace; font-size: 13px; font-weight: 600; }}
.frame-location {{ color: #888; font-family: "Fira Code", monospace; font-size: 12px; }}
.source-list {{ background: #1e1e1e; border-radius: 4px; padding: 12px; overflow-x: auto; font-family: "Fira Code", monospace; font-size: 13px; }}
.source-line {{ display: flex; color: #d4d4d4; }}
.source-line.error {{ background: rgba(210,63,49,0.2); }}
.line-num {{ color: #858585; min-width: 50px; text-align: right; padding-right: 16px; user-select: none; }}
.line-content {{ white-space: pre; }}
.request-table {{ width: 100%; border-collapse: collapse; font-size: 13px; }}
.request-table th, .request-table td {{ text-align: left; padding: 8px 12px; border-bottom: 1px solid #eee; }}
.request-table th {{ background: #fafafa; width: 200px; color: #555; font-weight: 600; }}
.request-table td {{ font-family: "Fira Code", monospace; word-break: break-all; }}
.env-grid {{ display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; font-size: 13px; }}
.env-item {{ padding: 8px 12px; background: #fafafa; border-radius: 4px; }}
.env-item strong {{ color: #555; display: inline-block; min-width: 120px; }}
.footer {{ text-align: center; padding: 24px; color: #999; font-size: 12px; }}
</style>
</head>
<body>
<div class="header">
  <h1>{title}</h1>
  <div class="location">{file_display}:{error_line}</div>
</div>
<div class="container">
"#
    );

    // 堆栈
    if config.show_stack && !error.stack.is_empty() {
        html.push_str("<div class=\"section\">\n");
        html.push_str("  <div class=\"section-title\">Stack frames (" );
        html.push_str(&error.stack.len().to_string());
        html.push_str(")</div>\n");
        html.push_str("  <div class=\"section-body\">\n");
        for frame in &error.stack {
            html.push_str(&render_stack_frame_html(frame));
        }
        html.push_str("  </div>\n</div>\n");
    }

    // 请求信息
    if config.show_request && !error.method.is_empty() {
        html.push_str("<div class=\"section\">\n");
        html.push_str("  <div class=\"section-title\">Request</div>\n");
        html.push_str("  <div class=\"section-body\">\n");
        html.push_str("    <table class=\"request-table\">\n");
        html.push_str(&format!(
            "      <tr><th>Method</th><td>{}</td></tr>\n",
            method_display
        ));
        html.push_str(&format!(
            "      <tr><th>URI</th><td>{}</td></tr>\n",
            uri_display
        ));
        for (key, value) in &error.headers {
            html.push_str(&format!(
                "      <tr><th>{}</th><td>{}</td></tr>\n",
                html_escape(key),
                html_escape(value)
            ));
        }
        for (key, value) in &error.query_params {
            html.push_str(&format!(
                "      <tr><th>Query: {}</th><td>{}</td></tr>\n",
                html_escape(key),
                html_escape(value)
            ));
        }
        html.push_str("    </table>\n  </div>\n</div>\n");
    }

    // 环境信息
    if config.show_environment {
        html.push_str("<div class=\"section\">\n");
        html.push_str("  <div class=\"section-title\">Environment</div>\n");
        html.push_str("  <div class=\"section-body\">\n");
        html.push_str("    <div class=\"env-grid\">\n");
        html.push_str(&format!(
            "      <div class=\"env-item\"><strong>Rust version</strong> {}</div>\n",
            env!("CARGO_PKG_VERSION")
        ));
        html.push_str(&format!(
            "      <div class=\"env-item\"><strong>PID</strong> {}</div>\n",
            std::process::id()
        ));
        if let Ok(cwd) = std::env::current_dir() {
            html.push_str(&format!(
                "      <div class=\"env-item\"><strong>Working dir</strong> {}</div>\n",
                html_escape(&cwd.display().to_string())
            ));
        }
        let now = chrono::Local::now();
        html.push_str(&format!(
            "      <div class=\"env-item\"><strong>Time</strong> {}</div>\n",
            html_escape(&now.format("%Y-%m-%d %H:%M:%S").to_string())
        ));
        html.push_str("    </div>\n  </div>\n</div>\n");
    }

    html.push_str("</div>\n");
    html.push_str("<div class=\"footer\">SZ-Rust Whoops-style Debugger — debug mode enabled</div>\n");
    html.push_str("</body>\n</html>\n");

    html
}

/// 渲染单个堆栈帧的 HTML
fn render_stack_frame_html(frame: &StackFrame) -> String {
    let function = html_escape(&frame.function);
    let location = html_escape(&format!("{}:{}", frame.file, frame.line));

    let mut html = format!(
        r#"    <div class="stack-frame">
      <div class="frame-header">
        <span class="frame-function">{function}</span>
        <span class="frame-location">{location}</span>
      </div>
"#
    );

    if !frame.source_lines.is_empty() {
        html.push_str("      <div class=\"source-list\">\n");
        for (line_num, content) in &frame.source_lines {
            let is_error_line = *line_num == frame.line;
            let line_class = if is_error_line { " error" } else { "" };
            html.push_str(&format!(
                "        <div class=\"source-line{}\"><span class=\"line-num\">{}</span><span class=\"line-content\">{}</span></div>\n",
                line_class,
                line_num,
                html_escape(content)
            ));
        }
        html.push_str("      </div>\n");
    }

    html.push_str("    </div>\n");
    html
}

/// 渲染生产环境简洁错误页（不泄露堆栈）
pub fn render_production_html(status: StatusCode, message: &str) -> String {
    let status_code = status.as_u16();
    let status_text = html_escape(status.canonical_reason().unwrap_or("Error"));
    let msg = html_escape(message);

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<title>{status_code} {status_text}</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #fafafa; color: #333; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; }}
.error-box {{ background: #fff; border-radius: 8px; box-shadow: 0 2px 8px rgba(0,0,0,0.1); padding: 48px 64px; text-align: center; max-width: 480px; }}
.error-code {{ font-size: 72px; font-weight: 700; color: #d23f31; line-height: 1; margin-bottom: 16px; }}
.error-title {{ font-size: 20px; color: #555; margin-bottom: 8px; }}
.error-message {{ font-size: 14px; color: #999; }}
</style>
</head>
<body>
<div class="error-box">
  <div class="error-code">{status_code}</div>
  <div class="error-title">{status_text}</div>
  <div class="error-message">{msg}</div>
</div>
</body>
</html>
"#
    )
}

// ============================================================================
// 响应构建
// ============================================================================

/// 根据调试配置构建错误响应
///
/// - `debug_mode = true`：返回 Whoops-style HTML（含堆栈/源码/请求信息）
/// - `debug_mode = false`：返回简洁 HTML（仅状态码 + 标准描述，不泄露堆栈和原始消息）
///
/// # 安全约束
///
/// 生产模式下绝不显示 `error.message`（可能含敏感信息如 SQL/密码），
/// 仅显示状态码的标准描述（如 `Internal Server Error`）。
pub fn debug_error_response(
    status: StatusCode,
    error: &DebugError,
    config: &DebugPageConfig,
) -> Response {
    let html = if config.debug_mode {
        render_debug_html(error, config)
    } else {
        // 生产模式：使用状态码的标准描述，不泄露原始错误消息
        let safe_message = status.canonical_reason().unwrap_or("Error");
        render_production_html(status, safe_message)
    };

    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/html; charset=utf-8",
        )],
        html,
    )
        .into_response()
}

/// 从 panic 信息构建调试错误
///
/// 用于 `std::panic::set_hook` 捕获 panic 并渲染调试页。
///
/// 注：`PanicHookInfo` 自 Rust 1.81.0 起稳定，本框架 MSRV 已提升至 1.81.0+。
pub fn from_panic(panic_info: &std::panic::PanicHookInfo<'_>) -> DebugError {
    let message = panic_info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| {
            panic_info
                .payload()
                .downcast_ref::<String>()
                .cloned()
        })
        .unwrap_or_else(|| "<unknown panic payload>".to_string());

    let (file, line) = panic_info
        .location()
        .map(|loc| (loc.file().to_string(), loc.line() as usize))
        .unwrap_or_else(|| ("<unknown>".to_string(), 0));

    DebugError::new("panic", message, file, line)
}

// ============================================================================
// 辅助函数
// ============================================================================

/// HTML 转义（防 XSS）
///
/// 转义字符：`&` → `&amp;`、`<` → `&lt;`、`>` → `&gt;`、`"` → `&quot;`、`'` → `&#39;`
fn html_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 脱敏请求头（移除敏感字段值，仅保留键名）
///
/// 对齐 PHP `whoops` 的 `RequestDataFormatter`：Authorization / Cookie / Set-Cookie 等
/// 敏感头仅显示 `<redacted>`。
fn sanitize_headers(headers: &HeaderMap) -> HashMap<String, String> {
    const SENSITIVE_HEADERS: &[&str] = &[
        "authorization",
        "cookie",
        "set-cookie",
        "x-api-key",
        "x-auth-token",
    ];

    let mut sanitized = HashMap::new();
    for (name, value) in headers.iter() {
        let name_str = name.as_str().to_lowercase();
        let value_str = if SENSITIVE_HEADERS.contains(&name_str.as_str()) {
            "<redacted>".to_string()
        } else {
            value.to_str().unwrap_or("<binary>").to_string()
        };
        sanitized.insert(name_str, value_str);
    }
    sanitized
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, Method};

    // --------------------------------------------------------------------
    // html_escape
    // --------------------------------------------------------------------

    #[test]
    fn test_html_escape_basic() {
        assert_eq!(html_escape("hello"), "hello");
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("\"quote\""), "&quot;quote&quot;");
        assert_eq!(html_escape("'apos'"), "&#39;apos&#39;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }

    #[test]
    fn test_html_escape_empty() {
        assert_eq!(html_escape(""), "");
    }

    #[test]
    fn test_html_escape_xss_payload() {
        let payload = "<script>alert('XSS')</script>";
        let escaped = html_escape(payload);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(escaped.contains("&lt;script&gt;"));
    }

    // --------------------------------------------------------------------
    // StackFrame::load_source_snippet
    // --------------------------------------------------------------------

    #[test]
    fn test_stack_frame_load_source_snippet() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("test.rs");
        std::fs::write(
            &file_path,
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\n",
        )
        .unwrap();

        let mut frame = StackFrame::new(file_path.to_str().unwrap(), 5, "test_fn");
        frame.load_source_snippet(2);

        // 应该包含 line 3-7（5-2=3, 5+2=7）
        assert_eq!(frame.source_lines.len(), 5);
        assert_eq!(frame.source_lines[0].0, 3);
        assert_eq!(frame.source_lines[0].1, "line3");
        assert_eq!(frame.source_lines[2].0, 5);
        assert_eq!(frame.source_lines[2].1, "line5");
        assert_eq!(frame.source_lines[4].0, 7);
        assert_eq!(frame.source_lines[4].1, "line7");
    }

    #[test]
    fn test_stack_frame_load_source_snippet_at_file_start() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("start.rs");
        std::fs::write(&file_path, "line1\nline2\nline3\n").unwrap();

        let mut frame = StackFrame::new(file_path.to_str().unwrap(), 1, "first");
        frame.load_source_snippet(10);

        // line=1, context=10, 但文件只有 3 行
        assert_eq!(frame.source_lines.len(), 3);
        assert_eq!(frame.source_lines[0].0, 1);
    }

    #[test]
    fn test_stack_frame_load_source_snippet_nonexistent_file() {
        let mut frame = StackFrame::new("/nonexistent/file.rs", 10, "missing");
        frame.load_source_snippet(5);
        assert!(frame.source_lines.is_empty());
    }

    #[test]
    fn test_stack_frame_load_source_snippet_zero_line() {
        let mut frame = StackFrame::new("test.rs", 0, "unknown");
        frame.load_source_snippet(5);
        assert!(frame.source_lines.is_empty());
    }

    // --------------------------------------------------------------------
    // DebugPageConfig
    // --------------------------------------------------------------------

    #[test]
    fn test_debug_page_config_default() {
        let config = DebugPageConfig::default();
        assert!(!config.debug_mode);
        assert_eq!(config.source_context, 10);
    }

    #[test]
    fn test_debug_page_config_development() {
        let config = DebugPageConfig::development();
        assert!(config.debug_mode);
        assert!(config.show_stack);
        assert!(config.show_request);
        assert!(config.show_environment);
    }

    #[test]
    fn test_debug_page_config_production() {
        let config = DebugPageConfig::production();
        assert!(!config.debug_mode);
        assert!(!config.show_stack);
        assert_eq!(config.source_context, 0);
    }

    // --------------------------------------------------------------------
    // render_debug_html
    // --------------------------------------------------------------------

    #[test]
    fn test_render_debug_html_contains_error_info() {
        let error = DebugError::new("TestError", "test message", "test.rs", 42);
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        assert!(html.contains("TestError"));
        assert!(html.contains("test message"));
        assert!(html.contains("test.rs:42"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_render_debug_html_escapes_xss() {
        let error = DebugError::new(
            "<script>alert('xss')</script>",
            "<img src=x onerror=alert(1)>",
            "test.rs",
            1,
        );
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        // 原始 <script> 不应出现
        assert!(!html.contains("<script>alert"));
        // 转义后应出现
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;img"));
    }

    #[test]
    fn test_render_debug_html_shows_stack() {
        let error = DebugError::new("Err", "msg", "test.rs", 1)
            .with_frame(StackFrame::new("frame1.rs", 10, "func1"))
            .with_frame(StackFrame::new("frame2.rs", 20, "func2"));
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        assert!(html.contains("func1"));
        assert!(html.contains("func2"));
        assert!(html.contains("frame1.rs:10"));
        assert!(html.contains("frame2.rs:20"));
        assert!(html.contains("Stack frames (2)"));
    }

    #[test]
    fn test_render_debug_html_hides_stack_when_disabled() {
        let error = DebugError::new("Err", "msg", "test.rs", 1)
            .with_frame(StackFrame::new("frame1.rs", 10, "func1"));
        let mut config = DebugPageConfig::development();
        config.show_stack = false;
        let html = render_debug_html(&error, &config);

        assert!(!html.contains("func1"));
        assert!(!html.contains("Stack frames"));
    }

    #[test]
    fn test_render_debug_html_shows_request_info() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", HeaderValue::from_static("value1"));
        let mut query = HashMap::new();
        query.insert("id".to_string(), "123".to_string());

        let error = DebugError::new("Err", "msg", "test.rs", 1).with_request(
            Method::GET.as_str(),
            "/api/users",
            headers,
            query,
        );
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        assert!(html.contains("GET"));
        assert!(html.contains("/api/users"));
        assert!(html.contains("x-custom"));
        assert!(html.contains("value1"));
        assert!(html.contains("Query: id"));
        assert!(html.contains("123"));
    }

    #[test]
    fn test_render_debug_html_redacts_sensitive_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret123"));
        headers.insert("cookie", HeaderValue::from_static("session=abc"));

        let error = DebugError::new("Err", "msg", "test.rs", 1).with_request(
            "POST",
            "/api",
            headers,
            HashMap::new(),
        );
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        // 敏感值不应出现
        assert!(!html.contains("secret123"));
        assert!(!html.contains("session=abc"));
        // 应显示 <redacted>
        assert!(html.contains("&lt;redacted&gt;"));
    }

    #[test]
    fn test_render_debug_html_shows_environment() {
        let error = DebugError::new("Err", "msg", "test.rs", 1);
        let config = DebugPageConfig::development();
        let html = render_debug_html(&error, &config);

        assert!(html.contains("PID"));
        assert!(html.contains("Rust version"));
    }

    #[tokio::test]
    async fn test_render_production_html_no_stack() {
        let error = DebugError::new("SecretError", "internal db password=xxx", "secret.rs", 100)
            .with_frame(StackFrame::new("frame.rs", 1, "secret_func"));
        let config = DebugPageConfig::production();
        // 通过 debug_error_response 验证生产模式行为（render_debug_html 始终渲染完整页，
        // 由 debug_error_response 根据 debug_mode 选择渲染策略）
        let response = debug_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error, &config);
        // 转 HTML 字符串验证内容
        use http_body_util::BodyExt;
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(bytes.to_vec()).unwrap();

        // 生产模式应使用 production_html（不含堆栈/敏感信息）
        assert!(!html.contains("SecretError"));
        assert!(!html.contains("secret.rs"));
        assert!(!html.contains("secret_func"));
        assert!(!html.contains("password=xxx"));
    }

    // --------------------------------------------------------------------
    // render_production_html
    // --------------------------------------------------------------------

    #[test]
    fn test_render_production_html_basic() {
        let html = render_production_html(StatusCode::INTERNAL_SERVER_ERROR, "Server Error");
        assert!(html.contains("500"));
        assert!(html.contains("Internal Server Error"));
        assert!(html.contains("Server Error"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[test]
    fn test_render_production_html_escapes_message() {
        let html = render_production_html(
            StatusCode::BAD_REQUEST,
            "<script>alert('xss')</script>",
        );
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
    }

    // --------------------------------------------------------------------
    // debug_error_response
    // --------------------------------------------------------------------

    #[tokio::test]
    async fn test_debug_error_response_debug_mode() {
        let error = DebugError::new("TestError", "test", "test.rs", 1);
        let config = DebugPageConfig::development();
        let response = debug_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error, &config);

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_debug_error_response_production_mode() {
        let error = DebugError::new("TestError", "secret", "test.rs", 1);
        let config = DebugPageConfig::production();
        let response = debug_error_response(StatusCode::NOT_FOUND, &error, &config);

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    // --------------------------------------------------------------------
    // sanitize_headers
    // --------------------------------------------------------------------

    #[test]
    fn test_sanitize_headers_redacts_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer xyz"));
        headers.insert("cookie", HeaderValue::from_static("sess=abc"));
        headers.insert("x-custom", HeaderValue::from_static("safe"));

        let sanitized = sanitize_headers(&headers);

        assert_eq!(sanitized.get("authorization").unwrap(), "<redacted>");
        assert_eq!(sanitized.get("cookie").unwrap(), "<redacted>");
        assert_eq!(sanitized.get("x-custom").unwrap(), "safe");
    }

    #[test]
    fn test_sanitize_headers_empty() {
        let headers = HeaderMap::new();
        let sanitized = sanitize_headers(&headers);
        assert!(sanitized.is_empty());
    }

    // --------------------------------------------------------------------
    // DebugError
    // --------------------------------------------------------------------

    #[test]
    fn test_debug_error_builder_pattern() {
        let error = DebugError::new("ErrType", "msg", "file.rs", 10)
            .with_frame(StackFrame::new("frame.rs", 5, "fn1"));

        assert_eq!(error.error_type, "ErrType");
        assert_eq!(error.message, "msg");
        assert_eq!(error.file, "file.rs");
        assert_eq!(error.line, 10);
        assert_eq!(error.stack.len(), 1);
        assert_eq!(error.stack[0].function, "fn1");
    }

    #[test]
    fn test_debug_error_with_source_snippet_loads_main_frame() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("main.rs");
        std::fs::write(&file_path, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let error = DebugError::new("Err", "msg", file_path.to_str().unwrap(), 3)
            .with_source_snippet(1);

        // 主错误位置应作为虚拟帧插入 stack[0]
        assert!(!error.stack.is_empty());
        assert_eq!(error.stack[0].line, 3);
        assert!(!error.stack[0].source_lines.is_empty());
    }
}
