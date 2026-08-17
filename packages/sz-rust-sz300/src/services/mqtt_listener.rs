use serde_json::Value;
use tokio::sync::watch;
use tracing;

use crate::services::mqtt_service::MqttMessageHandler;
use crate::state::AppState;

/// MQTT 单条消息 payload 上限（安全缓解 L-1：防内存 DoS，256KB）
const MAX_MQTT_PAYLOAD_BYTES: usize = 256 * 1024;

/// 校验 MQTT topic 中的 device_sn 格式（安全缓解 L-1）
///
/// 仅允许字母/数字/连字符，长度 1-64。拒绝包含 `/`、空白、控制字符或
/// 日志注入序列（`\n` 等）的异常标识。
fn is_valid_device_sn(sn: &str) -> bool {
    !sn.is_empty()
        && sn.len() <= 64
        && sn
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// MQTT 消息分发器
pub struct MqttDispatcher;

impl MqttDispatcher {
    /// 分发 MQTT 消息到对应的处理器
    #[tracing::instrument(skip(state, payload), fields(topic))]
    pub async fn dispatch(state: &AppState, topic: &str, payload: &[u8]) {
        // 安全缓解 L-1（2026-08-14）：payload 大小上限，防 MQTT 内存 DoS
        if payload.len() > MAX_MQTT_PAYLOAD_BYTES {
            tracing::warn!(topic, len = payload.len(), "MQTT: payload 超限丢弃");
            return;
        }

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

        // 安全缓解 L-1：device_sn 格式校验（字母数字连字符，防日志注入与异常标识）
        if !is_valid_device_sn(device_sn) {
            tracing::warn!(device_sn, "MQTT: 非法 device_sn 丢弃");
            return;
        }

        match action {
            "status" => {
                if let Err(e) =
                    MqttMessageHandler::handle_device_status(state, device_sn, &payload_value).await
                {
                    tracing::warn!(device_sn, error = %e, "MQTT: 处理设备状态失败");
                }
            }
            "order" => {
                if let Err(e) =
                    MqttMessageHandler::handle_device_order(state, device_sn, &payload_value).await
                {
                    tracing::warn!(device_sn, error = %e, "MQTT: 处理设备订单失败");
                }
            }
            "log" => {
                if let Err(e) =
                    MqttMessageHandler::handle_device_log(state, device_sn, &payload_value).await
                {
                    tracing::warn!(device_sn, error = %e, "MQTT: 处理设备日志失败");
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_device_sn_alphanumeric() {
        assert!(is_valid_device_sn("device001"));
        assert!(is_valid_device_sn("ABC123"));
    }

    #[test]
    fn is_valid_device_sn_with_hyphen_and_underscore() {
        assert!(is_valid_device_sn("dev-001"));
        assert!(is_valid_device_sn("dev_001"));
        assert!(is_valid_device_sn("a-b-c_1_2_3"));
    }

    #[test]
    fn is_valid_device_sn_empty_rejected() {
        assert!(!is_valid_device_sn(""));
    }

    #[test]
    fn is_valid_device_sn_too_long_rejected() {
        let long = "a".repeat(65);
        assert!(!is_valid_device_sn(&long));
        let max = "a".repeat(64);
        assert!(is_valid_device_sn(&max));
    }

    #[test]
    fn is_valid_device_sn_slash_rejected() {
        assert!(!is_valid_device_sn("dev/001"));
        assert!(!is_valid_device_sn("a/b"));
    }

    #[test]
    fn is_valid_device_cn_space_rejected() {
        assert!(!is_valid_device_sn("dev 001"));
        assert!(!is_valid_device_sn(" dev"));
        assert!(!is_valid_device_sn("dev "));
    }

    #[test]
    fn is_valid_device_sn_special_chars_rejected() {
        assert!(!is_valid_device_sn("dev@001"));
        assert!(!is_valid_device_sn("dev.001"));
        assert!(!is_valid_device_sn("dev#001"));
        assert!(!is_valid_device_sn("中文"));
    }

    #[test]
    fn is_valid_device_sn_control_chars_rejected() {
        assert!(!is_valid_device_sn("dev\n001"));
        assert!(!is_valid_device_sn("dev\t001"));
    }

    #[test]
    fn max_mqtt_payload_bytes_is_256kb() {
        assert_eq!(MAX_MQTT_PAYLOAD_BYTES, 256 * 1024);
    }
}
