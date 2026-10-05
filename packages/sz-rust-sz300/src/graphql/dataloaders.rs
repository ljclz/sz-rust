// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 GraphQL DataLoader 批量加载
//!
//! MerchantLoader / ProductLoader 去重分批加载，避免 N+1 查询。

use std::collections::HashMap;
use std::sync::Arc;

use async_graphql::dataloader::DataLoader;

use super::schema::{MerchantGql, ProductGql};

/// 商户批量加载器
pub struct MerchantLoader {
    store: Arc<std::sync::Mutex<HashMap<i64, MerchantGql>>>,
}

impl MerchantLoader {
    /// 创建商户加载器
    pub fn new(store: Arc<std::sync::Mutex<HashMap<i64, MerchantGql>>>) -> Self {
        Self { store }
    }
}

impl async_graphql::dataloader::Loader<i64> for MerchantLoader {
    type Value = MerchantGql;
    type Error = String;

    async fn load(&self, keys: &[i64]) -> Result<HashMap<i64, Self::Value>, Self::Error> {
        let store = self.store.lock().unwrap();
        let mut result = HashMap::new();
        for &key in keys {
            if let Some(merchant) = store.get(&key) {
                result.insert(key, merchant.clone());
            }
        }
        Ok(result)
    }
}

/// 商品批量加载器
pub struct ProductLoader {
    store: Arc<std::sync::Mutex<HashMap<i64, ProductGql>>>,
}

impl ProductLoader {
    /// 创建商品加载器
    pub fn new(store: Arc<std::sync::Mutex<HashMap<i64, ProductGql>>>) -> Self {
        Self { store }
    }
}

impl async_graphql::dataloader::Loader<i64> for ProductLoader {
    type Value = ProductGql;
    type Error = String;

    async fn load(&self, keys: &[i64]) -> Result<HashMap<i64, Self::Value>, Self::Error> {
        let store = self.store.lock().unwrap();
        let mut result = HashMap::new();
        for &key in keys {
            if let Some(product) = store.get(&key) {
                result.insert(key, product.clone());
            }
        }
        Ok(result)
    }
}

/// 创建商户 DataLoader
pub fn merchant_loader(
    store: Arc<std::sync::Mutex<HashMap<i64, MerchantGql>>>,
) -> DataLoader<MerchantLoader> {
    DataLoader::new(MerchantLoader::new(store), tokio::spawn)
}

/// 创建商品 DataLoader
pub fn product_loader(
    store: Arc<std::sync::Mutex<HashMap<i64, ProductGql>>>,
) -> DataLoader<ProductLoader> {
    DataLoader::new(ProductLoader::new(store), tokio::spawn)
}
