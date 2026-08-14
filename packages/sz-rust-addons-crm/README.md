# SZ-Rust CRM 插件

> 联系人/线索/商机管理骨架，提供 CRM 基础 CRUD、线索转化原子性与商机阶段流转校验。

## 1. 插件简介

**功能描述**：提供客户关系管理（CRM）基础骨架，包含联系人管理、线索跟进、商机阶段管理三大模块。支持线索转化原子性（三步操作 + 手动回滚）和商机阶段流转校验（6 阶段状态机），可通过 Capability Registry 统一调用。

**适用场景**：
- 销售团队客户关系管理
- 线索转化与商机跟进
- 销售漏斗分析

**版本信息**：v1.1.0，兼容 SZ-Rust >=1.1.0

## 2. 安装方法

```bash
# 使用 SZ-Rust CLI 安装
sz-rust-cli plugin install crm

# 或在 Cargo.toml 中添加依赖
[dependencies]
sz-rust-addons-crm = { workspace = true }
```

## 3. 配置说明

```rust,ignore
use sz_rust_addons_crm::CrmState;

// 默认配置（使用 InMemoryRepository）
let crm_state = CrmState::default();
```

`CrmState` 包含三个仓储字段：
- `contacts: ContactRepo` — 联系人仓储
- `leads: LeadRepo` — 线索仓储
- `deals: DealRepo` — 商机仓储

## 4. 路由表

| 方法 | 路径 | 处理函数 | 说明 |
|------|------|---------|------|
| GET | `/api/crm/contacts` | `ContactController::list` | 联系人列表 |
| POST | `/api/crm/contacts` | `ContactController::create` | 创建联系人 |
| GET | `/api/crm/contacts/:id` | `ContactController::get` | 获取联系人 |
| PUT | `/api/crm/contacts/:id` | `ContactController::update` | 更新联系人 |
| DELETE | `/api/crm/contacts/:id` | `ContactController::delete` | 删除联系人 |
| GET | `/api/crm/leads` | `LeadController::list` | 线索列表 |
| POST | `/api/crm/leads` | `LeadController::create` | 创建线索 |
| GET | `/api/crm/leads/:id` | `LeadController::get` | 获取线索 |
| PUT | `/api/crm/leads/:id` | `LeadController::update` | 更新线索 |
| DELETE | `/api/crm/leads/:id` | `LeadController::delete` | 删除线索 |
| POST | `/api/crm/leads/:id/convert` | `LeadController::convert` | **线索转化**（原子性） |
| GET | `/api/crm/deals` | `DealController::list` | 商机列表 |
| POST | `/api/crm/deals` | `DealController::create` | 创建商机 |
| GET | `/api/crm/deals/:id` | `DealController::Dget` | 获取商机 |
| PUT | `/api/crm/deals/:id` | `DealController::update` | 更新商机 |
| DELETE | `/api/crm/deals/:id` | `DealController::delete` | 删除商机 |
| GET | `/api/crm/deals/pipeline` | `DealController::pipeline` | **销售漏斗** |

## 5. 能力清单

本插件提供 7 个 Capability：

| 能力名称 | 描述 | 标签 | 需确认 |
|---------|------|------|--------|
| `crm.search_contact` | 搜索联系人列表 | crm, contact, search, read | 否 |
| `crm.create_contact` | 创建联系人 | crm, contact, create, write | 否 |
| `crm.search_lead` | 搜索线索列表 | crm, lead, search, read | 否 |
| `crm.convert_lead` | 线索转化（原子性） | crm, lead, convert, write | **是** |
| `crm.search_deal` | 搜索商机列表 | crm, deal, search, read | 否 |
| `crm.update_deal_stage` | 更新商机阶段 | crm, deal, update, write | **是** |
| `crm.query_pipeline` | 查询销售漏斗 | crm, deal, pipeline, read | 否 |

### 线索转化原子流程

```
步骤 ①：校验线索存在 + 未转化 → 标记为 converted
步骤 ②：创建 Contact（从 Lead 复制 name/phone/email）
步骤 ③：创建 Deal（name = company + " 商机"）
```

**手动回滚策略**（InMemoryRepository 无事务）：
- 步骤 ② 失败 → 回滚步骤 ①（恢复 lead.status）
- 步骤 ③ 失败 → 回滚步骤 ②（删除 Contact）+ 回滚步骤 ①

### 商机阶段流转

```
initial → requirement_confirmed → quoted → negotiating → won
                                                        ↘ lost
```

非法流转返回 `ValidationError`，终态（won/lost）不可回退。

## 6. 使用示例

### 注册路由

```rust,ignore
use sz_rust_addons_crm::{register_routes, CrmState};

let builder = RouterBuilder::new();
let crm_state = CrmState::default();
let builder = register_routes(builder, crm_state);
```

### 注册 Capability

```rust,ignore
use sz_rust_addons_crm::capability::CrmPlugin;
use sz_rust_addons_crm::CrmState;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_addons_loader::CapabilityHook;

let registry = CapabilityRegistry::new();
let plugin = CrmPlugin::new(CrmState::default());
let names = plugin.register_capabilities(&registry).unwrap();
assert_eq!(names.len(), 7);
```

### 线索转化

```rust,ignore
use sz_rust_capability::Capability;

let cap = registry.find("crm.convert_lead").unwrap();
let result = cap.call(serde_json::json!({"id": 1})).await.unwrap();
// result["data"]["lead"]["status"] == "converted"
// result["data"]["contact"] — 新创建的联系人
// result["data"]["deal"] — 新创建的商机
```

## 7. 常见问题

### 如何切换真实数据库？

将 `CrmState` 的仓储字段替换为基于 SZ-ORM 的实现。

### 线索转化失败时会怎样？

步骤 ② 失败 → 自动回滚步骤 ①（恢复 lead.status 原状态）
步骤 ③ 失败 → 自动回滚步骤 ②（删除已创建 Contact）+ 回滚步骤 ①

### 商机阶段可以跳过吗？

不可以。必须按 `initial → requirement_confirmed → quoted → negotiating → won/lost` 顺序流转。跳过中间阶段会返回 `ValidationError`。