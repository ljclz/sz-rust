use serde_json::Value;
use tokio::sync::watch;
use tracing;

use crate::services::mqtt_service::MqttMessageHandler;
use crate::state::AppState;

/// MQTT 消息分发器
pub struct MqttDispatcher;

impl MqttDispatcher {
    /// 分发 MQTT 消息到对应的处理器
    #[tracing::instrument(skip(state, payload), fields(topic))]
    pub async fn dispatch(state: &AppState, topic: &str, payload: &[u8]) {
        let payload_value: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("MQTT: invalid JSON payload from topic={}", topic);
                return;
            }
        };

        tracing::debug!(%topic, "MQTT 消息到达");

        // 解析 topic 提取 device_sn
        // topic 格式: /sz/device/{device_sn}/{action}
        let parts: Vec<&str> = topic.split('/').collect();
        if parts.len() < 5 {
            return;
        }

        let device_sn = parts[3];
        let action = parts[4];

        match action {
            "status" => {
                let _ = MqttMessageHandler::handle_device_status(state, device_sn, &payload_value)
                    .await;
            }
            "order" => {
                let _ =
                    MqttMessageHandler::handle_device_order(state, device_sn, &payload_value).await;
            }
            "log" => {
                let _ =
                    MqttMessageHandler::handle_device_log(state, device_sn, &payload_value).await;
            }
            _ => {
                tracing::warn!(action, device_sn, "MQTT: 未知 action");
            }
        }
    }

    /// 启动 MQTT 消费者（后台任务）— 支持优雅退出
    ///
    /// 当 `shutdown_rx` 收到 `true` 信号时，消费者会优雅退出。
    pub async fn start_consumer(state: AppState, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!("MQTT消费者: 启动完成 (模拟模式, 无真实broker连接)");

        // NOTE: 实际连接 MQTT Broker 并订阅 topic
        // 使用 sz-orm-mqtt 的 MqttPlugin（默认 in-memory mock）或 RealMqttClient（需要 real-broker feature）
        //
        // 当前为模拟模式：监听 shutdown 信号，收到后优雅退出
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        tracing::info!("MQTT消费者: 收到关闭信号，正在退出...");
                        break;
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    tracing::debug!("MQTT消费者: 心跳 (模拟模式)");
                }
            }
        }

        tracing::info!("MQTT消费者: 已退出");
        let _ = state;
    }
}
