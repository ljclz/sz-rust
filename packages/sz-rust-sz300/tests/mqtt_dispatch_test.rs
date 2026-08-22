//! MQTT 服务配置集成测试
//!
//! 验证 `sz_rust_sz300::services::mqtt_service` 的配置函数返回结构合法。
//! 这些函数是纯逻辑（不依赖真实 DB 连接），可直接测试。

use sz_rust_sz300::services::mqtt_service;

#[test]
fn test_get_mqtt_config() {
    let config = mqtt_service::get_mqtt_config();
    // 验证 broker_url 已设置
    assert!(!config.broker_url.is_empty());
    // 验证 keep_alive 配置
    assert_eq!(config.keep_alive, 30);
    // 验证 client_id 已生成
    assert!(config.client_id.is_some());
    let client_id = config.client_id.as_ref().unwrap();
    assert!(client_id.starts_with("sz300-server-"));
}

#[test]
fn test_get_subscribe_topics() {
    let topics = mqtt_service::get_subscribe_topics();
    // 验证返回非空列表
    assert!(!topics.is_empty());
    // 验证包含设备主题
    let names: Vec<&str> = topics.iter().map(|t| t.name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("/sz/device/")));
    assert!(names.iter().any(|n| n.contains("status")));
    assert!(names.iter().any(|n| n.contains("order")));
    assert!(names.iter().any(|n| n.contains("log")));
}

#[test]
fn test_get_subscribe_topics_count() {
    let topics = mqtt_service::get_subscribe_topics();
    // 应返回 3 个订阅主题
    assert_eq!(topics.len(), 3);
}

#[test]
fn test_get_subscribe_topics_use_wildcard() {
    let topics = mqtt_service::get_subscribe_topics();
    // 所有订阅主题应使用 + 通配符匹配任意 device_sn
    for topic in &topics {
        assert!(
            topic.name.contains('+'),
            "topic {} should contain '+' wildcard",
            topic.name
        );
    }
}

// ============================================================================
// send_ota_command 测试（纯逻辑，不依赖真实 broker）
// ============================================================================

#[tokio::test]
async fn test_send_ota_command_builds_message() {
    // send_ota_command 仅构建 MqttMessage 并返回 Ok，不实际发送
    let result =
        mqtt_service::send_ota_command("SN001", "http://ota.example.com/firmware.bin", "2.0").await;
    assert!(result.is_ok(), "send_ota_command 应成功构建消息");
}

#[tokio::test]
async fn test_send_ota_command_empty_sn() {
    let result = mqtt_service::send_ota_command("", "http://example.com/f.bin", "1.0").await;
    // 空 sn 仍应成功（topic 为 /sz/server//ota）
    assert!(result.is_ok());
}
