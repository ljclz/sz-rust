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
///
/// # 注意
/// 当前实现使用进程内 `GraphqlStore`（`Mutex<HashMap>`）作为数据源：
/// 数据**重启即丢失**、多实例间不共享，仅适用于演示/测试环境。
/// 生产环境请使用 [`build_schema_with_db`]（v1.9.0+，DB 持久化）或 REST API（`/api/v1/*`）。
pub fn build_schema() -> GraphQLSchema {
    tracing::warn!(
        "GraphQL 端点使用内存数据存储（GraphqlStore），数据重启即丢失、多实例不共享，仅限演示/测试环境；生产请使用 build_schema_with_db 或 REST API。"
    );
    let store = GraphqlStore::new(GraphqlStoreInner::default());
    async_graphql::Schema::build(QueryRoot, MutationRoot, async_graphql::EmptySubscription)
        .data(store)
        .limit_depth(10)
        .limit_complexity(1000)
        .finish()
}

// ---------------------------------------------------------------------------
// v1.9.0 DB-backed GraphQL（P0 债务 DB-2026-10-05-01 ②）
// ---------------------------------------------------------------------------

#[cfg(feature = "v19-graphql-persist")]
mod db_backed {
    use std::sync::Arc;

    use async_graphql::{Context, Object};
    use sz_rust_core::orm::{Pool, Value};

    use super::{MerchantGql, OrderGql, ProductGql};

    /// 从查询行提取商户
    fn row_to_merchant(row: &std::collections::HashMap<String, Value>) -> Option<MerchantGql> {
        Some(MerchantGql {
            merchant_id: row.get("merchant_id")?.as_i64()?,
            market_id: row.get("market_id")?.as_i64()?,
            name: row.get("name")?.as_str()?.to_string(),
            stall_no: row.get("stall_no")?.as_str()?.to_string(),
            contact_phone: row.get("contact_phone")?.as_str()?.to_string(),
            category: row.get("category")?.as_str()?.to_string(),
            status: row.get("status")?.as_i64()? as i8,
        })
    }

    /// 从查询行提取商品
    fn row_to_product(row: &std::collections::HashMap<String, Value>) -> Option<ProductGql> {
        Some(ProductGql {
            good_id: row.get("good_id")?.as_i64()?,
            merchant_id: row.get("merchant_id")?.as_i64()?,
            cat_id: row.get("cat_id")?.as_i64()?,
            name: row.get("name")?.as_str()?.to_string(),
            barcode: row.get("barcode")?.as_str()?.to_string(),
            price: row.get("price")?.as_i64()?,
            unit: row.get("unit")?.as_str()?.to_string(),
            status: row.get("status")?.as_i64()? as i8,
        })
    }

    /// 从查询行提取订单
    fn row_to_order(row: &std::collections::HashMap<String, Value>) -> Option<OrderGql> {
        Some(OrderGql {
            order_id: row.get("order_id")?.as_i64()?,
            order_no: row.get("order_no")?.as_str()?.to_string(),
            merchant_id: row.get("merchant_id")?.as_i64()?,
            device_id: row.get("device_id")?.as_i64()?,
            total_fen: row.get("total_fen")?.as_i64()?,
            item_count: row.get("item_count")?.as_i64()? as i32,
            status: row.get("status")?.as_i64()? as i8,
            pay_method: row.get("pay_method")?.as_i64()? as i8,
        })
    }

    /// DB-backed Query 根类型（v1.9.0）
    #[derive(Default)]
    pub struct DbQueryRoot;

    /// DB-backed Mutation 根类型（v1.9.0）
    #[derive(Default)]
    pub struct DbMutationRoot;

    #[Object]
    impl DbQueryRoot {
        /// 按 ID 查询商户（DB-backed）
        async fn merchant(&self, ctx: &Context<'_>, id: i64) -> Option<MerchantGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "SELECT merchant_id, market_id, name, stall_no, contact_phone, category, status FROM merchant WHERE merchant_id = ?";
            let params = [Value::I64(id)];
            let rows = conn.query_with_params(sql, &params).await.ok()?;
            rows.into_iter().next().and_then(|r| row_to_merchant(&r))
        }

        /// 查询所有商户（DB-backed，限 100 条）
        async fn merchants(&self, ctx: &Context<'_>) -> Vec<MerchantGql> {
            let pool = ctx.data_opt::<Arc<Pool>>();
            let Some(pool) = pool else { return Vec::new() };
            let Ok(mut conn) = pool.acquire().await else {
                return Vec::new();
            };
            let sql = "SELECT merchant_id, market_id, name, stall_no, contact_phone, category, status FROM merchant ORDER BY merchant_id DESC LIMIT 100";
            let Ok(rows) = conn.query(sql).await else {
                return Vec::new();
            };
            rows.into_iter()
                .filter_map(|r| row_to_merchant(&r))
                .collect()
        }

        /// 按 ID 查询商品（DB-backed）
        async fn product(&self, ctx: &Context<'_>, id: i64) -> Option<ProductGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "SELECT good_id, merchant_id, cat_id, name, barcode, price, unit, status FROM good WHERE good_id = ?";
            let params = [Value::I64(id)];
            let rows = conn.query_with_params(sql, &params).await.ok()?;
            rows.into_iter().next().and_then(|r| row_to_product(&r))
        }

        /// 查询所有商品（DB-backed，限 100 条）
        async fn products(&self, ctx: &Context<'_>) -> Vec<ProductGql> {
            let pool = ctx.data_opt::<Arc<Pool>>();
            let Some(pool) = pool else { return Vec::new() };
            let Ok(mut conn) = pool.acquire().await else {
                return Vec::new();
            };
            let sql = "SELECT good_id, merchant_id, cat_id, name, barcode, price, unit, status FROM good ORDER BY good_id DESC LIMIT 100";
            let Ok(rows) = conn.query(sql).await else {
                return Vec::new();
            };
            rows.into_iter()
                .filter_map(|r| row_to_product(&r))
                .collect()
        }

        /// 按 ID 查询订单（DB-backed）
        async fn order(&self, ctx: &Context<'_>, id: i64) -> Option<OrderGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "SELECT order_id, order_no, merchant_id, device_id, total_fen, item_count, status, pay_method FROM `order` WHERE order_id = ?";
            let params = [Value::I64(id)];
            let rows = conn.query_with_params(sql, &params).await.ok()?;
            rows.into_iter().next().and_then(|r| row_to_order(&r))
        }

        /// 查询所有订单（DB-backed，限 100 条）
        async fn orders(&self, ctx: &Context<'_>) -> Vec<OrderGql> {
            let pool = ctx.data_opt::<Arc<Pool>>();
            let Some(pool) = pool else { return Vec::new() };
            let Ok(mut conn) = pool.acquire().await else {
                return Vec::new();
            };
            let sql = "SELECT order_id, order_no, merchant_id, device_id, total_fen, item_count, status, pay_method FROM `order` ORDER BY order_id DESC LIMIT 100";
            let Ok(rows) = conn.query(sql).await else {
                return Vec::new();
            };
            rows.into_iter().filter_map(|r| row_to_order(&r)).collect()
        }
    }

    #[Object]
    impl DbMutationRoot {
        /// 创建商品（DB-backed）
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
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "INSERT INTO good (merchant_id, cat_id, name, barcode, price, unit, ai_class_id, image, status, created_at, updated_at) \
                       VALUES (?, ?, ?, ?, ?, ?, 0, '', 1, NOW(), NOW())";
            let params = [
                Value::I64(merchant_id),
                Value::I64(cat_id),
                Value::String(name.clone()),
                Value::String(barcode.clone()),
                Value::I64(price),
                Value::String(unit.clone()),
            ];
            conn.execute_with_params(sql, &params).await.ok()?;
            let id_rows = conn.query("SELECT LAST_INSERT_ID() as id").await.ok()?;
            let id = id_rows
                .first()
                .and_then(|r| r.get("id"))
                .and_then(|v| v.as_i64())?;
            Some(ProductGql {
                good_id: id,
                merchant_id,
                cat_id,
                name,
                barcode,
                price,
                unit,
                status: 1,
            })
        }

        /// 更新商品状态（DB-backed）
        async fn update_product(
            &self,
            ctx: &Context<'_>,
            id: i64,
            status: i8,
        ) -> Option<ProductGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "UPDATE good SET status = ?, updated_at = NOW() WHERE good_id = ?";
            let params = [Value::I8(status), Value::I64(id)];
            conn.execute_with_params(sql, &params).await.ok()?;
            let sql = "SELECT good_id, merchant_id, cat_id, name, barcode, price, unit, status FROM good WHERE good_id = ?";
            let params = [Value::I64(id)];
            let rows = conn.query_with_params(sql, &params).await.ok()?;
            rows.into_iter().next().and_then(|r| row_to_product(&r))
        }

        /// 创建订单（DB-backed）
        async fn create_order(
            &self,
            ctx: &Context<'_>,
            order_no: String,
            merchant_id: i64,
            device_id: i64,
            total_fen: i64,
            item_count: i32,
        ) -> Option<OrderGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, item_count, status, pay_method, offline_seq, created_at, updated_at) \
                       VALUES (?, ?, ?, ?, ?, 1, 0, '', NOW(), NOW())";
            let params = [
                Value::String(order_no.clone()),
                Value::I64(merchant_id),
                Value::I64(device_id),
                Value::I64(total_fen),
                Value::I32(item_count),
            ];
            conn.execute_with_params(sql, &params).await.ok()?;
            let id_rows = conn.query("SELECT LAST_INSERT_ID() as id").await.ok()?;
            let id = id_rows
                .first()
                .and_then(|r| r.get("id"))
                .and_then(|v| v.as_i64())?;
            Some(OrderGql {
                order_id: id,
                order_no,
                merchant_id,
                device_id,
                total_fen,
                item_count,
                status: 1,
                pay_method: 0,
            })
        }

        /// 更新订单状态（DB-backed）
        async fn update_order_status(
            &self,
            ctx: &Context<'_>,
            id: i64,
            status: i8,
            pay_method: i8,
        ) -> Option<OrderGql> {
            let pool = ctx.data_opt::<Arc<Pool>>()?;
            let mut conn = pool.acquire().await.ok()?;
            let sql = "UPDATE `order` SET status = ?, pay_method = ?, updated_at = NOW() WHERE order_id = ?";
            let params = [Value::I8(status), Value::I8(pay_method), Value::I64(id)];
            conn.execute_with_params(sql, &params).await.ok()?;
            let sql = "SELECT order_id, order_no, merchant_id, device_id, total_fen, item_count, status, pay_method FROM `order` WHERE order_id = ?";
            let params = [Value::I64(id)];
            let rows = conn.query_with_params(sql, &params).await.ok()?;
            rows.into_iter().next().and_then(|r| row_to_order(&r))
        }
    }
}

#[cfg(feature = "v19-graphql-persist")]
pub use db_backed::{DbMutationRoot, DbQueryRoot};

/// v1.9.0 DB-backed GraphQL Schema 类型
#[cfg(feature = "v19-graphql-persist")]
pub type GraphQLSchemaDb =
    async_graphql::Schema<DbQueryRoot, DbMutationRoot, async_graphql::EmptySubscription>;

/// 构建 DB-backed GraphQL Schema（v1.9.0，深度限制 ≤10，复杂度 ≤1000）
///
/// 使用 MySQL 连接池作为数据源，数据持久化、多实例共享。
/// 清偿 P0 债务 DB-2026-10-05-01 ②。
///
/// # 参数
///
/// - `pool`：MySQL 连接池（`Arc<Pool>`）
///
/// # 返回
///
/// 配置好深度限制（≤10）和复杂度限制（≤1000）的 GraphQL Schema，
/// Context 中注入 `Arc<Pool>` 供 resolver 使用。
#[cfg(feature = "v19-graphql-persist")]
pub fn build_schema_with_db(pool: std::sync::Arc<sz_rust_core::orm::Pool>) -> GraphQLSchemaDb {
    tracing::info!("GraphQL 端点使用 DB-backed 模式（MySQL 连接池），数据持久化、多实例共享。");
    async_graphql::Schema::build(
        DbQueryRoot,
        DbMutationRoot,
        async_graphql::EmptySubscription,
    )
    .data(pool)
    .limit_depth(10)
    .limit_complexity(1000)
    .finish()
}
