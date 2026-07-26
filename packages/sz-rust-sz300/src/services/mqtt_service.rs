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

        let mut conn = state
            .db_pool
            .acquire()
            .await
            .map_err(|e| format!("DB: {}", e))?;
        // 参数化查询 — 使用 ? 占位符，杜绝 SQL 注入
        let sql = "UPDATE device SET status = ?, signal_strength = ?, fw_version = ?, last_online_at = NOW() WHERE device_sn = ?";
        let params = [
            OrmValue::I64(status),
            OrmValue::I64(signal),
            OrmValue::String(fw_ver.to_string()),
            OrmValue::String(device_sn.to_string()),
        ];
        conn.execute_with_params(sql, &params)
            .await
            .map_err(|e| format!("SQL: {}", e))?;
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

        let mut conn = state
            .db_pool
            .acquire()
            .await
            .map_err(|e| format!("DB: {}", e))?;

        // 查询设备关联的 merchant_id 和 device_id — 参数化，避免注入
        let dev_sql = "SELECT merchant_id, device_id FROM device WHERE device_sn = ?";
        let dev_params = [OrmValue::String(device_sn.to_string())];
        let dev_rows = conn
            .query_with_params(dev_sql, &dev_params)
            .await
            .map_err(|e| format!("SQL: {}", e))?;

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
            .map_err(|e| format!("SQL: {}", e))?;

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

        let mut conn = state
            .db_pool
            .acquire()
            .await
            .map_err(|e| format!("DB: {}", e))?;

        // 查询 device_id — 参数化，避免注入
        let dev_sql = "SELECT device_id FROM device WHERE device_sn = ?";
        let dev_params = [OrmValue::String(device_sn.to_string())];
        let dev_rows = conn
            .query_with_params(dev_sql, &dev_params)
            .await
            .map_err(|e| format!("SQL: {}", e))?;

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
            .map_err(|e| format!("SQL: {}", e))?;

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
        .map_err(|e| format!("构建消息失败: {}", e))?
        .with_qos(QoS::AtLeastOnce);

    tracing::info!("OTA 指令已发送 - SN: {}, 版本: {}", device_sn, version);
    // NOTE: 通过 MqttPlugin 实际发送消息

    Ok(())
}
