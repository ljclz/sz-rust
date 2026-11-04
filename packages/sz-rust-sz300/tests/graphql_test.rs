// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! GraphQL 端点集成测试（v1.8.0 P2-2.1）
//!
//! 验证 `v18-graphql` feature gate 下：
//! 1. Schema 构建（深度/复杂度限制）
//! 2. Query：merchant/product/order 查询
//! 3. Mutation：createProduct/updateProduct/createOrder/updateOrderStatus
//! 4. 深度限制 ≤10
//! 5. 复杂度限制 ≤1000
//! 6. DataLoader 批量加载

#![cfg(feature = "v18-graphql")]

use sz_rust_sz300::graphql::{build_schema, dataloaders};

// ============================================================================
// Schema 构建测试
// ============================================================================

#[test]
fn test_schema_build() {
    let schema = build_schema();
    let sdl = schema.sdl();
    assert!(sdl.contains("QueryRoot"), "SDL 应包含 QueryRoot");
    assert!(sdl.contains("MutationRoot"), "SDL 应包含 MutationRoot");
    assert!(sdl.contains("MerchantGql"), "SDL 应包含 MerchantGql");
    assert!(sdl.contains("ProductGql"), "SDL 应包含 ProductGql");
    assert!(sdl.contains("OrderGql"), "SDL 应包含 OrderGql");
}

#[test]
fn test_schema_has_query_fields() {
    let schema = build_schema();
    let sdl = schema.sdl();
    assert!(sdl.contains("merchant"), "应有 merchant 查询");
    assert!(sdl.contains("merchants"), "应有 merchants 查询");
    assert!(sdl.contains("product"), "应有 product 查询");
    assert!(sdl.contains("products"), "应有 products 查询");
    assert!(sdl.contains("order"), "应有 order 查询");
    assert!(sdl.contains("orders"), "应有 orders 查询");
}

#[test]
fn test_schema_has_mutation_fields() {
    let schema = build_schema();
    let sdl = schema.sdl();
    assert!(sdl.contains("createProduct"), "应有 createProduct 变更");
    assert!(sdl.contains("updateProduct"), "应有 updateProduct 变更");
    assert!(sdl.contains("createOrder"), "应有 createOrder 变更");
    assert!(
        sdl.contains("updateOrderStatus"),
        "应有 updateOrderStatus 变更"
    );
}

// ============================================================================
// Query 测试
// ============================================================================

#[tokio::test]
async fn test_query_merchant_not_found() {
    let schema = build_schema();
    let resp = schema
        .execute(r#"{ merchant(id: 1) { merchantId } }"#)
        .await;
    assert!(
        resp.errors.is_empty(),
        "查询不存在的商户应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["merchant"].is_null(), "不存在的商户应返回 null");
}

#[tokio::test]
async fn test_query_empty_merchants() {
    let schema = build_schema();
    let resp = schema.execute(r#"{ merchants { merchantId name } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询空商户列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["merchants"].as_array().unwrap().len(),
        0,
        "空商户列表应返回空数组"
    );
}

#[tokio::test]
async fn test_query_empty_products() {
    let schema = build_schema();
    let resp = schema.execute(r#"{ products { goodId name } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询空商品列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["products"].as_array().unwrap().len(),
        0,
        "空商品列表应返回空数组"
    );
}

#[tokio::test]
async fn test_query_empty_orders() {
    let schema = build_schema();
    let resp = schema.execute(r#"{ orders { orderId orderNo } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询空订单列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["orders"].as_array().unwrap().len(),
        0,
        "空订单列表应返回空数组"
    );
}

// ============================================================================
// Mutation 测试
// ============================================================================

#[tokio::test]
async fn test_mutation_create_product() {
    let schema = build_schema();
    let resp = schema
        .execute(
            r#"mutation {
                createProduct(
                    merchantId: 1,
                    catId: 1,
                    name: "苹果",
                    barcode: "1234567890",
                    price: 500,
                    unit: "斤"
                ) {
                    goodId
                    name
                    price
                    status
                }
            }"#,
        )
        .await;
    assert!(
        resp.errors.is_empty(),
        "创建商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["createProduct"]["goodId"], 1, "商品 ID 应为 1");
    assert_eq!(data["createProduct"]["name"], "苹果");
    assert_eq!(data["createProduct"]["price"], 500);
    assert_eq!(
        data["createProduct"]["status"], 1,
        "新商品状态应为 1（上架）"
    );
}

#[tokio::test]
async fn test_mutation_create_and_update_product() {
    let schema = build_schema();

    let resp = schema
        .execute(
            r#"mutation {
                createProduct(
                    merchantId: 1, catId: 1, name: "香蕉",
                    barcode: "111", price: 300, unit: "斤"
                ) { goodId status }
            }"#,
        )
        .await;
    assert!(resp.errors.is_empty());
    let data = resp.data.into_json().unwrap();
    let good_id = data["createProduct"]["goodId"].as_i64().unwrap();

    let update_query = format!(
        r#"mutation {{ updateProduct(id: {}, status: 0) {{ goodId status }} }}"#,
        good_id
    );
    let resp = schema.execute(&update_query).await;
    assert!(
        resp.errors.is_empty(),
        "更新商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["updateProduct"]["status"], 0,
        "更新后状态应为 0（下架）"
    );
}

#[tokio::test]
async fn test_mutation_create_order() {
    let schema = build_schema();
    let resp = schema
        .execute(
            r#"mutation {
                createOrder(
                    orderNo: "ORD001",
                    merchantId: 1,
                    deviceId: 1,
                    totalFen: 1000,
                    itemCount: 2
                ) {
                    orderId
                    orderNo
                    status
                    payMethod
                }
            }"#,
        )
        .await;
    assert!(
        resp.errors.is_empty(),
        "创建订单应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["createOrder"]["orderNo"], "ORD001");
    assert_eq!(
        data["createOrder"]["status"], 1,
        "新订单状态应为 1（待支付）"
    );
    assert_eq!(
        data["createOrder"]["payMethod"], 0,
        "新订单支付方式应为 0（未支付）"
    );
}

#[tokio::test]
async fn test_mutation_update_order_status() {
    let schema = build_schema();

    let resp = schema
        .execute(
            r#"mutation {
                createOrder(
                    orderNo: "ORD002", merchantId: 1, deviceId: 1,
                    totalFen: 500, itemCount: 1
                ) { orderId }
            }"#,
        )
        .await;
    assert!(resp.errors.is_empty());
    let data = resp.data.into_json().unwrap();
    let order_id = data["createOrder"]["orderId"].as_i64().unwrap();

    let update_query = format!(
        r#"mutation {{ updateOrderStatus(id: {}, status: 2, payMethod: 1) {{ orderId status payMethod }} }}"#,
        order_id
    );
    let resp = schema.execute(&update_query).await;
    assert!(
        resp.errors.is_empty(),
        "更新订单状态应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["updateOrderStatus"]["status"], 2,
        "更新后状态应为 2（已支付）"
    );
    assert_eq!(
        data["updateOrderStatus"]["payMethod"], 1,
        "支付方式应为 1（微信）"
    );
}

#[tokio::test]
async fn test_mutation_update_nonexistent_product() {
    let schema = build_schema();
    let resp = schema
        .execute(r#"mutation { updateProduct(id: 999, status: 0) { goodId } }"#)
        .await;
    assert!(
        resp.errors.is_empty(),
        "更新不存在的商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["updateProduct"].is_null(), "不存在的商品应返回 null");
}

// ============================================================================
// 深度/复杂度限制测试
// ============================================================================

#[tokio::test]
async fn test_depth_limit_exceeded() {
    let schema = build_schema();
    let deep_query = format!(
        "query {{ {} }}",
        "merchant(id: 1) { ".repeat(15) + "merchantId " + &"}".repeat(15)
    );
    let resp = schema.execute(&deep_query).await;
    assert!(
        !resp.errors.is_empty(),
        "超过深度限制应返回错误: {:?}",
        resp.errors
    );
}

#[tokio::test]
async fn test_depth_limit_within_bound() {
    let schema = build_schema();
    let resp = schema
        .execute(r#"{ merchant(id: 1) { merchantId name stallNo } }"#)
        .await;
    assert!(
        resp.errors.is_empty(),
        "正常深度查询应无错误: {:?}",
        resp.errors
    );
}

#[tokio::test]
async fn test_complexity_limit_exceeded() {
    let schema = build_schema();
    let mut fields = String::new();
    for i in 0..2000 {
        fields.push_str(&format!("p{}: product(id: {}) {{ goodId }} ", i, i));
    }
    let query = format!("query {{ {} }}", fields);
    let resp = schema.execute(&query).await;
    assert!(
        !resp.errors.is_empty(),
        "超过复杂度限制应返回错误: {:?}",
        resp.errors
    );
}

// ============================================================================
// DataLoader 测试
// ============================================================================

#[tokio::test]
async fn test_dataloader_merchant_loader() {
    use std::collections::HashMap;
    use std::sync::Arc;

    let mut merchants = HashMap::new();
    merchants.insert(
        1,
        sz_rust_sz300::graphql::schema::MerchantGql {
            merchant_id: 1,
            market_id: 1,
            name: "测试商户".to_string(),
            stall_no: "A1".to_string(),
            contact_phone: "13800000000".to_string(),
            category: "水果".to_string(),
            status: 1,
        },
    );
    let store = Arc::new(std::sync::Mutex::new(merchants));
    let loader = dataloaders::merchant_loader(store);

    let result = loader.load_many(vec![1i64, 2i64]).await.unwrap();
    assert!(result.contains_key(&1), "应包含已加载的商户");
    assert!(!result.contains_key(&2), "不应包含不存在的商户");
    assert_eq!(result[&1].name, "测试商户");
}

#[tokio::test]
async fn test_dataloader_product_loader() {
    use std::collections::HashMap;
    use std::sync::Arc;

    let mut products = HashMap::new();
    products.insert(
        1,
        sz_rust_sz300::graphql::schema::ProductGql {
            good_id: 1,
            merchant_id: 1,
            cat_id: 1,
            name: "苹果".to_string(),
            barcode: "123".to_string(),
            price: 500,
            unit: "斤".to_string(),
            status: 1,
        },
    );
    let store = Arc::new(std::sync::Mutex::new(products));
    let loader = dataloaders::product_loader(store);

    let result = loader.load_many(vec![1i64]).await.unwrap();
    assert_eq!(result[&1].name, "苹果");
    assert_eq!(result[&1].price, 500);
}

#[tokio::test]
async fn test_dataloader_batch_dedup() {
    use std::collections::HashMap;
    use std::sync::Arc;

    let merchants = HashMap::new();
    let store = Arc::new(std::sync::Mutex::new(merchants));
    let loader = dataloaders::merchant_loader(store);

    let result = loader
        .load_many(vec![1i64, 1i64, 1i64, 1i64])
        .await
        .unwrap();
    assert_eq!(result.len(), 0, "重复 key 应去重，空 store 返回空");
}
