# 插件市场设计方案

> **目标**：建立 sz-rust 插件生态，允许开发者发布和分享插件  
> **核心组件**：`sz-rust-addons-loader`（已实现）+ Plugin trait（待实现）  
> **分发方式**：crates.io + Git 仓库 + 本地目录

---

## 一、架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                    sz-rust 插件市场                          │
├─────────────────────────────────────────────────────────────┤
│  插件分类                                                    │
│  ├── 🔐 认证插件（OAuth2 / SAML / LDAP / 企业微信）           │
│  ├── 💾 存储插件（OSS / S3 / 七牛云 / 又拍云）               │
│  ├── 📧 消息插件（短信 / 邮件 / 推送 / Slack）               │
│  ├── 💰 支付插件（微信支付 / 支付宝 / 银联 / Stripe）         │
│  ├── 📊 监控插件（Prometheus / SkyWalking / Sentry）         │
│  ├── 🏢 业务插件（CRM / ERP / 电商 / CMS）                   │
│  └── 🔧 工具插件（Excel / PDF / 二维码 / 图片处理）          │
├─────────────────────────────────────────────────────────────┤
│  插件格式                                                    │
│  ├── Cargo.toml（Rust crate）                               │
│  ├── plugin.json（插件元数据）                               │
│  ├── src/lib.rs（Plugin trait 实现）                        │
│  └── README.md（使用说明）                                   │
├─────────────────────────────────────────────────────────────┤
│  分发方式                                                    │
│  ├── crates.io（官方插件，版本管理）                         │
│  ├── Git 仓库（社区插件，灵活更新）                          │
│  └── 本地目录（私有插件，企业内部）                          │
└─────────────────────────────────────────────────────────────┘
```

---

## 二、Plugin Trait 设计

### 2.1 核心接口

```rust
// packages/sz-rust-core/src/plugin/mod.rs

use crate::router::RouterBuilder;
use crate::middleware::SzMiddleware;
use crate::container::App;
use std::collections::HashMap;

/// 插件上下文 — 提供插件注册所需的所有能力
pub struct PluginContext {
    /// 路由构建器（插件可注册路由）
    pub router: RouterBuilder,
    /// 中间件注册表
    pub middlewares: Vec<Box<dyn SzMiddleware>>,
    /// 配置注册表
    pub configs: HashMap<String, serde_json::Value>,
    /// 服务绑定
    pub container: &'static App,
}

/// 插件生命周期结果
pub type PluginResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// 插件 trait — 所有插件必须实现此 trait
///
/// # 示例
///
/// ```rust,ignore
/// use sz_rust_core::plugin::{Plugin, PluginContext, PluginResult};
///
/// pub struct Oauth2Plugin;
///
/// #[sz_rust_core::plugin]
/// impl Plugin for Oauth2Plugin {
///     fn name(&self) -> &'static str { "oauth2" }
///     
///     fn version(&self) -> &'static str { "1.0.0" }
///     
///     fn on_init(&self, ctx: &PluginContext) -> PluginResult {
///         // 注册路由
///         ctx.router.get("/auth/oauth2/login", login_handler);
///         ctx.router.get("/auth/oauth2/callback", callback_handler);
///         
///         // 注册中间件
///         ctx.middlewares.push(Box::new(Oauth2Middleware));
///         
///         // 注册配置
///         ctx.configs.insert("oauth2".to_string(), json!({
///             "client_id": "xxx",
///             "client_secret": "xxx"
///         }));
///         
///         Ok(())
///     }
///     
///     fn on_shutdown(&self) -> PluginResult {
///         // 清理资源
///         Ok(())
///     }
/// }
/// ```
pub trait Plugin: Send + Sync {
    /// 插件名称（唯一标识，小写字母+连字符）
    fn name(&self) -> &'static str;
    
    /// 插件版本（SemVer 格式）
    fn version(&self) -> &'static str;
    
    /// 插件描述
    fn description(&self) -> &'static str { "" }
    
    /// 插件作者
    fn author(&self) -> &'static str { "" }
    
    /// 初始化钩子 — 插件启动时调用
    /// 
    /// 在此注册路由、中间件、服务、配置等
    fn on_init(&self, ctx: &PluginContext) -> PluginResult;
    
    /// 关闭钩子 — 插件停止时调用
    /// 
    /// 在此清理资源、关闭连接等
    fn on_shutdown(&self) -> PluginResult {
        Ok(())
    }
    
    /// 插件依赖（其他插件的名称 + 版本约束）
    fn dependencies(&self) -> Vec<(&'static str, &'static str)> {
        vec![]
    }
}
```

### 2.2 过程宏

```rust
// packages/sz-rust-macros/src/plugin.rs

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemImpl};

/// `#[plugin]` 过程宏 — 自动生成插件注册代码
///
/// # 用法
///
/// ```rust,ignore
/// #[sz_rust_core::plugin]
/// impl Plugin for MyPlugin {
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn plugin(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let trait_name = &input.trait_;
    
    // 验证是否实现了 Plugin trait
    // 自动生成插件元数据注册
    
    quote! {
        #input
        
        // 自动生成插件注册函数
        #[allow(dead_code)]
        fn __register_plugin__() {
            // 注册到全局插件注册表
        }
    }.into()
}
```

---

## 三、插件元数据格式

### 3.1 plugin.json

```json
{
  "name": "sz-plugin-oauth2",
  "version": "1.0.0",
  "description": "OAuth2 认证插件 — 支持 GitHub、Google、微信登录",
  "author": "your-name <your@email.com>",
  "license": "MIT",
  "homepage": "https://github.com/your-name/sz-plugin-oauth2",
  "repository": "https://github.com/your-name/sz-plugin-oauth2",
  "keywords": ["oauth2", "sso", "authentication", "github", "google"],
  "sz_rust_version": ">=0.2.0",
  "category": "auth",
  "entry_point": "Oauth2Plugin",
  "config_schema": {
    "type": "object",
    "required": ["client_id", "client_secret", "redirect_uri"],
    "properties": {
      "client_id": {
        "type": "string",
        "description": "OAuth2 客户端 ID"
      },
      "client_secret": {
        "type": "string",
        "description": "OAuth2 客户端密钥",
        "sensitive": true
      },
      "redirect_uri": {
        "type": "string",
        "description": "回调地址"
      },
      "providers": {
        "type": "array",
        "items": {
          "type": "string",
          "enum": ["github", "google", "wechat", "custom"]
        },
        "default": ["github"]
      }
    }
  },
  "dependencies": {
    "sz-plugin-auth": ">=1.0.0"
  },
  "routes": [
    {
      "method": "GET",
      "path": "/auth/oauth2/login",
      "handler": "login_handler",
      "description": "OAuth2 登录入口"
    },
    {
      "method": "GET",
      "path": "/auth/oauth2/callback",
      "handler": "callback_handler",
      "description": "OAuth2 回调处理"
    }
  ],
  "middlewares": [
    {
      "name": "Oauth2Middleware",
      "order": 5,
      "description": "OAuth2 认证中间件"
    }
  ]
}
```

### 3.2 Cargo.toml

```toml
[package]
name = "sz-plugin-oauth2"
version = "1.0.0"
edition = "2021"
description = "OAuth2 认证插件"
license = "MIT"
authors = ["Your Name <your@email.com>"]
keywords = ["sz-rust", "plugin", "oauth2", "auth"]
categories = ["web-programming"]

[lib]
name = "sz_plugin_oauth2"
path = "src/lib.rs"

[dependencies]
sz-rust-core = "0.2.1"
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
```

---

## 四、插件加载器增强

### 4.1 插件发现

```rust
// packages/sz-rust-addons-loader/src/loader.rs

impl AddonLoader {
    /// 从目录发现所有插件
    pub fn discover(&self, addons_dir: &str) -> AddonLoaderResult<Vec<AddonInfo>> {
        let mut addons = Vec::new();
        
        for entry in std::fs::read_dir(addons_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if !path.is_dir() {
                continue;
            }
            
            // 检查 plugin.json
            let manifest_path = path.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }
            
            let manifest: AddonManifest = parse_manifest(&manifest_path)?;
            addons.push(AddonInfo {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                path,
                manifest,
            });
        }
        
        Ok(addons)
    }
    
    /// 加载插件（动态链接库方式）
    #[cfg(feature = "dynamic")]
    pub fn load_dynamic(&self, addon: &AddonInfo) -> AddonLoaderResult<Box<dyn Plugin>> {
        use libloading::Library;
        
        let lib_path = addon.path.join("target/release")
            .join(format!("lib{}.{}", addon.name, std::env::consts::DLL_EXTENSION));
        
        let lib = Library::new(lib_path)?;
        let factory: libloading::Symbol<fn() -> *mut dyn Plugin> = 
            unsafe { lib.get(b"__create_plugin__") }?;
        
        Ok(unsafe { Box::from_raw(factory()) })
    }
}
```

### 4.2 插件注册表

```rust
// packages/sz-rust-addons-loader/src/registry.rs

pub struct AddonRegistry {
    addons: RwLock<HashMap<String, AddonState>>,
}

struct AddonState {
    info: AddonInfo,
    plugin: Option<Box<dyn Plugin>>,
    status: AddonStatus, // Active, Inactive, Error
}

impl AddonRegistry {
    /// 启用插件
    pub fn enable(&self, name: &str) -> PluginResult {
        let mut addons = self.addons.write();
        let state = addons.get_mut(name)
            .ok_or("Plugin not found")?;
        
        // 检查依赖
        if let Some(plugin) = &state.info.manifest.dependencies {
            for (dep_name, _version_constraint) in plugin {
                if !self.is_enabled(dep_name) {
                    return Err(format!("Missing dependency: {}", dep_name).into());
                }
            }
        }
        
        // 初始化插件
        let ctx = PluginContext::new();
        state.plugin.as_ref().unwrap().on_init(&ctx)?;
        state.status = AddonStatus::Active;
        
        Ok(())
    }
    
    /// 禁用插件
    pub fn disable(&self, name: &str) -> PluginResult {
        let mut addons = self.addons.write();
        let state = addons.get_mut(name)
            .ok_or("Plugin not found")?;
        
        state.plugin.as_ref().unwrap().on_shutdown()?;
        state.status = AddonStatus::Inactive;
        
        Ok(())
    }
    
    /// 列出所有插件
    pub fn list(&self) -> Vec<AddonInfo> {
        self.addons.read().values()
            .map(|s| s.info.clone())
            .collect()
    }
}
```

---

## 五、示例插件

### 5.1 OAuth2 插件

```rust
// crates/sz-plugin-oauth2/src/lib.rs

use sz_rust_core::plugin::{Plugin, PluginContext, PluginResult};
use sz_rust_core::controller::SzController;
use axum::{body::Body, http::Request, response::Response};
use serde_json::json;

pub struct Oauth2Plugin;

#[sz_rust_core::plugin]
impl Plugin for Oauth2Plugin {
    fn name(&self) -> &'static str { "oauth2" }
    
    fn version(&self) -> &'static str { "1.0.0" }
    
    fn description(&self) -> &'static str {
        "OAuth2 认证插件 — 支持 GitHub、Google、微信登录"
    }
    
    fn author(&self) -> &'static str {
        "Your Name <your@email.com>"
    }
    
    fn on_init(&self, ctx: &PluginContext) -> PluginResult {
        // 注册路由
        ctx.router
            .get("/auth/oauth2/login", Self::login_handler)
            .get("/auth/oauth2/callback", Self::callback_handler)
            .get("/auth/oauth2/logout", Self::logout_handler);
        
        // 注册配置
        ctx.configs.insert("oauth2".to_string(), json!({
            "providers": ["github", "google", "wechat"]
        }));
        
        Ok(())
    }
}

struct Oauth2Controller;
impl SzController for Oauth2Controller {}

impl Oauth2Plugin {
    async fn login_handler(_state: &AppState, req: Request<Body>) -> Response {
        // OAuth2 登录逻辑
        let ctrl = Oauth2Controller;
        ctrl.render_success("跳转到 OAuth2 提供商", json!({
            "auth_url": "https://github.com/login/oauth/authorize?..."
        }))
    }
    
    async fn callback_handler(state: State<AppState>, req: Request<Body>) -> Response {
        // OAuth2 回调处理
        let code = req.uri().query()
            .and_then(|q| url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string()));
        
        // 交换 access_token
        // 获取用户信息
        // 创建或更新本地用户
        // 签发 JWT
        
        let ctrl = Oauth2Controller;
        ctrl.render_success("登录成功", json!({"token": token}))
    }
    
    async fn logout_handler(_state: &AppState, _req: Request<Body>) -> Response {
        let ctrl = Oauth2Controller;
        ctrl.render_success("已退出登录", json!({}))
    }
}
```

### 5.2 OSS 存储插件

```rust
// crates/sz-plugin-oss/src/lib.rs

pub struct OssPlugin;

#[sz_rust_core::plugin]
impl Plugin for OssPlugin {
    fn name(&self) -> &'static str { "oss" }
    
    fn version(&self) -> &'static str { "1.0.0" }
    
    fn description(&self) -> &'static str {
        "对象存储插件 — 支持阿里云 OSS、腾讯云 COS、七牛云 Kodo"
    }
    
    fn on_init(&self, ctx: &PluginContext) -> PluginResult {
        // 注册文件上传路由
        ctx.router
            .post("/api/upload/oss", upload_handler)
            .get("/api/upload/policy", policy_handler);
        
        // 注册存储服务
        ctx.container.singleton::<dyn StorageService, OssStorageService>();
        
        Ok(())
    }
}
```

---

## 六、插件市场架构

### 6.1 市场网站

```
┌─────────────────────────────────────────────────────────────┐
│                  plugins.sz-rust.dev                        │
├─────────────────────────────────────────────────────────────┤
│  🔍 搜索插件...                              分类 ▼  排序 ▼  │
├─────────────────────────────────────────────────────────────┤
│  精选插件                                                     │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐           │
│  │ 🔐 OAuth2   │ │ 💰 微信支付  │ │ 📊 Prometheus│           │
│  │ ⭐ 4.9      │ │ ⭐ 4.8      │ │ ⭐ 4.7      │           │
│  │ 10k+ 下载   │ │ 5k+ 下载    │ │ 3k+ 下载    │           │
│  └─────────────┘ └─────────────┘ └─────────────┘           │
├─────────────────────────────────────────────────────────────┤
│  最新插件                                                     │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐           │
│  │ 📧 邮件服务  │ │ 🏢 CRM     │ │ 🔧 Excel   │           │
│  │ v1.0.0      │ │ v2.1.0      │ │ v1.2.0      │           │
│  │ 今天发布    │ │ 昨天发布    │ │ 3天前发布   │           │
│  └─────────────┘ └─────────────┘ └─────────────┘           │
├─────────────────────────────────────────────────────────────┤
│  分类                                                        │
│  🔐 认证(12)  💾 存储(8)  📧 消息(15)  💰 支付(6)           │
│  📊 监控(10)  🏢 业务(20)  🔧 工具(25)                      │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 插件提交流程

```
1. 开发者创建插件
   └─> sz make:plugin my-plugin --category auth
   
2. 本地开发测试
   └─> cargo test && sz plugin:lint
   
3. 发布到 crates.io
   └─> cargo publish
   └─> sz plugin:publish --manifest plugin.json
   
4. 市场审核（自动）
   └─> 安全扫描 + 兼容性检查 + 文档检查
   
5. 上架展示
   └─> plugins.sz-rust.dev/plugins/my-plugin
```

---

## 七、实施计划

| 阶段 | 任务 | 工时 | 交付物 |
|------|------|------|--------|
| Phase 1 | Plugin trait + 过程宏 | 8h | `sz_rust_core::plugin` 模块 |
| Phase 2 | 插件加载器增强 | 8h | `AddonLoader::load_dynamic` |
| Phase 3 | 示例插件（OAuth2 + OSS） | 8h | 2 个可运行插件 |
| Phase 4 | CLI 插件命令 | 4h | `sz make:plugin` / `sz plugin:*` |
| Phase 5 | 市场网站（可选） | 16h | plugins.sz-rust.dev |

---

## 八、CLI 插件命令

```bash
# 创建插件模板
sz make:plugin my-plugin --category auth --author "Your Name"

# 本地测试插件
sz plugin:test my-plugin

# 插件 lint 检查
sz plugin:lint my-plugin

# 发布插件
sz plugin:publish my-plugin --crates-io

# 安装插件
sz plugin:install oauth2 --version 1.0.0

# 启用/禁用插件
sz plugin:enable oauth2
sz plugin:disable oauth2

# 列出已安装插件
sz plugin:list
```
