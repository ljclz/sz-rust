use serde_json::Value;
use sz_orm_core::Value as SzValue;
use sz_orm_mqtt::{MqttConfig, MqttMessage, MqttTopic, QoS};

use crate::state::AppState;

/// SZ-300 MQTT Topic 定义
pub struct SzMqttTopics;

impl SzMqttTopics {
    // 设备 -> 服务器
    pub const DEVICE_STATUS: &'static str = "/sz/device/{device_sn}/status";
    pub const DEVICE_ORDER: &'static str = "/sz/device/{device_sn}/order";
    pub const DEVICE_LOG: &'static str = "/sz/device/{device_sn}/log";

    // 服务器 -> 设备
    pub const SERVER_OTA: &'static str = "/sz/server/{device_sn}/ota";
    pub const SERVER_CONFIG: &'static str = "/sz/server/{device_sn}/config";
    pub const SERVER_COMMAND: &'static str = "/sz/server/{device_sn}/cmd";

    // 广播
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
        let sql = format!(
            "UPDATE device SET status={}, signal_strength={}, fw_version='{}', last_online_at=NOW() WHERE device_sn='{}'",
            status, signal, sql_escape(fw_ver), sql_escape(device_sn)
        );
        conn.execute(&sql)
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

        // 查询设备关联的 merchant_id 和 device_id
        let dev_rows = conn
            .query(&format!(
                "SELECT merchant_id, device_id FROM device WHERE device_sn='{}'",
                sql_escape(device_sn)
            ))
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

        // 插入订单
        let order_sql = format!(
            "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, offline_seq, item_count, status) VALUES (CONCAT('O',UNIX_TIMESTAMP()),{}, {}, {}, '{}', {}, 1)",
            merchant_id, device_id, total_fen, sql_escape(offline_seq), items.map(|a| a.len() as i64).unwrap_or(0)
        );
        conn.execute(&order_sql)
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

        // 查询 device_id
        let dev_rows = conn
            .query(&format!(
                "SELECT device_id FROM device WHERE device_sn='{}'",
                sql_escape(device_sn)
            ))
            .await
            .map_err(|e| format!("SQL: {}", e))?;

        let device_id = dev_rows
            .first()
            .and_then(|r| r.get("device_id"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        conn.execute(
            &format!(
                "INSERT INTO operate_log (operator, action, detail) VALUES ('device:{}', '[{}] {}', '{}')",
                device_sn, level, device_id, sql_escape(message)
            )
        ).await.map_err(|e| format!("SQL: {}", e))?;

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

/// SQL 字符串转义（防止注入）
fn sql_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// 获取 MQTT 配置
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
pub fn get_subscribe_topics() -> Vec<MqttTopic> {
    vec![
        MqttTopic::new("/sz/device/+/status"),
        MqttTopic::new("/sz/device/+/order"),
        MqttTopic::new("/sz/device/+/log"),
    ]
}

/// 发送 OTA 指令到设备
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
    // TODO: 通过 MqttPlugin 实际发送消息

    Ok(())
}
