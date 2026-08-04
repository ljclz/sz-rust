//! ERP 集成测试 — 覆盖 Product / Supplier / PurchaseOrder 模型与控制器
//!
//! 使用 `InMemoryRepository` 作为仓储实现，不依赖真实数据库。

use serde_json::json;
use std::sync::Arc;
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::repository::{InMemoryRepository, Repository};

use sz_rust_addons_erp::controller::product::ProductController;
use sz_rust_addons_erp::controller::purchase_order::PurchaseOrderController;
use sz_rust_addons_erp::controller::supplier::SupplierController;
use sz_rust_addons_erp::model::product::Product;
use sz_rust_addons_erp::model::purchase_order::PurchaseOrder;
use sz_rust_addons_erp::model::supplier::Supplier;

type ProductRepo = Arc<InMemoryRepository<Product>>;
type SupplierRepo = Arc<InMemoryRepository<Supplier>>;
type PurchaseOrderRepo = Arc<InMemoryRepository<PurchaseOrder>>;

// ============================================================================
// Product 测试
// ============================================================================

#[test]
fn product_default_values() {
    let p = Product::default();
    assert_eq!(p.id, 0);
    assert!(p.name.is_empty());
    assert_eq!(p.price, 0.0);
    assert_eq!(p.stock, 0);
}

#[test]
fn product_get_attribute_all_fields() {
    let p = Product {
        id: 1,
        name: "Widget".into(),
        sku: "W-001".into(),
        price: 99.9,
        stock: 500,
        supplier_id: 3,
        category: "Electronics".into(),
        status: "active".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    };
    assert_eq!(
        p.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(1))
    );
    assert_eq!(
        p.get_attribute("name"),
        Some(sz_rust_core::orm::Value::String("Widget".into()))
    );
    assert_eq!(
        p.get_attribute("sku"),
        Some(sz_rust_core::orm::Value::String("W-001".into()))
    );
    assert_eq!(
        p.get_attribute("price"),
        Some(sz_rust_core::orm::Value::F64(99.9))
    );
    assert_eq!(
        p.get_attribute("stock"),
        Some(sz_rust_core::orm::Value::I64(500))
    );
    assert_eq!(
        p.get_attribute("supplier_id"),
        Some(sz_rust_core::orm::Value::I64(3))
    );
    assert_eq!(
        p.get_attribute("category"),
        Some(sz_rust_core::orm::Value::String("Electronics".into()))
    );
    assert_eq!(
        p.get_attribute("status"),
        Some(sz_rust_core::orm::Value::String("active".into()))
    );
    assert_eq!(p.get_attribute("unknown"), None);
}

#[tokio::test]
async fn product_create_rejects_empty_name() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "name": "", "sku": "X", "price": 1.0, "stock": 0, "supplier_id": 0, "category": "", "status": "", "remark": "", "created_at": 0, "updated_at": 0});
    let r = ProductController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "name 必填");
}

#[tokio::test]
async fn product_create_success() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 1, "name": "Widget", "sku": "W-001", "price": 99.9, "stock": 500, "supplier_id": 3, "category": "Electronics", "status": "active", "remark": "", "created_at": 0, "updated_at": 0});
    let r = ProductController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["msg"], "created");
    assert_eq!(r["data"]["name"], "Widget");
    assert_eq!(r["data"]["id"], 0); // 控制器重置 id 为 0
}

#[tokio::test]
async fn product_get_found_and_not_found() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    repo.save(Product {
        id: 0,
        name: "Gadget".into(),
        sku: "G-001".into(),
        price: 49.9,
        stock: 100,
        supplier_id: 1,
        category: "Tools".into(),
        status: "active".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    assert_eq!(ProductController::get(&*repo, 0).await["code"], 0);
    assert_eq!(
        ProductController::get(&*repo, 0).await["data"]["name"],
        "Gadget"
    );
    assert_eq!(ProductController::get(&*repo, 99).await["code"], 404);
}

#[tokio::test]
async fn product_update_patches_fields() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    repo.save(Product {
        id: 0,
        name: "Original".into(),
        sku: "O-001".into(),
        price: 10.0,
        stock: 50,
        supplier_id: 1,
        category: "C".into(),
        status: "active".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    let r = ProductController::update(
        &*repo,
        0,
        json!({"price": 25.0, "stock": 200, "status": "inactive"}),
    )
    .await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Original"); // unchanged
    assert_eq!(r["data"]["price"], 25.0);
    assert_eq!(r["data"]["stock"], 200);
    assert_eq!(r["data"]["status"], "inactive");
}

#[tokio::test]
async fn product_delete_found_and_not_found() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    repo.save(Product {
        id: 7,
        name: "ToDelete".into(),
        sku: "D".into(),
        price: 1.0,
        stock: 0,
        supplier_id: 0,
        category: "".into(),
        status: "".into(),
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();
    assert_eq!(ProductController::delete(&*repo, 7).await["code"], 0);
    assert_eq!(ProductController::delete(&*repo, 7).await["code"], 404);
}

#[tokio::test]
async fn product_list_filter_by_category() {
    let repo: ProductRepo = Arc::new(InMemoryRepository::new());
    for (i, cat) in ["Electronics", "Tools", "Electronics"].iter().enumerate() {
        repo.save(Product {
            id: i as i64,
            name: format!("P{}", i),
            sku: format!("S{}", i),
            price: 10.0,
            stock: 10,
            supplier_id: 0,
            category: cat.to_string(),
            status: "active".into(),
            remark: "".into(),
            created_at: 0,
            updated_at: 0,
        })
        .unwrap();
    }
    let r = ProductController::list(&*repo, 1, 10, Some("Electronics".to_string())).await;
    assert_eq!(r["data"]["total"], 2);
    let r = ProductController::list(&*repo, 1, 2, None).await;
    assert_eq!(r["data"]["page_size"], 2);
}

// ============================================================================
// Supplier 测试
// ============================================================================

#[test]
fn supplier_default_values() {
    let s = Supplier::default();
    assert_eq!(s.id, 0);
    assert!(s.name.is_empty());
    assert_eq!(s.credit_level, 0);
}

#[test]
fn supplier_get_attribute() {
    let s = Supplier {
        id: 5,
        name: "Acme Corp".into(),
        contact: "John".into(),
        phone: "123".into(),
        email: "a@b.com".into(),
        address: "NY".into(),
        credit_level: 4,
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    };
    assert_eq!(
        s.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(5))
    );
    assert_eq!(
        s.get_attribute("name"),
        Some(sz_rust_core::orm::Value::String("Acme Corp".into()))
    );
    assert_eq!(
        s.get_attribute("credit_level"),
        Some(sz_rust_core::orm::Value::U8(4))
    );
}

#[tokio::test]
async fn supplier_create_rejects_empty_name() {
    let repo: SupplierRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "name": "", "contact": "", "phone": "", "email": "", "address": "", "credit_level": 0, "remark": "", "created_at": 0, "updated_at": 0});
    let r = SupplierController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "name 必填");
}

#[tokio::test]
async fn supplier_create_success() {
    let repo: SupplierRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 1, "name": "Acme Corp", "contact": "John", "phone": "555-0100", "email": "acme@example.com", "address": "123 Main St", "credit_level": 5, "remark": "VIP", "created_at": 0, "updated_at": 0});
    let r = SupplierController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Acme Corp");
    assert_eq!(r["data"]["id"], 0);
}

#[tokio::test]
async fn supplier_list_keyword_substring_match() {
    let repo: SupplierRepo = Arc::new(InMemoryRepository::new());
    // 供应商控制器使用 format!("%{}%", k) 作为 LIKE 模式，支持子串匹配
    for (i, name) in ["Acme Corp", "Beta Inc", "Acme Subsidiary"]
        .iter()
        .enumerate()
    {
        repo.save(Supplier {
            id: i as i64,
            name: name.to_string(),
            ..Default::default()
        })
        .unwrap();
    }
    let r = SupplierController::list(&*repo, 1, 10, Some("Acme".to_string())).await;
    assert_eq!(r["data"]["total"], 2);
}

#[tokio::test]
async fn supplier_update_credit_level() {
    let repo: SupplierRepo = Arc::new(InMemoryRepository::new());
    repo.save(Supplier {
        id: 0,
        name: "S".into(),
        ..Default::default()
    })
    .unwrap();
    let r = SupplierController::update(&*repo, 0, json!({"credit_level": 5})).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["credit_level"], 5);
}

#[tokio::test]
async fn supplier_delete_not_found() {
    let repo: SupplierRepo = Arc::new(InMemoryRepository::new());
    assert_eq!(SupplierController::delete(&*repo, 999).await["code"], 404);
}

// ============================================================================
// PurchaseOrder 测试
// ============================================================================

#[test]
fn purchase_order_default_values() {
    let po = PurchaseOrder::default();
    assert_eq!(po.id, 0);
    assert_eq!(po.supplier_id, 0);
    assert_eq!(po.product_id, 0);
    assert_eq!(po.quantity, 0);
    assert_eq!(po.status, "");
}

#[test]
fn purchase_order_get_attribute() {
    let po = PurchaseOrder {
        id: 10,
        supplier_id: 3,
        product_id: 7,
        quantity: 100,
        unit_price: 5.5,
        total_amount: 550.0,
        status: "pending".into(),
        order_date: 1700000000,
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    };
    assert_eq!(
        po.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(10))
    );
    assert_eq!(
        po.get_attribute("total_amount"),
        Some(sz_rust_core::orm::Value::F64(550.0))
    );
    assert_eq!(
        po.get_attribute("status"),
        Some(sz_rust_core::orm::Value::String("pending".into()))
    );
}

#[tokio::test]
async fn purchase_order_create_rejects_missing_ids() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    // supplier_id 为 0 触发 400
    let body = json!({"id": 0, "supplier_id": 0, "product_id": 5, "quantity": 10, "unit_price": 1.0, "total_amount": 10.0, "status": "pending", "order_date": 0, "remark": "", "created_at": 0, "updated_at": 0});
    let r = PurchaseOrderController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "supplier_id 和 product_id 必填");
}

#[tokio::test]
async fn purchase_order_create_success() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 1, "supplier_id": 3, "product_id": 7, "quantity": 100, "unit_price": 5.5, "total_amount": 550.0, "status": "pending", "order_date": 1700000000, "remark": "", "created_at": 0, "updated_at": 0});
    let r = PurchaseOrderController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["id"], 0); // 控制器重置 id
}

#[tokio::test]
async fn purchase_order_approve_state_machine() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    // 创建 pending 订单
    repo.save(PurchaseOrder {
        id: 0,
        supplier_id: 1,
        product_id: 1,
        quantity: 10,
        unit_price: 10.0,
        total_amount: 100.0,
        status: "pending".into(),
        order_date: 0,
        remark: "".into(),
        created_at: 0,
        updated_at: 0,
    })
    .unwrap();

    // 审批 pending -> approved
    let r = PurchaseOrderController::approve(&*repo, 0).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["msg"], "approved");
    assert_eq!(r["data"]["status"], "approved");

    // 已审批的订单不能再次审批
    let r = PurchaseOrderController::approve(&*repo, 0).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "仅 pending 状态的采购单可审批");
}

#[tokio::test]
async fn purchase_order_approve_not_found() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    let r = PurchaseOrderController::approve(&*repo, 999).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn purchase_order_lifecycle() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    // Create
    let body = json!({"id": 0, "supplier_id": 2, "product_id": 8, "quantity": 50, "unit_price": 20.0, "total_amount": 1000.0, "status": "pending", "order_date": 1700000000, "remark": "", "created_at": 0, "updated_at": 0});
    assert_eq!(
        PurchaseOrderController::create(&*repo, body).await["code"],
        0
    );

    // Get (id=0)
    let r = PurchaseOrderController::get(&*repo, 0).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["quantity"], 50);

    // Update
    let r = PurchaseOrderController::update(&*repo, 0, json!({"quantity": 75, "unit_price": 18.0}))
        .await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["quantity"], 75);

    // Delete
    assert_eq!(PurchaseOrderController::delete(&*repo, 0).await["code"], 0);
    assert_eq!(PurchaseOrderController::get(&*repo, 0).await["code"], 404);
}

#[tokio::test]
async fn purchase_order_list_filter_by_status() {
    let repo: PurchaseOrderRepo = Arc::new(InMemoryRepository::new());
    for (i, status) in ["pending", "approved", "pending"].iter().enumerate() {
        repo.save(PurchaseOrder {
            id: i as i64,
            supplier_id: 1,
            product_id: 1,
            quantity: 10,
            unit_price: 1.0,
            total_amount: 10.0,
            status: status.to_string(),
            order_date: 0,
            remark: "".into(),
            created_at: 0,
            updated_at: 0,
        })
        .unwrap();
    }
    let r = PurchaseOrderController::list(&*repo, 1, 10, Some("pending".to_string())).await;
    assert_eq!(r["data"]["total"], 2);
}
