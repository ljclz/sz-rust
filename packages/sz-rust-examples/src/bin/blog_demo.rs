// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 博客示例 — 文章 CRUD + 缓存 + 事件驱动（4.3 竞争力深化：完整示例项目）
//!
//! 演示 sz-rust 框架的多 facade 协作：
//! - controller facade：SzController / BaseController 控制器抽象
//! - cache facade：文章列表缓存（读多写少场景）
//! - state facade：发帖事件（EventDispatcher）驱动统计更新
//! - http facade：ApiResponse 统一响应
//!
//! ## 端点
//!
//! | 方法 | 路径 | 说明 |
//! |------|------|------|
//! | GET  | /post/list | 文章列表（走缓存） |
//! | GET  | /post/detail/{id} | 文章详情 |
//! | POST | /post/create | 发帖（body: {"title","content","author"}）|
//! | POST | /post/delete/{id} | 删帖（触发 PostDeleted 事件）|
//! | GET  | /post/stats | 统计（发帖总数 / 缓存命中计数）|
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin blog_demo
//! ```
//!
//! 使用内存存储 + 内存缓存，无需数据库。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sz_rust_core::controller::{BaseController, SzController};
use sz_rust_core::event::ClosureListener;
use sz_rust_core::{cache::Cache, cache::MemoryCacheDriver};

// ============================================================================
// 模型层
// ============================================================================

/// 文章模型
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Post {
    id: i64,
    title: String,
    content: String,
    author: String,
}

// ============================================================================
// 共享状态 — 文章存储 + 缓存 + 事件总线 + 统计
// ============================================================================

struct AppState {
    posts: std::sync::Mutex<Vec<Post>>,
    next_id: AtomicI64,
    cache: Cache,
    cache_hits: AtomicI64,
    /// 发帖计数（由事件监听器维护）
    post_count: Arc<AtomicI64>,
}

impl AppState {
    fn new() -> Self {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let state = Self {
            posts: std::sync::Mutex::new(Vec::new()),
            next_id: AtomicI64::new(1),
            cache,
            cache_hits: AtomicI64::new(0),
            post_count: Arc::new(AtomicI64::new(0)),
        };

        // 事件总线：监听 PostCreated 事件 → 更新统计
        let counter = state.post_count.clone();
        sz_rust_core::event::facade::dispatcher().listen(
            "PostCreated",
            Arc::new(ClosureListener::new(move |params: &Value| {
                let id = params["id"].as_i64().unwrap_or(0);
                counter.fetch_add(1, Ordering::SeqCst);
                tracing::info!("事件驱动：文章 {id} 已发布");
                Ok(Value::Null)
            })),
            false,
        );
        state
    }

    fn list(&self) -> Vec<Post> {
        self.posts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn find(&self, id: i64) -> Option<Post> {
        self.posts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    fn create(&self, title: &str, content: &str, author: &str) -> Post {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let post = Post {
            id,
            title: title.to_string(),
            content: content.to_string(),
            author: author.to_string(),
        };
        self.posts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(post.clone());

        // 缓存失效：新帖发布后列表缓存必须重建（读多写少的缓存一致性）
        self.cache.delete("posts:list").ok();

        // 发布事件（state-facade 事件总线）
        let _ = sz_rust_core::event::facade::dispatcher().trigger(
            "PostCreated",
            &json!({"id": id, "title": title}),
            false,
        );
        post
    }

    fn delete(&self, id: i64) -> bool {
        let mut posts = self.posts.lock().unwrap_or_else(|e| e.into_inner());
        let before = posts.len();
        posts.retain(|p| p.id != id);
        let removed = posts.len() != before;
        if removed {
            self.cache.delete("posts:list").ok();
        }
        removed
    }
}

// ============================================================================
// 控制器层
// ============================================================================

/// 文章控制器
#[allow(dead_code)] // 示例：展示控制器 trait 继承链（路由由 axum handler 直接注册）
struct PostController;

impl SzController for PostController {}
impl BaseController for PostController {}

// ============================================================================
// 处理器
// ============================================================================

async fn list_posts(State(state): State<Arc<AppState>>) -> axum::response::Response {
    // 缓存优先：命中直接返回（缓存降级策略）
    if let Ok(Some(cached)) = state.cache.get::<Vec<Post>>("posts:list") {
        state.cache_hits.fetch_add(1, Ordering::SeqCst);
        return sz_rust_core::response::render_success(json!(cached), "缓存命中");
    }
    let posts = state.list();
    state
        .cache
        .set(
            "posts:list",
            posts.clone(),
            Some(std::time::Duration::from_secs(60)),
        )
        .ok();
    sz_rust_core::response::render_success(json!(posts), "ok")
}

async fn detail_post(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> axum::response::Response {
    match state.find(id) {
        Some(post) => sz_rust_core::response::render_success(json!(post), "ok"),
        None => sz_rust_core::response::render_error("文章不存在"),
    }
}

async fn create_post(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> axum::response::Response {
    let title = payload["title"].as_str().unwrap_or_default().to_string();
    let content = payload["content"].as_str().unwrap_or_default().to_string();
    let author = payload["author"]
        .as_str()
        .unwrap_or("anonymous")
        .to_string();
    if title.is_empty() {
        return sz_rust_core::response::render_error("标题不能为空");
    }
    let post = state.create(&title, &content, &author);
    sz_rust_core::response::render_success(json!(post), "发布成功")
}

async fn delete_post(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> axum::response::Response {
    if state.delete(id) {
        sz_rust_core::response::render_success(json!({"id": id}), "删除成功")
    } else {
        sz_rust_core::response::render_error("文章不存在")
    }
}

async fn stats(State(state): State<Arc<AppState>>) -> axum::response::Response {
    sz_rust_core::response::render_success(
        json!({
            "post_count": state.post_count.load(Ordering::SeqCst),
            "cache_hits": state.cache_hits.load(Ordering::SeqCst),
            "total_posts": state.list().len(),
        }),
        "ok",
    )
}

// ============================================================================
// 入口
// ============================================================================

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let state = Arc::new(AppState::new());
    // 预热：发布 3 篇示例文章
    state.create(
        "Rust 异步运行时入门",
        "async/await 与 tokio 详解",
        "sz-team",
    );
    state.create("facade 拆包实战", "从 57K 单体到 11 个 facade", "sz-team");
    state.create("ThinkPHP 迁移指南", "PHP 开发者视角的 Rust 框架", "sz-team");

    let app = Router::new()
        .route("/post/list", get(list_posts))
        .route("/post/detail/{id}", get(detail_post))
        .route("/post/create", post(create_post))
        .route("/post/delete/{id}", post(delete_post))
        .route("/post/stats", get(stats))
        .with_state(state);

    let addr = "127.0.0.1:8081";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("绑定监听地址失败: {addr}: {e}"));
    tracing::info!("博客示例运行于 http://{addr} （/post/list /post/create /post/stats）");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("HTTP 服务启动失败: {e}"));
}
