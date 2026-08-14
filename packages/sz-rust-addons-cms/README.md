# SZ-Rust CMS 插件

> 文章/分类/标签管理骨架，提供 CMS 基础 CRUD 与状态机校验。

## 1. 插件简介

**功能描述**：提供内容管理系统（CMS）基础骨架，包含文章发布与管理、分类管理、标签管理三大模块。支持文章状态机校验（draft → published → archived），可通过 Capability Registry 统一调用。

**适用场景**：
- 博客系统、新闻门户、文档站点
- 需要文章状态流转管理的内容平台
- 作为 SZ-Rust 插件开发的参考模板

**版本信息**：v1.1.0，兼容 SZ-Rust >=1.1.0

## 2. 安装方法

```bash
# 使用 SZ-Rust CLI 安装
sz-rust-cli plugin install cms

# 或在 Cargo.toml 中添加依赖
[dependencies]
sz-rust-addons-cms = { workspace = true }
```

## 3. 配置说明

```rust,ignore
use sz_rust_addons_cms::CmsState;

// 默认配置（使用 InMemoryRepository）
let cms_state = CmsState::default();

// 自定义配置
let cms_state = CmsState {
    articles: Arc::new(InMemoryRepository::new()),
    categories: Arc::new(InMemoryRepository::new()),
    tags: Arc::new(InMemoryRepository::new()),
};
```

`CmsState` 包含三个仓储字段：
- `articles: ArticleRepo` — 文章仓储
- `categories: CategoryRepo` — 分类仓储
- `tags: TagRepo` — 标签仓储

## 4. 路由表

| 方法 | 路径 | 处理函数 | 说明 |
|------|------|---------|------|
| GET | `/api/cms/articles` | `ArticleController::list` | 文章列表（支持 keyword/category_id/status 过滤 + 分页） |
| POST | `/api/cms/articles` | `ArticleController::create` | 创建文章 |
| GET | `/api/cms/articles/:id` | `ArticleController::get` | 获取文章详情 |
| PUT | `/api/cms/articles/:id` | `ArticleController::update` | 更新文章 |
| DELETE | `/api/cms/articles/:id` | `ArticleController::delete` | 删除文章 |
| GET | `/api/cms/categories` | `CategoryController::list` | 分类列表 |
| POST | `/api/cms/categories` | `CategoryController::create` | 创建分类 |
| GET | `/api/cms/categories/:id` | `CategoryController::get` | 获取分类详情 |
| DELETE | `/api/cms/categories/:id` | `CategoryController::delete` | 删除分类 |
| GET | `/api/cms/tags` | `TagController::list` | 标签列表 |
| POST | `/api/cms/tags` | `TagController::create` | 创建标签 |
| DELETE | `/api/cms/tags/:id` | `TagController::delete` | 删除标签 |

## 5. 能力清单

本插件提供 5 个 Capability，通过 `CapabilityRegistry` 统一调用：

| 能力名称 | 描述 | 标签 | 需确认 |
|---------|------|------|--------|
| `cms.search_article` | 搜索文章列表，支持关键词、分类、状态过滤与分页 | cms, article, search, read | 否 |
| `cms.create_article` | 创建文章，title 必填，status 默认 draft | cms, article, create, write | 否 |
| `cms.publish_article` | 发布文章，校验当前状态为 draft，更新为 published | cms, article, publish, write | 否 |
| `cms.manage_category` | 分类管理，支持 list/create/get/delete 操作 | cms, category, manage, write | 否 |
| `cms.manage_tag` | 标签管理，支持 list/create/delete 操作 | cms, tag, manage, write | 否 |

### 文章状态机

```
draft ──→ published ──→ archived
  │                         ↑
  └─────────────────────────┘
```

- `draft → published`：发布
- `draft → archived`：直接下架
- `published → archived`：下架
- `archived` 为终态，不可再流转

## 6. 使用示例

### 注册路由

```rust,ignore
use sz_rust_addons_cms::{register_routes, CmsState};
use sz_rust_core::router::RouterBuilder;

let builder = RouterBuilder::new();
let cms_state = CmsState::default();
let builder = register_routes(builder, cms_state);
```

### 注册 Capability

```rust,ignore
use sz_rust_addons_cms::capability::CmsPlugin;
use sz_rust_addons_cms::CmsState;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_addons_loader::CapabilityHook;

let registry = CapabilityRegistry::new();
let cms_state = CmsState::default();
let plugin = CmsPlugin::new(cms_state);

// 注册 5 个能力到 Registry
let names = plugin.register_capabilities(&registry).unwrap();
assert_eq!(names.len(), 5);

// 调用能力
use sz_rust_capability::Capability;
let cap = registry.find("cms.search_article").unwrap();
let result = cap.call(serde_json::json!({"keyword": "Rust"})).await.unwrap();
```

### 卸载插件能力

```rust,ignore
use sz_rust_addons_loader::unregister_plugin_capabilities;

let removed = unregister_plugin_capabilities(&registry, "cms");
assert_eq!(removed.len(), 5);
```

## 7. 常见问题

### 如何切换真实数据库？

当前使用 `InMemoryRepository` 作为仓储实现。要切换到真实数据库，需将 `CmsState` 的仓储字段替换为基于 SZ-ORM 的实现：

```rust,ignore
// 替换为 MySQL/PostgreSQL 仓储
let cms_state = CmsState {
    articles: Arc::new(MySqlRepository::new(pool.clone())),
    categories: Arc::new(MySqlRepository::new(pool.clone())),
    tags: Arc::new(MySqlRepository::new(pool.clone())),
};
```

### 如何扩展文章字段？

修改 `src/model/article.rs` 中的 `Article` 结构体，同步更新 `EntityAttributes` 实现和 `ArticleController::update` 的 patch 逻辑。

### 如何添加新的 Capability？

1. 在 `src/capability/mod.rs` 中新增 Capability struct，实现 `Capability` trait
2. 在 `CmsPlugin::register_capabilities` 中注册新能力
3. 更新 `CMS_CAPABILITY_NAMES` 常量
4. 更新 `manifest.json` 的 `capabilities` 数组