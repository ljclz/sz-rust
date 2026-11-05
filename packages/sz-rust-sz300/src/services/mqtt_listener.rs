// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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

    /// dispatch 对无效 JSON payload 应提前 return，不 panic、不访问 state。
    #[tokio::test]
    async fn dispatch_invalid_json_payload_returns_early() {
        let state = crate::state::mock_app_state();
        // 非法 JSON（缺少闭合括号）
        let payload = b"{\"device_sn\":\"SN-1\"";
        // 仅验证不 panic / 不挂起
        MqttDispatcher::dispatch(&state, "/sz/device/SN-1/status", payload).await;
    }

    /// dispatch 对段数不足的 topic（parts < 5）应提前 return，不访问 state。
    #[tokio::test]
    async fn dispatch_short_topic_returns_early() {
        let state = crate::state::mock_app_state();
        let payload = br#"{"k":1}"#;
        // "/sz/device" split('/') = ["", "sz", "device"] → len=3 < 5
        MqttDispatcher::dispatch(&state, "/sz/device", payload).await;
        // "/sz/device/SN" split('/') = ["", "sz", "device", "SN"] → len=4 < 5
        MqttDispatcher::dispatch(&state, "/sz/device/SN", payload).await;
    }

    /// dispatch 对未知 action 应走到 `_ => warn` 分支，不调用 handler、不 panic。
    #[tokio::test]
    async fn dispatch_unknown_action_logs_warning_without_handler() {
        let state = crate::state::mock_app_state();
        let payload = br#"{"v":1}"#;
        // action="unknown" 不匹配 status/order/log
        MqttDispatcher::dispatch(&state, "/sz/device/SN-1/unknown", payload).await;
    }
}
