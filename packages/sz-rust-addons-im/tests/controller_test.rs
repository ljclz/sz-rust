mod common;

use serde_json::{json, Value};
use sz_rust_addons_im::controller::conversation::ConversationController;
use sz_rust_addons_im::controller::message::MessageController;
use sz_rust_addons_im::controller::user_status::UserStatusController;

#[tokio::test]
async fn test_conversation_list_empty() {
    let repo = common::conversation_repo();
    let result = ConversationController::list(&*repo, None, 1, 20).await;
    assert_eq!(result["code"], 0);
}

#[tokio::test]
async fn test_conversation_create_success() {
    let repo = common::conversation_repo();
    let body = json!({"id": 0, "user1_id": 100, "user2_id": 200});
    let result = ConversationController::create(&*repo, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
}

#[tokio::test]
async fn test_conversation_create_zero_users() {
    let repo = common::conversation_repo();
    let body = json!({"id": 0, "user1_id": 0, "user2_id": 200});
    let result = ConversationController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "user1_id and user2_id are required");
}

#[tokio::test]
async fn test_conversation_create_invalid_body() {
    let repo = common::conversation_repo();
    let body = json!("not an object");
    let result = ConversationController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
}

#[tokio::test]
async fn test_message_list_by_conversation_empty() {
    let repo = common::message_repo();
    let result = MessageController::list_by_conversation(&*repo, 1).await;
    assert_eq!(result["code"], 0);
    assert!(result["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_message_create_success() {
    let repo = common::message_repo();
    let body = json!({"id": 0, "conversation_id": 0, "sender_id": 100, "content": "Hello!"});
    let result = MessageController::create(&*repo, 1, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
    assert_eq!(result["data"]["conversation_id"], 1);
}

#[tokio::test]
async fn test_message_create_empty_content() {
    let repo = common::message_repo();
    let body = json!({"id": 0, "conversation_id": 0, "sender_id": 100, "content": ""});
    let result = MessageController::create(&*repo, 1, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "content is required");
}

#[tokio::test]
async fn test_message_create_default_msg_type() {
    let repo = common::message_repo();
    let body =
        json!({"id": 0, "conversation_id": 0, "sender_id": 100, "content": "Hi", "msg_type": ""});
    let result = MessageController::create(&*repo, 1, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["msg_type"], "text");
}

#[tokio::test]
async fn test_user_status_get_default_offline() {
    let repo = common::user_status_repo();
    let result = UserStatusController::get(&*repo, 999).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["is_online"], false);
}

#[tokio::test]
async fn test_user_status_update_creates_new() {
    let repo = common::user_status_repo();
    let body = json!({"is_online": true, "device_type": "mobile"});
    let result = UserStatusController::update(&*repo, 100, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
    assert_eq!(result["data"]["is_online"], true);
    assert_eq!(result["data"]["device_type"], "mobile");
}

#[tokio::test]
async fn test_user_status_update_existing() {
    let repo = common::user_status_repo();
    let body = json!({"is_online": true, "device_type": "mobile"});
    let _ = UserStatusController::update(&*repo, 100, body).await;
    let body2 = json!({"is_online": false, "device_type": "web"});
    let result = UserStatusController::update(&*repo, 100, body2).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "updated");
    assert_eq!(result["data"]["is_online"], false);
    assert_eq!(result["data"]["device_type"], "web");
}
