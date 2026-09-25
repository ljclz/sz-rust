// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! ModelFactory：快速生成测试数据。

use serde::Serialize;
use std::collections::HashMap;

/// 模型工厂：链式设置字段并生成测试数据。
///
/// ```
/// use sz_rust_testkit::FactoryBuilder;
/// let user = FactoryBuilder::new()
///     .with("name", "张三")
///     .with("age", 30)
///     .build();
/// assert_eq!(user["name"], "张三");
/// ```
pub struct FactoryBuilder {
    fields: HashMap<String, serde_json::Value>,
}

impl Default for FactoryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FactoryBuilder {
    /// 创建空工厂。
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }

    /// 设置字段值。
    pub fn with(mut self, key: &str, value: impl Serialize) -> Self {
        self.fields.insert(
            key.to_string(),
            serde_json::to_value(value).expect("testkit 字段值必须可序列化为 JSON"),
        );
        self
    }

    /// 构建为 JSON map。
    pub fn build(self) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        for (k, v) in self.fields {
            map.insert(k, v);
        }
        map
    }

    /// 构建为 JSON Value。
    pub fn build_value(self) -> serde_json::Value {
        serde_json::Value::Object(self.build())
    }

    /// 构建为指定类型。
    pub fn build_as<T: serde::de::DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.build_value())
    }
}

/// 泛型模型工厂 trait。
pub trait ModelFactory: Send + Sync {
    type Model: Serialize;

    /// 创建一条测试记录。
    fn create(&self) -> Self::Model;

    /// 批量创建。
    fn create_batch(&self, count: usize) -> Vec<Self::Model> {
        (0..count).map(|_| self.create()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_builder_chain() {
        let user = FactoryBuilder::new()
            .with("name", "张三")
            .with("age", 30u32)
            .build();
        assert_eq!(user["name"], "张三");
        assert_eq!(user["age"], 30);
    }

    #[test]
    fn factory_builder_build_value() {
        let val = FactoryBuilder::new()
            .with("x", 1u32)
            .with("y", 2u32)
            .build_value();
        assert!(val.is_object());
    }

    #[test]
    fn factory_builder_build_as() {
        #[derive(serde::Deserialize)]
        struct Point {
            x: i32,
            y: i32,
        }
        let point: Point = FactoryBuilder::new()
            .with("x", 1)
            .with("y", 2)
            .build_as()
            .unwrap();
        assert_eq!(point.x, 1);
        assert_eq!(point.y, 2);
    }

    #[test]
    fn factory_builder_default() {
        let builder = FactoryBuilder::default();
        assert!(builder.fields.is_empty());
    }

    #[test]
    fn factory_builder_overwrite() {
        let user = FactoryBuilder::new()
            .with("name", "old")
            .with("name", "new")
            .build();
        assert_eq!(user["name"], "new");
    }

    #[test]
    fn model_factory_batch() {
        struct IntFactory;
        impl ModelFactory for IntFactory {
            type Model = i32;
            fn create(&self) -> i32 {
                42
            }
        }

        let f = IntFactory;
        let batch = f.create_batch(5);
        assert_eq!(batch, vec![42, 42, 42, 42, 42]);
    }
}
