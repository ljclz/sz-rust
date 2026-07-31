//! TCP 透传代理 — 录制 Navicat 与真实数据库之间的完整字节流。
//!
//! 用法：
//! ```bash
//! # 启动透传代理（监听 13306，转发到真实 MySQL 3306）
//! cargo run --bin navicat-recorder -- proxy --listen 13306 --target 127.0.0.1:3306 --dialect mysql --capture-dir captures/mysql
//!
//! # 用 Navicat 连接 13306 端口，执行完整连接流程
//! # 录制文件自动保存到 captures/mysql/{timestamp}/
//! ```
//!
//! 录制格式：
//! ```text
//! captures/mysql/20260728_160000/
//!   meta.json              # 连接元信息
//!   client_to_server.bin   # 客户端发送的原始字节流
//!   server_to_client.bin   # 真实数据库返回的原始字节流
//!   pairs.json             # 请求/响应对（按消息边界切分）
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
#[command(name = "navicat-recorder", about = "Navicat 协议抓包与回放测试工具")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 启动透传代理录制字节流
    Proxy {
        /// 监听端口（Navicat 连接此端口）
        #[arg(long)]
        listen: u16,
        /// 真实数据库地址（host:port）
        #[arg(long)]
        target: String,
        /// 方言：mysql / pg / oracle / sqlserver
        #[arg(long)]
        dialect: String,
        /// 录制文件保存目录
        #[arg(long, default_value = "captures")]
        capture_dir: PathBuf,
    },
    /// 回放测试：将录制的请求发送给 sz-rust，比对响应字节
    Replay {
        /// sz-rust 地址（host:port）
        #[arg(long)]
        target: String,
        /// 方言
        #[arg(long)]
        dialect: String,
        /// 录制文件目录
        #[arg(long)]
        capture_dir: PathBuf,
        /// 失败时输出字节 diff 的上下文长度
        #[arg(long, default_value_t = 64)]
        diff_context: usize,
    },
}

/// 连接元信息
#[derive(serde::Serialize, Debug)]
struct ConnectionMeta {
    dialect: String,
    listen_port: u16,
    target_addr: String,
    recorded_at: String,
    unix_timestamp: u64,
    client_description: String,
}

/// 请求/响应对
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct RequestResponsePair {
    /// 请求字节（十六进制）
    request_hex: String,
    /// 响应字节（十六进制）
    response_hex: String,
    /// 请求序号
    seq: usize,
    /// 时间戳（毫秒）
    timestamp_ms: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Proxy {
            listen,
            target,
            dialect,
            capture_dir,
        } => run_proxy(listen, &target, &dialect, &capture_dir).await,
        Command::Replay {
            target,
            dialect,
            capture_dir,
            diff_context,
        } => run_replay(&target, &dialect, &capture_dir, diff_context).await,
    }
}

/// 启动透传代理，录制 Navicat 与真实数据库的字节流
async fn run_proxy(listen_port: u16, target: &str, dialect: &str, capture_dir: &PathBuf) -> Result<()> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", listen_port))
        .await
        .with_context(|| format!("failed to bind listen port {}", listen_port))?;

    println!("[recorder] 透传代理已启动");
    println!("[recorder]   监听端口: {}", listen_port);
    println!("[recorder]   目标数据库: {}", target);
    println!("[recorder]   方言: {}", dialect);
    println!("[recorder]   录制目录: {}", capture_dir.display());
    println!("[recorder] 请用 Navicat 连接 127.0.0.1:{} 开始录制", listen_port);

    let connection_counter = Arc::new(Mutex::new(0u64));

    loop {
        let (client_stream, peer_addr) = listener.accept().await?;
        println!("[recorder] 新连接来自 {}", peer_addr);

        let target_addr = target.to_string();
        let dialect = dialect.to_string();
        let capture_dir = capture_dir.clone();
        let conn_id = {
            let mut counter = connection_counter.lock().await;
            *counter += 1;
            *counter
        };

        tokio::spawn(async move {
            if let Err(e) = handle_proxy_connection(client_stream, &target_addr, &dialect, &capture_dir, conn_id).await {
                eprintln!("[recorder] 连接 {} 错误: {}", conn_id, e);
            }
        });
    }
}

/// 处理单个代理连接：双向透传 + 录制
async fn handle_proxy_connection(
    client_stream: TcpStream,
    target_addr: &str,
    dialect: &str,
    capture_dir: &PathBuf,
    conn_id: u64,
) -> Result<()> {
    // 连接真实数据库
    let server_stream = TcpStream::connect(target_addr).await
        .with_context(|| format!("failed to connect to target {}", target_addr))?;

    // 创建录制目录
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let timestamp_str = format_datetime(timestamp);
    let conn_dir = capture_dir.join(dialect).join(format!("conn_{}_{}", conn_id, timestamp_str));
    std::fs::create_dir_all(&conn_dir)?;

    // 写入元信息
    let meta = ConnectionMeta {
        dialect: dialect.to_string(),
        listen_port: 0,
        target_addr: target_addr.to_string(),
        recorded_at: timestamp_str,
        unix_timestamp: timestamp,
        client_description: "Navicat Premium 17".to_string(),
    };
    let meta_path = conn_dir.join("meta.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;

    // 拆分 client/server 为读写两半
    let (mut client_read, mut client_write) = client_stream.into_split();
    let (mut server_read, mut server_write) = server_stream.into_split();

    // 录制缓冲区
    let client_to_server_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let server_to_client_buf = Arc::new(Mutex::new(Vec::<u8>::new()));

    // 双向透传
    let c2s_buf = client_to_server_buf.clone();
    let s2c_buf = server_to_client_buf.clone();

    let c2s = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match client_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    {
                        let mut record = c2s_buf.lock().await;
                        record.extend_from_slice(&buf[..n]);
                    }
                    if server_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = server_write.shutdown().await;
    });

    let s2c = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match server_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    {
                        let mut record = s2c_buf.lock().await;
                        record.extend_from_slice(&buf[..n]);
                    }
                    if client_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = client_write.shutdown().await;
    });

    c2s.await;
    s2c.await;

    // 保存录制文件
    let c2s_data = client_to_server_buf.lock().await.clone();
    let s2c_data = server_to_client_buf.lock().await.clone();

    std::fs::write(conn_dir.join("client_to_server.bin"), &c2s_data)?;
    std::fs::write(conn_dir.join("server_to_client.bin"), &s2c_data)?;

    // 切分请求/响应对
    let pairs = split_request_response_pairs(&c2s_data, &s2c_data, dialect);
    let pairs_path = conn_dir.join("pairs.json");
    std::fs::write(&pairs_path, serde_json::to_string_pretty(&pairs)?)?;

    println!("[recorder] 连接 {} 录制完成: {} 字节请求 / {} 字节响应 / {} 对",
             conn_id, c2s_data.len(), s2c_data.len(), pairs.len());
    println!("[recorder] 文件保存到: {}", conn_dir.display());

    Ok(())
}

/// 按协议消息边界切分请求/响应对
///
/// 不同方言的消息边界规则：
/// - MySQL：请求前 3 字节是长度（小端），第 4 字节是 seq_id
/// - PG：请求前 1 字节是消息类型，接下来 4 字节是长度（大端）
/// - Oracle TNS：前 2 字节是长度（大端）
/// - SQL Server TDS：前 4 字节是长度（小端）
fn split_request_response_pairs(client_data: &[u8], server_data: &[u8], dialect: &str) -> Vec<RequestResponsePair> {
    let requests = split_messages(client_data, dialect);
    let responses = split_messages(server_data, dialect);

    requests.iter().enumerate().map(|(i, req)| {
        let resp = responses.get(i).cloned().unwrap_or_default();
        RequestResponsePair {
            request_hex: hex::encode(req),
            response_hex: hex::encode(&resp),
            seq: i + 1,
            timestamp_ms: 0,
        }
    }).collect()
}

/// 按协议消息边界切分字节流
fn split_messages(data: &[u8], dialect: &str) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    let mut offset = 0;

    while offset < data.len() {
        let msg_len = match dialect {
            "mysql" => {
                // MySQL 包头：3 字节长度（小端） + 1 字节 seq_id
                if offset + 4 > data.len() {
                    break;
                }
                let len = (data[offset] as usize)
                    | ((data[offset + 1] as usize) << 8)
                    | ((data[offset + 2] as usize) << 16);
                len + 4
            }
            "pg" => {
                // PG 消息头：1 字节类型 + 4 字节长度（大端，含自身）
                if offset + 5 > data.len() {
                    break;
                }
                let len = ((data[offset + 1] as usize) << 24)
                    | ((data[offset + 2] as usize) << 16)
                    | ((data[offset + 3] as usize) << 8)
                    | (data[offset + 4] as usize);
                len + 1
            }
            "oracle" => {
                // TNS 包头：2 字节长度（大端）
                if offset + 2 > data.len() {
                    break;
                }
                let len = ((data[offset] as usize) << 8) | (data[offset + 1] as usize);
                len
            }
            "sqlserver" => {
                // TDS 包头：4 字节长度（小端）
                if offset + 4 > data.len() {
                    break;
                }
                let len = (data[offset] as usize)
                    | ((data[offset + 1] as usize) << 8)
                    | ((data[offset + 2] as usize) << 16)
                    | ((data[offset + 3] as usize) << 24);
                len
            }
            _ => {
                // 未知方言，整段作为一个消息
                messages.push(data[offset..].to_vec());
                break;
            }
        };

        let end = std::cmp::min(offset + msg_len, data.len());
        messages.push(data[offset..end].to_vec());
        offset = end;
    }

    messages
}

/// 回放测试：将录制的请求发送给 sz-rust，比对响应字节
async fn run_replay(target: &str, dialect: &str, capture_dir: &PathBuf, diff_context: usize) -> Result<()> {
    let pairs_path = capture_dir.join(dialect).join("pairs.json");

    if !pairs_path.exists() {
        anyhow::bail!("录制文件不存在: {}. 请先运行 proxy 命令录制基线。", pairs_path.display());
    }

    let pairs_json = std::fs::read_to_string(&pairs_path)?;
    let pairs: Vec<RequestResponsePair> = serde_json::from_str(&pairs_json)?;

    println!("[replay] 加载 {} 对请求/响应", pairs.len());
    println!("[replay] 目标: {}", target);
    println!("[replay] 方言: {}", dialect);
    println!();

    // 连接 sz-rust
    let mut stream = TcpStream::connect(target).await
        .with_context(|| format!("failed to connect to sz-rust at {}", target))?;

    // 对于 MySQL/PG/Oracle/SQL Server，握手阶段服务器先发送握手包
    // 这里先读取握手包（不比对，因为 sz-rust 的握手包可能与真实数据库不同）
    let handshake = read_handshake(&mut stream, dialect).await?;
    println!("[replay] 握手包: {} 字节", handshake.len());

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures = Vec::new();

    for (i, pair) in pairs.iter().enumerate() {
        // 跳过握手阶段客户端发送的第一个包（认证响应）
        // 实际上 pairs[0] 是认证响应，我们需要发送它给 sz-rust
        let request = hex::decode(&pair.request_hex)
            .with_context(|| format!("failed to decode request hex at pair {}", i))?;
        let expected_response = hex::decode(&pair.response_hex)
            .with_context(|| format!("failed to decode response hex at pair {}", i))?;

        // 发送请求
        if stream.write_all(&request).await.is_err() {
            eprintln!("[replay] pair {} 发送失败（连接断开）", i + 1);
            failed += 1;
            failures.push((i + 1, pair.clone(), Vec::new()));
            break;
        }

        // 读取响应（按消息边界）
        let actual_response = read_response(&mut stream, dialect, expected_response.len()).await.unwrap_or_default();

        // 字节级比对
        if actual_response == expected_response {
            passed += 1;
            print!(".");
        } else {
            failed += 1;
            println!("\n[replay] pair {} 失败: 期望 {} 字节, 实际 {} 字节",
                     i + 1, expected_response.len(), actual_response.len());
            failures.push((i + 1, pair.clone(), actual_response));
        }
    }

    println!("\n");
    println!("==================== 回放测试结果 ====================");
    println!("  通过: {} / 失败: {} / 总计: {}", passed, failed, pairs.len());
    println!("=====================================================");

    if !failures.is_empty() {
        println!("\n失败详情:");
        for (seq, pair, actual) in &failures {
            println!("\n--- pair {} ---", seq);
            let expected = hex::decode(&pair.response_hex).unwrap_or_default();
            print_byte_diff(&expected, actual, diff_context);
        }
    }

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// 读取握手包（服务器先发送）
async fn read_handshake(stream: &mut TcpStream, dialect: &str) -> Result<Vec<u8>> {
    match dialect {
        "mysql" => {
            // MySQL 握手包：3 字节长度 + 1 字节 seq + payload
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).await?;
            let len = (header[0] as usize)
                | ((header[1] as usize) << 8)
                | ((header[2] as usize) << 16);
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await?;
            let mut full = header.to_vec();
            full.extend_from_slice(&payload);
            Ok(full)
        }
        "pg" => {
            // PG 握手：服务器发送 AuthenticationRequest
            // 类型 'R' + 长度 + auth_type
            let mut header = [0u8; 5];
            stream.read_exact(&mut header).await?;
            let len = ((header[1] as usize) << 24)
                | ((header[2] as usize) << 16)
                | ((header[3] as usize) << 8)
                | (header[4] as usize);
            let mut payload = vec![0u8; len - 4];
            stream.read_exact(&mut payload).await?;
            let mut full = header.to_vec();
            full.extend_from_slice(&payload);
            Ok(full)
        }
        _ => {
            // Oracle/SQL Server 握手较复杂，读取前 1024 字节作为握手
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            buf.truncate(n);
            Ok(buf)
        }
    }
}

/// 读取响应（按消息边界或按预期长度）
async fn read_response(stream: &mut TcpStream, dialect: &str, expected_len: usize) -> Result<Vec<u8>> {
    if expected_len == 0 {
        return Ok(Vec::new());
    }

    let mut response = Vec::with_capacity(expected_len);
    let mut buf = [0u8; 8192];

    // 简化策略：读取直到达到 expected_len 或连接关闭
    while response.len() < expected_len {
        let remaining = expected_len - response.len();
        let to_read = std::cmp::min(buf.len(), remaining);
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf[..to_read])).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
            Err(_) => {
                eprintln!("[replay] 读取超时（已读 {} / 期望 {}）", response.len(), expected_len);
                break;
            }
        }
    }

    let _ = dialect;
    Ok(response)
}

/// 打印字节 diff
fn print_byte_diff(expected: &[u8], actual: &[u8], context: usize) {
    let max_len = std::cmp::max(expected.len(), actual.len());
    let min_len = std::cmp::min(expected.len(), actual.len());

    let mut first_diff = None;
    for i in 0..max_len {
        let e = expected.get(i).copied().unwrap_or(0);
        let a = actual.get(i).copied().unwrap_or(0);
        if e != a {
            first_diff = Some(i);
            break;
        }
    }

    match first_diff {
        Some(pos) => {
            let start = pos.saturating_sub(context / 2);
            let end = std::cmp::min(pos + context / 2, max_len);

            println!("  首个差异位置: 字节 {}", pos);
            println!("  期望 (offset {}..{}): {}", start, end, hex::encode(&expected[start..end.min(expected.len())]));
            println!("  实际 (offset {}..{}): {}", start, end, hex::encode(&actual[start..end.min(actual.len())]));

            if expected.len() != actual.len() {
                println!("  长度差异: 期望 {} 字节, 实际 {} 字节", expected.len(), actual.len());
            }
        }
        None => {
            if expected.len() != actual.len() {
                println!("  长度差异: 期望 {} 字节, 实际 {} 字节", expected.len(), actual.len());
            } else {
                println!("  字节完全一致（不应到达此分支）");
            }
        }
    }
}

/// 格式化时间戳为可读字符串
fn format_datetime(unix_secs: u64) -> String {
    let secs = unix_secs;
    let days = secs / 86400;
    let remainder = secs % 86400;
    let hours = remainder / 3600;
    let minutes = (remainder % 3600) / 60;
    let seconds = remainder % 60;

    // 简化日期计算（从 1970-01-01 开始）
    let (year, month, day) = days_to_date(days);

    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", year, month, day, hours, minutes, seconds)
}

/// 将天数（从 1970-01-01）转换为年月日
fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    let mut remaining_days = days;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let month_lengths = if is_leap_year(year) {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &mlen in &month_lengths {
        if remaining_days < mlen {
            break;
        }
        remaining_days -= mlen;
        month += 1;
    }

    (year, month, remaining_days + 1)
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
