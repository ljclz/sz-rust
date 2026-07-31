//! Phase 4.5.8-4.5.10 — HTTP 管理端点。
//!
//! 提供轻量级 HTTP/1.1 管理服务器，支持：
//! - `GET /healthz` — 存活探针（始终返回 200 + `{"status":"ok"}`）
//! - `GET /readyz` — 就绪探针（服务器 Running 时返回 200，Draining/Closed 时返回 503）
//! - `GET /metrics` — Prometheus 文本格式指标（connections_total/queries_total/wal_lsn 等）
//! - `GET /api/v1/sessions` — 列出活跃会话（需 auth header）
//! - `POST /api/v1/cancel/{pid}` — 取消指定会话（需 auth header）
//! - `POST /api/v1/backup` — 触发备份（需 auth header）
//! - `POST /api/v1/config/reload` — 触发配置热重载（需 auth header）
//!
//! # 设计
//!
//! - **零外部 HTTP 依赖**：基于 tokio TCP 手动解析 HTTP/1.1，与 pgwire 风格一致
//! - **线程安全**：`MetricsRegistry` 使用 `Arc<AtomicU64>` 实现无锁计数
//! - **配置开关**：`http_port = 0`（默认）时不监听 HTTP 端口
//! - **绑定安全**：默认仅绑定 `127.0.0.1`，避免外部访问管理端点
//!
//! # HTTP/1.1 支持范围
//!
//! - 支持 GET / POST 方法
//! - 支持 Content-Length 头部确定 body 长度
//! - 支持 Connection: close（短连接，每个请求后关闭）
//! - 不支持 chunked transfer encoding、keep-alive、HTTP/2

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use crate::pgwire::lifecycle::ShutdownState;

// =====================================================================
//  错误类型
// =====================================================================

/// HTTP 服务器错误。
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// I/O 错误（TCP 绑定/读写失败）。
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// HTTP 请求解析失败（格式错误）。
    #[error("http parse error: {0}")]
    Parse(String),
}

// =====================================================================
//  HTTP 配置
// =====================================================================

/// HTTP 管理服务器配置。
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// HTTP 监听地址（默认 `127.0.0.1`，仅本地访问）。
    pub host: String,
    /// HTTP 监听端口（默认 `0` = 不监听）。
    pub port: u16,
    /// 管理端点鉴权 token（Bearer token，None = 不鉴权）。
    pub auth_token: Option<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0, // 默认不监听 HTTP
            auth_token: None,
        }
    }
}

impl HttpConfig {
    /// 创建默认配置（不监听 HTTP）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置监听地址。
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// 设置监听端口（0 = 不监听）。
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置管理端点鉴权 token。
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// 是否启用 HTTP 服务器（port != 0）。
    pub fn is_enabled(&self) -> bool {
        self.port != 0
    }
}

// =====================================================================
//  MetricsRegistry
// =====================================================================

/// 简单的指标注册表（无锁原子计数器）。
///
/// 提供 Prometheus 文本格式输出，包含：
/// - `szrsql_connections_total` — 累计连接数（Counter）
/// - `szrsql_queries_total` — 累计查询数（Counter）
/// - `szrsql_active_connections` — 当前活跃连接数（Gauge）
/// - `szrsql_wal_lsn` — 最后 WAL LSN（Gauge，占位 0）
#[derive(Debug, Default)]
pub struct MetricsRegistry {
    connections_total: AtomicU64,
    queries_total: AtomicU64,
    active_connections: AtomicU64,
    wal_lsn: AtomicU64,
}

impl MetricsRegistry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 累计连接数 +1。
    pub fn inc_connections(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计查询数 +1。
    pub fn inc_queries(&self) {
        self.queries_total.fetch_add(1, Ordering::Relaxed);
    }

    /// 活跃连接数 +1。
    pub fn inc_active_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// 活跃连接数 -1。
    pub fn dec_active_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// 设置最后 WAL LSN。
    pub fn set_wal_lsn(&self, lsn: u64) {
        self.wal_lsn.store(lsn, Ordering::Relaxed);
    }

    /// 输出 Prometheus 文本格式。
    pub fn to_prometheus_text(&self) -> String {
        let connections = self.connections_total.load(Ordering::Relaxed);
        let queries = self.queries_total.load(Ordering::Relaxed);
        let active = self.active_connections.load(Ordering::Relaxed);
        let lsn = self.wal_lsn.load(Ordering::Relaxed);

        format!(
            "# HELP szrsql_connections_total Total number of connections accepted since startup.\n\
             # TYPE szrsql_connections_total counter\n\
             szrsql_connections_total {connections}\n\
             # HELP szrsql_queries_total Total number of queries executed since startup.\n\
             # TYPE szrsql_queries_total counter\n\
             szrsql_queries_total {queries}\n\
             # HELP szrsql_active_connections Current number of active connections.\n\
             # TYPE szrsql_active_connections gauge\n\
             szrsql_active_connections {active}\n\
             # HELP szrsql_wal_lsn Last Write-Ahead Log LSN (0 = WAL not yet implemented).\n\
             # TYPE szrsql_wal_lsn gauge\n\
             szrsql_wal_lsn {lsn}\n"
        )
    }
}

// =====================================================================
//  HttpRequest / HttpResponse
// =====================================================================

/// HTTP 请求（最小化解析）。
#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    /// 获取指定 header 的值（不区分大小写）。
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 获取 Authorization header 中的 Bearer token。
    fn bearer_token(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
    }
}

/// HTTP 响应。
struct HttpResponse {
    status: u16,
    status_text: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    /// JSON 响应。
    fn json(status: u16, status_text: &'static str, json: &str) -> Self {
        Self {
            status,
            status_text,
            content_type: "application/json; charset=utf-8",
            body: json.as_bytes().to_vec(),
        }
    }

    /// 文本响应。
    fn text(status: u16, status_text: &'static str, text: &str) -> Self {
        Self {
            status,
            status_text,
            content_type: "text/plain; charset=utf-8",
            body: text.as_bytes().to_vec(),
        }
    }

    /// 200 OK + JSON。
    fn ok_json(json: &str) -> Self {
        Self::json(200, "OK", json)
    }

    /// 503 Service Unavailable + JSON。
    fn service_unavailable(json: &str) -> Self {
        Self::json(503, "Service Unavailable", json)
    }

    /// 401 Unauthorized + JSON。
    fn unauthorized() -> Self {
        Self::json(401, "Unauthorized", r#"{"error":"unauthorized"}"#)
    }

    /// 404 Not Found + JSON。
    fn not_found() -> Self {
        Self::json(404, "Not Found", r#"{"error":"not found"}"#)
    }

    /// 405 Method Not Allowed + JSON。
    fn method_not_allowed() -> Self {
        Self::json(
            405,
            "Method Not Allowed",
            r#"{"error":"method not allowed"}"#,
        )
    }

    /// 200 OK + Prometheus 文本。
    fn prometheus(text: &str) -> Self {
        Self::text(200, "OK", text)
    }

    /// 序列化为 HTTP/1.1 响应字节。
    fn to_bytes(&self) -> Vec<u8> {
        let header = format!(
            "HTTP/1.1 {} {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            self.status,
            self.status_text,
            self.content_type,
            self.body.len()
        );
        let mut result = header.into_bytes();
        result.extend_from_slice(&self.body);
        result
    }
}

// =====================================================================
//  HttpServer
// =====================================================================

/// HTTP 管理服务器。
///
/// 监听指定端口，提供 healthz/readyz/metrics 等管理端点。
/// 与 pgwire 服务器共享 `ShutdownState` watch 通道以实现就绪探针。
pub struct HttpServer {
    config: HttpConfig,
    metrics: Arc<MetricsRegistry>,
    shutdown_rx: watch::Receiver<ShutdownState>,
    /// P8-2：CDC 服务注入（可选），用于暴露 /api/v1/cdc/* REST API
    cdc_service: Option<Arc<szrsql_cdc::service::CdcService>>,
}

impl HttpServer {
    /// 创建 HTTP 服务器。
    ///
    /// # 参数
    /// - `config`：HTTP 配置（port=0 时不监听）
    /// - `metrics`：共享的指标注册表
    /// - `shutdown_rx`：关闭状态 watch 接收端（与 pgwire 服务器共享）
    pub fn new(
        config: HttpConfig,
        metrics: Arc<MetricsRegistry>,
        shutdown_rx: watch::Receiver<ShutdownState>,
    ) -> Self {
        Self {
            config,
            metrics,
            shutdown_rx,
            cdc_service: None,
        }
    }

    /// P8-2：注入 CDC 服务，启用 /api/v1/cdc/* REST API 端点。
    ///
    /// 注入后，HTTP 服务器将暴露以下端点（均需 auth header）：
    /// - `GET    /api/v1/cdc/tenants`           — 列出所有租户
    /// - `POST   /api/v1/cdc/tenants`           — 注册租户（body: TenantConfig JSON）
    /// - `GET    /api/v1/cdc/tenants/{id}`      — 获取租户配置
    /// - `DELETE /api/v1/cdc/tenants/{id}`      — 注销租户
    /// - `PATCH  /api/v1/cdc/tenants/{id}/tier` — 更新租户等级（body: `{"tier":"free"}`)
    /// - `GET    /api/v1/cdc/tasks?tenant_id=x` — 列出租户的所有任务
    /// - `GET    /api/v1/cdc/tasks/{id}?tenant_id=x` — 获取任务详情
    /// - `POST   /api/v1/cdc/tasks/{id}/start?tenant_id=x` — 启动任务
    /// - `POST   /api/v1/cdc/tasks/{id}/stop?tenant_id=x`  — 停止任务
    /// - `POST   /api/v1/cdc/tasks/{id}/pause?tenant_id=x` — 暂停任务
    /// - `POST   /api/v1/cdc/tasks/{id}/resume?tenant_id=x` — 恢复任务
    /// - `DELETE /api/v1/cdc/tasks/{id}?tenant_id=x` — 删除任务
    /// - `GET    /api/v1/cdc/usage/{tenant_id}` — 获取租户使用量
    pub fn with_cdc_service(mut self, cdc_service: Arc<szrsql_cdc::service::CdcService>) -> Self {
        self.cdc_service = Some(cdc_service);
        self
    }

    /// 启动 HTTP 服务器，阻塞直到收到关闭信号。
    ///
    /// 如果 `config.port == 0`，立即返回 `Ok(())`（不监听）。
    pub async fn serve(self) -> Result<(), HttpError> {
        if !self.config.is_enabled() {
            tracing::debug!("http server disabled (port=0)");
            return Ok(());
        }

        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;
        tracing::info!(addr = %local_addr, "http management server listening");

        let auth_token = self.config.auth_token.clone();
        let metrics = Arc::clone(&self.metrics);
        let cdc_service = self.cdc_service.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                // 检查关闭信号
                changed = shutdown_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    if *shutdown_rx.borrow() == ShutdownState::Closed {
                        tracing::info!("http server shutting down (state=Closed)");
                        break;
                    }
                }

                // 接受新连接
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, peer)) => {
                            let metrics = Arc::clone(&metrics);
                            let auth_token = auth_token.clone();
                            let cdc_service = cdc_service.clone();
                            let shutdown_rx = self.shutdown_rx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, peer, &auth_token, &metrics, &cdc_service, &shutdown_rx).await {
                                    tracing::debug!(error = %e, "http connection error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "http accept failed");
                            return Err(e.into());
                        }
                    }
                }
            }
        }

        tracing::info!("http management server stopped");
        Ok(())
    }
}

/// 处理单个 HTTP 连接（短连接，处理一个请求后关闭）。
async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    auth_token: &Option<String>,
    metrics: &MetricsRegistry,
    cdc_service: &Option<Arc<szrsql_cdc::service::CdcService>>,
    shutdown_rx: &watch::Receiver<ShutdownState>,
) -> Result<(), HttpError> {
    // 读取请求数据（最多 64KB）
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];

    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break; // 客户端关闭连接
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 65536 {
            let _ = stream
                .write_all(
                    &HttpResponse::json(
                        413,
                        "Payload Too Large",
                        r#"{"error":"request too large"}"#,
                    )
                    .to_bytes(),
                )
                .await;
            return Ok(());
        }
        // 检测 \r\n\r\n（headers 结束）
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    if buf.is_empty() {
        return Ok(()); // 空请求，直接关闭
    }

    // 解析 HTTP 请求
    let request = match parse_http_request(&buf) {
        Ok(req) => req,
        Err(e) => {
            let resp = HttpResponse::json(400, "Bad Request", &format!(r#"{{"error":"{e}"}}"#));
            let _ = stream.write_all(&resp.to_bytes()).await;
            return Ok(());
        }
    };

    tracing::debug!(
        method = %request.method,
        path = %request.path,
        peer = %peer,
        "http request"
    );

    // 路由请求
    let response = route_request(&request, auth_token, metrics, cdc_service, shutdown_rx);

    // 发送响应
    stream.write_all(&response.to_bytes()).await?;
    stream.flush().await?;

    Ok(())
}

/// 解析 HTTP/1.1 请求。
fn parse_http_request(buf: &[u8]) -> Result<HttpRequest, HttpError> {
    let text = String::from_utf8_lossy(buf);

    // 找到 headers 和 body 的分界
    let header_end = text
        .find("\r\n\r\n")
        .ok_or_else(|| HttpError::Parse("missing header terminator".into()))?;
    let header_section = &text[..header_end];
    let body = if header_end + 4 < buf.len() {
        buf[header_end + 4..].to_vec()
    } else {
        Vec::new()
    };

    let mut lines = header_section.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| HttpError::Parse("empty request".into()))?;

    // 解析请求行：METHOD PATH HTTP/1.1
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return Err(HttpError::Parse(format!(
            "invalid request line: {request_line}"
        )));
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // 解析 headers
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(": ") {
            headers.push((name.to_string(), value.to_string()));
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.to_string(), value.trim().to_string()));
        }
    }

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

/// 路由 HTTP 请求到对应的处理器。
fn route_request(
    request: &HttpRequest,
    auth_token: &Option<String>,
    metrics: &MetricsRegistry,
    cdc_service: &Option<Arc<szrsql_cdc::service::CdcService>>,
    shutdown_rx: &watch::Receiver<ShutdownState>,
) -> HttpResponse {
    match (request.method.as_str(), request.path.as_str()) {
        // ==================== 公开端点（无需鉴权） ====================

        // 存活探针：始终返回 200
        ("GET", "/healthz") => HttpResponse::ok_json(r#"{"status":"ok"}"#),

        // 就绪探针：Running 时返回 200，Draining/Closed 时返回 503
        ("GET", "/readyz") => {
            let state = *shutdown_rx.borrow();
            match state {
                ShutdownState::Running => HttpResponse::ok_json(r#"{"status":"ready"}"#),
                ShutdownState::Draining => {
                    HttpResponse::service_unavailable(r#"{"status":"draining"}"#)
                }
                ShutdownState::Closed => {
                    HttpResponse::service_unavailable(r#"{"status":"closed"}"#)
                }
            }
        }

        // Prometheus 指标
        ("GET", "/metrics") => HttpResponse::prometheus(&metrics.to_prometheus_text()),

        // ==================== 文档端点（无需鉴权） ====================

        // OpenAPI 3.0 规范 JSON（Phase 7d.17）
        ("GET", "/api/v1/openapi.json") => {
            let spec = crate::openapi::openapi_spec_to_json();
            HttpResponse::json(200, "OK", &spec)
        }

        // Swagger UI HTML 页面（Phase 7d.17）
        ("GET", "/api/v1/swagger") => {
            let html = crate::openapi::render_swagger_ui();
            HttpResponse {
                status: 200,
                status_text: "OK",
                content_type: "text/html; charset=utf-8",
                body: html.into_bytes(),
            }
        }

        // ==================== 管理端点（需鉴权） ====================

        // 列出活跃会话
        ("GET", "/api/v1/sessions") => {
            if !check_auth(request, auth_token) {
                return HttpResponse::unauthorized();
            }
            // Phase 4.5.9 占位：实际会话列表需要从 PgwireServer 获取
            // 当前返回空列表，待后续集成真实会话管理
            HttpResponse::ok_json(r#"{"sessions":[]}"#)
        }

        // 取消指定会话
        ("POST", path) if path.starts_with("/api/v1/cancel/") => {
            if !check_auth(request, auth_token) {
                return HttpResponse::unauthorized();
            }
            let pid_str = path.strip_prefix("/api/v1/cancel/").unwrap_or("");
            let pid: i32 = match pid_str.parse() {
                Ok(p) => p,
                Err(_) => {
                    return HttpResponse::json(400, "Bad Request", r#"{"error":"invalid pid"}"#);
                }
            };
            // Phase 4.5.9 占位：实际取消需要通过 PgwireServer 的 cancel 机制
            // 当前返回成功，待后续集成真实取消逻辑
            tracing::info!(pid, "cancel session requested (stub)");
            HttpResponse::ok_json(&format!(r#"{{"cancelled":{pid}}}"#))
        }

        // 触发备份
        ("POST", "/api/v1/backup") => {
            if !check_auth(request, auth_token) {
                return HttpResponse::unauthorized();
            }
            // Phase 4.5.9 占位：实际备份需要 WAL/快照机制
            // 当前返回成功，待 Phase 5 持久化层实现后补充
            tracing::info!("backup requested (stub)");
            HttpResponse::ok_json(r#"{"status":"backup completed (stub)"}"#)
        }

        // 触发配置热重载
        ("POST", "/api/v1/config/reload") => {
            if !check_auth(request, auth_token) {
                return HttpResponse::unauthorized();
            }
            // Phase 4.5.9 占位：实际热重载需要 SIGHUP 机制
            // 当前返回成功，待后续集成真实配置重载
            tracing::info!("config reload requested (stub)");
            HttpResponse::ok_json(r#"{"status":"config reloaded (stub)"}"#)
        }

        // ==================== P8-2: CDC REST API 端点（需鉴权 + CdcService 注入） ====================

        // CDC API 前缀统一分发到 cdc_route_request
        (method, path) if path.starts_with("/api/v1/cdc/") => {
            if !check_auth(request, auth_token) {
                return HttpResponse::unauthorized();
            }
            match cdc_service {
                Some(svc) => cdc_route_request(method, path, request, svc),
                None => HttpResponse::json(
                    503,
                    "Service Unavailable",
                    r#"{"error":"CDC service not enabled on this server"}"#,
                ),
            }
        }

        // ==================== 兜底 ====================
        (_, "/healthz")
        | (_, "/readyz")
        | (_, "/metrics")
        | (_, "/api/v1/openapi.json")
        | (_, "/api/v1/swagger") => HttpResponse::method_not_allowed(),
        _ => HttpResponse::not_found(),
    }
}

/// 检查请求的 Bearer token 是否匹配配置的 auth_token。
///
/// 如果 `auth_token` 为 None，允许所有请求（不鉴权）。
fn check_auth(request: &HttpRequest, auth_token: &Option<String>) -> bool {
    match auth_token {
        None => true, // 未配置 auth token，允许所有
        Some(expected) => request
            .bearer_token()
            .map(|token| token == expected)
            .unwrap_or(false),
    }
}

// =====================================================================
//  P8-2: CDC REST API 路由
// =====================================================================

/// 从完整 path（含 query string）中分离出纯 path 和 query 参数。
///
/// 例：`/api/v1/cdc/tasks?tenant_id=t1` → (`/api/v1/cdc/tasks`, `Some("tenant_id=t1")`)
fn split_path_query(full_path: &str) -> (&str, Option<&str>) {
    match full_path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (full_path, None),
    }
}

/// 从 query string 中提取指定参数的值。
fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    let q = query?;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v);
            }
        }
    }
    None
}

/// 从 path 中提取最后一段作为资源 ID。
///
/// 例：`/api/v1/cdc/tenants/t1` → `Some("t1")`
fn last_path_segment(path: &str) -> Option<&str> {
    path.rsplit('/').next().filter(|s| !s.is_empty())
}

/// 将 ServiceError 映射为对应的 HTTP 状态码。
fn service_error_to_status(e: &szrsql_cdc::service::ServiceError) -> u16 {
    use szrsql_cdc::service::ServiceError;
    match e {
        ServiceError::TenantNotFound(_) | ServiceError::TaskNotFound(_) => 404,
        ServiceError::TenantAlreadyExists(_) | ServiceError::TaskAlreadyExists(_) => 409,
        ServiceError::TenantLimitExceeded { .. } | ServiceError::QuotaExceeded { .. } => 429,
        ServiceError::Unauthorized => 401,
        ServiceError::Forbidden { .. } => 403,
        ServiceError::InvalidConfig(_) => 400,
        ServiceError::ClusterNotAssociated => 503,
        ServiceError::Cluster(_) | ServiceError::Internal(_) => 500,
    }
}

/// 将 ServiceError 转换为 HTTP 错误响应（JSON 格式）。
fn service_error_response(e: &szrsql_cdc::service::ServiceError) -> HttpResponse {
    let status = service_error_to_status(e);
    let status_text = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let body = serde_json::json!({ "error": e.to_string() }).to_string();
    HttpResponse::json(status, status_text, &body)
}

/// CDC REST API 路由处理器 — 将 HTTP 请求分发到 CdcService 方法。
fn cdc_route_request(
    method: &str,
    full_path: &str,
    request: &HttpRequest,
    svc: &Arc<szrsql_cdc::service::CdcService>,
) -> HttpResponse {
    use szrsql_cdc::service::{TenantConfig, TenantTier};

    let (path, query) = split_path_query(full_path);

    match (method, path) {
        // ==================== 租户管理 ====================

        // 列出所有租户
        ("GET", "/api/v1/cdc/tenants") => {
            let tenants = svc.list_tenants();
            match serde_json::to_string(&tenants) {
                Ok(json) => HttpResponse::json(200, "OK", &json),
                Err(e) => HttpResponse::json(500, "Internal Server Error", &format!(r#"{{"error":"serialize failed: {e}"}}"#)),
            }
        }

        // 注册租户（body: TenantConfig JSON）
        ("POST", "/api/v1/cdc/tenants") => {
            let config: TenantConfig = match serde_json::from_slice(&request.body) {
                Ok(c) => c,
                Err(e) => {
                    return HttpResponse::json(400, "Bad Request", &format!(r#"{{"error":"invalid JSON body: {e}"}}"#));
                }
            };
            match svc.register_tenant(config) {
                Ok(()) => HttpResponse::json(201, "Created", r#"{"status":"tenant registered"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 获取租户配置
        ("GET", p) if p.starts_with("/api/v1/cdc/tenants/") && !p.ends_with("/tier") => {
            let tenant_id = match last_path_segment(p) {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id"}"#),
            };
            match svc.get_tenant(tenant_id) {
                Ok(config) => match serde_json::to_string(&config) {
                    Ok(json) => HttpResponse::json(200, "OK", &json),
                    Err(e) => HttpResponse::json(500, "Internal Server Error", &format!(r#"{{"error":"serialize failed: {e}"}}"#)),
                },
                Err(e) => service_error_response(&e),
            }
        }

        // 注销租户
        ("DELETE", p) if p.starts_with("/api/v1/cdc/tenants/") => {
            let tenant_id = match last_path_segment(p) {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id"}"#),
            };
            match svc.unregister_tenant(tenant_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"tenant unregistered"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 更新租户等级（body: {"tier":"free"}）
        ("PATCH", p) if p.ends_with("/tier") && p.starts_with("/api/v1/cdc/tenants/") => {
            let tenant_id = p
                .strip_prefix("/api/v1/cdc/tenants/")
                .and_then(|s| s.strip_suffix("/tier"))
                .unwrap_or("");
            if tenant_id.is_empty() {
                return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id"}"#);
            }
            let tier_str: String = match serde_json::from_slice::<serde_json::Value>(&request.body) {
                Ok(v) => v.get("tier").and_then(|t| t.as_str()).map(String::from).unwrap_or_default(),
                Err(e) => {
                    return HttpResponse::json(400, "Bad Request", &format!(r#"{{"error":"invalid JSON body: {e}"}}"#));
                }
            };
            let tier = match tier_str.as_str() {
                "free" => TenantTier::Free,
                "pro" => TenantTier::Pro,
                "enterprise" => TenantTier::Enterprise,
                other => {
                    return HttpResponse::json(400, "Bad Request", &format!(r#"{{"error":"invalid tier: {other}"}}"#));
                }
            };
            match svc.update_tenant_tier(tenant_id, tier) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"tier updated"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // ==================== 任务管理 ====================

        // 列出租户的所有任务（?tenant_id=xxx）
        ("GET", "/api/v1/cdc/tasks") => {
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.list_tasks(tenant_id) {
                Ok(tasks) => match serde_json::to_string(&tasks) {
                    Ok(json) => HttpResponse::json(200, "OK", &json),
                    Err(e) => HttpResponse::json(500, "Internal Server Error", &format!(r#"{{"error":"serialize failed: {e}"}}"#)),
                },
                Err(e) => service_error_response(&e),
            }
        }

        // 获取任务详情（?tenant_id=xxx）
        ("GET", p) if p.starts_with("/api/v1/cdc/tasks/") && !p.contains("/start") && !p.contains("/stop") && !p.contains("/pause") && !p.contains("/resume") => {
            let task_id = match last_path_segment(p) {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing task_id"}"#),
            };
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.get_task(tenant_id, task_id) {
                Ok(info) => match serde_json::to_string(&info) {
                    Ok(json) => HttpResponse::json(200, "OK", &json),
                    Err(e) => HttpResponse::json(500, "Internal Server Error", &format!(r#"{{"error":"serialize failed: {e}"}}"#)),
                },
                Err(e) => service_error_response(&e),
            }
        }

        // 启动任务（?tenant_id=xxx）
        ("POST", p) if p.ends_with("/start") && p.starts_with("/api/v1/cdc/tasks/") => {
            let task_id = p.strip_prefix("/api/v1/cdc/tasks/").and_then(|s| s.strip_suffix("/start")).unwrap_or("");
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.start_task(tenant_id, task_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"task started"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 停止任务（?tenant_id=xxx）
        ("POST", p) if p.ends_with("/stop") && p.starts_with("/api/v1/cdc/tasks/") => {
            let task_id = p.strip_prefix("/api/v1/cdc/tasks/").and_then(|s| s.strip_suffix("/stop")).unwrap_or("");
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.stop_task(tenant_id, task_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"task stopped"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 暂停任务（?tenant_id=xxx）
        ("POST", p) if p.ends_with("/pause") && p.starts_with("/api/v1/cdc/tasks/") => {
            let task_id = p.strip_prefix("/api/v1/cdc/tasks/").and_then(|s| s.strip_suffix("/pause")).unwrap_or("");
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.pause_task(tenant_id, task_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"task paused"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 恢复任务（?tenant_id=xxx）
        ("POST", p) if p.ends_with("/resume") && p.starts_with("/api/v1/cdc/tasks/") => {
            let task_id = p.strip_prefix("/api/v1/cdc/tasks/").and_then(|s| s.strip_suffix("/resume")).unwrap_or("");
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.resume_task(tenant_id, task_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"task resumed"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // 删除任务（?tenant_id=xxx）
        ("DELETE", p) if p.starts_with("/api/v1/cdc/tasks/") => {
            let task_id = match last_path_segment(p) {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing task_id"}"#),
            };
            let tenant_id = match query_param(query, "tenant_id") {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id query parameter"}"#),
            };
            match svc.delete_task(tenant_id, task_id) {
                Ok(()) => HttpResponse::ok_json(r#"{"status":"task deleted"}"#),
                Err(e) => service_error_response(&e),
            }
        }

        // ==================== 使用量查询 ====================

        ("GET", p) if p.starts_with("/api/v1/cdc/usage/") => {
            let tenant_id = match last_path_segment(p) {
                Some(id) => id,
                None => return HttpResponse::json(400, "Bad Request", r#"{"error":"missing tenant_id"}"#),
            };
            match svc.get_usage(tenant_id) {
                Ok(usage) => match serde_json::to_string(&usage) {
                    Ok(json) => HttpResponse::json(200, "OK", &json),
                    Err(e) => HttpResponse::json(500, "Internal Server Error", &format!(r#"{{"error":"serialize failed: {e}"}}"#)),
                },
                Err(e) => service_error_response(&e),
            }
        }

        // ==================== 兜底 ====================
        _ => HttpResponse::not_found(),
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::watch;

    /// 辅助：创建测试用 watch::Receiver<ShutdownState>。
    fn make_shutdown_rx() -> (watch::Sender<ShutdownState>, watch::Receiver<ShutdownState>) {
        watch::channel(ShutdownState::Running)
    }

    // ==================== HttpConfig ====================

    #[test]
    fn test_http_config_default_disabled() {
        let config = HttpConfig::default();
        assert!(!config.is_enabled(), "default should be disabled (port=0)");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 0);
        assert!(config.auth_token.is_none());
    }

    #[test]
    fn test_http_config_builder() {
        let config = HttpConfig::new()
            .with_host("0.0.0.0")
            .with_port(8080)
            .with_auth_token("secret-token");
        assert!(config.is_enabled());
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.auth_token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn test_http_config_port_zero_disabled() {
        let config = HttpConfig::new().with_port(0);
        assert!(!config.is_enabled());
    }

    // ==================== MetricsRegistry ====================

    #[test]
    fn test_metrics_registry_default() {
        let metrics = MetricsRegistry::new();
        let text = metrics.to_prometheus_text();
        assert!(text.contains("szrsql_connections_total 0"));
        assert!(text.contains("szrsql_queries_total 0"));
        assert!(text.contains("szrsql_active_connections 0"));
        assert!(text.contains("szrsql_wal_lsn 0"));
    }

    #[test]
    fn test_metrics_registry_increment() {
        let metrics = MetricsRegistry::new();
        metrics.inc_connections();
        metrics.inc_connections();
        metrics.inc_queries();
        metrics.inc_queries();
        metrics.inc_queries();
        metrics.inc_active_connections();
        metrics.set_wal_lsn(12345);

        let text = metrics.to_prometheus_text();
        assert!(text.contains("szrsql_connections_total 2"));
        assert!(text.contains("szrsql_queries_total 3"));
        assert!(text.contains("szrsql_active_connections 1"));
        assert!(text.contains("szrsql_wal_lsn 12345"));
    }

    #[test]
    fn test_metrics_registry_active_connections_decrement() {
        let metrics = MetricsRegistry::new();
        metrics.inc_active_connections();
        metrics.inc_active_connections();
        metrics.dec_active_connections();

        let text = metrics.to_prometheus_text();
        assert!(text.contains("szrsql_active_connections 1"));
    }

    #[test]
    fn test_metrics_prometheus_format_contains_help_and_type() {
        let metrics = MetricsRegistry::new();
        let text = metrics.to_prometheus_text();
        assert!(text.contains("# HELP szrsql_connections_total"));
        assert!(text.contains("# TYPE szrsql_connections_total counter"));
        assert!(text.contains("# TYPE szrsql_active_connections gauge"));
        assert!(text.contains("# TYPE szrsql_wal_lsn gauge"));
    }

    // ==================== HttpRequest 解析 ====================

    #[test]
    fn test_parse_http_request_get() {
        let raw = b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req = parse_http_request(raw).expect("parse failed");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/healthz");
        assert_eq!(req.header("host"), Some("localhost"));
        assert!(req.body.is_empty());
    }

    #[test]
    fn test_parse_http_request_post_with_body() {
        let raw =
            b"POST /api/v1/backup HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\nbackup!";
        let req = parse_http_request(raw).expect("parse failed");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api/v1/backup");
        assert_eq!(req.body, b"backup!");
    }

    #[test]
    fn test_parse_http_request_with_auth_header() {
        let raw = b"GET /api/v1/sessions HTTP/1.1\r\nAuthorization: Bearer my-token\r\n\r\n";
        let req = parse_http_request(raw).expect("parse failed");
        assert_eq!(req.bearer_token(), Some("my-token"));
    }

    #[test]
    fn test_parse_http_request_invalid() {
        let raw = b"invalid request";
        let result = parse_http_request(raw);
        assert!(result.is_err());
    }

    // ==================== HttpResponse ====================

    #[test]
    fn test_http_response_ok_json() {
        let resp = HttpResponse::ok_json(r#"{"status":"ok"}"#);
        let bytes = resp.to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.contains(r#"{"status":"ok"}"#));
        assert!(text.contains("Connection: close"));
    }

    #[test]
    fn test_http_response_503() {
        let resp = HttpResponse::service_unavailable(r#"{"status":"draining"}"#);
        let bytes = resp.to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    }

    #[test]
    fn test_http_response_404() {
        let resp = HttpResponse::not_found();
        let bytes = resp.to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[test]
    fn test_http_response_prometheus_format() {
        let resp = HttpResponse::prometheus("szrsql_connections_total 1\n");
        let bytes = resp.to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Content-Type: text/plain"));
        assert!(text.contains("szrsql_connections_total 1"));
    }

    // ==================== 路由 ====================

    #[test]
    fn test_route_healthz() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"GET /healthz HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains(r#"{"status":"ok"}"#));
        let _ = tx; // 保持 tx 存活
    }

    #[test]
    fn test_route_readyz_running() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"GET /readyz HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains(r#"{"status":"ready"}"#));
        let _ = tx;
    }

    #[test]
    fn test_route_readyz_draining() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        tx.send_modify(|v| *v = ShutdownState::Draining);
        let req = parse_http_request(b"GET /readyz HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("503 Service Unavailable"));
        assert!(text.contains(r#"{"status":"draining"}"#));
    }

    #[test]
    fn test_route_metrics() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        metrics.inc_connections();
        let req = parse_http_request(b"GET /metrics HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("szrsql_connections_total 1"));
        let _ = tx;
    }

    #[test]
    fn test_route_not_found() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"GET /nonexistent HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("404 Not Found"));
        let _ = tx;
    }

    #[test]
    fn test_route_method_not_allowed() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /healthz HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("405 Method Not Allowed"));
        let _ = tx;
    }

    // ==================== 鉴权 ====================

    #[test]
    fn test_route_sessions_without_auth_when_required() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = Some("secret".to_string());
        let req = parse_http_request(b"GET /api/v1/sessions HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("401 Unauthorized"));
        let _ = tx;
    }

    #[test]
    fn test_route_sessions_with_correct_auth() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = Some("secret".to_string());
        let req = parse_http_request(
            b"GET /api/v1/sessions HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n",
        )
        .unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains(r#""sessions""#));
        let _ = tx;
    }

    #[test]
    fn test_route_sessions_with_wrong_auth() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = Some("secret".to_string());
        let req = parse_http_request(
            b"GET /api/v1/sessions HTTP/1.1\r\nAuthorization: Bearer wrong\r\n\r\n",
        )
        .unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("401 Unauthorized"));
        let _ = tx;
    }

    #[test]
    fn test_route_sessions_no_auth_when_not_required() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = None;
        let req = parse_http_request(b"GET /api/v1/sessions HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        let _ = tx;
    }

    // ==================== 管理端点 ====================

    #[test]
    fn test_route_cancel_valid_pid() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/cancel/123 HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains(r#""cancelled":123"#));
        let _ = tx;
    }

    #[test]
    fn test_route_cancel_invalid_pid() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/cancel/abc HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("400 Bad Request"));
        let _ = tx;
    }

    #[test]
    fn test_route_backup_stub() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/backup HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains("backup"));
        let _ = tx;
    }

    #[test]
    fn test_route_config_reload_stub() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/config/reload HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("200 OK"));
        assert!(text.contains("config reloaded"));
        let _ = tx;
    }

    // ==================== Phase 7d.17：OpenAPI + Swagger UI ====================

    #[test]
    fn test_route_openapi_json_returns_spec() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"GET /api/v1/openapi.json HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        // 应返回 200 + application/json
        assert!(text.contains("200 OK"));
        assert!(text.contains("application/json"));
        // 应包含 OpenAPI 3.0.3 版本（pretty JSON 带空格）
        assert!(text.contains("\"openapi\""));
        assert!(text.contains("3.0.3"));
        // 应覆盖所有端点
        assert!(text.contains("/healthz"));
        assert!(text.contains("/readyz"));
        assert!(text.contains("/metrics"));
        assert!(text.contains("/api/v1/sessions"));
        assert!(text.contains("/api/v1/cancel/{pid}"));
        assert!(text.contains("/api/v1/backup"));
        assert!(text.contains("/api/v1/config/reload"));
        // 应包含 Bearer 鉴权方案
        assert!(text.contains("bearerAuth"));
        let _ = tx;
    }

    #[test]
    fn test_route_swagger_returns_html() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"GET /api/v1/swagger HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        let bytes = resp.to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        // 应返回 200 + text/html
        assert!(text.contains("200 OK"));
        assert!(text.contains("text/html"));
        // 应包含 Swagger UI 关键元素
        assert!(text.contains("<!DOCTYPE html>"));
        assert!(text.contains("swagger-ui-bundle.js"));
        assert!(text.contains(r#"url: "/api/v1/openapi.json""#));
        let _ = tx;
    }

    #[test]
    fn test_route_openapi_json_method_not_allowed() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/openapi.json HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        assert_eq!(resp.status, 405);
        let _ = tx;
    }

    #[test]
    fn test_route_swagger_method_not_allowed() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let req = parse_http_request(b"POST /api/v1/swagger HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &None, &metrics, &None, &rx);
        assert_eq!(resp.status, 405);
        let _ = tx;
    }

    #[test]
    fn test_route_openapi_no_auth_required() {
        // 即使配置了 auth_token，文档端点也不应需要鉴权
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = Some("secret".to_string());
        let req = parse_http_request(b"GET /api/v1/openapi.json HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        assert_eq!(resp.status, 200, "openapi.json should not require auth");
        let _ = tx;
    }

    #[test]
    fn test_route_swagger_no_auth_required() {
        let (tx, rx) = make_shutdown_rx();
        let metrics = MetricsRegistry::new();
        let auth_token = Some("secret".to_string());
        let req = parse_http_request(b"GET /api/v1/swagger HTTP/1.1\r\n\r\n").unwrap();
        let resp = route_request(&req, &auth_token, &metrics, &None, &rx);
        assert_eq!(resp.status, 200, "swagger page should not require auth");
        let _ = tx;
    }

    // ==================== 集成测试：HTTP 服务器 ====================

    #[tokio::test]
    async fn test_http_server_openapi_json() {
        let port = find_free_port(18500);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new().with_port(port);
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        sleep(Duration::from_millis(200)).await;

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /api/v1/openapi.json HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        let response = String::from_utf8_lossy(&buf);

        assert!(response.contains("200 OK"));
        assert!(response.contains("application/json"));
        assert!(response.contains("\"openapi\""));
        assert!(response.contains("3.0.3"));
        assert!(response.contains("/healthz"));
        assert!(response.contains("/api/v1/sessions"));

        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_http_server_swagger_page() {
        let port = find_free_port(18600);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new().with_port(port);
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        sleep(Duration::from_millis(200)).await;

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /api/v1/swagger HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        let response = String::from_utf8_lossy(&buf);

        assert!(response.contains("200 OK"));
        assert!(response.contains("text/html"));
        assert!(response.contains("<!DOCTYPE html>"));
        assert!(response.contains("swagger-ui-bundle.js"));

        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_http_server_healthz() {
        let port = find_free_port(18100);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new().with_port(port);
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        // 等待服务器就绪
        sleep(Duration::from_millis(200)).await;

        // 发送 GET /healthz 请求
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        let response = String::from_utf8_lossy(&buf);

        assert!(response.contains("200 OK"));
        assert!(response.contains(r#"{"status":"ok"}"#));

        // 关闭服务器
        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_http_server_metrics() {
        let port = find_free_port(18200);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        metrics.inc_connections();
        metrics.inc_queries();
        let config = HttpConfig::new().with_port(port);
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        sleep(Duration::from_millis(200)).await;

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        let response = String::from_utf8_lossy(&buf);

        assert!(response.contains("200 OK"));
        assert!(response.contains("szrsql_connections_total 1"));
        assert!(response.contains("szrsql_queries_total 1"));

        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_http_server_readyz_draining() {
        let port = find_free_port(18300);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new().with_port(port);
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        sleep(Duration::from_millis(200)).await;

        // 切换到 Draining
        tx.send_modify(|v| *v = ShutdownState::Draining);
        sleep(Duration::from_millis(100)).await;

        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        let response = String::from_utf8_lossy(&buf);

        assert!(response.contains("503 Service Unavailable"));
        assert!(response.contains(r#"{"status":"draining"}"#));

        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_http_server_disabled_when_port_zero() {
        let (_tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new(); // port=0
        let server = HttpServer::new(config, metrics, rx);

        // port=0 应立即返回 Ok(())
        let result = tokio::time::timeout(Duration::from_secs(1), server.serve()).await;
        assert!(
            result.is_ok(),
            "server should return immediately when disabled"
        );
        assert!(result.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_http_server_auth_required() {
        let port = find_free_port(18400);
        let (tx, rx) = make_shutdown_rx();
        let metrics = Arc::new(MetricsRegistry::new());
        let config = HttpConfig::new()
            .with_port(port)
            .with_auth_token("secret-token");
        let server = HttpServer::new(config, Arc::clone(&metrics), rx);

        let handle = tokio::spawn(async move { server.serve().await });

        sleep(Duration::from_millis(200)).await;

        // 无 auth header → 401
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /api/v1/sessions HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write failed");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        assert!(String::from_utf8_lossy(&buf).contains("401 Unauthorized"));

        // 有正确 auth header → 200
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect failed");
        stream
            .write_all(b"GET /api/v1/sessions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret-token\r\n\r\n")
            .await
            .expect("write failed");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read failed");
        assert!(String::from_utf8_lossy(&buf).contains("200 OK"));

        tx.send_modify(|v| *v = ShutdownState::Closed);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    /// 辅助：查找可用端口。
    fn find_free_port(start: u16) -> u16 {
        for port in start..start + 100 {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        panic!("no free port found");
    }

    /// 辅助：sleep（避免在每个测试中导入 tokio::time::sleep）。
    async fn sleep(duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
