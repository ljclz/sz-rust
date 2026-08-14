use sz_rust_addons_im::model::conversation::Conversation;
use sz_rust_addons_im::model::message::Message;
use sz_rust_addons_im::model::user_status::UserStatus;
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value as OrmValue;

#[test]
fn test_conversation_default() {
    let conv = Conversation::default();
    assert_eq!(conv.id, 0);
    assert_eq!(conv.user1_id, 0);
    assert_eq!(conv.user2_id, 0);
    assert!(conv.last_message.is_empty());
    assert_eq!(conv.unread_count, 0);
}

#[test]
fn test_conversation_new() {
    let conv = Conversation::new();
    assert_eq!(conv.id, 0);
}

#[test]
fn test_conversation_get_attribute() {
    let conv = Conversation {
        id: 1,
        user1_id: 100,
        user2_id: 200,
        last_message: "hi".to_string(),
        last_message_at: 5000,
        unread_count: 3,
        created_at: 1000,
        updated_at: 2000,
    };
    assert_eq!(conv.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(conv.get_attribute("user1_id"), Some(OrmValue::I64(100)));
    assert_eq!(conv.get_attribute("user2_id"), Some(OrmValue::I64(200)));
    assert_eq!(
        conv.get_attribute("last_message"),
        Some(OrmValue::String("hi".to_string()))
    );
    assert_eq!(conv.get_attribute("unread_count"), Some(OrmValue::I64(3)));
    assert_eq!(conv.get_attribute("nonexistent"), None);
}

#[test]
fn test_conversation_serialize_deserialize() {
    let conv = Conversation {
        id: 1,
        user1_id: 100,
        user2_id: 200,
        last_message: "hello".to_string(),
        last_message_at: 1000,
        unread_count: 2,
        created_at: 100,
        updated_at: 200,
    };
    let json = serde_json::to_value(&conv).expect("serialize conversation");
    let deserialized: Conversation =
        serde_json::from_value(json).expect("deserialize conversation");
    assert_eq!(conv, deserialized);
}

#[test]
fn test_message_default() {
    let msg = Message::default();
    assert_eq!(msg.id, 0);
    assert_eq!(msg.conversation_id, 0);
    assert_eq!(msg.sender_id, 0);
    assert!(msg.content.is_empty());
    assert!(!msg.is_read);
}

#[test]
fn test_message_new() {
    let msg = Message::new();
    assert_eq!(msg.id, 0);
}

#[test]
fn test_message_get_attribute() {
    let msg = Message {
        id: 1,
        conversation_id: 5,
        sender_id: 100,
        content: "Hello".to_string(),
        msg_type: "text".to_string(),
        is_read: true,
        created_at: 1000,
    };
    assert_eq!(msg.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(msg.get_attribute("conversation_id"), Some(OrmValue::I64(5)));
    assert_eq!(msg.get_attribute("sender_id"), Some(OrmValue::I64(100)));
    assert_eq!(
        msg.get_attribute("content"),
        Some(OrmValue::String("Hello".to_string()))
    );
    assert_eq!(msg.get_attribute("is_read"), Some(OrmValue::Bool(true)));
    assert_eq!(msg.get_attribute("nonexistent"), None);
}

#[test]
fn test_message_serialize_deserialize() {
    let msg = Message {
        id: 1,
        conversation_id: 1,
        sender_id: 100,
        content: "Hi".to_string(),
        msg_type: "text".to_string(),
        is_read: false,
        created_at: 1000,
    };
    let json = serde_json::to_value(&msg).expect("serialize message");
    let deserialized: Message = serde_json::from_value(json).expect("deserialize message");
    assert_eq!(msg, deserialized);
}

#[test]
fn test_user_status_default() {
    let status = UserStatus::default();
    assert_eq!(status.id, 0);
    assert_eq!(status.user_id, 0);
    assert!(!status.is_online);
    assert!(status.device_type.is_empty());
}

#[test]
fn test_user_status_new() {
    let status = UserStatus::new();
    assert_eq!(status.id, 0);
}

#[test]
fn test_user_status_get_attribute() {
    let status = UserStatus {
        id: 1,
        user_id: 100,
        is_online: true,
        last_seen: 5000,
        device_type: "mobile".to_string(),
    };
    assert_eq!(status.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(status.get_attribute("user_id"), Some(OrmValue::I64(100)));
    assert_eq!(
        status.get_attribute("is_online"),
        Some(OrmValue::Bool(true))
    );
    assert_eq!(status.get_attribute("last_seen"), Some(OrmValue::I64(5000)));
    assert_eq!(
        status.get_attribute("device_type"),
        Some(OrmValue::String("mobile".to_string()))
    );
    assert_eq!(status.get_attribute("nonexistent"), None);
}

#[test]
fn test_user_status_serialize_deserialize() {
    let status = UserStatus {
        id: 1,
        user_id: 100,
        is_online: true,
        last_seen: 1000,
        device_type: "web".to_string(),
    };
    let json = serde_json::to_value(&status).expect("serialize user_status");
    let deserialized: UserStatus = serde_json::from_value(json).expect("deserialize user_status");
    assert_eq!(status, deserialized);
}
