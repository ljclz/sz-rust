//! P2-2.2: TCP 复制传输层 — 跨节点流复制
//!
//! # 设计
//!
//! 将 `ReplicationPrimary` 的进程内 `mpsc` 通道替换为 TCP socket，支持跨节点复制：
//!
//! - **`TcpReplicationServer`**（主库侧）：监听 TCP 端口，接受备库连接，
//!   为每个备库创建一个 `ReplicaConn` 任务，从 `ReplicationPrimary` 接收消息
//!   并通过 TCP 转发。
//! - **`TcpReplicationClient`**（备库侧）：连接主库 TCP 地址，接收 `ReplicationMessage`，
//!   通过 `UnboundedSender` 注入到 `ReplicationReplica::run` 消费的消息通道。
//!
//! # 帧格式
//!
//! 使用长度前缀帧（length-prefixed framing）：
//! ```text
//! +-------------------+-----------------------+
//! | payload_len (u32) | payload bytes (bincode)|
//! | 4 bytes BE        | variable length       |
//! +-------------------+-----------------------+
//! ```
//!
//! - `payload_len`：4 字节大端 u32，标识后续 payload 字节数
//! - `payload`：bincode 序列化的 `ReplicationMessage`
//!
//! # 心跳与连接保活
//!
//! - 主库周期性发送 `ReplicationMessage::Heartbeat`（默认 10 秒间隔）
//! - 备库收到心跳后更新本地 LSN，不回写（简化协议，与 PG 的 standby status update 不同）
//! - TCP keepalive 由 tokio TcpStream 的 `set_nodelay` 控制
//!
//! # 集成
//!
//! - 主库侧：`main.rs` 启动 `TcpReplicationServer::spawn(primary, addr)`
//! - 备库侧：`main.rs` 通过 `--replica-of <addr>` 参数启动 `TcpReplicationClient::connect`
//!
//! # 错误处理
//!
//! - TCP 连接断开时，主库侧移除备库注册，备库侧退出接收循环
//! - 序列化/反序列化错误导致连接关闭（协议不兼容）
//! - 主库 `ReplicationPrimary::accept_replica` 仍可用（进程内通道），TCP 是额外传输层

use std::net::SocketAddr;
use std::sync::Arc;

use bincode::{deserialize, serialize};
use szrsql_tx::wal::WalRecord;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::stream::{ReplicationMessage, ReplicationPrimary};

// =====================================================================
//  错误类型
// =====================================================================

/// TCP 复制传输错误
#[derive(Debug, Error)]
pub enum TcpTransportError {
    /// I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 序列化错误
    #[error("serialize error: {0}")]
    Serialize(String),
    /// 反序列化错误
    #[error("deserialize error: {0}")]
    Deserialize(String),
    /// 帧长度超过上限（防止恶意大帧导致 OOM）
    #[error("frame too large: {len} bytes (max {max})")]
    FrameTooLarge { len: usize, max: usize },
    /// 连接已关闭
    #[error("connection closed")]
    ConnectionClosed,
}

// =====================================================================
//  常量
// =====================================================================

/// 最大帧 payload 长度（64 MB），防止恶意大帧导致 OOM
const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

/// 默认心跳间隔（秒）
const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 10;

// =====================================================================
//  TcpReplicationServer — 主库侧 TCP 复制服务器
// =====================================================================

/// 主库侧 TCP 复制服务器
///
/// 监听指定地址，接受备库连接，为每个备库创建独立的转发任务。
/// 从 `ReplicationPrimary` 获取消息接收端，将消息通过 TCP 转发给备库。
pub struct TcpReplicationServer {
    /// 监听地址
    addr: SocketAddr,
    /// 主库实例
    primary: Arc<ReplicationPrimary>,
    /// 心跳间隔（秒）
    heartbeat_interval_secs: u64,
}

impl TcpReplicationServer {
    /// 创建 TCP 复制服务器
    ///
    /// # 参数
    /// - `primary` — 主库实例（`Arc` 共享）
    /// - `addr` — 监听地址（如 `0.0.0.0:5434`）
    pub fn new(primary: Arc<ReplicationPrimary>, addr: SocketAddr) -> Self {
        Self {
            addr,
            primary,
            heartbeat_interval_secs: DEFAULT_HEARTBEAT_INTERVAL_SECS,
        }
    }

    /// 设置心跳间隔（秒）
    pub fn with_heartbeat_interval(mut self, secs: u64) -> Self {
        self.heartbeat_interval_secs = secs;
        self
    }

    /// 启动 TCP 复制服务器（异步）
    ///
    /// 返回 `JoinHandle`，调用方可通过 `.await` 等待服务器退出（通常不会退出，
    /// 除非监听失败或所有任务完成）。
    ///
    /// # 错误
    /// - 绑定地址失败（端口被占用）
    pub async fn spawn(self) -> Result<JoinHandle<()>, TcpTransportError> {
        let listener = TcpListener::bind(self.addr).await?;
        let primary = self.primary.clone();
        let heartbeat_interval = self.heartbeat_interval_secs;

        info!(
            listen_addr = %self.addr,
            heartbeat_interval_secs = heartbeat_interval,
            "P2-2.2: TCP replication server listening"
        );

        let handle = tokio::spawn(async move {
            let mut replica_counter: u64 = 0;
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        replica_counter += 1;
                        let replica_id = format!("tcp-replica-{}-{}", replica_counter, peer_addr);
                        info!(
                            replica_id = %replica_id,
                            peer_addr = %peer_addr,
                            "P2-2.2: replica connected"
                        );

                        // 为备库创建接收通道
                        match primary.accept_replica(&replica_id, 0) {
                            Ok(rx) => {
                                let primary_clone = primary.clone();
                                let interval = heartbeat_interval;
                                tokio::spawn(async move {
                                    if let Err(e) = serve_replica(
                                        stream,
                                        rx,
                                        &primary_clone,
                                        &replica_id,
                                        interval,
                                    )
                                    .await
                                    {
                                        warn!(
                                            replica_id = %replica_id,
                                            error = %e,
                                            "P2-2.2: replica connection ended with error"
                                        );
                                    }
                                    // 清理：移除备库注册
                                    let _ = primary_clone.remove_replica(&replica_id);
                                });
                            }
                            Err(e) => {
                                warn!(
                                    replica_id = %replica_id,
                                    error = %e,
                                    "P2-2.2: failed to accept replica into primary"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "P2-2.2: accept failed, retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });

        Ok(handle)
    }
}

/// 服务单个备库连接
///
/// 从 `ReplicationPrimary` 的消息接收端读取 `ReplicationMessage`，
/// 序列化为长度前缀帧并写入 TCP 流。
async fn serve_replica(
    mut stream: TcpStream,
    mut rx: UnboundedReceiver<ReplicationMessage>,
    primary: &ReplicationPrimary,
    replica_id: &str,
    heartbeat_interval_secs: u64,
) -> Result<(), TcpTransportError> {
    stream.set_nodelay(true)?;

    let mut heartbeat_timer =
        tokio::time::interval(std::time::Duration::from_secs(heartbeat_interval_secs));
    // 第一帧立即触发（tokio::interval 默认行为），跳过避免重复
    heartbeat_timer.tick().await;

    loop {
        tokio::select! {
            // 主库推送的消息
            msg = rx.recv() => {
                match msg {
                    Some(message) => {
                        if let Err(e) = write_frame(&mut stream, &message).await {
                            warn!(replica_id = %replica_id, error = %e, "P2-2.2: write frame failed");
                            return Err(e);
                        }
                        // 更新备库确认 LSN（基于消息内容）
                        match &message {
                            ReplicationMessage::WalBatch { end_lsn, .. } => {
                                primary.update_confirmed_lsn(replica_id, *end_lsn);
                            }
                            ReplicationMessage::Heartbeat { current_lsn } => {
                                primary.update_confirmed_lsn(replica_id, *current_lsn);
                            }
                            ReplicationMessage::Eof => {
                                info!(replica_id = %replica_id, "P2-2.2: sent Eof, closing");
                                return Ok(());
                            }
                        }
                    }
                    None => {
                        // 通道关闭（主库崩溃或主动 shutdown）
                        info!(replica_id = %replica_id, "P2-2.2: primary channel closed, sending Eof");
                        let _ = write_frame(&mut stream, &ReplicationMessage::Eof).await;
                        return Ok(());
                    }
                }
            }
            // 心跳定时器
            _ = heartbeat_timer.tick() => {
                let current_lsn = primary.current_lsn();
                let hb = ReplicationMessage::Heartbeat { current_lsn };
                if let Err(e) = write_frame(&mut stream, &hb).await {
                    warn!(replica_id = %replica_id, error = %e, "P2-2.2: heartbeat write failed");
                    return Err(e);
                }
            }
        }
    }
}

// =====================================================================
//  TcpReplicationClient — 备库侧 TCP 复制客户端
// =====================================================================

/// 备库侧 TCP 复制客户端
///
/// 连接主库 TCP 地址，接收 `ReplicationMessage`，通过 `UnboundedSender` 注入到
/// `ReplicationReplica::run` 消费的消息通道。
pub struct TcpReplicationClient {
    /// 主库地址
    primary_addr: SocketAddr,
}

impl TcpReplicationClient {
    /// 创建 TCP 复制客户端
    ///
    /// # 参数
    /// - `primary_addr` — 主库地址（如 `192.168.1.10:5434`）
    pub fn new(primary_addr: SocketAddr) -> Self {
        Self { primary_addr }
    }

    /// 连接主库并运行接收循环
    ///
    /// 连接成功后持续读取 TCP 帧并反序列化为 `ReplicationMessage`，
    /// 通过 `tx` 发送到 `ReplicationReplica::run` 消费的通道。
    ///
    /// 当连接断开或收到 Eof 时退出循环。
    ///
    /// # 参数
    /// - `tx` — 消息发送端（注入到 `ReplicationReplica::run` 的通道）
    ///
    /// # 错误
    /// - 连接主库失败
    /// - 读帧或反序列化失败
    pub async fn run(
        self,
        tx: UnboundedSender<ReplicationMessage>,
    ) -> Result<(), TcpTransportError> {
        let mut stream = TcpStream::connect(self.primary_addr).await?;
        stream.set_nodelay(true)?;

        info!(
            primary_addr = %self.primary_addr,
            "P2-2.2: connected to primary, receiving replication stream"
        );

        loop {
            match read_frame(&mut stream).await? {
                Some(message) => {
                    let is_eof = matches!(message, ReplicationMessage::Eof);
                    if tx.send(message).is_err() {
                        warn!("P2-2.2: replica channel closed, disconnecting from primary");
                        return Ok(());
                    }
                    if is_eof {
                        info!("P2-2.2: received Eof from primary, disconnecting");
                        return Ok(());
                    }
                }
                None => {
                    warn!("P2-2.2: primary connection closed (EOF)");
                    return Ok(());
                }
            }
        }
    }

    /// 连接主库（带重试）
    ///
    /// 在 `max_retries` 次内尝试连接，每次间隔 `retry_delay`。
    /// 成功后返回 `(TcpStream, JoinHandle)`，JoinHandle 为接收循环任务。
    ///
    /// # 参数
    /// - `max_retries` — 最大重试次数（0 表示不重试）
    /// - `retry_delay` — 重试间隔
    /// - `tx` — 消息发送端
    pub async fn connect_with_retry(
        self,
        max_retries: u32,
        retry_delay: std::time::Duration,
        tx: UnboundedSender<ReplicationMessage>,
    ) -> Result<JoinHandle<()>, TcpTransportError> {
        let mut last_err = None;
        for attempt in 0..=max_retries {
            match TcpStream::connect(self.primary_addr).await {
                Ok(stream) => {
                    info!(
                        primary_addr = %self.primary_addr,
                        attempt = attempt,
                        "P2-2.2: connected to primary"
                    );
                    let handle = tokio::spawn(async move {
                        if let Err(e) = Self::run_with_stream(stream, tx).await {
                            warn!(error = %e, "P2-2.2: replication client exited with error");
                        }
                    });
                    return Ok(handle);
                }
                Err(e) => {
                    if attempt < max_retries {
                        warn!(
                            primary_addr = %self.primary_addr,
                            attempt = attempt,
                            max_retries = max_retries,
                            error = %e,
                            "P2-2.2: connect failed, retrying"
                        );
                        tokio::time::sleep(retry_delay).await;
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(TcpTransportError::Io(last_err.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connect failed")
        })))
    }

    /// 使用已建立的 TCP 流运行接收循环（内部辅助方法）
    async fn run_with_stream(
        mut stream: TcpStream,
        tx: UnboundedSender<ReplicationMessage>,
    ) -> Result<(), TcpTransportError> {
        stream.set_nodelay(true)?;
        loop {
            match read_frame(&mut stream).await? {
                Some(message) => {
                    let is_eof = matches!(message, ReplicationMessage::Eof);
                    if tx.send(message).is_err() {
                        return Ok(());
                    }
                    if is_eof {
                        return Ok(());
                    }
                }
                None => return Ok(()),
            }
        }
    }
}

// =====================================================================
//  帧编解码 — 长度前缀帧
// =====================================================================

/// 写入一个长度前缀帧
///
/// 格式：`[u32 BE payload_len][payload bytes]`
///
/// # 参数
/// - `stream` — TCP 流（或任何实现 `AsyncWrite + Unpin` 的对象）
/// - `message` — 要发送的复制消息
pub async fn write_frame<W>(
    stream: &mut W,
    message: &ReplicationMessage,
) -> Result<(), TcpTransportError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serialize(message).map_err(|e| TcpTransportError::Serialize(e.to_string()))?;
    let len = payload.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

/// 读取一个长度前缀帧
///
/// 返回 `Ok(Some(message))` 成功读取一帧；
/// 返回 `Ok(None)` 表示对端关闭连接（EOF）；
/// 返回 `Err` 表示 I/O 或协议错误。
///
/// # 参数
/// - `stream` — TCP 流（或任何实现 `AsyncRead + Unpin` 的对象）
pub async fn read_frame<R>(stream: &mut R) -> Result<Option<ReplicationMessage>, TcpTransportError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    // 读 4 字节长度前缀，EOF 时返回 None
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(TcpTransportError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let message: ReplicationMessage =
        deserialize(&payload).map_err(|e| TcpTransportError::Deserialize(e.to_string()))?;
    Ok(Some(message))
}

// =====================================================================
//  辅助：构造测试用 WAL 批次
// =====================================================================

/// 构造测试用 WAL 批次消息（供测试和 main.rs 初始化使用）
pub fn make_wal_batch(records: Vec<WalRecord>, start_lsn: u64, end_lsn: u64) -> ReplicationMessage {
    ReplicationMessage::WalBatch {
        records,
        start_lsn,
        end_lsn,
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_tx::wal::{WalOpType, WalRecord};
    use tokio::sync::mpsc;

    /// 测试帧编解码往返（WalBatch）
    #[tokio::test]
    async fn test_frame_roundtrip_wal_batch() {
        let records = vec![
            WalRecord::new(1, 10, WalOpType::Insert, 5, vec![1, 2, 3]),
            WalRecord::new(2, 10, WalOpType::Update, 6, vec![4, 5, 6, 7]),
            WalRecord::new(3, 11, WalOpType::Commit, 0, vec![]),
        ];
        let original = ReplicationMessage::WalBatch {
            records,
            start_lsn: 1,
            end_lsn: 3,
        };

        // 使用内存双向流模拟 TCP
        let (mut server, mut client) = tokio::io::duplex(8192);

        // 写入帧
        write_frame(&mut server, &original).await.unwrap();

        // 读取帧
        let decoded = read_frame(&mut client).await.unwrap().unwrap();

        assert_eq!(decoded, original);
    }

    /// 测试帧编解码往返（Heartbeat）
    #[tokio::test]
    async fn test_frame_roundtrip_heartbeat() {
        let original = ReplicationMessage::Heartbeat { current_lsn: 42 };

        let (mut server, mut client) = tokio::io::duplex(8192);
        write_frame(&mut server, &original).await.unwrap();
        let decoded = read_frame(&mut client).await.unwrap().unwrap();

        assert_eq!(decoded, original);
    }

    /// 测试帧编解码往返（Eof）
    #[tokio::test]
    async fn test_frame_roundtrip_eof() {
        let original = ReplicationMessage::Eof;

        let (mut server, mut client) = tokio::io::duplex(8192);
        write_frame(&mut server, &original).await.unwrap();
        let decoded = read_frame(&mut client).await.unwrap().unwrap();

        assert_eq!(decoded, original);
    }

    /// 测试 EOF 时返回 None
    #[tokio::test]
    async fn test_read_frame_eof_returns_none() {
        let (_server, mut client) = tokio::io::duplex(8192);
        drop(_server);
        let result = read_frame(&mut client).await.unwrap();
        assert!(result.is_none());
    }

    /// 测试多帧连续传输
    #[tokio::test]
    async fn test_multiple_frames_sequential() {
        let (mut server, mut client) = tokio::io::duplex(8192);

        let messages = vec![
            ReplicationMessage::Heartbeat { current_lsn: 1 },
            ReplicationMessage::WalBatch {
                records: vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0xAA])],
                start_lsn: 1,
                end_lsn: 1,
            },
            ReplicationMessage::Heartbeat { current_lsn: 2 },
            ReplicationMessage::Eof,
        ];

        // 写入所有帧
        for msg in &messages {
            write_frame(&mut server, msg).await.unwrap();
        }

        // 依次读取并验证
        for expected in &messages {
            let decoded = read_frame(&mut client).await.unwrap().unwrap();
            assert_eq!(&decoded, expected);
        }
    }

    /// 测试 TCP 复制服务器端到端
    #[tokio::test]
    async fn test_tcp_replication_server_end_to_end() {
        let primary = Arc::new(ReplicationPrimary::new("test_primary"));

        // 推送一条 WAL 记录
        let records = vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0xBB; 100])];
        let end_lsn = primary.append_records(records);

        // 启动 TCP 服务器
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(addr).await.unwrap();
        let actual_addr = listener.local_addr().unwrap();

        let primary_clone = primary.clone();
        let server_handle = tokio::spawn(async move {
            // 接受一个备库连接
            let (stream, peer_addr) = listener.accept().await.unwrap();
            let replica_id = format!("tcp-test-{}", peer_addr);
            let rx = primary_clone.accept_replica(&replica_id, 0).unwrap();
            serve_replica(stream, rx, &primary_clone, &replica_id, 60).await
        });

        // 启动客户端连接
        let (tx, mut rx) = mpsc::unbounded_channel::<ReplicationMessage>();
        let client_handle =
            tokio::spawn(async move { TcpReplicationClient::new(actual_addr).run(tx).await });

        // 等待接收 WalBatch
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for message");

        match msg {
            Some(ReplicationMessage::WalBatch {
                records,
                start_lsn,
                end_lsn: end,
            }) => {
                assert!(!records.is_empty());
                // WalRecord::new(1, 1, ...) 的 lsn=1，所以 start_lsn=1
                assert_eq!(start_lsn, 1);
                assert_eq!(end, end_lsn);
            }
            other => panic!("expected WalBatch, got {:?}", other),
        }

        // 推送一条新记录
        let records2 = vec![WalRecord::new(2, 2, WalOpType::Update, 1, vec![0xCC; 50])];
        let end_lsn2 = primary.append_records(records2);

        let msg2 = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for second message");

        match msg2 {
            Some(ReplicationMessage::WalBatch {
                records, end_lsn, ..
            }) => {
                assert_eq!(records.len(), 1);
                assert_eq!(end_lsn, end_lsn2);
            }
            other => panic!("expected WalBatch, got {:?}", other),
        }

        // 优雅关闭
        primary.shutdown();
        let _ = server_handle.await;
        let _ = client_handle.await;
    }

    /// 测试大帧传输（10000 条 WAL 记录）
    ///
    /// 使用并发写入/读取，避免 duplex 缓冲区满导致死锁
    /// （10000 条 64B 记录 ≈ 700KB，远超 duplex 默认缓冲区）。
    #[tokio::test]
    async fn test_large_frame_10k_records() {
        let records: Vec<WalRecord> = (0..10000)
            .map(|i| WalRecord::new(i, 1, WalOpType::Insert, i as u32, vec![i as u8; 64]))
            .collect();
        let original = ReplicationMessage::WalBatch {
            records,
            start_lsn: 0,
            end_lsn: 9999,
        };

        // 并发写入和读取，避免缓冲区满死锁
        let (mut server, mut client) = tokio::io::duplex(65536);
        let write_handle = tokio::spawn(async move { write_frame(&mut server, &original).await });
        let read_handle = tokio::spawn(async move { read_frame(&mut client).await });

        write_handle.await.unwrap().unwrap();
        let decoded = read_handle.await.unwrap().unwrap().unwrap();

        if let ReplicationMessage::WalBatch {
            records,
            start_lsn,
            end_lsn,
        } = decoded
        {
            assert_eq!(records.len(), 10000);
            assert_eq!(start_lsn, 0);
            assert_eq!(end_lsn, 9999);
        } else {
            panic!("expected WalBatch");
        }
    }

    /// 测试 connect_with_retry 在主库不可用时重试
    #[tokio::test]
    async fn test_connect_with_retry_failure() {
        // 使用一个未监听的端口
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = TcpReplicationClient::new(addr);
        let (tx, _rx) = mpsc::unbounded_channel::<ReplicationMessage>();

        let result = client
            .connect_with_retry(2, std::time::Duration::from_millis(10), tx)
            .await;
        assert!(
            result.is_err(),
            "should fail to connect to unavailable primary"
        );
    }

    /// 测试 make_wal_batch 辅助函数
    #[test]
    fn test_make_wal_batch() {
        let records = vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![])];
        let msg = make_wal_batch(records, 1, 1);
        match msg {
            ReplicationMessage::WalBatch {
                records,
                start_lsn,
                end_lsn,
            } => {
                assert_eq!(records.len(), 1);
                assert_eq!(start_lsn, 1);
                assert_eq!(end_lsn, 1);
            }
            _ => panic!("expected WalBatch"),
        }
    }
}
