// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 GraphQL Schema 定义（Query + Mutation）
//!
//! - Query: merchant, product, order, merchants, products
//! - Mutation: createProduct, updateProduct, createOrder, updateOrderStatus
//! - 深度限制 ≤10，复杂度 ≤1000

use async_graphql::{Context, Object, SimpleObject};

/// 商户 GraphQL 类型
#[derive(Debug, Clone, SimpleObject)]
pub struct MerchantGql {
    /// 商户主键 ID
    pub merchant_id: i64,
    /// 所属市场 ID
    pub market_id: i64,
    /// 商户名称
    pub name: String,
    /// 摊位号
    pub stall_no: String,
    /// 联系电话
    pub contact_phone: String,
    /// 经营品类
    pub category: String,
    /// 状态（0=禁用，1=启用）
    pub status: i8,
}

/// 商品 GraphQL 类型
#[derive(Debug, Clone, SimpleObject)]
pub struct ProductGql {
    /// 商品主键 ID
    pub good_id: i64,
    /// 商户 ID
    pub merchant_id: i64,
    /// 类目 ID
    pub cat_id: i64,
    /// 商品名称
    pub name: String,
    /// 条形码
    pub barcode: String,
    /// 单价（单位：分）
    pub price: i64,
    /// 计价单位
    pub unit: String,
    /// 状态（0=下架，1=上架）
    pub status: i8,
}

/// 订单 GraphQL 类型
#[derive(Debug, Clone, SimpleObject)]
pub struct OrderGql {
    /// 订单主键 ID
    pub order_id: i64,
    /// 订单号
    pub order_no: String,
    /// 商户 ID
    pub merchant_id: i64,
    /// 设备 ID
    pub device_id: i64,
    /// 订单总金额（单位：分）
    pub total_fen: i64,
    /// 订单项数量
    pub item_count: i32,
    /// 订单状态
    pub status: i8,
    /// 支付方式
    pub pay_method: i8,
}

/// Query 根类型
#[derive(Default)]
pub struct QueryRoot;

/// Mutation 根类型
#[derive(Default)]
pub struct MutationRoot;

#[Object]
impl QueryRoot {
    /// 按 ID 查询商户
    async fn merchant(&self, ctx: &Context<'_>, id: i64) -> Option<MerchantGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let store = store.lock().ok()?;
        store.merchants.get(&id).cloned()
    }

    /// 查询所有商户
    async fn merchants(&self, ctx: &Context<'_>) -> Vec<MerchantGql> {
        let store = ctx.data_opt::<GraphqlStore>();
        match store {
            Some(s) => match s.lock() {
                Ok(g) => g.merchants.values().cloned().collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// 按 ID 查询商品
    async fn product(&self, ctx: &Context<'_>, id: i64) -> Option<ProductGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let store = store.lock().ok()?;
        store.products.get(&id).cloned()
    }

    /// 查询所有商品
    async fn products(&self, ctx: &Context<'_>) -> Vec<ProductGql> {
        let store = ctx.data_opt::<GraphqlStore>();
        match store {
            Some(s) => match s.lock() {
                Ok(g) => g.products.values().cloned().collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// 按 ID 查询订单
    async fn order(&self, ctx: &Context<'_>, id: i64) -> Option<OrderGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let store = store.lock().ok()?;
        store.orders.get(&id).cloned()
    }

    /// 查询所有订单
    async fn orders(&self, ctx: &Context<'_>) -> Vec<OrderGql> {
        let store = ctx.data_opt::<GraphqlStore>();
        match store {
            Some(s) => match s.lock() {
                Ok(g) => g.orders.values().cloned().collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    }
}

#[Object]
impl MutationRoot {
    /// 创建商品
    #[allow(clippy::too_many_arguments)]
    async fn create_product(
        &self,
        ctx: &Context<'_>,
        merchant_id: i64,
        cat_id: i64,
        name: String,
        barcode: String,
        price: i64,
        unit: String,
    ) -> Option<ProductGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let mut store = store.lock().ok()?;
        let id = store.next_product_id();
        let product = ProductGql {
            good_id: id,
            merchant_id,
            cat_id,
            name,
            barcode,
            price,
            unit,
            status: 1,
        };
        store.products.insert(id, product.clone());
        Some(product)
    }

    /// 更新商品状态
    async fn update_product(&self, ctx: &Context<'_>, id: i64, status: i8) -> Option<ProductGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let mut store = store.lock().ok()?;
        if let Some(product) = store.products.get_mut(&id) {
            product.status = status;
            return Some(product.clone());
        }
        None
    }

    /// 创建订单
    async fn create_order(
        &self,
        ctx: &Context<'_>,
        order_no: String,
        merchant_id: i64,
        device_id: i64,
        total_fen: i64,
        item_count: i32,
    ) -> Option<OrderGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let mut store = store.lock().ok()?;
        let id = store.next_order_id();
        let order = OrderGql {
            order_id: id,
            order_no,
            merchant_id,
            device_id,
            total_fen,
            item_count,
            status: 1,
            pay_method: 0,
        };
        store.orders.insert(id, order.clone());
        Some(order)
    }

    /// 更新订单状态
    async fn update_order_status(
        &self,
        ctx: &Context<'_>,
        id: i64,
        status: i8,
        pay_method: i8,
    ) -> Option<OrderGql> {
        let store = ctx.data_opt::<GraphqlStore>()?;
        let mut store = store.lock().ok()?;
        if let Some(order) = store.orders.get_mut(&id) {
            order.status = status;
            order.pay_method = pay_method;
            return Some(order.clone());
        }
        None
    }
}

/// GraphQL 内存数据存储（测试用，生产环境使用 DB pool）
pub type GraphqlStore = std::sync::Mutex<GraphqlStoreInner>;

/// GraphQL 内存数据存储内部结构
#[derive(Default)]
pub struct GraphqlStoreInner {
    /// 商户数据
    pub merchants: std::collections::HashMap<i64, MerchantGql>,
    /// 商品数据
    pub products: std::collections::HashMap<i64, ProductGql>,
    /// 订单数据
    pub orders: std::collections::HashMap<i64, OrderGql>,
    next_product_id: i64,
    next_order_id: i64,
}

impl GraphqlStoreInner {
    /// 生成下一个商品 ID
    pub fn next_product_id(&mut self) -> i64 {
        self.next_product_id += 1;
        self.next_product_id
    }

    /// 生成下一个订单 ID
    pub fn next_order_id(&mut self) -> i64 {
        self.next_order_id += 1;
        self.next_order_id
    }
}

/// GraphQL Schema 类型
pub type GraphQLSchema =
    async_graphql::Schema<QueryRoot, MutationRoot, async_graphql::EmptySubscription>;

/// 构建 GraphQL Schema（深度限制 ≤10，复杂度 ≤1000）
pub fn build_schema() -> GraphQLSchema {
    let store = GraphqlStore::new(GraphqlStoreInner::default());
    async_graphql::Schema::build(QueryRoot, MutationRoot, async_graphql::EmptySubscription)
        .data(store)
        .limit_depth(10)
        .limit_complexity(1000)
        .finish()
}
