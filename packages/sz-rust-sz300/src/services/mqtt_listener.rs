use serde_json::Value;
use tracing;

use crate::services::mqtt_service::MqttMessageHandler;
use crate::state::AppState;

/// MQTT 消息分发器
pub struct MqttDispatcher;

impl MqttDispatcher {
    /// 分发 MQTT 消息到对应的处理器
    pub async fn dispatch(state: &AppState, topic: &str, payload: &[u8]) {
        let payload_value: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("MQTT: invalid JSON payload from topic={}", topic);
                return;
            }
        };

        tracing::debug!("MQTT 消息到达 - Topic: {}", topic);

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
                tracing::warn!("MQTT: 未知 action={} from device={}", action, device_sn);
            }
        }
    }

    /// 启动 MQTT 消费者（后台任务）
    pub async fn start_consumer(state: AppState) {
        tracing::info!("MQTT消费者: 启动完成 (模拟模式, 无真实broker连接)");
        // TODO: 实际连接 MQTT Broker 并订阅 topic
        // 使用 sz-orm-mqtt 的 MqttPlugin（默认 in-memory mock）或 RealMqttClient（需要 real-broker feature）
        let _ = state;
    }
}
