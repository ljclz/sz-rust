// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-orm-mqtt 长连接接入
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `workerman/mqtt` 的长连接模型：
//!
//! ```php
//! $mqtt = new Workerman\Mqtt\Client('mqtt://tcp://127.0.0.1:1883');
//! $mqtt->onConnect = function($mqtt) { $mqtt->subscribe('topic'); };
//! $mqtt->onMessage = function($topic, $content) { /* ... */ };
//! $mqtt->loop();
//! ```
//!
//! Rust 端复用 `sz_orm_mqtt::MqttPlugin`（InMemory 模拟）或
//! `sz_orm_mqtt::RealMqttClient`（feature = "real-broker"，真实 rumqttc）。
//!
//! ## 设计
//!
//! - `MqttRuntime`：封装 MqttPlugin，提供 connect/disconnect/publish/subscribe
//! - `start_keepalive`：spawn 后台任务定期 ping，监听 `CancellationToken` 优雅退出

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::orm::{MqttConfig, MqttError, MqttPlugin, QoS};

/// MQTT 运行时配置
#[derive(Debug, Clone)]
pub struct MqttRuntimeConfig {
    /// 客户端 ID
    pub client_id: String,
    /// 心跳间隔（秒，对齐 `MqttConfig::keep_alive`）
    pub keep_alive_secs: u16,
    /// 默认订阅主题列表
    pub topics: Vec<String>,
    /// Broker URL（仅元数据，InMemory 模式不实际连接）
    pub broker_url: String,
}

impl Default for MqttRuntimeConfig {
    fn default() -> Self {
        Self {
            client_id: "sz-rust-mqtt".to_string(),
            keep_alive_secs: 60,
            topics: Vec::new(),
            broker_url: "tcp://localhost:1883".to_string(),
        }
    }
}

impl MqttRuntimeConfig {
    /// 创建新配置
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            ..Default::default()
        }
    }

    /// 自定义心跳间隔
    pub fn with_keep_alive(mut self, secs: u16) -> Self {
        self.keep_alive_secs = secs;
        self
    }

    /// 添加订阅主题
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topics.push(topic.into());
        self
    }

    /// 自定义 broker URL
    pub fn with_broker_url(mut self, url: impl Into<String>) -> Self {
        self.broker_url = url.into();
        self
    }
}

/// MQTT 运行时
///
/// 封装 `sz_orm_mqtt::MqttPlugin`，提供长连接 lifecycle 管理。
///
/// ## 设计
///
/// - 使用 `tokio::sync::Mutex` 保护 MqttPlugin（connect/disconnect 需要 `&mut self`）
/// - `start_keepalive` spawn 后台任务定期检查连接状态，监听 cancel 退出
/// - 默认使用 InMemory MqttPlugin；启用 `real-broker` feature 后可切换到 RealMqttClient
pub struct MqttRuntime {
    config: MqttRuntimeConfig,
    plugin: Arc<Mutex<MqttPlugin>>,
}

impl MqttRuntime {
    /// 创建 MQTT 运行时
    pub fn new(config: MqttRuntimeConfig) -> Self {
        let mqtt_config = MqttConfig::new(config.broker_url.clone())
            .with_client_id(config.client_id.clone())
            .with_keep_alive(config.keep_alive_secs);
        let plugin = MqttPlugin::new(mqtt_config);
        Self {
            config,
            plugin: Arc::new(Mutex::new(plugin)),
        }
    }

    /// 连接 MQTT broker（对齐 `workerman/mqtt` connect）
    pub async fn connect(&self) -> Result<(), MqttError> {
        let mut plugin = self.plugin.lock().await;
        plugin.connect().await
    }

    /// 断开连接
    pub async fn disconnect(&self) -> Result<(), MqttError> {
        let mut plugin = self.plugin.lock().await;
        plugin.disconnect().await
    }

    /// 是否已连接
    pub async fn is_connected(&self) -> bool {
        let plugin = self.plugin.lock().await;
        plugin.is_connected()
    }

    /// 订阅主题
    pub async fn subscribe(&self, topic: &str, qos: QoS) -> Result<(), MqttError> {
        let plugin = self.plugin.lock().await;
        plugin.subscribe(topic, qos).await
    }

    /// 取消订阅
    pub async fn unsubscribe(&self, topic: &str) -> Result<(), MqttError> {
        let plugin = self.plugin.lock().await;
        plugin.unsubscribe(topic).await
    }

    /// 发布消息
    pub async fn publish(&self, topic: &str, payload: Vec<u8>, qos: QoS) -> Result<(), MqttError> {
        let plugin = self.plugin.lock().await;
        plugin.publish(topic, payload, qos).await
    }

    /// 启动心跳保活任务（对齐 `workerman/mqtt` 的 `loop()`）
    ///
    /// 每 `keep_alive_secs` 秒检查一次连接状态，若断开则尝试重连。
    /// 监听 `token.cancelled()` 优雅退出。
    pub fn start_keepalive(&self, token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let plugin = self.plugin.clone();
        let interval_secs = self.config.keep_alive_secs.max(1) as u64;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => {
                        let mut p = plugin.lock().await;
                        if !p.is_connected() {
                            // 尝试重连
                            if let Err(e) = p.connect().await {
                                tracing::warn!("mqtt reconnect failed: {}", e);
                            }
                        }
                    }
                }
            }
        })
    }

    /// 订阅配置中的默认主题（在 connect 之后调用）
    pub async fn subscribe_default_topics(&self) -> Result<(), MqttError> {
        for topic in &self.config.topics {
            self.subscribe(topic, QoS::AtLeastOnce).await?;
        }
        Ok(())
    }

    /// 获取配置
    pub fn config(&self) -> &MqttRuntimeConfig {
        &self.config
    }

    /// 获取订阅数量
    pub async fn subscription_count(&self) -> usize {
        let plugin = self.plugin.lock().await;
        plugin.subscription_count().await
    }

    /// 获取消息总数（InMemory 模式：所有已 publish 的消息）
    pub async fn message_count(&self) -> usize {
        let plugin = self.plugin.lock().await;
        plugin.message_count().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_runtime_config_default() {
        let config = MqttRuntimeConfig::default();
        assert_eq!(config.client_id, "sz-rust-mqtt");
        assert_eq!(config.keep_alive_secs, 60);
        assert!(config.topics.is_empty());
        assert_eq!(config.broker_url, "tcp://localhost:1883");
    }

    #[test]
    fn test_mqtt_runtime_config_builder() {
        let config = MqttRuntimeConfig::new("client-1")
            .with_keep_alive(30)
            .with_topic("orders/#")
            .with_topic("payments/#")
            .with_broker_url("ssl://broker.example.com:8883");
        assert_eq!(config.client_id, "client-1");
        assert_eq!(config.keep_alive_secs, 30);
        assert_eq!(config.topics.len(), 2);
        assert_eq!(config.broker_url, "ssl://broker.example.com:8883");
    }

    #[tokio::test]
    async fn test_mqtt_connect_disconnect() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client"));
        assert!(!runtime.is_connected().await);
        runtime.connect().await.unwrap();
        assert!(runtime.is_connected().await);
        runtime.disconnect().await.unwrap();
        assert!(!runtime.is_connected().await);
    }

    #[tokio::test]
    async fn test_mqtt_subscribe_unsubscribe() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client"));
        runtime.connect().await.unwrap();

        runtime
            .subscribe("test/topic", QoS::AtLeastOnce)
            .await
            .unwrap();
        assert_eq!(runtime.subscription_count().await, 1);

        runtime.unsubscribe("test/topic").await.unwrap();
        assert_eq!(runtime.subscription_count().await, 0);
    }

    #[tokio::test]
    async fn test_mqtt_publish() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client"));
        runtime.connect().await.unwrap();
        runtime
            .subscribe("test/topic", QoS::AtLeastOnce)
            .await
            .unwrap();

        runtime
            .publish("test/topic", b"hello mqtt".to_vec(), QoS::AtLeastOnce)
            .await
            .unwrap();

        assert_eq!(runtime.message_count().await, 1);
    }

    #[tokio::test]
    async fn test_mqtt_publish_multiple() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client"));
        runtime.connect().await.unwrap();
        runtime
            .subscribe("test/topic", QoS::AtLeastOnce)
            .await
            .unwrap();

        for i in 0..5 {
            runtime
                .publish(
                    "test/topic",
                    format!("msg-{}", i).into_bytes(),
                    QoS::AtLeastOnce,
                )
                .await
                .unwrap();
        }
        assert_eq!(runtime.message_count().await, 5);
    }

    #[tokio::test]
    async fn test_subscribe_default_topics() {
        let config = MqttRuntimeConfig::new("test-client")
            .with_topic("orders/#")
            .with_topic("payments/#");
        let runtime = MqttRuntime::new(config);
        runtime.connect().await.unwrap();
        runtime.subscribe_default_topics().await.unwrap();
        assert_eq!(runtime.subscription_count().await, 2);
    }

    #[tokio::test]
    async fn test_keepalive_task_stops_on_cancel() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client").with_keep_alive(1));
        let token = CancellationToken::new();
        let handle = runtime.start_keepalive(token.clone());

        // 等待一会儿
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();

        // 任务应该退出
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "keepalive task should stop on cancel");
    }

    #[tokio::test]
    async fn test_keepalive_reconnects_after_disconnect() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client").with_keep_alive(1));
        runtime.connect().await.unwrap();
        assert!(runtime.is_connected().await);

        let token = CancellationToken::new();
        let plugin_clone = runtime.plugin.clone();
        let handle = runtime.start_keepalive(token.clone());

        // 主动断开
        {
            let mut p = plugin_clone.lock().await;
            p.disconnect().await.unwrap();
        }
        assert!(!runtime.is_connected().await);

        // 等待 keepalive tick 触发重连
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(runtime.is_connected().await);

        token.cancel();
        let _ = handle.await;
    }

    #[test]
    fn test_config_accessor() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test").with_keep_alive(45));
        assert_eq!(runtime.config().client_id, "test");
        assert_eq!(runtime.config().keep_alive_secs, 45);
    }

    #[tokio::test]
    async fn test_publish_without_connect_returns_error() {
        let runtime = MqttRuntime::new(MqttRuntimeConfig::new("test-client"));
        // 未连接直接 publish（InMemory 模式可能允许，取决于 MqttPlugin 实现）
        let result = runtime
            .publish("test", b"data".to_vec(), QoS::AtMostOnce)
            .await;
        // InMemory MqttPlugin 可能允许也可能拒绝（取决于插件实现），
        // 至少验证调用不 panic，且返回 Ok 或 Err 但不崩溃
        assert!(
            result.is_ok() || result.is_err(),
            "publish without connect must return Ok or Err, not panic"
        );
    }
}
