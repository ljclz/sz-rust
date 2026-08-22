# sz-rust API 快速入门指南

> **适用版本**：sz-rust-core v0.2.1 + sz-orm v1.2.1  
> **Rust Edition**：2021 | **MSRV**：1.81  
> **生成日期**：2026-08-02

---

## 目录

1. [快速开始](#1-快速开始)
2. [控制器](#2-控制器)
3. [路由](#3-路由)
4. [请求与响应](#4-请求与响应)
5. [中间件](#5-中间件)
6. [ORM 数据访问](#6-orm-数据访问)
7. [缓存](#7-缓存)
8. [验证](#8-验证)
9. [认证与授权](#9-认证与授权)
10. [DI 容器](#10-di-容器)
11. [配置管理](#11-配置管理)
12. [错误处理](#12-错误处理)
13. [测试](#13-测试)
14. [部署](#14-部署)

---

## 1. 快速开始

### 1.1 创建新项目

```bash
# 使用 CLI 脚手架（T7 完成后）
sz new my-app --db mysql

# 或手动创建
cargo new my-app
cd my-app
```

### 1.2 Cargo.toml 配置

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2021"

[dependencies]
sz-rust-core = "0.2.1"
axum = "0.8"
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### 1.3 最小可运行示例

```rust
// src/main.rs
use sz_rust_core::router::RouterBuilder;
use sz_rust_core::server::Server;
use axum::{body::Body, http::Request, response::Response};
use sz_rust_core::controller::SzController;
use serde_json::json;

struct HelloController;
impl SzController for HelloController {}

async fn index(_state: &sz_rust_core::state::AppState, _req: Request<Body>) -> Response {
    let ctrl = HelloController;
    ctrl.render_success("Hello, sz-rust!", json!({"version": "0.2.1"}))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 构建路由
    let router = RouterBuilder::new()
        .get("/", index)
        .get("/api/hello", index)
        .build();

    // 启动服务器（默认 0.0.0.0:8080）
    Server::new(router).run().await?;
    Ok(())
}
```

### 1.4 运行

```bash
# 设置必要环境变量
export SZ_JWT_SECRET="your-secret-key-at-least-32-chars"
export DATABASE_URL="mysql://user:pass@localhost:3306/mydb"

# 开发模式
cargo run

# 生产模式
cargo run --release
```

### 1.5 验证

```bash
curl http://localhost:8080/api/hello
# {"code":1,"msg":"Hello, sz-rust!","data":{"version":"0.2.1"},"total":0}
```

---

## 2. 控制器

控制器处理 HTTP 请求，调用服务层，返回标准化响应。

### 2.1 基本结构

```rust
use sz_rust_core::controller::SzController;
use axum::{body::Body, http::Request, response::Response};
use serde_json::json;

/// 用户控制器
struct UserController;
impl SzController for UserController {}

impl UserController {
    /// 用户列表
    async fn list(&self, req: Request<Body>) -> Response {
        let data = self.post_data(req).await.unwrap_or(json!({}));
        let page = data.get("page").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
        let page_size = data.get("page_size").and_then(|v| v.as_i64()).unwrap_or(20) as u32;

        // TODO: 调用服务层查询
        self.render_success("ok", json!({
            "list": [],
            "page": page,
            "page_size": page_size,
            "total": 0
        }))
    }

    /// 用户详情
    async fn info(&self, req: Request<Body>) -> Response {
        let data = self.post_data(req).await?;
        let id = data.get("id").and_then(|v| v.as_i64())
            .ok_or("缺少用户 ID")?;

        // TODO: 调用服务层查询
        self.render_success("ok", json!({"id": id, "name": "张三"}))
    }
}

/// 路由 handler（axum 提取 State）
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    UserController.list(req).await
}
```

### 2.2 响应方法

| 方法 | 说明 | HTTP 状态 |
|------|------|----------|
| `render_success(msg, data)` | 操作成功 | 200 |
| `render_error(msg, data, code)` | 操作失败 | 200（业务失败） |
| `render_json(code, msg, data)` | 自定义响应 | 200 |

```rust
// 成功响应
ctrl.render_success("创建成功", json!({"id": 42}))
// → {"code":1,"msg":"创建成功","data":{"id":42},"total":0}

// 失败响应
ctrl.render_error("参数错误", json!({}), 1001)
// → {"code":1001,"msg":"参数错误","data":{},"total":0}

// 分页响应
ctrl.render_success("ok", json!({
    "list": [...],
    "total": 100,
    "page": 1,
    "page_size": 20
}))
```

### 2.3 请求参数获取

```rust
// 合并 POST body + query 参数
let data = self.post_data(req).await?;

// 获取单个字段
let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
let age = data.get("age").and_then(|v| v.as_i64()).unwrap_or(0);
let active = data.get("active").and_then(|v| v.as_bool()).unwrap_or(false);

// query 参数
let query_data = self.get_data(&req);
let keyword = query_data.get("keyword").and_then(|v| v.as_str()).unwrap_or("");
```

---

## 3. 路由

### 3.1 路由定义

```rust
use sz_rust_core::router::RouterBuilder;

let router = RouterBuilder::new()
    // 基础 HTTP 方法
    .get("/", home_handler)
    .get("/users", list_users)
    .post("/users", create_user)
    .put("/users/:id", update_user)
    .delete("/users/:id", delete_user)

    // 路径参数
    .get("/users/:id", get_user)
    .get("/posts/:post_id/comments/:cid", get_comment)

    // 可选参数
    .get("/users/:id?", list_users_optional)

    // 通配符
    .get("/files/*path", serve_file)

    // 资源路由（自动生成 CRUD 路由）
    .resource("/articles", ArticleController)

    .build();
```

### 3.2 路由优先级

```
精确匹配 > 路径参数 > 可选参数 > 通配符

GET /users       → 精确匹配
GET /users/123   → 路径参数 :id
GET /users/      → 可选参数 :id?
GET /files/a/b   → 通配符 *path
```

### 3.3 路由分组

```rust
let api = RouterBuilder::new()
    .prefix("/api")
    .get("/users", list_users)
    .post("/users", create_user);

let admin = RouterBuilder::new()
    .prefix("/api/admin")
    .get("/stats", get_stats);

let router = RouterBuilder::new()
    .merge(api)
    .merge(admin)
    .build();
```

### 3.4 API 版本控制

```rust
// URL 版本
GET /api/v1/users
GET /api/v2/users

// Header 版本
Accept: application/vnd.myapp.v1+json

// Query 版本
GET /api/users?version=1
```

---

## 4. 请求与响应

### 4.1 请求体解析

```rust
use sz_rust_core::request::fetch_post_data;

async fn handler(req: Request<Body>) -> Response {
    let data: serde_json::Value = fetch_post_data(req).await?;
    // data = {"name": "张三", "age": 25}
}
```

### 4.2 文件上传

```rust
use sz_rust_core::upload::{handle_multipart, UploadedFile};

async fn upload(req: Request<Body>) -> Response {
    let files = handle_multipart(req).await?;
    for file in files {
        println!("文件名: {}, 大小: {} bytes", file.name, file.size);
        // 保存到存储
        let path = file.save_to("uploads/").await?;
    }
    ctrl.render_success("上传成功", json!({"files": files.len()}))
}
```

### 4.3 响应格式

所有响应遵循统一格式：

```json
{
  "code": 1,
  "msg": "操作成功",
  "data": { ... },
  "total": 0
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `code` | int | 1=成功, 0或其他=失败（业务错误码） |
| `msg` | string | 响应消息 |
| `data` | object/array | 响应数据 |
| `total` | int | 分页总数（列表接口） |

---

## 5. 中间件

### 5.1 内置中间件

| 中间件 | 说明 | 配置 |
|--------|------|------|
| `Cors` | CORS 跨域 | `CorsConfig` |
| `Auth` | JWT 认证 | `AuthConfig` |
| `RateLimit` | 频率限制 | `RateLimitConfig` |
| `Log` | 请求日志 | `LogConfig` |
| `Trace` | 链路追踪 | `TraceConfig` |
| `Csrf` | CSRF 防护 | `CsrfConfig` |

### 5.2 中间件链配置

```rust
use sz_rust_core::middleware::{MiddlewareChain, MiddlewareKind};

let chain = MiddlewareChain::new()
    .add(MiddlewareKind::Trace)
    .add(MiddlewareKind::Cors)
    .add(MiddlewareKind::Log)
    .add(MiddlewareKind::RateLimit)
    .add(MiddlewareKind::Auth);

// 默认顺序：Trace → Cors → Log → RateLimit → Auth
```

### 5.3 自定义中间件

```rust
use sz_rust_core::middleware::{SzMiddleware, MiddlewareConfig};
use axum::{body::Body, http::Request, response::Response};
use std::future::Future;

struct MyMiddleware;

#[sz_rust_core::middleware]
impl SzMiddleware for MyMiddleware {
    async fn handle(
        &self,
        req: Request<Body>,
        next: impl FnOnce(Request<Body>) -> Fut,
    ) -> Response {
        // 前置处理
        println!("请求路径: {}", req.uri().path());

        let response = next(req).await;

        // 后置处理
        println!("响应状态: {}", response.status());

        response
    }
}
```

### 5.4 频率限制配置

```rust
use sz_rust_core::middleware::rate_limit::{RateLimitConfig, RateLimiterType};

let config = RateLimitConfig {
    limiter_type: RateLimiterType::SlidingWindow, // 或 TokenBucket
    max_requests: 100,      // 窗口内最大请求数
    window_seconds: 60,     // 窗口大小（秒）
    key_extractor: KeyExtractor::Ip, // Ip / UserId / IpPlusRoute
    exclude_paths: vec!["/health", "/favicon.ico"],
};
```

---

## 6. ORM 数据访问

### 6.1 模型定义

```rust
use sz_rust_core::orm::{Model, Relation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[orm(table = "users")]
pub struct User {
    #[orm(primary)]
    pub id: i64,
    pub username: String,
    pub email: String,
    pub phone: Option<String>,
    pub status: i32,
    pub created_at: i64,
    pub updated_at: i64,
}
```

### 6.2 查询操作

```rust
use sz_rust_core::orm::{ModelExt, Connection};

// 查询单个
let user = User::find_by_id(&mut conn, 1).await?;

// 条件查询
let users = User::where_eq("status", 1)
    .where_gt("created_at", 1609459200)
    .order_by("id", "DESC")
    .limit(20)
    .offset(0)
    .select::<User>(&mut conn)
    .await?;

// 分页查询
let page = User::paginate(&mut conn, 1, 20).await?;
println!("总数: {}, 当前页: {}", page.total, page.items.len());

// 聚合
let count = User::count(&mut conn).await?;
let max_id = User::max("id", &mut conn).await?;
```

### 6.3 关联查询

```rust
#[derive(Model)]
#[orm(table = "posts")]
pub struct Post {
    #[orm(primary)]
    pub id: i64,
    pub user_id: i64,
    pub title: String,
}

impl Post {
    /// 属于用户（BelongsTo）
    pub fn user(&self) -> BelongsTo<User> {
        BelongsTo::new(self, "user_id", "id")
    }

    /// 有多个评论（HasMany）
    pub fn comments(&self) -> HasMany<Comment> {
        HasMany::new(self, "id", "post_id")
    }
}

// 预加载关联（避免 N+1）
let posts = Post::with_relation("user")
    .select::<Post>(&mut conn)
    .await?;
```

### 6.4 写入操作

```rust
// 插入
let user = User {
    id: 0, // 自增 ID，插入后自动填充
    username: "张三".to_string(),
    email: "zhang@example.com".to_string(),
    status: 1,
    created_at: now(),
    updated_at: now(),
};
let id = User::insert(&mut conn, &user).await?;

// 更新
User::where_eq("id", 1)
    .update(&mut conn, &[("status", 0)]).await?;

// 删除
User::where_eq("id", 1).delete(&mut conn).await?;

// Upsert
User::upsert(&mut conn, &user, &["email"]).await?;
```

---

## 7. 缓存

### 7.1 缓存驱动

```rust
use sz_rust_core::cache::{Cache, CacheDriver};

// 内存缓存（开发/测试）
let cache = Cache::memory();

// Redis 缓存（生产）
let cache = Cache::redis("redis://localhost:6379")?;

// Memcached
let cache = Cache::memcached("localhost:11211")?;
```

### 7.2 基本操作

```rust
// 设置（默认 TTL 3600s）
cache.set("key", "value")?;
cache.set_ex("key", "value", 300)?; // 5 分钟

// 获取
let val: Option<String> = cache.get("key")?;

// 删除
cache.del("key")?;

// 存在性
let exists = cache.has("key")?;

// 自增/自减
cache.incr("counter", 1)?;
cache.decr("counter", 1)?;
```

### 7.3 批量操作

```rust
// 批量设置
cache.set_many(&[("k1", "v1"), ("k2", "v2")])?;

// 批量获取
let vals: HashMap<String, String> = cache.get_many(&["k1", "k2"])?;

// 批量删除
cache.del_many(&["k1", "k2"])?;
```

### 7.4 缓存标签

```rust
// 带标签存储
cache.tags(&["user:123", "profile"]).set("key", "value")?;

// 按标签清除
cache.tags(&["user:123"]).flush()?;
```

---

## 8. 验证

### 8.1 验证规则

```rust
use sz_rust_core::validate::{Validate, Validator};

let validator = Validator::new()
    .rule("username", "required|min:3|max:20|alpha_dash")
    .rule("email", "required|email")
    .rule("age", "integer|between:18,100")
    .rule("password", "required|min:8|regex:^[a-zA-Z0-9!@#$%]+$")
    .rule("confirm_password", "required|confirm:password")
    .rule("phone", "mobile")
    .rule("status", "in:0,1,2");

let data = json!({
    "username": "zhangsan",
    "email": "zhang@example.com",
    "age": 25,
    "password": "abc123!@#",
    "confirm_password": "abc123!@#",
});

match validator.validate(&data) {
    Ok(_) => { /* 验证通过 */ }
    Err(errors) => {
        // errors = {"username": ["用户名至少 3 个字符"], ...}
        return ctrl.render_error("参数验证失败", json!(errors), 1002);
    }
}
```

### 8.2 常用规则

| 规则 | 说明 |
|------|------|
| `required` | 必填 |
| `min:N` / `max:N` | 最小/最大长度 |
| `between:N,M` | 范围 |
| `email` | 邮箱格式 |
| `mobile` | 手机号 |
| `integer` / `float` | 数字类型 |
| `alpha` / `alpha_dash` | 字母/字母数字下划线 |
| `url` / `ip` | URL/IP 格式 |
| `in:a,b,c` | 枚举值 |
| `not_in:a,b,c` | 排除值 |
| `confirm:field` | 确认字段匹配 |
| `regex:pattern` | 正则匹配 |

---

## 9. 认证与授权

### 9.1 JWT 认证

```rust
use sz_rust_core::middleware::auth::{AuthMiddleware, JwtClaims};

// 登录签发 token
let claims = JwtClaims {
    id: user.id,
    username: user.username.clone(),
    roles: vec!["admin".to_string()],
    exp: chrono::Utc::now().timestamp() + 86400, // 24 小时
};
let token = JwtToken::sign(&claims, &config.secret)?;

// 中间件验证
let auth_middleware = AuthMiddleware::new(config);
// 自动从 Authorization: Bearer <token> 提取并验证
```

### 9.2 权限检查

```rust
use sz_rust_core::guard::Guard;

struct AdminGuard;

#[sz_rust_core::guard]
impl Guard for AdminGuard {
    async fn check(&self, ctx: &RequestContext) -> Result<(), AuthError> {
        let user = ctx.get::<JwtClaims>("user")?;
        if !user.roles.contains(&"admin".to_string()) {
            return Err(AuthError::Forbidden("需要管理员权限"));
        }
        Ok(())
    }
}
```

---

## 10. DI 容器

### 10.1 服务绑定

```rust
use sz_rust_core::container::App;

// 绑定接口实现
App::singleton::<dyn UserService, UserServiceImpl>();
App::transient::<dyn EmailService, SmtpEmailService>();

// 标签绑定
App::tag::<[UserService, EmailService]>("notifiers");
```

### 10.2 服务解析

```rust
// 单例（整个应用生命周期共享）
let service = App::make::<dyn UserService>()?;

// 瞬态（每次解析创建新实例）
let email = App::make::<dyn EmailService>()?;

// 标签解析
let notifiers = App::tagged::<dyn Notifier>("notifiers")?;
```

---

## 11. 配置管理

### 11.1 配置文件格式

```yaml
# config/app.yaml
app:
  name: "My App"
  debug: true
  url: "https://api.example.com"

database:
  default: mysql
  connections:
    mysql:
      host: localhost
      port: 3306
      database: mydb
      username: root
      password: secret
      charset: utf8mb4

cache:
  default: redis
  prefix: "myapp_"
  stores:
    redis:
      host: localhost
      port: 6379
      db: 0
```

### 11.2 读取配置

```rust
use sz_rust_core::config::Config;

let name = Config::get("app.name").unwrap_or("Default".to_string());
let debug = Config::get::<bool>("app.debug").unwrap_or(false);
let db_host = Config::get("database.connections.mysql.host").unwrap();
```

---

## 12. 错误处理

### 12.1 自定义错误

```rust
use sz_rust_core::error::{BaseException, ErrorCode};

#[derive(Debug)]
enum BusinessException {
    UserNotFound(i64),
    InvalidPassword,
    AccountLocked,
}

impl BaseException for BusinessException {
    fn code(&self) -> i32 {
        match self {
            Self::UserNotFound(_) => 2001,
            Self::InvalidPassword => 2002,
            Self::AccountLocked => 2003,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::UserNotFound(id) => format!("用户 {} 不存在", id),
            Self::InvalidPassword => "密码错误".to_string(),
            Self::AccountLocked => "账户已被锁定".to_string(),
        }
    }
}
```

### 12.2 全局错误处理

```rust
use sz_rust_core::error_handler::ErrorHandler;

// 框架自动捕获 panic 和 Err，返回标准化 JSON：
// {"code": 500, "msg": "Internal Server Error", "data": {}, "total": 0}
```

---

## 13. 测试

### 13.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::controller::SzController;
    use serde_json::json;

    #[test]
    fn test_render_success() {
        let ctrl = TestController;
        let resp = ctrl.render_success("ok", json!({"id": 1}));
        let body = resp.into_response().into_body();
        // 断言响应内容
    }
}
```

### 13.2 集成测试

```rust
#[cfg(test)]
mod integration {
    use sz_rust_core::router::RouterBuilder;
    use tower::ServiceExt;
    use http::{Request, StatusCode};

    #[tokio::test]
    async fn test_hello_endpoint() {
        let router = RouterBuilder::new()
            .get("/hello", hello_handler)
            .build();

        let response = router
            .oneshot(Request::builder().uri("/hello").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
```

---

## 14. 部署

### 14.1 Docker 部署

```dockerfile
FROM rust:1.81-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/my-app /usr/local/bin/
EXPOSE 8080
CMD ["my-app"]
```

### 14.2 环境变量

| 变量 | 说明 | 必填 |
|------|------|------|
| `SZ_JWT_SECRET` | JWT 签名密钥 | ✅ |
| `SZ_JWT_ISSUER` | JWT 签发人 | ❌ |
| `DATABASE_URL` | 数据库连接串 | ✅ |
| `REDIS_URL` | Redis 连接串 | ❌ |
| `RUST_LOG` | 日志级别 | ❌ |
| `APP_PORT` | 服务端口 | ❌ (默认 8080) |

### 14.3 健康检查

```bash
# K8s liveness/readiness
GET /health
# {"status":"ok","timestamp":1609459200}

GET /health/db
# {"status":"ok","latency_ms":2}

GET /health/cache
# {"status":"ok","driver":"redis"}
```

---

## 附录 A：PHP → Rust 对照表

| PHP (ThinkPHP 8) | Rust (sz-rust) |
|------------------|----------------|
| `controller('User')` | `UserController` (struct) |
| `$this->request->post()` | `self.post_data(req).await` |
| `$this->request->get()` | `self.get_data(&req)` |
| `json(['code'=>1,'msg'=>'ok'])` | `ctrl.render_success("ok", data)` |
| `Db::name('user')->find($id)` | `User::find_by_id(&mut conn, id).await` |
| `Db::name('user')->where('status',1)->select()` | `User::where_eq("status",1).select().await` |
| `Cache::get('key')` | `cache.get("key")` |
| `Cache::set('key','val',300)` | `cache.set_ex("key","val",300)` |
| `validate([...])->check($data)` | `Validator::new().rule(...).validate(&data)` |
| `middleware()` | `SzMiddleware::handle()` |
| `app()->bind()` | `App::singleton::<T, Impl>()` |
| `app()->make()` | `App::make::<T>()` |

---

## 附录 B：常见问题

### Q: 如何处理异步错误？

```rust
async fn handler(req: Request<Body>) -> Response {
    let ctrl = MyController;

    // 使用 ? 传播错误，框架自动转为 JSON 错误响应
    let data = fetch_post_data(req).await?;
    let result = some_async_op(data).await?;

    ctrl.render_success("ok", result)
}
```

### Q: 如何实现事务？

```rust
let mut tx = conn.begin().await?;
User::insert(&mut tx, &user).await?;
Profile::insert(&mut tx, &profile).await?;
tx.commit().await?; // 或 tx.rollback().await?
```

### Q: 如何配置多数据库？

```yaml
database:
  default: mysql
  connections:
    mysql:
      host: localhost
      database: mydb
    pg:
      host: localhost
      database: mydb_pg
```

```rust
let mysql_conn = App::make::<MySqlConnection>()?;
let pg_conn = App::make::<PgConnection>()?;
```

---

*本文档由 sz-rust 团队维护，如有问题请访问 https://github.com/ljclz/sz-rust*
