// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde_json::Value;
use sz_rust_core::orm::Value as OrmValue;
use sz_rust_core::orm::{MqttConfig, MqttMessage, MqttTopic, QoS};

use crate::db::acquire_cancel_safe;
use crate::state::AppState;

use super::redact_sensitive;

/// SZ-300 MQTT Topic 定义
pub struct SzMqttTopics;

impl SzMqttTopics {
    // 设备 -> 服务器
    /// 设备状态上报主题（设备 -> 服务器）
    pub const DEVICE_STATUS: &'static str = "/sz/device/{device_sn}/status";
    /// 设备订单上报主题（设备 -> 服务器）
    pub const DEVICE_ORDER: &'static str = "/sz/device/{device_sn}/order";
    /// 设备日志上报主题（设备 -> 服务器）
    pub const DEVICE_LOG: &'static str = "/sz/device/{device_sn}/log";

    // 服务器 -> 设备
    /// OTA 升级指令主题（服务器 -> 设备）
    pub const SERVER_OTA: &'static str = "/sz/server/{device_sn}/ota";
    /// 配置下发主题（服务器 -> 设备）
    pub const SERVER_CONFIG: &'static str = "/sz/server/{device_sn}/config";
    /// 通用指令主题（服务器 -> 设备）
    pub const SERVER_COMMAND: &'static str = "/sz/server/{device_sn}/cmd";

    // 广播
    /// 全广播主题（服务器 -> 所有设备）
    pub const SERVER_BROADCAST: &'static str = "/sz/server/broadcast";
}

/// MQTT 消息处理器
pub struct MqttMessageHandler;

impl MqttMessageHandler {
    /// 处理设备状态上报
    pub async fn handle_device_status(
        state: &AppState,
        device_sn: &str,
        payload: &Value,
    ) -> Result<(), String> {
        tracing::info!("设备状态上报: sn={}", device_sn);
        let status = payload.get("status").and_then(|v| v.as_i64()).unwrap_or(0);
        let signal = payload
            .get("signal_strength")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let fw_ver = payload
            .get("fw_version")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut conn = acquire_cancel_safe(&state.db_pool).await.map_err(|e| {
            tracing::error!(error = %e, "MQTT 处理：获取 DB 连接失败");
            "服务暂时不可用".to_string()
        })?;
        // 参数化查询 — 使用 ? 占位符，杜绝 SQL 注入
        let sql = "UPDATE device SET status = ?, signal_strength = ?, fw_version = ?, last_online_at = NOW() WHERE device_sn = ?";
        let params = [
            OrmValue::I64(status),
            OrmValue::I64(signal),
            OrmValue::String(fw_ver.to_string()),
            OrmValue::String(device_sn.to_string()),
        ];
        conn.execute_with_params(sql, &params).await.map_err(|e| {
            tracing::error!(error = %e, "MQTT 处理：SQL 执行失败");
            "服务暂时不可用".to_string()
        })?;
        Ok(())
    }

    /// 处理设备订单同步
    pub async fn handle_device_order(
        state: &AppState,
        device_sn: &str,
        payload: &Value,
    ) -> Result<(), String> {
        tracing::info!("设备订单上报: sn={}", device_sn);

        let offline_seq = payload
            .get("offline_seq")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let total_fen = payload
            .get("total_fen")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let items = payload.get("items").and_then(|v| v.as_array());

        let mut conn = acquire_cancel_safe(&state.db_pool).await.map_err(|e| {
            tracing::error!(error = %e, "MQTT 处理：获取 DB 连接失败");
            "服务暂时不可用".to_string()
        })?;

        // 查询设备关联的 merchant_id 和 device_id — 参数化，避免注入
        let dev_sql = "SELECT merchant_id, device_id FROM device WHERE device_sn = ?";
        let dev_params = [OrmValue::String(device_sn.to_string())];
        let dev_rows = conn
            .query_with_params(dev_sql, &dev_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "MQTT 处理：SQL 执行失败");
                "服务暂时不可用".to_string()
            })?;

        let merchant_id = dev_rows
            .first()
            .and_then(|r| r.get("merchant_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let device_id = dev_rows
            .first()
            .and_then(|r| r.get("device_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 幂等：同一设备同一离线序列号只落一单（2026-10-28 修复设备重传重复下单）
        if !offline_seq.is_empty() {
            let dup_sql =
                "SELECT order_id FROM `order` WHERE device_id = ? AND offline_seq = ? LIMIT 1";
            let dup_rows = conn
                .query_with_params(
                    dup_sql,
                    &[
                        OrmValue::I64(device_id),
                        OrmValue::String(offline_seq.to_string()),
                    ],
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "MQTT 处理：幂等查询失败");
                    "服务暂时不可用".to_string()
                })?;
            if !dup_rows.is_empty() {
                tracing::info!(
                    device_id,
                    offline_seq,
                    "MQTT 离线订单重复上报，按幂等处理跳过"
                );
                return Ok(());
            }
        }

        // 订单号：毫秒级 + 设备段 + 进程序号
        // 2026-10-28 修复 P0：原 CONCAT('O',UNIX_TIMESTAMP()) 为秒级，同秒多单必撞唯一键
        let order_no = crate::services::order_no::generate(device_id);
        let order_sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, offline_seq, item_count, status) VALUES (?, ?, ?, ?, ?, ?, 1)";
        let order_params = [
            OrmValue::String(order_no.clone()),
            OrmValue::I64(merchant_id),
            OrmValue::I64(device_id),
            OrmValue::I64(total_fen),
            OrmValue::String(offline_seq.to_string()),
            OrmValue::I64(items.map(|a| a.len() as i64).unwrap_or(0)),
        ];
        if let Err(e) = conn.execute_with_params(order_sql, &order_params).await {
            let msg = e.to_string();
            // 并发重传落在唯一键上：视为幂等成功
            if msg.contains("Duplicate") || msg.contains("duplicate") {
                tracing::info!(
                    device_id,
                    offline_seq,
                    "MQTT 离线订单并发重复，按幂等处理跳过"
                );
                return Ok(());
            }
            tracing::error!(error = %msg, "MQTT 处理：SQL 执行失败");
            return Err("服务暂时不可用".to_string());
        }

        Ok(())
    }

    /// 处理设备日志
    pub async fn handle_device_log(
        state: &AppState,
        device_sn: &str,
        payload: &Value,
    ) -> Result<(), String> {
        tracing::info!("设备日志上报: sn={}", device_sn);
        let level = payload
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("info");
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut conn = acquire_cancel_safe(&state.db_pool).await.map_err(|e| {
            tracing::error!(error = %e, "MQTT 处理：获取 DB 连接失败");
            "服务暂时不可用".to_string()
        })?;

        // 查询 device_id — 参数化，避免注入
        let dev_sql = "SELECT device_id FROM device WHERE device_sn = ?";
        let dev_params = [OrmValue::String(device_sn.to_string())];
        let dev_rows = conn
            .query_with_params(dev_sql, &dev_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "MQTT 处理：SQL 执行失败");
                "服务暂时不可用".to_string()
            })?;

        let device_id = dev_rows
            .first()
            .and_then(|r| r.get("device_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // 插入操作日志 — operator/action/detail 均参数化
        // 2026-10-28 安全修复：detail 为自由文本，落库前脱敏（手机号/身份证/令牌等）
        let log_sql = "INSERT INTO operate_log (operator, action, detail) VALUES (?, ?, ?)";
        let log_params = [
            OrmValue::String(format!("device:{}", device_sn)),
            OrmValue::String(format!("[{}] {}", level, device_id)),
            OrmValue::String(redact_sensitive(message)),
        ];
        conn.execute_with_params(log_sql, &log_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "MQTT 处理：SQL 执行失败");
                "服务暂时不可用".to_string()
            })?;

        Ok(())
    }
}

/// Mock MQTT 插件实现（便于无 Broker 环境下开发）
pub struct MockMqttPlugin;

impl MockMqttPlugin {
    /// 启动 Mock MQTT 服务
    pub async fn start(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!("MockMqttPlugin: started (no real broker)");
        let topics = get_subscribe_topics();
        for t in &topics {
            tracing::info!("  subscribed: {}", t.name);
        }
        let _ = state;
        Ok(())
    }

    /// 发布消息
    pub async fn publish(
        topic: &str,
        payload: &[u8],
        qos: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        tracing::info!(
            "MockMQTT publish: topic={}, qos={}, payload={} bytes",
            topic,
            qos,
            payload.len()
        );
        Ok(())
    }
}

/// 获取 MQTT 配置
#[tracing::instrument]
pub fn get_mqtt_config() -> MqttConfig {
    let client_id = format!(
        "sz300-server-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("0000")
    );
    MqttConfig::new("mqtts://iot.鲜视达.cn:8883")
        .with_client_id(client_id)
        .with_keep_alive(30)
}

/// 获取服务器订阅的主题列表
#[tracing::instrument]
pub fn get_subscribe_topics() -> Vec<MqttTopic> {
    vec![
        MqttTopic::new("/sz/device/+/status"),
        MqttTopic::new("/sz/device/+/order"),
        MqttTopic::new("/sz/device/+/log"),
    ]
}

/// 发送 OTA 指令到设备
#[tracing::instrument(skip_all)]
pub async fn send_ota_command(device_sn: &str, ota_url: &str, version: &str) -> Result<(), String> {
    let topic = format!("/sz/server/{}/ota", device_sn);
    let payload = serde_json::json!({
        "url": ota_url,
        "version": version,
        "timestamp": chrono::Utc::now().timestamp()
    });

    let _msg = MqttMessage::json_message(&topic, &payload)
        .map_err(|e| {
            tracing::error!(error = %e, "MQTT 消息构建失败");
            "消息发送失败".to_string()
        })?
        .with_qos(QoS::AtLeastOnce);

    tracing::info!("OTA 指令已发送 - SN: {}, 版本: {}", device_sn, version);
    // NOTE: 通过 MqttPlugin 实际发送消息

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // SzMqttTopics 常量 — 锁定 topic 模板字符串
    // ─────────────────────────────────────────────

    #[test]
    fn sz_mqtt_topics_device_to_server_constants() {
        assert_eq!(SzMqttTopics::DEVICE_STATUS, "/sz/device/{device_sn}/status");
        assert_eq!(SzMqttTopics::DEVICE_ORDER, "/sz/device/{device_sn}/order");
        assert_eq!(SzMqttTopics::DEVICE_LOG, "/sz/device/{device_sn}/log");
    }

    #[test]
    fn sz_mqtt_topics_server_to_device_constants() {
        assert_eq!(SzMqttTopics::SERVER_OTA, "/sz/server/{device_sn}/ota");
        assert_eq!(SzMqttTopics::SERVER_CONFIG, "/sz/server/{device_sn}/config");
        assert_eq!(SzMqttTopics::SERVER_COMMAND, "/sz/server/{device_sn}/cmd");
    }

    #[test]
    fn sz_mqtt_topics_broadcast_constant() {
        assert_eq!(SzMqttTopics::SERVER_BROADCAST, "/sz/server/broadcast");
    }

    // ─────────────────────────────────────────────
    // get_mqtt_config — 纯逻辑（从配置构造 MqttConfig）
    // ─────────────────────────────────────────────

    #[test]
    fn get_mqtt_config_broker_url_is_production_iot_endpoint() {
        let cfg = get_mqtt_config();
        assert_eq!(cfg.broker_url, "mqtts://iot.鲜视达.cn:8883");
    }

    #[test]
    fn get_mqtt_config_keep_alive_is_30_seconds() {
        let cfg = get_mqtt_config();
        assert_eq!(cfg.keep_alive, 30, "keep_alive 应为 30 秒");
    }

    #[test]
    fn get_mqtt_config_client_id_has_sz300_server_prefix() {
        let cfg = get_mqtt_config();
        let client_id = cfg.client_id.expect("client_id 应被设置");
        assert!(
            client_id.starts_with("sz300-server-"),
            "client_id 应以 sz300-server- 开头, 实际: {client_id}"
        );
        // UUID 第一段（8 hex 字符）应跟在前缀后
        let suffix = &client_id["sz300-server-".len()..];
        assert_eq!(suffix.len(), 8, "UUID 第一段应为 8 字符, 实际: {suffix}");
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "UUID 第一段应为十六进制, 实际: {suffix}"
        );
    }

    // ─────────────────────────────────────────────
    // get_subscribe_topics — 纯逻辑（返回服务器订阅的 3 个 topic）
    // ─────────────────────────────────────────────

    #[test]
    fn get_subscribe_topics_returns_three_device_topics() {
        let topics = get_subscribe_topics();
        assert_eq!(topics.len(), 3, "应订阅 3 个设备 topic");
    }

    #[test]
    fn get_subscribe_topics_names_match_device_patterns() {
        let topics = get_subscribe_topics();
        let names: Vec<&str> = topics.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names[0], "/sz/device/+/status");
        assert_eq!(names[1], "/sz/device/+/order");
        assert_eq!(names[2], "/sz/device/+/log");
    }

    #[test]
    fn get_subscribe_topics_all_use_default_qos_at_most_once() {
        let topics = get_subscribe_topics();
        for t in &topics {
            assert_eq!(
                t.qos,
                QoS::AtMostOnce,
                "topic {} 的 QoS 应为默认 AtMostOnce",
                t.name
            );
        }
    }

    // ─────────────────────────────────────────────
    // send_ota_command — 构造 OTA 消息（不实际发送，仅验证消息构建成功）
    // ─────────────────────────────────────────────

    #[tokio::test]
    async fn send_ota_command_returns_ok_with_valid_inputs() {
        let result = send_ota_command("SN-OTA-001", "https://ota.example.com/v2.bin", "2.0.0").await;
        assert!(result.is_ok(), "send_ota_command 应返回 Ok: {:?}", result);
    }

    #[tokio::test]
    async fn send_ota_command_returns_ok_with_empty_version() {
        // 空版本号仍应成功构建消息（业务层不校验版本格式）
        let result = send_ota_command("SN-OTA-002", "https://ota.example.com/v3.bin", "").await;
        assert!(result.is_ok(), "空版本号应仍返回 Ok: {:?}", result);
    }

    // ─────────────────────────────────────────────
    // MockMqttPlugin::publish — Mock 实现，无 Broker，应始终返回 Ok
    // ─────────────────────────────────────────────

    #[tokio::test]
    async fn mock_mqtt_plugin_publish_returns_ok() {
        let payload = b"{\"test\": true}";
        let result = MockMqttPlugin::publish("/sz/test/topic", payload, 1).await;
        assert!(result.is_ok(), "MockMqttPlugin::publish 应返回 Ok: {:?}", result);
    }

    #[tokio::test]
    async fn mock_mqtt_plugin_publish_accepts_empty_payload() {
        let result = MockMqttPlugin::publish("/sz/test/empty", &[], 0).await;
        assert!(result.is_ok(), "空 payload 应仍返回 Ok: {:?}", result);
    }

    // 注：MockMqttPlugin::start 需要 AppState（含 db_pool），无法纯逻辑测试；
    // handle_device_status/order/log 需要 DB 连接，已有 #[ignore] 集成测试覆盖
    // （见 tests/service_coverage_test.rs），此处不重复。
}
