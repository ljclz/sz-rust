//! 阶段 4：生产级 TCP 网络层
//!
//! 替代 `InMemoryNetwork`（仅用于单进程测试），提供基于 TCP 的真实跨进程/跨节点通信。
//!
//! # 设计
//!
//! - **`TcpNetwork`** 实现 `RaftNetwork` trait，通过 TCP 发送 `RpcMessage`
//! - **监听线程**：每个节点启动一个 `TcpListener`，后台线程接受连接、反序列化消息、入队
//! - **连接复用**：可选的连接池（当前简化为每次新建连接，Raft 消息频率不高）
//! - **序列化**：使用 `bincode` 对 `RpcMessage` 进行二进制编码
//! - **线程安全**：inbox 使用 `Mutex<Vec<RpcMessage>>`，与 `InMemoryNetwork` API 一致
//!
//! # 与 `InMemoryNetwork` 的对比
//!
//! | 特性        | `InMemoryNetwork` | `TcpNetwork`           |
//! |------------|-------------------|------------------------|
//! | 部署模式    | 单进程多节点测试    | 多进程/多机部署         |
//! | 传输        | 内存队列           | TCP socket             |
//! | 故障注入    | 支持（offline/partition） | 不支持（生产环境）|
//! | 消息延迟    | ~0                | 网络 RTT               |
//! | 持久连接    | N/A               | 可选连接池              |
//!
//! # 用法
//!
//! ```ignore
//! use std::net::SocketAddr;
//! use szrsql_dist::network::TcpNetwork;
//! use szrsql_dist::raft::{RaftNetwork, RpcMessage};
//!
//! // 节点 1 监听 127.0.0.1:5001，节点 2 监听 127.0.0.1:5002
//! let mut net1 = TcpNetwork::new(1);
//! net1.start_listener("127.0.0.1:5001".parse().unwrap());
//! net1.add_peer(2, "127.0.0.1:5002".parse().unwrap());
//!
//! let mut net2 = TcpNetwork::new(2);
//! net2.start_listener("127.0.0.1:5002".parse().unwrap());
//! net2.add_peer(1, "127.0.0.1:5001".parse().unwrap());
//!
//! // 节点 1 发送消息给节点 2
//! net1.send(1, 2, RpcMessage::new(1, 2, MessageType::AppendEntriesRequest, vec![]));
//! std::thread::sleep(std::time::Duration::from_millis(10));
//! let received = net2.drain();
//! assert_eq!(received.len(), 1);
//! ```

use crate::raft::{NodeId, RaftNetwork, RpcMessage};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// =====================================================================
//  TcpNetwork — TCP 网络实现
// =====================================================================

/// TCP 网络实现：生产环境使用的跨节点通信层。
///
/// 每个 `TcpNetwork` 实例：
/// 1. 持有本节点 ID
/// 2. 维护 peer 节点 ID → SocketAddr 映射
/// 3. 后台监听线程接受连接，将消息入队
/// 4. `send()` 通过 TCP 连接到目标节点并发送序列化消息
///
/// **线程安全**：inbox 和 peer_map 均使用 `Mutex` 保护，
/// 监听线程通过 `Arc` 共享 inbox。
pub struct TcpNetwork {
    /// 本节点 ID
    node_id: NodeId,
    /// 收到的消息队列（监听线程写入，drain 读取）
    inbox: Arc<Mutex<Vec<RpcMessage>>>,
    /// peer 节点 ID → SocketAddr 映射
    peers: Mutex<HashMap<NodeId, SocketAddr>>,
    /// 监听线程停止标志
    stop_flag: Arc<AtomicBool>,
    /// 监听线程句柄（Option 用于支持 take 停止）
    listener_handle: Mutex<Option<thread::JoinHandle<()>>>,
    /// 本节点监听地址（start_listener 后填充）
    listen_addr: Mutex<Option<SocketAddr>>,
}

impl TcpNetwork {
    /// 创建 TCP 网络
    ///
    /// # 参数
    /// - `node_id`：本节点 ID
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            inbox: Arc::new(Mutex::new(Vec::new())),
            peers: Mutex::new(HashMap::new()),
            stop_flag: Arc::new(AtomicBool::new(false)),
            listener_handle: Mutex::new(None),
            listen_addr: Mutex::new(None),
        }
    }

    /// 添加 peer 节点地址
    ///
    /// 在发送消息前，必须先添加目标节点的地址。
    pub fn add_peer(&self, peer_id: NodeId, addr: SocketAddr) {
        if let Ok(mut peers) = self.peers.lock() {
            peers.insert(peer_id, addr);
        }
    }

    /// 批量添加 peer 节点地址
    pub fn add_peers(&self, peers: impl IntoIterator<Item = (NodeId, SocketAddr)>) {
        if let Ok(mut p) = self.peers.lock() {
            for (id, addr) in peers {
                p.insert(id, addr);
            }
        }
    }

    /// 启动监听线程
    ///
    /// 在指定地址绑定 `TcpListener`，后台线程接受连接并处理消息。
    /// 必须在 `send()` 之前调用（其他节点需要连接到本节点）。
    ///
    /// # Errors
    /// - 绑定失败（端口被占用）
    pub fn start_listener(&self, addr: SocketAddr) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(false)?;
        // 设置 SO_REUSEADDR 避免测试时 TIME_WAIT 占用端口
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                let optval: libc::c_int = 1;
                libc::setsockopt(
                    listener.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_REUSEADDR,
                    &optval as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }

        if let Ok(mut la) = self.listen_addr.lock() {
            *la = Some(addr);
        }

        let inbox = Arc::clone(&self.inbox);
        let stop_flag = Arc::clone(&self.stop_flag);
        let local_node_id = self.node_id;

        let handle = thread::Builder::new()
            .name(format!("tcp-listener-{}", local_node_id))
            .spawn(move || {
                // 设置读取超时，便于周期性检查 stop_flag
                listener
                    .set_nonblocking(true)
                    .unwrap_or_else(|e| tracing::warn!("set_nonblocking 失败: {}", e));

                while !stop_flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, peer_addr)) => {
                            // 读取 4 字节长度前缀
                            let mut len_buf = [0u8; 4];
                            if let Err(e) = stream.read_exact(&mut len_buf) {
                                tracing::warn!(
                                    node = local_node_id,
                                    peer = %peer_addr,
                                    error = %e,
                                    "读取消息长度失败"
                                );
                                continue;
                            }
                            let msg_len = u32::from_be_bytes(len_buf) as usize;

                            // 限制消息大小（防止恶意大消息）
                            if msg_len > 64 * 1024 * 1024 {
                                tracing::warn!(
                                    node = local_node_id,
                                    peer = %peer_addr,
                                    len = msg_len,
                                    "消息过大，丢弃"
                                );
                                continue;
                            }

                            // 读取消息体
                            let mut msg_buf = vec![0u8; msg_len];
                            if let Err(e) = stream.read_exact(&mut msg_buf) {
                                tracing::warn!(
                                    node = local_node_id,
                                    peer = %peer_addr,
                                    error = %e,
                                    "读取消息体失败"
                                );
                                continue;
                            }

                            // 反序列化
                            match bincode::deserialize::<RpcMessage>(&msg_buf) {
                                Ok(msg) => {
                                    if let Ok(mut inbox) = inbox.lock() {
                                        inbox.push(msg);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        node = local_node_id,
                                        peer = %peer_addr,
                                        error = %e,
                                        "反序列化消息失败"
                                    );
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // 非阻塞模式无连接，短暂休眠
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(e) => {
                            tracing::warn!(
                                node = local_node_id,
                                error = %e,
                                "accept 失败"
                            );
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
                tracing::info!(node = local_node_id, "TCP 监听线程退出");
            })?;

        if let Ok(mut h) = self.listener_handle.lock() {
            *h = Some(handle);
        }
        tracing::info!(
            node = self.node_id,
            addr = %addr,
            "TCP 监听线程已启动"
        );
        Ok(())
    }

    /// 停止监听线程
    pub fn stop_listener(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut h) = self.listener_handle.lock() {
            if let Some(handle) = h.take() {
                let _ = handle.join();
            }
        }
    }

    /// 取出所有已收到的消息（清空队列）
    ///
    /// 与 `InMemoryNetwork::drain` API 一致，便于在 `DistCluster` 或
    /// 生产代码中复用相同的 tick + drain + deliver 模式。
    pub fn drain(&self) -> Vec<RpcMessage> {
        if let Ok(mut inbox) = self.inbox.lock() {
            std::mem::take(&mut *inbox)
        } else {
            Vec::new()
        }
    }

    /// 待投递消息数
    pub fn pending_count(&self) -> usize {
        self.inbox.lock().map(|i| i.len()).unwrap_or(0)
    }

    /// 获取本节点 ID
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// 获取本节点监听地址
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr.lock().ok().and_then(|a| *a)
    }

    /// 获取 peer 数量
    pub fn peer_count(&self) -> usize {
        self.peers.lock().map(|p| p.len()).unwrap_or(0)
    }

    /// 内部发送方法：序列化消息并通过 TCP 发送到目标节点
    fn send_internal(
        &self,
        from: NodeId,
        to: NodeId,
        msg: RpcMessage,
    ) -> Result<(), TcpNetworkError> {
        // 查找目标节点地址
        let addr = {
            let peers = self
                .peers
                .lock()
                .map_err(|_| TcpNetworkError::LockPoisoned)?;
            peers.get(&to).copied()
        };

        let addr = addr.ok_or(TcpNetworkError::PeerNotFound(to))?;

        // 序列化消息
        let encoded =
            bincode::serialize(&msg).map_err(|e| TcpNetworkError::Serialize(e.to_string()))?;

        // 连接目标节点并发送（带超时）
        let stream_result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));
        let mut stream = stream_result.map_err(|e| TcpNetworkError::Connect {
            peer: to,
            addr,
            source: e,
        })?;

        // 写入 4 字节长度前缀 + 消息体
        let len = encoded.len() as u32;
        stream
            .write_all(&len.to_be_bytes())
            .map_err(|e| TcpNetworkError::Io(e))?;
        stream
            .write_all(&encoded)
            .map_err(|e| TcpNetworkError::Io(e))?;
        stream.flush().map_err(|e| TcpNetworkError::Io(e))?;

        let _ = from; // from 用于日志，此处不记录
        Ok(())
    }
}

impl RaftNetwork for TcpNetwork {
    fn send(&self, from: NodeId, to: NodeId, msg: RpcMessage) {
        if let Err(e) = self.send_internal(from, to, msg) {
            tracing::warn!(
                node = self.node_id,
                from,
                to,
                error = %e,
                "TCP 发送消息失败"
            );
        }
    }
}

impl Drop for TcpNetwork {
    fn drop(&mut self) {
        self.stop_listener();
    }
}

impl std::fmt::Debug for TcpNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpNetwork")
            .field("node_id", &self.node_id)
            .field(
                "listen_addr",
                &self.listen_addr.lock().ok().and_then(|a| *a),
            )
            .field("peer_count", &self.peer_count())
            .field("pending_count", &self.pending_count())
            .finish()
    }
}

// =====================================================================
//  TcpNetworkError — 网络错误
// =====================================================================

/// TCP 网络错误
#[derive(Debug, thiserror::Error)]
pub enum TcpNetworkError {
    /// peer 节点地址未配置
    #[error("peer {0} not found in peer map")]
    PeerNotFound(NodeId),

    /// 连接失败
    #[error("connect to peer {peer} at {addr} failed: {source}")]
    Connect {
        peer: NodeId,
        addr: SocketAddr,
        source: std::io::Error,
    },

    /// I/O 错误
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),

    /// 序列化失败
    #[error("serialize error: {0}")]
    Serialize(String),

    /// 锁中毒
    #[error("lock poisoned")]
    LockPoisoned,
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::{
        AppendEntriesRequest, AppendEntriesResponse, LogEntry, MessageType, RequestVoteRequest,
        RequestVoteResponse,
    };

    /// 构造测试用的 AppendEntriesRequest（带 N 条空 entries）
    fn make_append_entries(term: u64, leader_id: NodeId, entries: Vec<LogEntry>) -> MessageType {
        MessageType::AppendEntriesRequest(AppendEntriesRequest {
            term,
            leader_id,
            prev_log_index: 0,
            prev_log_term: 0,
            entries,
            leader_commit: 0,
        })
    }

    /// 构造测试用的 RequestVoteRequest
    fn make_request_vote(term: u64, candidate_id: NodeId) -> MessageType {
        MessageType::RequestVoteRequest(RequestVoteRequest {
            term,
            candidate_id,
            last_log_index: 0,
            last_log_term: 0,
        })
    }

    /// 构造测试用的 RequestVoteResponse
    fn make_request_vote_response(term: u64, granted: bool) -> MessageType {
        MessageType::RequestVoteResponse(RequestVoteResponse {
            term,
            vote_granted: granted,
        })
    }

    /// 绑定一个临时端口并返回 SocketAddr（绑定后立即释放，让 TcpNetwork 重新使用）
    fn bind_temp_addr() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        addr
    }

    /// 阶段 4：TCP 网络基础消息传递
    ///
    /// 启动两个 TcpNetwork 节点，验证消息可以从节点 1 发送到节点 2。
    #[test]
    fn test_tcp_network_basic_message_passing() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        // 等待监听线程就绪
        thread::sleep(Duration::from_millis(50));

        // 节点 1 发送消息给节点 2
        let msg = RpcMessage::new(1, 2, make_append_entries(1, 1, vec![]));
        net1.send(1, 2, msg);

        // 等待消息投递
        thread::sleep(Duration::from_millis(50));

        // 节点 2 应收到消息
        let received = net2.drain();
        assert_eq!(received.len(), 1, "节点 2 应收到 1 条消息");
        assert_eq!(received[0].from, 1);
        assert_eq!(received[0].to, 2);
        assert!(matches!(
            received[0].message_type,
            MessageType::AppendEntriesRequest(_)
        ));
    }

    /// 阶段 4：多条消息顺序传递
    #[test]
    fn test_tcp_network_multiple_messages() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        thread::sleep(Duration::from_millis(50));

        // 发送 5 条消息（每条用不同的 term 区分）
        for i in 0..5u64 {
            net1.send(
                1,
                2,
                RpcMessage::new(1, 2, make_append_entries(i + 1, 1, vec![])),
            );
        }

        thread::sleep(Duration::from_millis(100));

        let received = net2.drain();
        assert_eq!(received.len(), 5, "节点 2 应收到 5 条消息");
        // 验证顺序（TCP 保证顺序）和 term 递增
        for (i, msg) in received.iter().enumerate() {
            if let MessageType::AppendEntriesRequest(req) = &msg.message_type {
                assert_eq!(req.term, i as u64 + 1, "消息 {} term 错误", i);
            } else {
                panic!("消息 {} 类型错误", i);
            }
        }
    }

    /// 阶段 4：双向通信
    #[test]
    fn test_tcp_network_bidirectional() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        thread::sleep(Duration::from_millis(50));

        // 节点 1 → 节点 2 (RequestVote)
        net1.send(1, 2, RpcMessage::new(1, 2, make_request_vote(1, 1)));
        // 节点 2 → 节点 1 (RequestVoteResponse)
        net2.send(
            2,
            1,
            RpcMessage::new(2, 1, make_request_vote_response(1, true)),
        );

        thread::sleep(Duration::from_millis(50));

        let recv1 = net1.drain();
        let recv2 = net2.drain();

        assert_eq!(recv1.len(), 1, "节点 1 应收到 1 条消息");
        assert_eq!(recv2.len(), 1, "节点 2 应收到 1 条消息");
        assert_eq!(recv1[0].from, 2);
        assert_eq!(recv2[0].from, 1);
        assert!(matches!(
            recv1[0].message_type,
            MessageType::RequestVoteResponse(_)
        ));
        assert!(matches!(
            recv2[0].message_type,
            MessageType::RequestVoteRequest(_)
        ));
    }

    /// 阶段 4：发送到未知 peer 应静默失败（不 panic）
    #[test]
    fn test_tcp_network_unknown_peer_no_panic() {
        let net1 = TcpNetwork::new(1);
        // 不添加 peer 2 的地址
        net1.send(
            1,
            2,
            RpcMessage::new(1, 2, make_append_entries(1, 1, vec![])),
        );
        // 应静默失败，不 panic
        assert_eq!(net1.pending_count(), 0);
    }

    /// 阶段 4：drain 清空队列
    #[test]
    fn test_tcp_network_drain_clears_inbox() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        thread::sleep(Duration::from_millis(50));

        net1.send(
            1,
            2,
            RpcMessage::new(1, 2, make_append_entries(1, 1, vec![])),
        );
        thread::sleep(Duration::from_millis(50));

        let first = net2.drain();
        assert_eq!(first.len(), 1);

        // 第二次 drain 应为空
        let second = net2.drain();
        assert_eq!(second.len(), 0);
    }

    /// 阶段 4：大消息传递（验证长度前缀正确处理）
    #[test]
    fn test_tcp_network_large_message() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        thread::sleep(Duration::from_millis(50));

        // 构造大量 LogEntry（每个 1KB，共 64 个 = 64KB+）
        let large_entries: Vec<LogEntry> = (0..64)
            .map(|i| LogEntry {
                term: 1,
                index: i + 1,
                command: vec![0xABu8; 1024],
                config_change: None,
            })
            .collect();
        let msg = RpcMessage::new(1, 2, make_append_entries(1, 1, large_entries.clone()));
        net1.send(1, 2, msg);

        thread::sleep(Duration::from_millis(200));

        let received = net2.drain();
        assert_eq!(received.len(), 1, "应收到 1 条消息");
        if let MessageType::AppendEntriesRequest(req) = &received[0].message_type {
            assert_eq!(req.entries.len(), 64, "应有 64 个 entries");
            assert_eq!(req.entries[0].command.len(), 1024, "每个 entry 应为 1KB");
            assert_eq!(req.entries[0].command, vec![0xABu8; 1024]);
        } else {
            panic!("消息类型错误");
        }
    }

    /// 阶段 4：3 节点全互联
    #[test]
    fn test_tcp_network_three_node_mesh() {
        let addrs: Vec<SocketAddr> = (0..3).map(|_| bind_temp_addr()).collect();

        let mut nets: Vec<TcpNetwork> = Vec::new();
        for i in 0..3u8 {
            let net = TcpNetwork::new(i as NodeId + 1);
            net.start_listener(addrs[i as usize]).unwrap();
            nets.push(net);
        }

        // 配置全互联
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    nets[i].add_peer((j + 1) as NodeId, addrs[j as usize]);
                }
            }
        }

        thread::sleep(Duration::from_millis(100));

        // 节点 1 广播给节点 2 和节点 3
        nets[0].send(
            1,
            2,
            RpcMessage::new(1, 2, make_append_entries(1, 1, vec![])),
        );
        nets[0].send(
            1,
            3,
            RpcMessage::new(1, 3, make_append_entries(1, 1, vec![])),
        );

        thread::sleep(Duration::from_millis(100));

        assert_eq!(nets[1].drain().len(), 1, "节点 2 应收到 1 条消息");
        assert_eq!(nets[2].drain().len(), 1, "节点 3 应收到 1 条消息");
    }

    /// 阶段 4：AppendEntriesResponse 也能正确传递
    #[test]
    fn test_tcp_network_append_entries_response() {
        let addr1 = bind_temp_addr();
        let addr2 = bind_temp_addr();

        let net1 = TcpNetwork::new(1);
        net1.start_listener(addr1).unwrap();
        net1.add_peer(2, addr2);

        let net2 = TcpNetwork::new(2);
        net2.start_listener(addr2).unwrap();
        net2.add_peer(1, addr1);

        thread::sleep(Duration::from_millis(50));

        // 节点 2 发送 AppendEntriesResponse 给节点 1
        let resp = MessageType::AppendEntriesResponse(AppendEntriesResponse {
            term: 1,
            success: true,
            match_index: 42,
        });
        net2.send(2, 1, RpcMessage::new(2, 1, resp));

        thread::sleep(Duration::from_millis(50));

        let received = net1.drain();
        assert_eq!(received.len(), 1);
        if let MessageType::AppendEntriesResponse(r) = &received[0].message_type {
            assert!(r.success);
            assert_eq!(r.match_index, 42);
        } else {
            panic!("消息类型错误");
        }
    }
}
