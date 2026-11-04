# gRPC 流式传输使用指南

> **版本**：v1.4.0 T16-T18
> **模块**：`sz-rust-api-gateway::grpc_streaming`

## 1. 概述

提供三种 gRPC 流式通信模式：
- **服务端流式**（ServerStreaming）：单请求 → 消息流
- **客户端流式**（ClientStreaming）：消息流 → 单响应
- **双向流式**（BidiStreaming）：双向消息流

## 2. 核心类型

| 类型 | 说明 |
|------|------|
| `StreamingSender<T>` | 流式发送器，带背压缓冲区（默认 1024） |
| `StreamingReceiver<T>` | 流式接收器 |
| `GrpcStreamError` | 错误类型（ChannelClosed/Backpressure/Unauthenticated/Internal） |
| `ServerStreamingHandler<Req, Resp>` | 服务端流式处理 trait |
| `ClientStreamingHandler<Req, Resp>` | 客户端流式处理 trait |
| `BidiStreamingHandler<Req, Resp>` | 双向流式处理 trait |

## 3. 使用示例

### 3.1 服务端流式

```rust
use sz_rust_api_gateway::grpc_streaming::*;

struct MyHandler;
#[async_trait::async_trait]
impl ServerStreamingHandler<String, String> for MyHandler {
    async fn handle(&self, req: String) -> Result<StreamingSender<String>, GrpcStreamError> {
        let (sender, _rx) = StreamingSender::new(DEFAULT_BUFFER_SIZE);
        sender.send(format!("响应: {req}")).await?;
        Ok(sender)
    }
}
```

### 3.2 客户端流式

```rust
#[async_trait::async_trait]
impl ClientStreamingHandler<i32, i64> for MyHandler {
    async fn handle(&self, receiver: &mut StreamingReceiver<i32>) -> Result<i64, GrpcStreamError> {
        let mut sum = 0i64;
        while let Some(val) = receiver.recv().await {
            sum += val as i64;
        }
        Ok(sum)
    }
}
```

### 3.3 双向流式

```rust
#[async_trait::async_trait]
impl BidiStreamingHandler<String, String> for MyHandler {
    async fn handle(&self, receiver: &mut StreamingReceiver<String>, sender: &StreamingSender<String>) -> Result<(), GrpcStreamError> {
        while let Some(msg) = receiver.recv().await {
            sender.send(format!("echo: {msg}")).await?;
        }
        Ok(())
    }
}
```

## 4. 背压机制

当缓冲区满时，`send()` 会等待消费者接收消息后才返回，实现自然背压。缓冲区大小可通过 `StreamingSender::new(buffer_size)` 自定义。

## 5. 错误处理

| 错误 | 触发条件 |
|------|---------|
| `ChannelClosed` | 接收端已关闭 |
| `Backpressure` | 缓冲区满且接收端不消费 |
| `Unauthenticated` | 未通过鉴权 |
| `Internal` | 内部错误 |
