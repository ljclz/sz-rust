//! addons-ecommerce 集成测试
//!
//! 覆盖 CartItem / Order / OrderItem 三个模型及其控制器，
//! 使用 InMemoryRepository 模拟持久层，验证 CRUD 与状态机行为。
//!
//! 注意：InMemoryRepository 不自增主键，控制器 create 会将 id 重置为 0。
//! 需要特定 ID 的实体通过 repo.save() 直接预置。

use serde_json::json;
use std::sync::Arc;
use sz_rust_core::orm::repository::{EntityAttributes, InMemoryRepository, Repository};
use sz_rust_core::orm::Value as OrmValue;

use sz_rust_addons_ecommerce::controller::cart::CartController;
use sz_rust_addons_ecommerce::controller::order::OrderController;
use sz_rust_addons_ecommerce::controller::order_item::OrderItemController;
use sz_rust_addons_ecommerce::model::cart::CartItem;
use sz_rust_addons_ecommerce::model::order::Order;
use sz_rust_addons_ecommerce::model::order_item::OrderItem;

type CartRepo = Arc<InMemoryRepository<CartItem>>;
type OrderRepo = Arc<InMemoryRepository<Order>>;
type OrderItemRepo = Arc<InMemoryRepository<OrderItem>>;

// ─────────────────────────────────────────────
// CartItem
// ─────────────────────────────────────────────

#[tokio::test]
async fn cart_item_default_values() {
    let item = CartItem::default();
    assert_eq!(item.id, 0);
    assert_eq!(item.user_id, 0);
    assert_eq!(item.product_id, 0);
    assert_eq!(item.quantity, 0);
    assert!(!item.selected);
}

#[tokio::test]
async fn cart_item_get_attribute() {
    let item = CartItem {
        id: 1,
        user_id: 10,
        product_id: 20,
        quantity: 3,
        selected: true,
        created_at: 1000,
        updated_at: 1000,
    };
    assert_eq!(item.get_attribute("id"), Some(OrmValue::I64(1)));
    assert_eq!(item.get_attribute("selected"), Some(OrmValue::Bool(true)));
    assert_eq!(item.get_attribute("unknown"), None);
}

#[tokio::test]
async fn cart_add_rejects_missing_ids() {
    let repo: CartRepo = Arc::new(InMemoryRepository::new());
    let r = CartController::add(&*repo, json!({"id": 0, "user_id": 0, "product_id": 1, "quantity": 1, "selected": false, "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
    let r = CartController::add(&*repo, json!({"id": 0, "user_id": 1, "product_id": 0, "quantity": 1, "selected": false, "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn cart_add_success() {
    let repo: CartRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "user_id": 1, "product_id": 10, "quantity": 2, "selected": true, "created_at": 0, "updated_at": 0});
    let r = CartController::add(&*repo, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["id"], 0); // controller resets id
    assert_eq!(r["data"]["quantity"], 2);
}

#[tokio::test]
async fn cart_list_by_user_id() {
    let repo: CartRepo = Arc::new(InMemoryRepository::new());
    repo.save(CartItem {
        id: 1,
        user_id: 10,
        product_id: 1,
        quantity: 1,
        selected: false,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(CartItem {
        id: 2,
        user_id: 10,
        product_id: 2,
        quantity: 2,
        selected: true,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(CartItem {
        id: 3,
        user_id: 20,
        product_id: 3,
        quantity: 1,
        selected: false,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let list = CartController::list(&*repo, 10).await;
    assert_eq!(list["code"], 0);
    assert_eq!(list["data"]["total_count"], 2);

    let list2 = CartController::list(&*repo, 20).await;
    assert_eq!(list2["data"]["total_count"], 1);
}

#[tokio::test]
async fn cart_update_qty_found_and_not_found() {
    let repo: CartRepo = Arc::new(InMemoryRepository::new());
    repo.save(CartItem {
        id: 5,
        user_id: 1,
        product_id: 10,
        quantity: 2,
        selected: false,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = CartController::update_qty(&*repo, 5, 5).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["quantity"], 5);

    let r = CartController::update_qty(&*repo, 999, 5).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn cart_delete_and_clear() {
    let repo: CartRepo = Arc::new(InMemoryRepository::new());
    repo.save(CartItem {
        id: 1,
        user_id: 1,
        product_id: 10,
        quantity: 1,
        selected: false,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(CartItem {
        id: 2,
        user_id: 1,
        product_id: 20,
        quantity: 1,
        selected: false,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = CartController::delete(&*repo, 1).await;
    assert_eq!(r["code"], 0);

    let r = CartController::clear(&*repo, 1).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["rows"], 1);

    let list = CartController::list(&*repo, 1).await;
    assert_eq!(list["data"]["total_count"], 0);
}

// ─────────────────────────────────────────────
// Order
// ─────────────────────────────────────────────

#[tokio::test]
async fn order_default_values() {
    let order = Order::default();
    assert_eq!(order.id, 0);
    assert_eq!(order.user_id, 0);
    assert_eq!(order.order_no, "");
    assert_eq!(order.status, "");
    assert_eq!(order.total_amount, 0.0);
}

#[tokio::test]
async fn order_get_attribute() {
    let order = Order {
        id: 1,
        user_id: 10,
        order_no: "ORD001".into(),
        total_amount: 99.9,
        paid_amount: 99.9,
        status: "paid".into(),
        shipping_address: "北京".into(),
        remark: "".into(),
        created_at: 1000,
        updated_at: 1000,
    };
    assert_eq!(
        order.get_attribute("order_no"),
        Some(OrmValue::String("ORD001".into()))
    );
    assert_eq!(
        order.get_attribute("total_amount"),
        Some(OrmValue::F64(99.9))
    );
    assert_eq!(order.get_attribute("unknown"), None);
}

#[tokio::test]
async fn order_create_rejects_invalid() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    let r = OrderController::create(&*repo, json!({"id": 0, "user_id": 0, "order_no": "ORD001", "total_amount": 10.0, "paid_amount": 0.0, "status": "pending", "shipping_address": "", "remark": "", "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
    let r = OrderController::create(&*repo, json!({"id": 0, "user_id": 1, "order_no": "", "total_amount": 10.0, "paid_amount": 0.0, "status": "pending", "shipping_address": "", "remark": "", "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn order_create_success() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "user_id": 1, "order_no": "ORD001", "total_amount": 99.9, "paid_amount": 0.0, "status": "pending", "shipping_address": "北京", "remark": "", "created_at": 0, "updated_at": 0});
    let r = OrderController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["id"], 0);
    assert_eq!(r["data"]["order_no"], "ORD001");
}

#[tokio::test]
async fn order_get_found_and_not_found() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    repo.save(Order {
        id: 10,
        user_id: 1,
        order_no: "ORD010".into(),
        total_amount: 50.0,
        paid_amount: 0.0,
        status: "pending".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = OrderController::get(&*repo, 10).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["order_no"], "ORD010");

    let r = OrderController::get(&*repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn order_update_found_and_not_found() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    repo.save(Order {
        id: 5,
        user_id: 1,
        order_no: "ORD005".into(),
        total_amount: 30.0,
        paid_amount: 0.0,
        status: "pending".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = OrderController::update(&*repo, 5, json!({"status": "paid"})).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["status"], "paid");

    let r = OrderController::update(&*repo, 99, json!({"status": "paid"})).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn order_delete_found_and_not_found() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    repo.save(Order {
        id: 7,
        user_id: 1,
        order_no: "ORD007".into(),
        total_amount: 30.0,
        paid_amount: 0.0,
        status: "pending".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = OrderController::delete(&*repo, 7).await;
    assert_eq!(r["code"], 0);

    let r = OrderController::delete(&*repo, 7).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn order_cancel_state_machine() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    // pending → cancelled
    repo.save(Order {
        id: 1,
        user_id: 1,
        order_no: "ORD001".into(),
        total_amount: 50.0,
        paid_amount: 0.0,
        status: "pending".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    let r = OrderController::cancel(&*repo, 1).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["status"], "cancelled");

    // paid → cancelled
    repo.save(Order {
        id: 2,
        user_id: 1,
        order_no: "ORD002".into(),
        total_amount: 50.0,
        paid_amount: 50.0,
        status: "paid".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    let r = OrderController::cancel(&*repo, 2).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["status"], "cancelled");

    // shipped → cannot cancel
    repo.save(Order {
        id: 3,
        user_id: 1,
        order_no: "ORD003".into(),
        total_amount: 50.0,
        paid_amount: 50.0,
        status: "shipped".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    let r = OrderController::cancel(&*repo, 3).await;
    assert_eq!(r["code"], 400);
    assert!(r["msg"]
        .as_str()
        .unwrap()
        .contains("仅 pending/paid 状态可取消"));
}

#[tokio::test]
async fn order_list_filter_by_status() {
    let repo: OrderRepo = Arc::new(InMemoryRepository::new());
    repo.save(Order {
        id: 1,
        user_id: 1,
        order_no: "ORD001".into(),
        total_amount: 10.0,
        paid_amount: 0.0,
        status: "pending".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(Order {
        id: 2,
        user_id: 1,
        order_no: "ORD002".into(),
        total_amount: 20.0,
        paid_amount: 20.0,
        status: "paid".into(),
        shipping_address: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let all = OrderController::list(&*repo, 1, 10, None).await;
    assert_eq!(all["data"]["total"], 2);

    let pending = OrderController::list(&*repo, 1, 10, Some("pending".into())).await;
    assert_eq!(pending["data"]["total"], 1);
}

// ─────────────────────────────────────────────
// OrderItem
// ─────────────────────────────────────────────

#[tokio::test]
async fn order_item_default_values() {
    let item = OrderItem::default();
    assert_eq!(item.id, 0);
    assert_eq!(item.order_id, 0);
    assert_eq!(item.product_id, 0);
    assert_eq!(item.product_name, "");
    assert_eq!(item.unit_price, 0.0);
    assert_eq!(item.quantity, 0);
    assert_eq!(item.subtotal, 0.0);
}

#[tokio::test]
async fn order_item_get_attribute() {
    let item = OrderItem {
        id: 1,
        order_id: 10,
        product_id: 20,
        product_name: "手机".into(),
        unit_price: 2999.0,
        quantity: 2,
        subtotal: 5998.0,
        created_at: 1000,
        updated_at: 1000,
    };
    assert_eq!(
        item.get_attribute("product_name"),
        Some(OrmValue::String("手机".into()))
    );
    assert_eq!(item.get_attribute("subtotal"), Some(OrmValue::F64(5998.0)));
    assert_eq!(item.get_attribute("unknown"), None);
}

#[tokio::test]
async fn order_item_create_rejects_missing_ids() {
    let repo: OrderItemRepo = Arc::new(InMemoryRepository::new());
    let r = OrderItemController::create(&*repo, json!({"id": 0, "order_id": 0, "product_id": 1, "product_name": "手机", "unit_price": 10.0, "quantity": 1, "subtotal": 10.0, "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
    let r = OrderItemController::create(&*repo, json!({"id": 0, "order_id": 1, "product_id": 0, "product_name": "手机", "unit_price": 10.0, "quantity": 1, "subtotal": 10.0, "created_at": 0, "updated_at": 0})).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn order_item_create_and_delete() {
    let repo: OrderItemRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "order_id": 1, "product_id": 10, "product_name": "手机", "unit_price": 2999.0, "quantity": 1, "subtotal": 2999.0, "created_at": 0, "updated_at": 0});
    let r = OrderItemController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);

    repo.save(OrderItem {
        id: 5,
        order_id: 1,
        product_id: 10,
        product_name: "手机".into(),
        unit_price: 10.0,
        quantity: 1,
        subtotal: 10.0,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    let r = OrderItemController::delete(&*repo, 5).await;
    assert_eq!(r["code"], 0);
    let r = OrderItemController::delete(&*repo, 5).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn order_item_list_filter_by_order_id() {
    let repo: OrderItemRepo = Arc::new(InMemoryRepository::new());
    repo.save(OrderItem {
        id: 1,
        order_id: 1,
        product_id: 10,
        product_name: "手机".into(),
        unit_price: 10.0,
        quantity: 1,
        subtotal: 10.0,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(OrderItem {
        id: 2,
        order_id: 1,
        product_id: 20,
        product_name: "耳机".into(),
        unit_price: 5.0,
        quantity: 2,
        subtotal: 10.0,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    repo.save(OrderItem {
        id: 3,
        order_id: 2,
        product_id: 30,
        product_name: "充电器".into(),
        unit_price: 3.0,
        quantity: 1,
        subtotal: 3.0,
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let all = OrderItemController::list(&*repo, 1, 10, None).await;
    assert_eq!(all["data"]["total"], 3);

    let for_order_1 = OrderItemController::list(&*repo, 1, 10, Some(1)).await;
    assert_eq!(for_order_1["data"]["total"], 2);
}
