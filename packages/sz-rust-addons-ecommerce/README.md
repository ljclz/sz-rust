# SZ-Rust 电商插件

> 订单/订单项/购物车骨架，提供电商基础 CRUD、购物车同用户同商品数量累加与订单正向流转状态机。

## 1. 插件简介

**功能描述**：提供电商基础骨架，包含订单管理、订单项管理、购物车管理三大模块。支持购物车同用户同商品数量累加策略和订单正向流转状态机（pending → paid → shipped → completed），可通过 Capability Registry 统一调用。

**适用场景**：
- 电商订单全流程管理
- 购物车合并与结算
- 订单状态流转与追踪

**版本信息**：v1.1.0，兼容 SZ-Rust >=1.1.0

## 2. 安装方法

```bash
# 使用 SZ-Rust CLI 安装
sz-rust-cli plugin install ecommerce

# 或在 Cargo.toml 中添加依赖
[dependencies]
sz-rust-addons-ecommerce = { workspace = true }
```

## 3. 配置说明

```rust,ignore
use sz_rust_addons_ecommerce::EcommerceState;

// 默认配置（使用 InMemoryRepository）
let ec_state = EcommerceState::default();
```

`EcommerceState` 包含三个仓储字段：
- `orders: OrderRepo` — 订单仓储
- `order_items: OrderItemRepo` — 订单项仓储
- `carts: CartRepo` — 购物车仓储

## 4. 路由表

| 方法 | 路径 | 处理函数 | 说明 |
|------|------|---------|------|
| GET | `/api/ecommerce/orders` | `OrderController::list` | 订单列表 |
| POST | `/api/ecommerce/orders` | `OrderController::create` | 创建订单 |
| GET | `/api/ecommerce/orders/:id` | `OrderController::get` | 获取订单 |
| PUT | `/api/ecommerce/orders/:id` | `OrderController::update` | 更新订单 |
| DELETE | `/api/ecommerce/orders/:id` | `OrderController::delete` | 删除订单 |
| POST | `/api/ecommerce/orders/:id/cancel` | `OrderController::cancel` | **取消订单** |
| POST | `/api/ecommerce/orders/:id/pay` | `OrderController::pay` | **订单支付** |
| POST | `/api/ecommerce/orders/:id/ship` | `OrderController::ship` | **订单发货** |
| POST | `/api/ecommerce/orders/:id/complete` | `OrderController::complete` | **订单完成** |
| GET | `/api/ecommerce/order_items` | `OrderItemController::list` | 订单项列表 |
| POST | `/api/ecommerce/order_items` | `OrderItemController::create` | 创建订单项 |
| DELETE | `/api/ecommerce/order_items/:id` | `OrderItemController::delete` | 删除订单项 |
| GET | `/api/ecommerce/cart` | `CartController::list` | 购物车列表 |
| POST | `/api/ecommerce/cart` | `CartController::add` | **添加购物车**（累加） |
| PUT | `/api/ecommerce/cart/:id` | `CartController::update_qty` | 更新数量 |
| DELETE | `/api/ecommerce/cart/:id` | `CartController::delete` | 删除购物车项 |
| DELETE | `/api/ecommerce/cart/clear/:user_id` | `CartController::clear` | **清空购物车** |

## 5. 能力清单

本插件提供 6 个 Capability：

| 能力名称 | 描述 | 标签 | 需确认 |
|---------|------|------|--------|
| `ecommerce.create_order` | 创建订单 | ecommerce, order, create, write | 否 |
| `ecommerce.search_order` | 搜索订单列表 | ecommerce, order, search, read | 否 |
| `ecommerce.cancel_order` | 取消订单 | ecommerce, order, cancel, write | **是** |
| `ecommerce.query_cart` | 查询购物车 | ecommerce, cart, query, read | 否 |
| `ecommerce.add_to_cart` | 添加到购物车 | ecommerce, cart, add, write | 否 |
| `ecommerce.clear_cart` | 清空购物车 | ecommerce, cart, clear, write | **是** |

### 购物车累加策略

同一用户同一商品的多次添加，数量自动累加，不创建新记录：

```
用户 1 添加商品 A ×2 → 购物车记录：{user:1, product:A, qty:2}
用户 1 添加商品 A ×3 → 购物车记录：{user:1, product:A, qty:5}（累加，非新记录）
用户 2 添加商品 A ×1 → 购物车记录：{user:2, product:A, qty:1}（不同用户，新记录）
```

### 订单正向流转状态机

```
pending → paid → shipped → completed
   ↘        ↘
    cancelled  cancelled
```

- `pay`：pending → paid
- `ship`：paid → shipped
- `complete`：shipped → completed
- `cancel`：pending/paid → cancelled

非法流转（如 pending → shipped 跳过 pay）返回 `ValidationError`。

## 6. 使用示例

### 注册路由

```rust,ignore
use sz_rust_addons_ecommerce::{register_routes, EcommerceState};

let builder = RouterBuilder::new();
let ec_state = EcommerceState::default();
let builder = register_routes(builder, ec_state);
```

### 注册 Capability

```rust,ignore
use sz_rust_addons_ecommerce::capability::EcommercePlugin;
use sz_rust_addons_ecommerce::EcommerceState;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_addons_loader::CapabilityHook;

let registry = CapabilityRegistry::new();
let plugin = EcommercePlugin::new(EcommerceState::default());
let names = plugin.register_capabilities(&registry).unwrap();
assert_eq!(names.len(), 6);
```

### 创建订单

```rust,ignore
use sz_rust_capability::Capability;

let cap = registry.find("ecommerce.create_order").unwrap();
let result = cap.call(serde_json::json!({
    "user_id": 1,
    "shipping_address": "北京",
    "items": [
        {"product_id": 10, "product_name": "手机", "unit_price": 2999.0, "quantity": 2},
        {"product_id": 20, "product_name": "耳机", "unit_price": 99.0, "quantity": 1}
    ]
})).await.unwrap();
// result["data"]["order"]["status"] == "pending"
// result["data"]["order"]["total_amount"] == 6097.0
// result["data"]["items_count"] == 2
```

### 添加购物车

```rust,ignore
let cap = registry.find("ecommerce.add_to_cart").unwrap();
cap.call(serde_json::json!({"user_id": 1, "product_id": 10, "quantity": 2})).await.unwrap();
let result = cap.call(serde_json::json!({"user_id": 1, "product_id": 10, "quantity": 3})).await.unwrap();
// result["msg"] == "merged"
// result["data"]["quantity"] == 5
```

## 7. 常见问题

### 如何切换真实数据库？

将 `EcommerceState` 的仓储字段替换为基于 SZ-ORM 的实现。

### 购物车添加相同商品会创建多条记录吗？

不会。同用户同商品自动累加数量，购物车记录数不变。不同用户或不同商品才会创建新记录。

### 订单状态可以跳过吗？

不可以。正向流转必须按 `pending → paid → shipped → completed` 顺序执行。跳过中间阶段（如 pending 直接 ship）会返回 `ValidationError`。取消操作仅限 pending/paid 状态。

### InMemoryRepository 不自增 ID 有什么影响？

通过 Controller 的 create/add 方法创建记录时，ID 会重置为 0。如需创建多条记录，应通过 `repo.save()` 直接设置具体 ID。