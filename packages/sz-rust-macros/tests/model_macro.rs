//! `#[model]` 宏集成测试
//!
//! 验证宏生成的 `Model` + `ModelExt` trait 实现是否正确。

use std::collections::HashMap;

use sz_orm_core::{Model, ModelExt, Value};
use sz_rust_macros::model;

/// 测试用模型
#[derive(Clone, Debug, PartialEq)]
#[model(table = "users", pk = "user_id")]
pub struct User {
    pub user_id: i64,
    pub name: String,
    pub age: i64,
    pub score: f64,
    pub active: bool,
}

#[test]
fn test_model_table_name() {
    assert_eq!(User::table_name(), "users");
}

#[test]
fn test_model_pk_name() {
    assert_eq!(User::pk_name(), "user_id");
}

#[test]
fn test_model_columns() {
    let cols = User::columns();
    assert_eq!(cols.len(), 5);
    assert!(cols.contains(&"user_id"));
    assert!(cols.contains(&"name"));
    assert!(cols.contains(&"age"));
    assert!(cols.contains(&"score"));
    assert!(cols.contains(&"active"));
}

#[test]
fn test_model_fillable_excludes_pk() {
    let fillable = User::fillable();
    assert!(!fillable.contains(&"user_id"));
    assert!(fillable.contains(&"name"));
    assert!(fillable.contains(&"age"));
    assert!(fillable.contains(&"score"));
    assert!(fillable.contains(&"active"));
}

#[test]
fn test_model_guarded_contains_pk() {
    let guarded = User::guarded();
    assert_eq!(guarded, vec!["user_id"]);
}

#[test]
fn test_model_pk_get_set() {
    let mut user = User {
        user_id: 0,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(user.pk(), 0);

    user.set_pk(42);
    assert_eq!(user.pk(), 42);
    assert_eq!(user.user_id, 42);
}

#[test]
fn test_model_get_column_value_i64() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(user.get_column_value("user_id"), Some(Value::I64(1)));
    assert_eq!(user.get_column_value("age"), Some(Value::I64(30)));
}

#[test]
fn test_model_get_column_value_string() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(
        user.get_column_value("name"),
        Some(Value::String("alice".to_string()))
    );
}

#[test]
fn test_model_get_column_value_f64() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(user.get_column_value("score"), Some(Value::F64(95.5)));
}

#[test]
fn test_model_get_column_value_bool() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(user.get_column_value("active"), Some(Value::Bool(true)));
}

#[test]
fn test_model_get_column_value_unknown() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };
    assert_eq!(user.get_column_value("nonexistent"), None);
}

#[test]
fn test_model_from_value_i64_fields() {
    let mut user = User {
        user_id: 0,
        name: String::new(),
        age: 0,
        score: 0.0,
        active: false,
    };

    let mut map = HashMap::new();
    map.insert("user_id".to_string(), Value::I64(42));
    map.insert("age".to_string(), Value::I64(25));

    user.from_value(map);

    assert_eq!(user.user_id, 42);
    assert_eq!(user.age, 25);
}

#[test]
fn test_model_from_value_string_field() {
    let mut user = User {
        user_id: 0,
        name: String::new(),
        age: 0,
        score: 0.0,
        active: false,
    };

    let mut map = HashMap::new();
    map.insert("name".to_string(), Value::String("bob".to_string()));

    user.from_value(map);

    assert_eq!(user.name, "bob");
}

#[test]
fn test_model_from_value_f64_field() {
    let mut user = User {
        user_id: 0,
        name: String::new(),
        age: 0,
        score: 0.0,
        active: false,
    };

    let mut map = HashMap::new();
    map.insert("score".to_string(), Value::F64(88.5));

    user.from_value(map);

    assert!((user.score - 88.5).abs() < f64::EPSILON);
}

#[test]
fn test_model_from_value_bool_field() {
    let mut user = User {
        user_id: 0,
        name: String::new(),
        age: 0,
        score: 0.0,
        active: false,
    };

    let mut map = HashMap::new();
    map.insert("active".to_string(), Value::Bool(true));

    user.from_value(map);

    assert!(user.active);
}

#[test]
fn test_model_from_value_ignores_unknown_keys() {
    let mut user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };

    let mut map = HashMap::new();
    map.insert("unknown_field".to_string(), Value::I64(999));

    user.from_value(map);

    // 原值不变
    assert_eq!(user.user_id, 1);
    assert_eq!(user.name, "alice");
}

#[test]
fn test_model_to_json() {
    let user = User {
        user_id: 1,
        name: "alice".to_string(),
        age: 30,
        score: 95.5,
        active: true,
    };

    let json = user.to_json();
    assert_eq!(json["user_id"], 1);
    assert_eq!(json["name"], "alice");
    assert_eq!(json["age"], 30);
    assert_eq!(json["active"], true);
}

#[test]
fn test_model_default_pk_name() {
    // 不指定 pk 时，默认为 "id"
    #[derive(Clone)]
    #[model(table = "posts")]
    pub struct Post {
        pub id: i64,
        pub title: String,
    }

    assert_eq!(Post::pk_name(), "id");
    assert_eq!(Post::table_name(), "posts");
    assert_eq!(Post::guarded(), vec!["id"]);
    assert_eq!(Post::fillable(), vec!["title"]);
}

#[test]
fn test_model_foreign_key() {
    // 测试默认 foreign_key 推导
    let fk = User::foreign_key("orders");
    assert_eq!(fk, "orders_id");
}
