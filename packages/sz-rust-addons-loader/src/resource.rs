//! 插件 RESTful 资源路由注入（Phase 10.3.5）
//!
//! 借鉴 Rails 资源路由 + ThinkPHP 6 多应用机制。
//!
//! ## ThinkPHP 6 资源路由对齐
//!
//! ThinkPHP 6 `Route::resource($rule, $route)` 生成 7 个 RESTful 路由
//! （源码：`vendor/topthink/framework/src/think/Route.php` 第 39-47 行 `$rest` 默认数组）：
//!
//! | 动作 | HTTP 方法 | URL 后缀 | 控制器方法 |
//! |------|-----------|----------|-----------|
//! | index | GET | `''` | index |
//! | create | GET | `/create` | create |
//! | edit | GET | `/<id>/edit` | edit |
//! | read | GET | `/<id>` | read |
//! | save | POST | `''` | save |
//! | update | PUT | `/<id>` | update |
//! | delete | DELETE | `/<id>` | delete |
//!
//! ## addon 路由前缀注入（Phase 10.3.5 核心）
//!
//! 插件内 `Route::resource` 自动注入 addon 路由前缀：
//!
//! - 普通资源：`/blog` → `/addons/<addon>/<controller>`
//! - 嵌套资源：`blog.comment` → `/addons/<addon>/blog/<blog_id>/comment`
//!
//! ## PHP 项目使用情况
//!
//! 调研结论：PHP 项目业务代码 0 处使用 `Route::resource`，addons 0 处使用，
//! 项目使用控制器自动路由机制（URL 直接映射 `模块/控制器/方法`）。
//! 本模块为 Rust 侧扩展能力，对齐 ThinkPHP 6 资源路由机制作为框架能力。

use std::fmt;

use crate::error::{AddonLoaderError, AddonLoaderResult};

/// RESTful 动作枚举（对齐 ThinkPHP 6 `$rest` 默认数组的 7 个键）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceAction {
    /// 列表页（GET，URL 后缀 `''`）
    Index,
    /// 新建表单页（GET，URL 后缀 `/create`）
    Create,
    /// 编辑表单页（GET，URL 后缀 `/<id>/edit`）
    Edit,
    /// 单个资源详情（GET，URL 后缀 `/<id>`）
    Read,
    /// 新建保存（POST，URL 后缀 `''`）
    Save,
    /// 更新（PUT，URL 后缀 `/<id>`）
    Update,
    /// 删除（DELETE，URL 后缀 `/<id>`）
    Delete,
}

impl ResourceAction {
    /// 返回动作名称（对齐 ThinkPHP 6 `$rest` 数组的键名）
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceAction::Index => "index",
            ResourceAction::Create => "create",
            ResourceAction::Edit => "edit",
            ResourceAction::Read => "read",
            ResourceAction::Save => "save",
            ResourceAction::Update => "update",
            ResourceAction::Delete => "delete",
        }
    }

    /// 返回对应的 HTTP 方法
    pub fn http_method(&self) -> HttpMethod {
        match self {
            ResourceAction::Index
            | ResourceAction::Create
            | ResourceAction::Edit
            | ResourceAction::Read => HttpMethod::Get,
            ResourceAction::Save => HttpMethod::Post,
            ResourceAction::Update => HttpMethod::Put,
            ResourceAction::Delete => HttpMethod::Delete,
        }
    }

    /// 返回 URL 后缀（对齐 ThinkPHP 6 `$rest` 数组的第二元素）
    pub fn url_suffix(&self) -> &'static str {
        match self {
            ResourceAction::Index | ResourceAction::Save => "",
            ResourceAction::Create => "/create",
            ResourceAction::Edit => "/<id>/edit",
            ResourceAction::Read | ResourceAction::Update | ResourceAction::Delete => "/<id>",
        }
    }

    /// 返回控制器方法名（对齐 ThinkPHP 6 `$rest` 数组的第三元素）
    pub fn controller_method(&self) -> &'static str {
        self.as_str()
    }

    /// 返回全部 7 个动作（按 ThinkPHP 6 `$rest` 数组顺序）
    pub fn all() -> &'static [ResourceAction] {
        &[
            ResourceAction::Index,
            ResourceAction::Create,
            ResourceAction::Edit,
            ResourceAction::Read,
            ResourceAction::Save,
            ResourceAction::Update,
            ResourceAction::Delete,
        ]
    }

    /// 从字符串解析动作名（便捷方法，返回 Option）
    ///
    /// 注：对齐 `std::str::FromStr` trait 的语义，但保留 Option 返回类型以便测试使用。
    pub fn parse_name(name: &str) -> Option<ResourceAction> {
        match name {
            "index" => Some(ResourceAction::Index),
            "create" => Some(ResourceAction::Create),
            "edit" => Some(ResourceAction::Edit),
            "read" => Some(ResourceAction::Read),
            "save" => Some(ResourceAction::Save),
            "update" => Some(ResourceAction::Update),
            "delete" => Some(ResourceAction::Delete),
            _ => None,
        }
    }
}

/// 实现 `std::str::FromStr` trait（对齐 Rust 标准库约定）
impl std::str::FromStr for ResourceAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_name(s).ok_or_else(|| format!("unknown ResourceAction: {}", s))
    }
}

/// HTTP 方法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl HttpMethod {
    /// 返回 HTTP 方法名（大写）
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 资源路由条目（单个生成的路由）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRouteEntry {
    /// 动作名
    pub action: ResourceAction,
    /// HTTP 方法
    pub method: HttpMethod,
    /// 完整 URL（含 addon 前缀）
    pub url: String,
    /// 控制器方法名
    pub controller_method: String,
}

/// 资源路由选项（only/except 过滤，对齐 ThinkPHP 6 `Resource::only()` / `except()`）
#[derive(Debug, Clone, Default)]
pub struct ResourceOptions {
    /// 仅生成指定的动作（对齐 `->only([...])`）
    pub only: Vec<ResourceAction>,
    /// 排除指定的动作（对齐 `->except([...])`）
    pub except: Vec<ResourceAction>,
}

impl ResourceOptions {
    /// 创建空选项（生成全部 7 条路由）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 only 过滤
    pub fn with_only(mut self, only: Vec<ResourceAction>) -> Self {
        self.only = only;
        self
    }

    /// 设置 except 过滤
    pub fn with_except(mut self, except: Vec<ResourceAction>) -> Self {
        self.except = except;
        self
    }

    /// 判断指定动作是否应该生成
    ///
    /// 对齐 ThinkPHP 6 `Resource.php` 第 106-109 行的过滤逻辑：
    /// `if (isset($option['only']) && !in_array($key, $option['only'])) continue;`
    /// `if (isset($option['except']) && in_array($key, $option['except'])) continue;`
    pub fn should_include(&self, action: ResourceAction) -> bool {
        if !self.only.is_empty() && !self.only.contains(&action) {
            return false;
        }
        if self.except.contains(&action) {
            return false;
        }
        true
    }
}

/// 生成的资源路由集合（Phase 10.3.5 主交付物）
#[derive(Debug, Clone)]
pub struct ResourceRoute {
    /// 插件名
    pub addon: String,
    /// 控制器名（原始形式，可能含点号表示嵌套）
    pub controller: String,
    /// 基础 URL（含 addon 前缀，如 `/addons/operate/blog`）
    pub base_url: String,
    /// 生成的路由条目列表
    pub entries: Vec<ResourceRouteEntry>,
}

impl ResourceRoute {
    /// 获取指定动作的路由条目
    pub fn get(&self, action: ResourceAction) -> Option<&ResourceRouteEntry> {
        self.entries.iter().find(|e| e.action == action)
    }

    /// 返回生成的路由数量
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 构建资源路由（Phase 10.3.5 主入口）
///
/// 借鉴 Rails 资源路由 + ThinkPHP 6 多应用机制，
/// 自动注入 addon 路由前缀 `/addons/<addon>/<controller>`。
///
/// ## 参数
///
/// - `addon`：插件名（如 `"operate"`）
/// - `controller`：控制器名（如 `"Blog"`），支持点号嵌套（如 `"blog.comment"`）
/// - `options`：过滤选项（only/except）
///
/// ## 嵌套资源（对齐 ThinkPHP 6 `Resource.php` 第 89-100 行）
///
/// - `blog.comment` → 基础 URL `/addons/<addon>/blog/<blog_id>/comment`
/// - 父级资源 ID 参数名默认为 `{name}_id`（对齐 PHP `$option['var'][$val] ?? $val . '_id'`）
///
/// ## 生成规则（对齐 ThinkPHP 6 `$rest` 默认数组）
///
/// 假设调用 `build_resource_routes("operate", "Blog", ResourceOptions::new())`：
///
/// | 动作 | HTTP 方法 | URL | 控制器方法 |
/// |------|-----------|-----|-----------|
/// | index | GET | `/addons/operate/Blog` | index |
/// | create | GET | `/addons/operate/Blog/create` | create |
/// | edit | GET | `/addons/operate/Blog/<id>/edit` | edit |
/// | read | GET | `/addons/operate/Blog/<id>` | read |
/// | save | POST | `/addons/operate/Blog` | save |
/// | update | PUT | `/addons/operate/Blog/<id>` | update |
/// | delete | DELETE | `/addons/operate/Blog/<id>` | delete |
///
/// ## 错误
///
/// - `RouteParse`：addon 或 controller 为空
pub fn build_resource_routes(
    addon: &str,
    controller: &str,
    options: &ResourceOptions,
) -> AddonLoaderResult<ResourceRoute> {
    if addon.is_empty() {
        return Err(AddonLoaderError::RouteParse {
            url: String::new(),
            reason: "addon cannot be empty for resource route".to_string(),
        });
    }
    if controller.is_empty() {
        return Err(AddonLoaderError::RouteParse {
            url: String::new(),
            reason: "controller cannot be empty for resource route".to_string(),
        });
    }

    // 构建基础 URL（自动注入 addon 前缀 + 处理嵌套资源）
    let base_url = build_resource_base_url(addon, controller);

    // 生成 7 条路由（对齐 ThinkPHP 6 `$rest` 默认数组）
    let mut entries = Vec::new();
    for &action in ResourceAction::all() {
        if !options.should_include(action) {
            continue;
        }
        let url = format!("{}{}", base_url, action.url_suffix());
        entries.push(ResourceRouteEntry {
            action,
            method: action.http_method(),
            url,
            controller_method: action.controller_method().to_string(),
        });
    }

    Ok(ResourceRoute {
        addon: addon.to_string(),
        controller: controller.to_string(),
        base_url,
        entries,
    })
}

/// 构建资源路由基础 URL（对齐 ThinkPHP 6 嵌套资源处理）
///
/// ## 嵌套资源处理（对齐 `Resource.php` 第 89-100 行）
///
/// ```php
/// if (strpos($rule, '.')) {
///     $array = explode('.', $rule);
///     $last  = array_pop($array);
///     $item  = [];
///     foreach ($array as $val) {
///         $item[] = $val . '/<' . ($option['var'][$val] ?? $val . '_id') . '>';
///     }
///     $rule = implode('/', $item) . '/' . $last;
/// }
/// ```
///
/// ## 示例
///
/// - `build_resource_base_url("operate", "Blog")` → `/addons/operate/Blog`
/// - `build_resource_base_url("operate", "blog.comment")` → `/addons/operate/blog/<blog_id>/comment`
/// - `build_resource_base_url("operate", "blog.post.comment")` → `/addons/operate/blog/<blog_id>/post/<post_id>/comment`
fn build_resource_base_url(addon: &str, controller: &str) -> String {
    if !controller.contains('.') {
        return format!("/addons/{}/{}", addon, controller);
    }

    // 嵌套资源：按点号拆分，末段为资源名，前段为父级资源
    let mut parts: Vec<&str> = controller.split('.').collect();
    let last = parts.pop().unwrap_or(controller);

    // 对齐 PHP：父级资源段格式为 `{val}/<{val}_id>`
    let mut segments: Vec<String> = Vec::new();
    for val in &parts {
        segments.push(format!("{}/<{}_id>", val, val));
    }
    segments.push(last.to_string());

    format!("/addons/{}/{}", addon, segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== ResourceAction 测试 ====================

    #[test]
    fn test_resource_action_as_str() {
        assert_eq!(ResourceAction::Index.as_str(), "index");
        assert_eq!(ResourceAction::Create.as_str(), "create");
        assert_eq!(ResourceAction::Edit.as_str(), "edit");
        assert_eq!(ResourceAction::Read.as_str(), "read");
        assert_eq!(ResourceAction::Save.as_str(), "save");
        assert_eq!(ResourceAction::Update.as_str(), "update");
        assert_eq!(ResourceAction::Delete.as_str(), "delete");
    }

    #[test]
    fn test_resource_action_http_method() {
        assert_eq!(ResourceAction::Index.http_method(), HttpMethod::Get);
        assert_eq!(ResourceAction::Create.http_method(), HttpMethod::Get);
        assert_eq!(ResourceAction::Edit.http_method(), HttpMethod::Get);
        assert_eq!(ResourceAction::Read.http_method(), HttpMethod::Get);
        assert_eq!(ResourceAction::Save.http_method(), HttpMethod::Post);
        assert_eq!(ResourceAction::Update.http_method(), HttpMethod::Put);
        assert_eq!(ResourceAction::Delete.http_method(), HttpMethod::Delete);
    }

    #[test]
    fn test_resource_action_url_suffix() {
        assert_eq!(ResourceAction::Index.url_suffix(), "");
        assert_eq!(ResourceAction::Create.url_suffix(), "/create");
        assert_eq!(ResourceAction::Edit.url_suffix(), "/<id>/edit");
        assert_eq!(ResourceAction::Read.url_suffix(), "/<id>");
        assert_eq!(ResourceAction::Save.url_suffix(), "");
        assert_eq!(ResourceAction::Update.url_suffix(), "/<id>");
        assert_eq!(ResourceAction::Delete.url_suffix(), "/<id>");
    }

    #[test]
    fn test_resource_action_controller_method() {
        assert_eq!(ResourceAction::Index.controller_method(), "index");
        assert_eq!(ResourceAction::Save.controller_method(), "save");
        assert_eq!(ResourceAction::Delete.controller_method(), "delete");
    }

    #[test]
    fn test_resource_action_all_returns_seven() {
        let all = ResourceAction::all();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], ResourceAction::Index);
        assert_eq!(all[1], ResourceAction::Create);
        assert_eq!(all[2], ResourceAction::Edit);
        assert_eq!(all[3], ResourceAction::Read);
        assert_eq!(all[4], ResourceAction::Save);
        assert_eq!(all[5], ResourceAction::Update);
        assert_eq!(all[6], ResourceAction::Delete);
    }

    #[test]
    fn test_resource_action_parse_name_valid() {
        assert_eq!(
            ResourceAction::parse_name("index"),
            Some(ResourceAction::Index)
        );
        assert_eq!(
            ResourceAction::parse_name("create"),
            Some(ResourceAction::Create)
        );
        assert_eq!(
            ResourceAction::parse_name("edit"),
            Some(ResourceAction::Edit)
        );
        assert_eq!(
            ResourceAction::parse_name("read"),
            Some(ResourceAction::Read)
        );
        assert_eq!(
            ResourceAction::parse_name("save"),
            Some(ResourceAction::Save)
        );
        assert_eq!(
            ResourceAction::parse_name("update"),
            Some(ResourceAction::Update)
        );
        assert_eq!(
            ResourceAction::parse_name("delete"),
            Some(ResourceAction::Delete)
        );
    }

    #[test]
    fn test_resource_action_parse_name_invalid() {
        assert_eq!(ResourceAction::parse_name("unknown"), None);
        assert_eq!(ResourceAction::parse_name(""), None);
        assert_eq!(ResourceAction::parse_name("INDEX"), None);
    }

    #[test]
    fn test_resource_action_from_str_trait() {
        // 验证 std::str::FromStr trait 实现
        use std::str::FromStr;
        assert_eq!(ResourceAction::from_str("index"), Ok(ResourceAction::Index));
        assert_eq!(
            ResourceAction::from_str("delete"),
            Ok(ResourceAction::Delete)
        );
        assert!(ResourceAction::from_str("unknown").is_err());
        assert!(ResourceAction::from_str("").is_err());
    }

    // ==================== HttpMethod 测试 ====================

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
    }

    #[test]
    fn test_http_method_display() {
        assert_eq!(format!("{}", HttpMethod::Get), "GET");
        assert_eq!(format!("{}", HttpMethod::Post), "POST");
        assert_eq!(format!("{}", HttpMethod::Put), "PUT");
        assert_eq!(format!("{}", HttpMethod::Delete), "DELETE");
        assert_eq!(format!("{}", HttpMethod::Patch), "PATCH");
    }

    // ==================== ResourceOptions 测试 ====================

    #[test]
    fn test_resource_options_new() {
        let opts = ResourceOptions::new();
        assert!(opts.only.is_empty());
        assert!(opts.except.is_empty());
    }

    #[test]
    fn test_resource_options_with_only() {
        let opts =
            ResourceOptions::new().with_only(vec![ResourceAction::Index, ResourceAction::Read]);
        assert_eq!(opts.only.len(), 2);
        assert!(opts.except.is_empty());
    }

    #[test]
    fn test_resource_options_with_except() {
        let opts = ResourceOptions::new().with_except(vec![ResourceAction::Delete]);
        assert!(opts.only.is_empty());
        assert_eq!(opts.except.len(), 1);
    }

    #[test]
    fn test_resource_options_with_only_and_except() {
        let opts = ResourceOptions::new()
            .with_only(vec![
                ResourceAction::Index,
                ResourceAction::Read,
                ResourceAction::Delete,
            ])
            .with_except(vec![ResourceAction::Delete]);
        assert_eq!(opts.only.len(), 3);
        assert_eq!(opts.except.len(), 1);
    }

    #[test]
    fn test_should_include_no_filter() {
        let opts = ResourceOptions::new();
        for &action in ResourceAction::all() {
            assert!(
                opts.should_include(action),
                "{:?} should be included",
                action
            );
        }
    }

    #[test]
    fn test_should_include_only_filter() {
        let opts =
            ResourceOptions::new().with_only(vec![ResourceAction::Index, ResourceAction::Read]);
        assert!(opts.should_include(ResourceAction::Index));
        assert!(opts.should_include(ResourceAction::Read));
        assert!(!opts.should_include(ResourceAction::Create));
        assert!(!opts.should_include(ResourceAction::Edit));
        assert!(!opts.should_include(ResourceAction::Save));
        assert!(!opts.should_include(ResourceAction::Update));
        assert!(!opts.should_include(ResourceAction::Delete));
    }

    #[test]
    fn test_should_include_except_filter() {
        let opts = ResourceOptions::new()
            .with_except(vec![ResourceAction::Delete, ResourceAction::Create]);
        assert!(opts.should_include(ResourceAction::Index));
        assert!(!opts.should_include(ResourceAction::Create));
        assert!(opts.should_include(ResourceAction::Edit));
        assert!(opts.should_include(ResourceAction::Read));
        assert!(opts.should_include(ResourceAction::Save));
        assert!(opts.should_include(ResourceAction::Update));
        assert!(!opts.should_include(ResourceAction::Delete));
    }

    #[test]
    fn test_should_include_only_and_except_combined() {
        // only 包含 Delete，但 except 也排除 Delete → Delete 被排除
        let opts = ResourceOptions::new()
            .with_only(vec![
                ResourceAction::Index,
                ResourceAction::Read,
                ResourceAction::Delete,
            ])
            .with_except(vec![ResourceAction::Delete]);
        assert!(opts.should_include(ResourceAction::Index));
        assert!(opts.should_include(ResourceAction::Read));
        assert!(!opts.should_include(ResourceAction::Delete));
        assert!(!opts.should_include(ResourceAction::Create));
    }

    #[test]
    fn test_should_include_empty_only_includes_all() {
        // only 为空时不做 only 过滤（对齐 PHP `isset($option['only'])` 判断）
        let opts = ResourceOptions::new().with_only(vec![]);
        for &action in ResourceAction::all() {
            assert!(opts.should_include(action));
        }
    }

    // ==================== build_resource_base_url 测试 ====================

    #[test]
    fn test_build_resource_base_url_simple() {
        let url = build_resource_base_url("operate", "Blog");
        assert_eq!(url, "/addons/operate/Blog");
    }

    #[test]
    fn test_build_resource_base_url_nested_two_levels() {
        let url = build_resource_base_url("operate", "blog.comment");
        assert_eq!(url, "/addons/operate/blog/<blog_id>/comment");
    }

    #[test]
    fn test_build_resource_base_url_nested_three_levels() {
        let url = build_resource_base_url("operate", "blog.post.comment");
        assert_eq!(url, "/addons/operate/blog/<blog_id>/post/<post_id>/comment");
    }

    #[test]
    fn test_build_resource_base_url_nested_four_levels() {
        let url = build_resource_base_url("operate", "a.b.c.d");
        assert_eq!(url, "/addons/operate/a/<a_id>/b/<b_id>/c/<c_id>/d");
    }

    // ==================== build_resource_routes 测试 ====================

    #[test]
    fn test_build_resource_routes_default_seven() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        assert_eq!(route.addon, "operate");
        assert_eq!(route.controller, "Blog");
        assert_eq!(route.base_url, "/addons/operate/Blog");
        assert_eq!(route.len(), 7);
        assert!(!route.is_empty());
    }

    #[test]
    fn test_build_resource_routes_index_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Index).unwrap();
        assert_eq!(entry.action, ResourceAction::Index);
        assert_eq!(entry.method, HttpMethod::Get);
        assert_eq!(entry.url, "/addons/operate/Blog");
        assert_eq!(entry.controller_method, "index");
    }

    #[test]
    fn test_build_resource_routes_create_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Create).unwrap();
        assert_eq!(entry.action, ResourceAction::Create);
        assert_eq!(entry.method, HttpMethod::Get);
        assert_eq!(entry.url, "/addons/operate/Blog/create");
        assert_eq!(entry.controller_method, "create");
    }

    #[test]
    fn test_build_resource_routes_edit_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Edit).unwrap();
        assert_eq!(entry.action, ResourceAction::Edit);
        assert_eq!(entry.method, HttpMethod::Get);
        assert_eq!(entry.url, "/addons/operate/Blog/<id>/edit");
        assert_eq!(entry.controller_method, "edit");
    }

    #[test]
    fn test_build_resource_routes_read_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Read).unwrap();
        assert_eq!(entry.action, ResourceAction::Read);
        assert_eq!(entry.method, HttpMethod::Get);
        assert_eq!(entry.url, "/addons/operate/Blog/<id>");
        assert_eq!(entry.controller_method, "read");
    }

    #[test]
    fn test_build_resource_routes_save_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Save).unwrap();
        assert_eq!(entry.action, ResourceAction::Save);
        assert_eq!(entry.method, HttpMethod::Post);
        assert_eq!(entry.url, "/addons/operate/Blog");
        assert_eq!(entry.controller_method, "save");
    }

    #[test]
    fn test_build_resource_routes_update_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Update).unwrap();
        assert_eq!(entry.action, ResourceAction::Update);
        assert_eq!(entry.method, HttpMethod::Put);
        assert_eq!(entry.url, "/addons/operate/Blog/<id>");
        assert_eq!(entry.controller_method, "update");
    }

    #[test]
    fn test_build_resource_routes_delete_entry() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Delete).unwrap();
        assert_eq!(entry.action, ResourceAction::Delete);
        assert_eq!(entry.method, HttpMethod::Delete);
        assert_eq!(entry.url, "/addons/operate/Blog/<id>");
        assert_eq!(entry.controller_method, "delete");
    }

    #[test]
    fn test_build_resource_routes_all_urls() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let urls: Vec<&str> = route.entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "/addons/operate/Blog",
                "/addons/operate/Blog/create",
                "/addons/operate/Blog/<id>/edit",
                "/addons/operate/Blog/<id>",
                "/addons/operate/Blog",
                "/addons/operate/Blog/<id>",
                "/addons/operate/Blog/<id>",
            ]
        );
    }

    #[test]
    fn test_build_resource_routes_all_methods() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let methods: Vec<HttpMethod> = route.entries.iter().map(|e| e.method).collect();
        assert_eq!(
            methods,
            vec![
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Put,
                HttpMethod::Delete,
            ]
        );
    }

    #[test]
    fn test_build_resource_routes_only_index_read() {
        let opts =
            ResourceOptions::new().with_only(vec![ResourceAction::Index, ResourceAction::Read]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 2);
        assert!(route.get(ResourceAction::Index).is_some());
        assert!(route.get(ResourceAction::Read).is_some());
        assert!(route.get(ResourceAction::Create).is_none());
        assert!(route.get(ResourceAction::Edit).is_none());
        assert!(route.get(ResourceAction::Save).is_none());
        assert!(route.get(ResourceAction::Update).is_none());
        assert!(route.get(ResourceAction::Delete).is_none());
    }

    #[test]
    fn test_build_resource_routes_except_delete() {
        let opts = ResourceOptions::new().with_except(vec![ResourceAction::Delete]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 6);
        assert!(route.get(ResourceAction::Delete).is_none());
        assert!(route.get(ResourceAction::Index).is_some());
    }

    #[test]
    fn test_build_resource_routes_only_create_save() {
        let opts =
            ResourceOptions::new().with_only(vec![ResourceAction::Create, ResourceAction::Save]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 2);
        // 顺序按 only 指定？不对，按 $rest 默认数组顺序
        assert_eq!(route.entries[0].action, ResourceAction::Create);
        assert_eq!(route.entries[1].action, ResourceAction::Save);
    }

    #[test]
    fn test_build_resource_routes_except_all() {
        let opts = ResourceOptions::new().with_except(ResourceAction::all().to_vec());
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 0);
        assert!(route.is_empty());
    }

    #[test]
    fn test_build_resource_routes_only_empty() {
        let opts = ResourceOptions::new().with_only(vec![]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 7);
    }

    #[test]
    fn test_build_resource_routes_nested_resource() {
        let route =
            build_resource_routes("operate", "blog.comment", &ResourceOptions::new()).unwrap();
        assert_eq!(route.base_url, "/addons/operate/blog/<blog_id>/comment");
        assert_eq!(route.len(), 7);

        let index = route.get(ResourceAction::Index).unwrap();
        assert_eq!(index.url, "/addons/operate/blog/<blog_id>/comment");

        let read = route.get(ResourceAction::Read).unwrap();
        assert_eq!(read.url, "/addons/operate/blog/<blog_id>/comment/<id>");

        let create = route.get(ResourceAction::Create).unwrap();
        assert_eq!(create.url, "/addons/operate/blog/<blog_id>/comment/create");

        let edit = route.get(ResourceAction::Edit).unwrap();
        assert_eq!(edit.url, "/addons/operate/blog/<blog_id>/comment/<id>/edit");
    }

    #[test]
    fn test_build_resource_routes_nested_three_levels() {
        let route =
            build_resource_routes("operate", "blog.post.comment", &ResourceOptions::new()).unwrap();
        assert_eq!(
            route.base_url,
            "/addons/operate/blog/<blog_id>/post/<post_id>/comment"
        );

        let read = route.get(ResourceAction::Read).unwrap();
        assert_eq!(
            read.url,
            "/addons/operate/blog/<blog_id>/post/<post_id>/comment/<id>"
        );
    }

    #[test]
    fn test_build_resource_routes_different_addon() {
        let route = build_resource_routes("cashier", "Order", &ResourceOptions::new()).unwrap();
        assert_eq!(route.base_url, "/addons/cashier/Order");
        assert_eq!(route.addon, "cashier");
        assert_eq!(route.controller, "Order");
    }

    #[test]
    fn test_build_resource_routes_empty_addon() {
        let result = build_resource_routes("", "Blog", &ResourceOptions::new());
        assert!(result.is_err());
        match result {
            Err(AddonLoaderError::RouteParse { reason, .. }) => {
                assert!(reason.contains("addon cannot be empty"));
            }
            _ => panic!("expected RouteParse error"),
        }
    }

    #[test]
    fn test_build_resource_routes_empty_controller() {
        let result = build_resource_routes("operate", "", &ResourceOptions::new());
        assert!(result.is_err());
        match result {
            Err(AddonLoaderError::RouteParse { reason, .. }) => {
                assert!(reason.contains("controller cannot be empty"));
            }
            _ => panic!("expected RouteParse error"),
        }
    }

    #[test]
    fn test_build_resource_routes_get_returns_none_for_excluded() {
        let opts = ResourceOptions::new().with_only(vec![ResourceAction::Index]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert!(route.get(ResourceAction::Index).is_some());
        assert!(route.get(ResourceAction::Read).is_none());
    }

    #[test]
    fn test_build_resource_routes_entries_order() {
        // 验证生成顺序对齐 ThinkPHP 6 $rest 默认数组顺序
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let actions: Vec<ResourceAction> = route.entries.iter().map(|e| e.action).collect();
        assert_eq!(
            actions,
            vec![
                ResourceAction::Index,
                ResourceAction::Create,
                ResourceAction::Edit,
                ResourceAction::Read,
                ResourceAction::Save,
                ResourceAction::Update,
                ResourceAction::Delete,
            ]
        );
    }

    // ==================== ResourceRoute 测试 ====================

    #[test]
    fn test_resource_route_len() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        assert_eq!(route.len(), 7);
    }

    #[test]
    fn test_resource_route_is_empty_false() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        assert!(!route.is_empty());
    }

    #[test]
    fn test_resource_route_is_empty_true() {
        let opts = ResourceOptions::new().with_except(ResourceAction::all().to_vec());
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert!(route.is_empty());
    }

    #[test]
    fn test_resource_route_get_existing() {
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let entry = route.get(ResourceAction::Save).unwrap();
        assert_eq!(entry.method, HttpMethod::Post);
    }

    #[test]
    fn test_resource_route_get_nonexistent() {
        let opts = ResourceOptions::new().with_only(vec![ResourceAction::Index]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert!(route.get(ResourceAction::Save).is_none());
    }

    // ==================== R5 ThinkPHP 6 行为对齐测试 ====================

    #[test]
    fn test_r5_thinkphp_rest_default_mapping() {
        // 对齐 ThinkPHP 6 `$rest` 默认数组（Route.php 第 39-47 行）
        // 验证 7 个动作的 HTTP 方法 + URL 后缀 + 控制器方法名
        let expected: &[(ResourceAction, HttpMethod, &str, &str)] = &[
            (ResourceAction::Index, HttpMethod::Get, "", "index"),
            (ResourceAction::Create, HttpMethod::Get, "/create", "create"),
            (ResourceAction::Edit, HttpMethod::Get, "/<id>/edit", "edit"),
            (ResourceAction::Read, HttpMethod::Get, "/<id>", "read"),
            (ResourceAction::Save, HttpMethod::Post, "", "save"),
            (ResourceAction::Update, HttpMethod::Put, "/<id>", "update"),
            (
                ResourceAction::Delete,
                HttpMethod::Delete,
                "/<id>",
                "delete",
            ),
        ];

        for &(action, method, suffix, ctrl_method) in expected {
            assert_eq!(
                action.http_method(),
                method,
                "{:?} HTTP method mismatch",
                action
            );
            assert_eq!(
                action.url_suffix(),
                suffix,
                "{:?} URL suffix mismatch",
                action
            );
            assert_eq!(
                action.controller_method(),
                ctrl_method,
                "{:?} controller method mismatch",
                action
            );
        }
    }

    #[test]
    fn test_r5_thinkphp_resource_seven_routes() {
        // 对齐 ThinkPHP 6 `Route::resource('blog', 'Blog')` 生成 7 条路由
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        assert_eq!(route.len(), 7, "ThinkPHP 6 resource must generate 7 routes");
    }

    #[test]
    fn test_r5_thinkphp_only_filter() {
        // 对齐 ThinkPHP 6 `Resource::only(['index', 'read'])` 过滤逻辑
        let opts =
            ResourceOptions::new().with_only(vec![ResourceAction::Index, ResourceAction::Read]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 2);
    }

    #[test]
    fn test_r5_thinkphp_except_filter() {
        // 对齐 ThinkPHP 6 `Resource::except(['delete'])` 过滤逻辑
        let opts = ResourceOptions::new().with_except(vec![ResourceAction::Delete]);
        let route = build_resource_routes("operate", "Blog", &opts).unwrap();
        assert_eq!(route.len(), 6);
    }

    #[test]
    fn test_r5_thinkphp_nested_resource() {
        // 对齐 ThinkPHP 6 `Route::resource('blog.comment', 'Comment')` 嵌套资源
        // 父级资源 ID 参数名默认为 `{name}_id`
        let route =
            build_resource_routes("operate", "blog.comment", &ResourceOptions::new()).unwrap();
        assert_eq!(route.base_url, "/addons/operate/blog/<blog_id>/comment");
    }

    #[test]
    fn test_r5_thinkphp_no_add_action() {
        // 对齐 ThinkPHP 6 默认映射中没有 `add` 动作
        // 新建资源使用 `save`（POST 到根 URL）
        assert!(ResourceAction::parse_name("add").is_none());
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        assert!(route.get(ResourceAction::Save).is_some());
        // 确认 save 是 POST 方法
        assert_eq!(
            route.get(ResourceAction::Save).unwrap().method,
            HttpMethod::Post
        );
    }

    #[test]
    fn test_r5_thinkphp_update_uses_put_not_patch() {
        // 对齐 ThinkPHP 6 默认 `$rest` 数组中 update 使用 PUT 方法（不是 PATCH）
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let update = route.get(ResourceAction::Update).unwrap();
        assert_eq!(update.method, HttpMethod::Put);
    }

    #[test]
    fn test_r5_thinkphp_index_and_save_share_url() {
        // 对齐 ThinkPHP 6：index（GET）和 save（POST）共享相同的 URL 后缀 `''`
        // 通过 HTTP 方法区分
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let index = route.get(ResourceAction::Index).unwrap();
        let save = route.get(ResourceAction::Save).unwrap();
        assert_eq!(index.url, save.url);
        assert_ne!(index.method, save.method);
    }

    #[test]
    fn test_r5_thinkphp_read_update_delete_share_url() {
        // 对齐 ThinkPHP 6：read（GET）、update（PUT）、delete（DELETE）共享 URL 后缀 `/<id>`
        // 通过 HTTP 方法区分
        let route = build_resource_routes("operate", "Blog", &ResourceOptions::new()).unwrap();
        let read = route.get(ResourceAction::Read).unwrap();
        let update = route.get(ResourceAction::Update).unwrap();
        let delete = route.get(ResourceAction::Delete).unwrap();
        assert_eq!(read.url, update.url);
        assert_eq!(read.url, delete.url);
        assert_ne!(read.method, update.method);
        assert_ne!(read.method, delete.method);
        assert_ne!(update.method, delete.method);
    }

    #[test]
    fn test_r5_addon_prefix_injection() {
        // Phase 10.3.5 核心：自动注入 addon 路由前缀
        let route = build_resource_routes("cashier", "Order", &ResourceOptions::new()).unwrap();
        assert!(route.base_url.starts_with("/addons/cashier/"));
        for entry in &route.entries {
            assert!(entry.url.starts_with("/addons/cashier/Order"));
        }
    }

    #[test]
    fn test_r5_addon_prefix_injection_nested() {
        // 嵌套资源也注入 addon 前缀
        let route =
            build_resource_routes("operate", "blog.comment", &ResourceOptions::new()).unwrap();
        assert!(route.base_url.starts_with("/addons/operate/"));
        for entry in &route.entries {
            assert!(entry.url.starts_with("/addons/operate/blog/"));
        }
    }
}
