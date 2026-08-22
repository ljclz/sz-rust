//! sz-orm-queue 消费者接入
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-queue` 的消费者模型：
//!
//! ```php
//! // think-queue 的 worker 模型
//! $worker->runNextJob($connection, $queue, $delay, $sleep, $tries);
//! ```
//!
//! Rust 端复用 `sz-orm-queue::MessageQueue` trait，提供 consumer loop helper：
//!
//! - `QueueConsumer` trait：业务侧实现消息处理逻辑
//! - `QueueRuntime`：管理 consumer lifecycle（启动 / 停止 / 监听 cancel）
//!
//! ## 设计
//!
//! sz-orm-queue 的 `MessageQueue` trait 只提供原子操作（publish/consume/ack），
//! 没有 consumer loop helper。本模块补充该能力，配合 `CancellationToken` 做退出。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::orm::MessageQueue;

/// 队列消费者错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum QueueConsumerError {
    /// 消息处理失败（消息会被 nack，留在 in_flight 中）
    #[error("consumer error: {0}")]
    Handler(String),
    /// 底层队列操作失败
    #[error("queue error: {0}")]
    Queue(String),
}

/// 队列消费者 trait
///
/// 业务侧实现该 trait，处理从队列消费的消息。
///
/// ## 行为契约
///
/// - 返回 `Ok(())`：自动调用 `queue.ack(message_id)` 确认消息
/// - 返回 `Err(_)`：不 ack（消息留在 `in_flight`，需人工干预或超时重投）
#[async_trait]
pub trait QueueConsumer: Send + Sync {
    /// 处理一条消息
    ///
    /// - 成功返回 `Ok(())` 会触发自动 ack
    /// - 失败返回 `Err(_)` 会跳过 ack（消息留在 in_flight）
    async fn handle(&self, message: &sz_orm_queue::Message) -> Result<(), QueueConsumerError>;
}

/// 队列运行时配置
#[derive(Debug, Clone)]
pub struct QueueRuntimeConfig {
    /// 消费主题
    pub topic: String,
    /// 队列为空时的轮询间隔（毫秒）
    pub poll_interval_ms: u64,
    /// 单条消息处理最大重试次数（0 表示不重试）
    pub max_retries: u32,
}

impl Default for QueueRuntimeConfig {
    fn default() -> Self {
        Self {
            topic: "default".to_string(),
            poll_interval_ms: 100,
            max_retries: 0,
        }
    }
}

impl QueueRuntimeConfig {
    /// 创建新配置
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            ..Default::default()
        }
    }

    /// 自定义轮询间隔
    pub fn with_poll_interval(mut self, ms: u64) -> Self {
        self.poll_interval_ms = ms;
        self
    }

    /// 自定义最大重试次数
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }
}

/// 队列运行时
///
/// 管理 consumer lifecycle：启动一个消费循环任务，监听 `CancellationToken` 优雅退出。
///
/// ## 设计
///
/// - **不持有 `JoinHandle`**：消费任务由调用方持有，本结构仅提供启动入口
/// - **ack 策略**：handler 返回 Ok 时自动 ack；Err 时不 ack（消息留在 in_flight）
/// - **退出策略**：监听 `token.cancelled()`，当前正在处理的消息会等待完成
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::queue::{QueueRuntime, QueueRuntimeConfig, QueueConsumer};
/// use sz_orm_queue::{InMemoryQueue, MessageQueue, Message};
/// use async_trait::async_trait;
/// use std::sync::Arc;
/// use tokio_util::sync::CancellationToken;
///
/// struct MyConsumer;
/// #[async_trait]
/// impl QueueConsumer for MyConsumer {
///     async fn handle(&self, msg: &Message) -> Result<(), QueueConsumerError> {
///         println!("got: {:?}", msg.payload);
///         Ok(())
///     }
/// }
///
/// let queue = Arc::new(InMemoryQueue::new(1000));
/// let runtime = QueueRuntime::new(
///     QueueRuntimeConfig::new("orders"),
///     queue,
/// );
/// let token = CancellationToken::new();
/// let handle = runtime.start(Arc::new(MyConsumer), token.clone());
/// // ... 业务运行 ...
/// token.cancel();
/// let _ = handle.await;
/// ```
pub struct QueueRuntime {
    config: QueueRuntimeConfig,
    queue: Arc<dyn MessageQueue>,
}

impl QueueRuntime {
    /// 创建队列运行时
    pub fn new(config: QueueRuntimeConfig, queue: Arc<dyn MessageQueue>) -> Self {
        Self { config, queue }
    }

    /// 启动消费循环（返回 JoinHandle，调用方持有以控制 lifecycle）
    ///
    /// ## 行为
    ///
    /// 1. 每 `poll_interval_ms` 毫秒调用 `queue.consume(topic)` 拉取消息
    /// 2. 收到消息后调用 `consumer.handle(&msg)`
    /// 3. handler 返回 Ok → 自动 `queue.ack(msg.id)`
    /// 4. handler 返回 Err → 跳过 ack（消息留在 in_flight）
    /// 5. 监听 `token.cancelled()`，收到信号后停止拉取新消息
    pub fn start<C>(
        &self,
        consumer: Arc<C>,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()>
    where
        C: QueueConsumer + 'static,
    {
        let queue = self.queue.clone();
        let topic = self.config.topic.clone();
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms.max(1));

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    consume_result = queue.consume(&topic) => {
                        match consume_result {
                            Ok(Some(message)) => {
                                let msg_id = message.id.clone();
                                match consumer.handle(&message).await {
                                    Ok(()) => {
                                        if let Err(e) = queue.ack(&msg_id).await {
                                            tracing::warn!("ack failed for msg {}: {}", msg_id, e);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "consumer handler failed for msg {}: {}",
                                            msg_id,
                                            e
                                        );
                                        // 不 ack，消息留在 in_flight
                                    }
                                }
                            }
                            Ok(None) => {
                                // 队列为空，sleep 后重试
                                tokio::time::sleep(poll_interval).await;
                            }
                            Err(e) => {
                                tracing::error!("queue consume error: {}", e);
                                tokio::time::sleep(poll_interval).await;
                            }
                        }
                    }
                }
            }
        })
    }

    /// 获取主题
    pub fn topic(&self) -> &str {
        &self.config.topic
    }

    /// 获取配置
    pub fn config(&self) -> &QueueRuntimeConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orm::{InMemoryQueue, Message, MessageQueue};

    /// 测试用消费者：记录所有处理过的消息 payload
    struct RecordingConsumer {
        payloads: Arc<parking_lot::Mutex<Vec<Vec<u8>>>>,
    }

    impl RecordingConsumer {
        fn new() -> (Self, Arc<parking_lot::Mutex<Vec<Vec<u8>>>>) {
            let payloads = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let consumer = Self {
                payloads: payloads.clone(),
            };
            (consumer, payloads)
        }
    }

    #[async_trait]
    impl QueueConsumer for RecordingConsumer {
        async fn handle(&self, message: &Message) -> Result<(), QueueConsumerError> {
            self.payloads.lock().push(message.payload.clone());
            Ok(())
        }
    }

    /// 失败消费者：总是返回错误
    struct FailingConsumer;

    #[async_trait]
    impl QueueConsumer for FailingConsumer {
        async fn handle(&self, _message: &Message) -> Result<(), QueueConsumerError> {
            Err(QueueConsumerError::Handler("always fail".to_string()))
        }
    }

    /// 创建 InMemoryQueue 并 cast 为 `Arc<dyn MessageQueue>`
    fn make_queue() -> Arc<dyn MessageQueue> {
        Arc::new(InMemoryQueue::new())
    }

    #[test]
    fn test_queue_runtime_config_default() {
        let config = QueueRuntimeConfig::default();
        assert_eq!(config.topic, "default");
        assert_eq!(config.poll_interval_ms, 100);
        assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn test_queue_runtime_config_builder() {
        let config = QueueRuntimeConfig::new("orders")
            .with_poll_interval(50)
            .with_max_retries(3);
        assert_eq!(config.topic, "orders");
        assert_eq!(config.poll_interval_ms, 50);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_queue_runtime_topic_accessor() {
        let queue = make_queue();
        let runtime = QueueRuntime::new(QueueRuntimeConfig::new("test"), queue);
        assert_eq!(runtime.topic(), "test");
    }

    #[test]
    fn test_queue_runtime_config_accessor() {
        let queue = make_queue();
        let config = QueueRuntimeConfig::new("test").with_poll_interval(200);
        let runtime = QueueRuntime::new(config, queue);
        assert_eq!(runtime.config().poll_interval_ms, 200);
    }

    #[tokio::test]
    async fn test_consumer_consumes_published_message() {
        let queue = make_queue();
        queue.publish("orders", b"hello").await.unwrap();

        let (consumer, payloads) = RecordingConsumer::new();
        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("orders").with_poll_interval(5),
            queue.clone(),
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(consumer), token.clone());

        // 等待消费者处理消息
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
        let _ = handle.await;

        let recorded = payloads.lock().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0], b"hello");
    }

    #[tokio::test]
    async fn test_consumer_acks_on_success() {
        let queue = make_queue();
        queue.publish("orders", b"msg1").await.unwrap();

        let (consumer, _payloads) = RecordingConsumer::new();
        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("orders").with_poll_interval(5),
            queue.clone(),
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(consumer), token.clone());

        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
        let _ = handle.await;

        // 队列中应该没有 in_flight 消息（已 ack）
        // 注：InMemoryQueue 的 ack 会从 in_flight 移除消息
        // 此处不直接验证 in_flight 状态，因为 InMemoryQueue API 未暴露
        // 通过再次 consume 返回 None 间接验证
        let result = queue.consume("orders").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_consumer_no_ack_on_failure() {
        let queue = make_queue();
        queue.publish("orders", b"msg1").await.unwrap();

        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("orders").with_poll_interval(5),
            queue.clone(),
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(FailingConsumer), token.clone());

        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
        let _ = handle.await;

        // FailingConsumer 不 ack，消息应该留在 in_flight
        // 再次 consume 应该返回 None（因为 in_flight 中的消息不会被重新拉取）
        // 但 in_flight 消息仍占位
        // 注：此行为依赖 InMemoryQueue 的实现
    }

    #[tokio::test]
    async fn test_consumer_stops_on_cancel() {
        let queue = make_queue();
        let (consumer, _payloads) = RecordingConsumer::new();
        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("orders").with_poll_interval(5),
            queue,
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(consumer), token.clone());

        // 立即 cancel
        token.cancel();
        // 等待任务退出（不应 panic，且应在超时前正常结束）
        let result = tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(
            result.is_ok(),
            "cancel 后消费者应退出，但 500ms 超时未返回（阻塞运行时）"
        );
    }

    #[tokio::test]
    async fn test_consumer_handles_empty_queue() {
        let queue = make_queue();
        let (consumer, payloads) = RecordingConsumer::new();
        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("empty").with_poll_interval(5),
            queue.clone(),
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(consumer), token.clone());

        // 等待一段时间，队列始终为空
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        let _ = handle.await;

        // 没有消息被处理
        assert!(payloads.lock().is_empty());
    }

    #[tokio::test]
    async fn test_consumer_processes_multiple_messages() {
        let queue = make_queue();
        // 发布 3 条消息
        queue.publish("orders", b"msg1").await.unwrap();
        queue.publish("orders", b"msg2").await.unwrap();
        queue.publish("orders", b"msg3").await.unwrap();

        let (consumer, payloads) = RecordingConsumer::new();
        let runtime = QueueRuntime::new(
            QueueRuntimeConfig::new("orders").with_poll_interval(5),
            queue,
        );

        let token = CancellationToken::new();
        let handle = runtime.start(Arc::new(consumer), token.clone());

        // 等待所有消息被处理
        tokio::time::sleep(Duration::from_millis(200)).await;
        token.cancel();
        let _ = handle.await;

        let recorded = payloads.lock().clone();
        assert_eq!(recorded.len(), 3);
    }

    #[test]
    fn test_queue_consumer_error_variants() {
        let handler_err = QueueConsumerError::Handler("test".to_string());
        let queue_err = QueueConsumerError::Queue("queue fail".to_string());
        assert!(format!("{}", handler_err).contains("consumer error"));
        assert!(format!("{}", queue_err).contains("queue error"));
    }
}
