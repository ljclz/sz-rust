//! sz-rust-mcp 入口 — stdio transport 循环
//!
//! 逐行读取 stdin 的 JSON-RPC 请求，将响应写入 stdout（MCP 标准 transport）。
//! 每行一个 JSON 消息，响应以单行 JSON 输出。

use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = sz_rust_mcp::handle_request(&line);
        // 通知类请求（id 为 null 且无需响应）返回 Value::Null
        if response.is_null() {
            continue;
        }
        if writeln!(stdout, "{response}").is_err() {
            break;
        }
        let _ = stdout.flush();
    }
}
