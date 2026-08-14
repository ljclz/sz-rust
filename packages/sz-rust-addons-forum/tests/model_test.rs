use sz_rust_addons_forum::model::board::Board;
use sz_rust_addons_forum::model::reply::Reply;
use sz_rust_addons_forum::model::topic::Topic;
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value as OrmValue;

#[test]
fn test_board_default() {
    let board = Board::default();
    assert_eq!(board.id, 0);
    assert!(board.name.is_empty());
    assert_eq!(board.topic_count, 0);
}

#[test]
fn test_board_new() {
    let board = Board::new();
    assert_eq!(board.id, 0);
}

#[test]
fn test_board_get_attribute() {
    let board = Board {
        id: 1,
        name: "Test".to_string(),
        description: "Desc".to_string(),
        sort: 5,
        topic_count: 10,
        created_at: 1000,
    };
    assert_eq!(board.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(
        board.get_attribute("name"),
        Some(OrmValue::String("Test".to_string()))
    );
    assert_eq!(board.get_attribute("sort"), Some(OrmValue::I64(5)));
    assert_eq!(board.get_attribute("nonexistent"), None);
}

#[test]
fn test_topic_default() {
    let topic = Topic::default();
    assert_eq!(topic.id, 0);
    assert!(topic.title.is_empty());
    assert!(!topic.is_pinned);
    assert!(!topic.is_closed);
}

#[test]
fn test_topic_get_attribute() {
    let topic = Topic {
        id: 1,
        board_id: 2,
        title: "Hello".to_string(),
        content: "World".to_string(),
        author_id: 100,
        reply_count: 5,
        view_count: 50,
        is_pinned: true,
        is_closed: false,
        created_at: 1000,
        updated_at: 2000,
    };
    assert_eq!(topic.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(topic.get_attribute("board_id"), Some(OrmValue::I64(2)));
    assert_eq!(
        topic.get_attribute("title"),
        Some(OrmValue::String("Hello".to_string()))
    );
    assert_eq!(topic.get_attribute("is_pinned"), Some(OrmValue::Bool(true)));
    assert_eq!(
        topic.get_attribute("is_closed"),
        Some(OrmValue::Bool(false))
    );
    assert_eq!(topic.get_attribute("nonexistent"), None);
}

#[test]
fn test_reply_default() {
    let reply = Reply::default();
    assert_eq!(reply.id, 0);
    assert_eq!(reply.topic_id, 0);
    assert!(reply.content.is_empty());
}

#[test]
fn test_reply_get_attribute() {
    let reply = Reply {
        id: 1,
        topic_id: 5,
        author_id: 200,
        content: "Nice".to_string(),
        created_at: 1000,
    };
    assert_eq!(reply.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(reply.get_attribute("topic_id"), Some(OrmValue::I64(5)));
    assert_eq!(
        reply.get_attribute("content"),
        Some(OrmValue::String("Nice".to_string()))
    );
    assert_eq!(reply.get_attribute("nonexistent"), None);
}

#[test]
fn test_board_serialize_deserialize() {
    let board = Board {
        id: 1,
        name: "Test".to_string(),
        description: "Desc".to_string(),
        sort: 0,
        topic_count: 0,
        created_at: 0,
    };
    let json = serde_json::to_value(&board).expect("serialize board");
    let deserialized: Board = serde_json::from_value(json).expect("deserialize board");
    assert_eq!(board, deserialized);
}

#[test]
fn test_topic_serialize_deserialize() {
    let topic = Topic {
        id: 1,
        board_id: 1,
        title: "Test".to_string(),
        content: "".to_string(),
        author_id: 1,
        reply_count: 0,
        view_count: 0,
        is_pinned: false,
        is_closed: false,
        created_at: 0,
        updated_at: 0,
    };
    let json = serde_json::to_value(&topic).expect("serialize topic");
    let deserialized: Topic = serde_json::from_value(json).expect("deserialize topic");
    assert_eq!(topic, deserialized);
}
