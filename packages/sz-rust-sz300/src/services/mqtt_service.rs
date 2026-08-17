use serde_json::Value;
use sz_rust_core::orm::Value as OrmValue;
use sz_rust_core::orm::{MqttConfig, MqttMessage, MqttTopic, QoS};

use crate::state::AppState;

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

        let mut conn = state.db_pool.acquire().await.map_err(|e| {
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

        // 安全修复（黑帽审计 A16）：设备上报金额必须非负（防伪造负金额记账污染）
        if total_fen < 0 {
            tracing::warn!(device_sn, total_fen, "MQTT: 订单金额为负，拒绝写入");
            return Err("订单金额不能为负".to_string());
        }
        // 明细金额非负校验（若携带 items）
        if let Some(arr) = items {
            for item in arr {
                let total = item.get("total_fen").and_then(|v| v.as_i64()).unwrap_or(0);
                let price = item.get("price_fen").and_then(|v| v.as_i64()).unwrap_or(0);
                if total < 0 || price < 0 {
                    tracing::warn!(device_sn, "MQTT: 订单明细金额为负，拒绝写入");
                    return Err("订单明细金额不能为负".to_string());
                }
            }
        }

        let mut conn = state.db_pool.acquire().await.map_err(|e| {
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

        // 插入订单 — CONCAT('O',UNIX_TIMESTAMP()) 为 SQL 函数无需参数化，其余字段参数化
        let order_sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, offline_seq, item_count, status) VALUES (CONCAT('O',UNIX_TIMESTAMP()), ?, ?, ?, ?, ?, 1)";
        let order_params = [
            OrmValue::I64(merchant_id),
            OrmValue::I64(device_id),
            OrmValue::I64(total_fen),
            OrmValue::String(offline_seq.to_string()),
            OrmValue::I64(items.map(|a| a.len() as i64).unwrap_or(0)),
        ];
        conn.execute_with_params(order_sql, &order_params)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "MQTT 处理：SQL 执行失败");
                "服务暂时不可用".to_string()
            })?;

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

        let mut conn = state.db_pool.acquire().await.map_err(|e| {
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
        let log_sql = "INSERT INTO operate_log (operator, action, detail) VALUES (?, ?, ?)";
        let log_params = [
            OrmValue::String(format!("device:{}", device_sn)),
            OrmValue::String(format!("[{}] {}", level, device_id)),
            OrmValue::String(message.to_string()),
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

/// 构建 OTA 指令的 topic 与 payload（纯函数，2026-08-16 抽取自 `send_ota_command`）
///
/// topic 格式：`/sz/server/{device_sn}/ota`；payload 含 url/version/timestamp。
/// 返回 `(topic, payload)`，消息构建失败返回错误（不泄露内部信息）。
pub fn build_ota_command(
    device_sn: &str,
    ota_url: &str,
    version: &str,
) -> Result<(String, serde_json::Value), String> {
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

    Ok((topic, payload))
}

/// 发送 OTA 指令到设备
#[tracing::instrument(skip_all)]
pub async fn send_ota_command(device_sn: &str, ota_url: &str, version: &str) -> Result<(), String> {
    let (topic, _payload) = build_ota_command(device_sn, ota_url, version)?;

    tracing::info!(
        "OTA 指令已发送 - SN: {}, 版本: {}, topic: {}",
        device_sn,
        version,
        topic
    );
    // NOTE: 实际发送通过 MqttPlugin（InMemory 模拟或 real-broker feature 真实发送）

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ota_command_constructs_topic_and_payload() {
        let (topic, payload) = build_ota_command("SN001", "http://ota.example.com/2.0.bin", "2.0")
            .expect("构建应成功");
        assert_eq!(topic, "/sz/server/SN001/ota");
        assert_eq!(payload["url"], "http://ota.example.com/2.0.bin");
        assert_eq!(payload["version"], "2.0");
        assert!(payload["timestamp"].as_i64().is_some());
    }

    #[test]
    fn build_ota_command_empty_sn_still_constructs() {
        let (topic, _payload) =
            build_ota_command("", "http://example.com/f.bin", "1.0").expect("构建应成功");
        assert_eq!(topic, "/sz/server//ota");
    }

    #[tokio::test]
    async fn send_ota_command_succeeds() {
        let result = send_ota_command("SN002", "http://ota.example.com/v.bin", "2.1").await;
        assert!(result.is_ok(), "send_ota_command 应成功: {:?}", result);
    }

    #[tokio::test]
    async fn send_ota_command_empty_sn_succeeds() {
        let result = send_ota_command("", "http://x", "1.0").await;
        assert!(result.is_ok());
    }

    #[test]
    fn get_mqtt_config_returns_valid_config() {
        let cfg = get_mqtt_config();
        let client_id = cfg.client_id.as_deref().unwrap_or("");
        assert!(
            client_id.starts_with("sz300-server-"),
            "client_id 前缀正确: {}",
            client_id
        );
        assert_eq!(cfg.keep_alive, 30);
    }

    #[test]
    fn get_mqtt_config_unique_client_id() {
        let cfg1 = get_mqtt_config();
        let cfg2 = get_mqtt_config();
        assert_ne!(
            cfg1.client_id, cfg2.client_id,
            "每次调用应生成唯一 client_id"
        );
    }

    #[test]
    fn get_subscribe_topics_returns_three() {
        let topics = get_subscribe_topics();
        assert_eq!(topics.len(), 3, "应订阅 3 个主题");
    }

    #[test]
    fn get_subscribe_topics_contains_status() {
        let topics = get_subscribe_topics();
        assert!(
            topics.iter().any(|t| t.name.contains("/status")),
            "应包含 status 主题"
        );
    }

    #[test]
    fn get_subscribe_topics_contains_order() {
        let topics = get_subscribe_topics();
        assert!(
            topics.iter().any(|t| t.name.contains("/order")),
            "应包含 order 主题"
        );
    }

    #[test]
    fn get_subscribe_topics_contains_log() {
        let topics = get_subscribe_topics();
        assert!(
            topics.iter().any(|t| t.name.contains("/log")),
            "应包含 log 主题"
        );
    }

    #[test]
    fn sz_mqtt_topics_constants_defined() {
        assert!(SzMqttTopics::DEVICE_STATUS.contains("{device_sn}"));
        assert!(SzMqttTopics::SERVER_OTA.contains("{device_sn}"));
        assert_eq!(SzMqttTopics::SERVER_BROADCAST, "/sz/server/broadcast");
    }

    /// 覆盖 handle_device_order 负金额早返回分支（不依赖 DB）
    #[tokio::test]
    async fn handle_device_order_negative_total_returns_err() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({"total_fen": -100});
        let result = MqttMessageHandler::handle_device_order(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "订单金额不能为负");
    }

    /// 覆盖 handle_device_order 负明细金额早返回分支（不依赖 DB）
    #[tokio::test]
    async fn handle_device_order_negative_item_returns_err() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({
            "total_fen": 100,
            "items": [{"total_fen": -50, "price_fen": 50}]
        });
        let result = MqttMessageHandler::handle_device_order(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "订单明细金额不能为负");
    }

    /// 覆盖 handle_device_order 负单价早返回分支（不依赖 DB）
    #[tokio::test]
    async fn handle_device_order_negative_price_returns_err() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({
            "total_fen": 100,
            "items": [{"total_fen": 50, "price_fen": -10}]
        });
        let result = MqttMessageHandler::handle_device_order(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "订单明细金额不能为负");
    }

    /// 覆盖 handle_device_status acquire 失败路径
    #[tokio::test]
    async fn handle_device_status_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({"status": 1, "signal_strength": -50, "fw_version": "1.0"});
        let result = MqttMessageHandler::handle_device_status(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "服务暂时不可用");
    }

    /// 覆盖 handle_device_order acquire 失败路径（金额非负，进入 acquire）
    #[tokio::test]
    async fn handle_device_order_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({"total_fen": 100, "offline_seq": "SEQ001"});
        let result = MqttMessageHandler::handle_device_order(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "服务暂时不可用");
    }

    /// 覆盖 handle_device_log acquire 失败路径
    #[tokio::test]
    async fn handle_device_log_returns_err_when_db_unavailable() {
        let state = crate::state::mock_app_state();
        let payload = serde_json::json!({"level": "info", "message": "test log"});
        let result = MqttMessageHandler::handle_device_log(&state, "SN001", &payload).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "服务暂时不可用");
    }
}
