mod common;

use serde_json::{json, Value};
use sz_rust_addons_forum::controller::board::BoardController;
use sz_rust_addons_forum::controller::reply::ReplyController;
use sz_rust_addons_forum::controller::topic::TopicController;

#[tokio::test]
async fn test_board_list_empty() {
    let repo = common::board_repo();
    let result = BoardController::list(&*repo).await;
    assert_eq!(result["code"], 0);
    assert!(result["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_board_create_success() {
    let repo = common::board_repo();
    let body = json!({"id": 0, "name": "General", "description": "General discussion"});
    let result = BoardController::create(&*repo, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
}

#[tokio::test]
async fn test_board_create_empty_name() {
    let repo = common::board_repo();
    let body = json!({"id": 0, "name": ""});
    let result = BoardController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "name is required");
}

#[tokio::test]
async fn test_board_create_invalid_body() {
    let repo = common::board_repo();
    let body = json!(123);
    let result = BoardController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
}

#[tokio::test]
async fn test_topic_list_empty() {
    let repo = common::topic_repo();
    let result = TopicController::list(&*repo, 1, 20, None, None).await;
    assert_eq!(result["code"], 0);
}

#[tokio::test]
async fn test_topic_create_success() {
    let repo = common::topic_repo();
    let body = json!({"id": 0, "title": "Hello", "board_id": 1, "author_id": 100});
    let result = TopicController::create(&*repo, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
}

#[tokio::test]
async fn test_topic_create_empty_title() {
    let repo = common::topic_repo();
    let body = json!({"id": 0, "title": "", "board_id": 1, "author_id": 100});
    let result = TopicController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "title is required");
}

#[tokio::test]
async fn test_topic_get_not_found() {
    let repo = common::topic_repo();
    let result = TopicController::get(&*repo, 999).await;
    assert_eq!(result["code"], 404);
}

#[tokio::test]
async fn test_topic_get_success() {
    let repo = common::topic_repo();
    let body = json!({"id": 0, "title": "Test", "board_id": 1, "author_id": 1});
    let created = TopicController::create(&*repo, body).await;
    let id = created["data"]["id"].as_i64().unwrap();
    let result = TopicController::get(&*repo, id).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["title"], "Test");
}

#[tokio::test]
async fn test_topic_delete_not_found() {
    let repo = common::topic_repo();
    let result = TopicController::delete(&*repo, 999).await;
    assert_eq!(result["code"], 404);
}

#[tokio::test]
async fn test_topic_delete_success() {
    let repo = common::topic_repo();
    let body = json!({"id": 0, "title": "ToDelete", "board_id": 1, "author_id": 1});
    let created = TopicController::create(&*repo, body).await;
    let id = created["data"]["id"].as_i64().unwrap();
    let result = TopicController::delete(&*repo, id).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "deleted");
}

#[tokio::test]
async fn test_reply_list_by_topic_empty() {
    let repo = common::reply_repo();
    let result = ReplyController::list_by_topic(&*repo, 1).await;
    assert_eq!(result["code"], 0);
    assert!(result["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_reply_create_success() {
    let repo = common::reply_repo();
    let body = json!({"id": 0, "topic_id": 0, "content": "Nice post!", "author_id": 200});
    let result = ReplyController::create(&*repo, 1, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
    assert_eq!(result["data"]["topic_id"], 1);
}

#[tokio::test]
async fn test_reply_create_empty_content() {
    let repo = common::reply_repo();
    let body = json!({"id": 0, "topic_id": 0, "content": "", "author_id": 200});
    let result = ReplyController::create(&*repo, 1, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "content is required");
}
