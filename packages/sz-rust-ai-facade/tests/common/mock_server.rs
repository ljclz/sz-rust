//! Mock HTTP Server — 本地 127.0.0.1 随机端口 axum server
//!
//! 按预设 `VecDeque<MockResponse>` 序列返回响应，供集成测试使用。
//! 所有 HTTP 指向 127.0.0.1 随机端口，禁止真实网络（spec 5.1.1.7）。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 预设的 mock 响应
#[derive(Debug, Clone)]
pub struct MockResponse {
    /// HTTP 状态码
    pub status: u16,
    /// 响应体
    pub body: String,
    /// 响应延迟（毫秒），仅在 mock server 内部使用
    pub delay_ms: u64,
    /// 响应头
    pub headers: Vec<(String, String)>,
}

impl MockResponse {
    /// 创建 200 JSON 响应
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            delay_ms: 0,
            headers: vec![("content-type".into(), "application/json".into())],
        }
    }

    /// 创建指定状态码响应
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
            delay_ms: 0,
            headers: Vec::new(),
        }
    }

    /// 设置延迟
    pub fn with_delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

/// Mock HTTP Server
pub struct MockHttpServer {
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl MockHttpServer {
    /// 启动 mock server，按预设序列返回响应
    ///
    /// 响应按 FIFO 顺序返回，队列为空时返回 500。
    pub async fn start(responses: Vec<MockResponse>) -> std::io::Result<Self> {
        let queue: Arc<Mutex<VecDeque<MockResponse>>> = Arc::new(Mutex::new(responses.into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{}", addr);

        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let q = queue_clone.clone();
                tokio::spawn(async move {
                    Self::handle_connection(stream, q).await;
                });
            }
        });

        Ok(Self { base_url, handle })
    }

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        queue: Arc<Mutex<VecDeque<MockResponse>>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = [0u8; 4096];
        let mut stream = stream;
        let _ = stream.read(&mut buf).await;

        let resp = {
            let mut q = queue.lock().await;
            q.pop_front()
        };

        let resp = resp.unwrap_or_else(|| MockResponse::status(500));
        if resp.delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(resp.delay_ms)).await;
        }

        let mut header_str = String::new();
        for (k, v) in &resp.headers {
            header_str.push_str(&format!("{k}: {v}\r\n"));
        }

        let response = format!(
            "HTTP/1.1 {} OK\r\n{header_str}content-length: {}\r\n\r\n{}",
            resp.status,
            resp.body.len(),
            resp.body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
    }

    /// 获取 base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 停止 server 并释放端口
    pub fn stop(self) {
        self.handle.abort();
    }
}
