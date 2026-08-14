//! CMS 插件能力实现模块。
//!
//! 提供 5 个 Capability 实现，对齐 design.md 2.2.2.4 节：
//!
//! | struct | 能力名 | tags | requires_confirmation |
//! |--------|--------|------|----------------------|
//! | `SearchArticleCapability` | `cms.search_article` | `["cms","article","search","read"]` | false |
//! | `CreateArticleCapability` | `cms.create_article` | `["cms","article","create","write"]` | false |
//! | `PublishArticleCapability` | `cms.publish_article` | `["cms","article","publish","write"]` | false |
//! | `ManageCategoryCapability` | `cms.manage_category` | `["cms","category","manage","write"]` | false |
//! | `ManageTagCapability` | `cms.manage_tag` | `["cms","tag","manage","write"]` | false |

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_capability::{CapError, CapResult, Capability, CapabilitySource};

use crate::controller::article::ArticleController;
use crate::controller::category::CategoryController;
use crate::controller::tag::TagController;
use crate::CmsState;

/// 将 Controller 返回的 JSON Value（含 code/msg/data）转换为 CapResult。
///
/// code == 0 → Ok(value)，否则 → Err(ExecutionError(msg))。
fn controller_result_to_cap_result(value: Value) -> CapResult<Value> {
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 {
        Ok(value)
    } else {
        let msg = value
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string();
        if code == 404 {
            Err(CapError::NotFound(msg))
        } else if code == 422 || code == 400 {
            Err(CapError::ValidationError(msg))
        } else {
            Err(CapError::ExecutionError(msg))
        }
    }
}

// ============================================================================
// 1. SearchArticleCapability — cms.search_article
// ============================================================================

/// 搜索文章能力。委托 `ArticleController::list`，支持 keyword/category_id/status 过滤 + 分页。
pub struct SearchArticleCapability {
    state: CmsState,
}

impl SearchArticleCapability {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for SearchArticleCapability {
    fn name(&self) -> &'static str {
        "cms.search_article"
    }

    fn description(&self) -> &'static str {
        "搜索文章列表，支持关键词、分类、状态过滤与分页"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string", "description": "标题关键词（Like 匹配）" },
                "category_id": { "type": "integer", "description": "分类 ID" },
                "status": { "type": "string", "enum": ["draft", "published", "archived"], "description": "文章状态" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            }
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["cms", "article", "search", "read"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let category_id = args.get("category_id").and_then(|v| v.as_i64());
        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let result = ArticleController::list(
            &*self.state.articles,
            page,
            page_size,
            keyword,
            category_id,
            status,
        )
        .await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 2. CreateArticleCapability — cms.create_article
// ============================================================================

/// 创建文章能力。委托 `ArticleController::create`，校验 title 必填。
pub struct CreateArticleCapability {
    state: CmsState,
}

impl CreateArticleCapability {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for CreateArticleCapability {
    fn name(&self) -> &'static str {
        "cms.create_article"
    }

    fn description(&self) -> &'static str {
        "创建文章，title 必填，status 默认 draft"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "minLength": 1, "description": "文章标题（必填）" },
                "content": { "type": "string", "description": "文章内容" },
                "summary": { "type": "string", "description": "文章摘要" },
                "category_id": { "type": "integer", "description": "分类 ID" },
                "author_id": { "type": "integer", "description": "作者 ID" },
                "status": { "type": "string", "enum": ["draft", "published"], "description": "初始状态" }
            },
            "required": ["title"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["cms", "article", "create", "write"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
        if title.is_empty() {
            return Err(CapError::ValidationError("title is required".to_string()));
        }
        let mut body = args;
        if body.get("id").is_none() {
            body["id"] = json!(0);
        }
        let result = ArticleController::create(&*self.state.articles, body).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 3. PublishArticleCapability — cms.publish_article
// ============================================================================

/// 发布文章能力。委托 `ArticleController::publish`，校验状态机 draft → published。
pub struct PublishArticleCapability {
    state: CmsState,
}

impl PublishArticleCapability {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for PublishArticleCapability {
    fn name(&self) -> &'static str {
        "cms.publish_article"
    }

    fn description(&self) -> &'static str {
        "发布文章，校验当前状态为 draft，更新为 published"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "文章 ID" }
            },
            "required": ["id"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["cms", "article", "publish", "write"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("id is required".to_string()))?;
        let result = ArticleController::publish(&*self.state.articles, id).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 4. ManageCategoryCapability — cms.manage_category
// ============================================================================

/// 分类管理能力。支持 action 分发：list/create/get/delete。
pub struct ManageCategoryCapability {
    state: CmsState,
}

impl ManageCategoryCapability {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for ManageCategoryCapability {
    fn name(&self) -> &'static str {
        "cms.manage_category"
    }

    fn description(&self) -> &'static str {
        "分类管理，支持 list/create/get/delete 操作"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "get", "delete"], "description": "操作类型" },
                "id": { "type": "integer", "description": "分类 ID（get/delete 必填）" },
                "name": { "type": "string", "description": "分类名称（create 必填）" },
                "keyword": { "type": "string", "description": "搜索关键词（list 可选）" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            },
            "required": ["action"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["cms", "category", "manage", "write"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("action is required".to_string()))?;
        let result = match action {
            "list" => {
                let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
                let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
                let keyword = args
                    .get("keyword")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                CategoryController::list(&*self.state.categories, page, page_size, keyword).await
            }
            "create" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    return Err(CapError::ValidationError("name is required".to_string()));
                }
                let body = json!({ "id": 0, "name": name });
                CategoryController::create(&*self.state.categories, body).await
            }
            "get" => {
                let id = args.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
                    CapError::ValidationError("id is required for get".to_string())
                })?;
                CategoryController::get(&*self.state.categories, id).await
            }
            "delete" => {
                let id = args.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
                    CapError::ValidationError("id is required for delete".to_string())
                })?;
                CategoryController::delete(&*self.state.categories, id).await
            }
            other => {
                return Err(CapError::ValidationError(format!(
                    "unsupported action: {other}"
                )));
            }
        };
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::article::Article;
    use sz_rust_capability::CapabilityRegistry;
    use sz_rust_core::orm::repository::{InMemoryRepository, Repository};

    fn test_state() -> CmsState {
        CmsState::default()
    }

    // --- Capability 层测试 ---

    #[tokio::test]
    async fn search_article_capability_returns_results() {
        let state = test_state();
        state
            .articles
            .save(Article {
                id: 1,
                title: "Rust".to_string(),
                status: "draft".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchArticleCapability::new(state.clone());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn create_article_capability_creates_and_returns() {
        let state = test_state();
        let cap = CreateArticleCapability::new(state.clone());
        let result = cap.call(json!({"title": "New Article"})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["title"], "New Article");
        assert_eq!(result["data"]["status"], "draft");
    }

    #[tokio::test]
    async fn publish_article_capability_changes_status() {
        let state = test_state();
        state
            .articles
            .save(Article {
                id: 1,
                title: "Draft".to_string(),
                status: "draft".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = PublishArticleCapability::new(state.clone());
        let result = cap.call(json!({"id": 1})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["status"], "published");
    }

    #[tokio::test]
    async fn publish_article_capability_rejects_archived() {
        let state = test_state();
        state
            .articles
            .save(Article {
                id: 1,
                title: "Draft".to_string(),
                status: "draft".to_string(),
                ..Default::default()
            })
            .unwrap();
        ArticleController::archive(&*state.articles, 1).await;
        let cap = PublishArticleCapability::new(state.clone());
        let result = cap.call(json!({"id": 1})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn manage_category_capability_executes_actions() {
        let state = test_state();
        let cap = ManageCategoryCapability::new(state.clone());
        let created = cap
            .call(json!({"action": "create", "name": "Tech"}))
            .await
            .unwrap();
        assert_eq!(created["code"], 0);
        let id = created["data"]["id"].as_i64().unwrap();
        let got = cap.call(json!({"action": "get", "id": id})).await.unwrap();
        assert_eq!(got["data"]["name"], "Tech");
        let deleted = cap
            .call(json!({"action": "delete", "id": id}))
            .await
            .unwrap();
        assert_eq!(deleted["code"], 0);
    }

    #[tokio::test]
    async fn manage_tag_capability_executes_actions() {
        let state = test_state();
        let cap = ManageTagCapability::new(state.clone());
        let created = cap
            .call(json!({"action": "create", "name": "rust"}))
            .await
            .unwrap();
        assert_eq!(created["code"], 0);
        let id = created["data"]["id"].as_i64().unwrap();
        let listed = cap.call(json!({"action": "list"})).await.unwrap();
        assert_eq!(listed["code"], 0);
        let deleted = cap
            .call(json!({"action": "delete", "id": id}))
            .await
            .unwrap();
        assert_eq!(deleted["code"], 0);
    }

    // --- CapabilityHook 层测试 ---

    #[test]
    fn cms_plugin_registers_5_capabilities() {
        let state = test_state();
        let plugin = CmsPlugin::new(state);
        let registry = CapabilityRegistry::new();
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 5);
        assert_eq!(registry.len(), 5);
    }

    #[test]
    fn cms_capability_names_returns_correct_list() {
        let state = test_state();
        let plugin = CmsPlugin::new(state);
        let names = plugin.capability_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"cms.search_article".to_string()));
        assert!(names.contains(&"cms.create_article".to_string()));
        assert!(names.contains(&"cms.publish_article".to_string()));
        assert!(names.contains(&"cms.manage_category".to_string()));
        assert!(names.contains(&"cms.manage_tag".to_string()));
    }

    // --- 铁律层测试 ---

    #[test]
    fn cms_capabilities_have_correct_prefix() {
        let state = test_state();
        let plugin = CmsPlugin::new(state);
        let names = plugin.capability_names();
        for name in &names {
            assert!(name.starts_with("cms."), "能力名 {name} 不以 cms. 开头");
        }
    }
}

// ============================================================================
// CmsPlugin — CapabilityHook 实现
// ============================================================================

use std::sync::Arc;
use sz_rust_addons_loader::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;

/// CMS 插件 CapabilityHook 实现。
///
/// 持有 `CmsState`，在激活时将 5 个能力注册到全局 `CapabilityRegistry`。
pub struct CmsPlugin {
    state: CmsState,
}

impl CmsPlugin {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

/// CMS 插件的 5 个能力名称常量。
pub const CMS_CAPABILITY_NAMES: [&str; 5] = [
    "cms.search_article",
    "cms.create_article",
    "cms.publish_article",
    "cms.manage_category",
    "cms.manage_tag",
];

impl CapabilityHook for CmsPlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(SearchArticleCapability::new(self.state.clone())),
            Arc::new(CreateArticleCapability::new(self.state.clone())),
            Arc::new(PublishArticleCapability::new(self.state.clone())),
            Arc::new(ManageCategoryCapability::new(self.state.clone())),
            Arc::new(ManageTagCapability::new(self.state.clone())),
        ];
        let mut names = Vec::with_capacity(caps.len());
        for cap in caps {
            let name = cap.name().to_string();
            registry.register(cap);
            names.push(name);
        }
        Ok(names)
    }

    fn capability_names(&self) -> Vec<String> {
        CMS_CAPABILITY_NAMES.iter().map(|s| s.to_string()).collect()
    }
}

// ============================================================================
// 5. ManageTagCapability — cms.manage_tag
// ============================================================================

/// 标签管理能力。支持 action 分发：list/create/delete。
pub struct ManageTagCapability {
    state: CmsState,
}

impl ManageTagCapability {
    pub fn new(state: CmsState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for ManageTagCapability {
    fn name(&self) -> &'static str {
        "cms.manage_tag"
    }

    fn description(&self) -> &'static str {
        "标签管理，支持 list/create/delete 操作"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "delete"], "description": "操作类型" },
                "id": { "type": "integer", "description": "标签 ID（delete 必填）" },
                "name": { "type": "string", "description": "标签名称（create 必填）" }
            },
            "required": ["action"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["cms", "tag", "manage", "write"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("action is required".to_string()))?;
        let result = match action {
            "list" => TagController::list(&*self.state.tags).await,
            "create" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    return Err(CapError::ValidationError("name is required".to_string()));
                }
                let body = json!({ "id": 0, "name": name });
                TagController::create(&*self.state.tags, body).await
            }
            "delete" => {
                let id = args.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
                    CapError::ValidationError("id is required for delete".to_string())
                })?;
                TagController::delete(&*self.state.tags, id).await
            }
            other => {
                return Err(CapError::ValidationError(format!(
                    "unsupported action: {other}"
                )));
            }
        };
        controller_result_to_cap_result(result)
    }
}
