//! 完整 CRUD 示例 — User 模型 + UserController 控制器
//!
//! 演示 sz-rust-core 控制器 trait（SzController / BaseController）的完整使用，
//! 路由路径对齐 PHP `app/controller/action` 约定，响应格式对齐 PHP `renderJson`。
//!
//! ## 端点
//!
//! | 方法 | 路径 | 说明 |
//! |------|------|------|
//! | GET  | /user/list?page=1&size=10 | 列表查询（分页）|
//! | GET  | /user/detail/{id} | 详情查询 |
//! | POST | /user/create | 创建（body: {"name","age"}）|
//! | POST | /user/update/{id} | 更新（body: {"name"?,"age"?}）|
//! | POST | /user/delete/{id} | 删除 |
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin crud_demo
//! ```
//!
//! 使用内存 Vec 存储数据，无需真实数据库，可直接运行演示。

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::Request;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use sz_rust_core::controller::{BaseController, SzController};

// ============================================================================
// 模型层 — 简化的 User struct
// ============================================================================
// 说明：BaseModel trait 需实现 SZ-ORM 的 Model + ModelExt + RelationLoader，
// 涉及数据库映射，较重。本示例使用简化 struct 演示控制器层 API。
// 实际项目中实现 BaseModel 即可获得 $append / $hidden / $fillable / 访问器等能力，
// 详见 sz_rust_core::model::BaseModel 文档。

/// 用户模型（简化版，字段对齐 PHP think\Model）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    /// 用户 ID（主键，对齐 PHP $pk）
    id: i64,
    /// 用户名（对齐 PHP $fillable 字段）
    name: String,
    /// 年龄
    age: i32,
}

// ============================================================================
// 内存存储 — 模拟数据库表 + 自增主键
// ============================================================================

struct Store {
    users: Vec<User>,
    next_id: i64,
}

/// 共享存储类型（`Arc<Mutex>` 保证线程安全）
type SharedStore = Arc<Mutex<Store>>;

// ============================================================================
// 控制器层 — UserController 实现 SzController + BaseController
// ============================================================================
// 对齐 PHP 继承链：BaseController → SzController → 业务控制器
// Rust 等价：UserController 实现 BaseController（自动获得 SzController 全部方法）

/// 用户控制器（对齐 PHP app\controller\User）
struct UserController;

impl SzController for UserController {}

impl BaseController for UserController {
    // 声明控制器中间件（对齐 PHP protected array $middleware）
    fn middlewares(&self) -> Vec<String> {
        vec!["cors".to_string()]
    }
}

impl UserController {
    // 列表查询：对齐 PHP public function list()

    async fn list(store: &SharedStore, page: i64, size: i64) -> Response {
        let ctrl = UserController;
        let users = store.lock().expect("锁被毒化").users.clone();
        let total = users.len() as i64;

        // 分页计算（边界安全：start 不超过 users.len()）
        let start = (((page - 1) * size).max(0) as usize).min(users.len());
        let end = (start + size as usize).min(users.len());
        let page_list: Vec<Value> = users[start..end]
            .iter()
            .map(|u| serde_json::to_value(u).expect("序列化用户数据失败"))
            .collect();

        let data = json!({
            "list": page_list,
            "total": total,
            "page": page,
            "size": size,
        });
        ctrl.render_success("success", data)
    }

    // 详情查询：对齐 PHP public function detail()

    async fn detail(store: &SharedStore, id: i64) -> Response {
        let ctrl = UserController;
        // clone 后立即释放锁
        let user = store
            .lock()
            .expect("锁被毒化")
            .users
            .iter()
            .find(|u| u.id == id)
            .cloned();
        match user {
            Some(user) => {
                let data = serde_json::to_value(&user).expect("序列化用户数据失败");
                ctrl.render_success("success", data)
            }
            None => ctrl.render_error("用户不存在", json!({"id": id}), 0),
        }
    }

    // 创建：对齐 PHP public function create()

    async fn create(store: &SharedStore, req: Request<Body>) -> Response {
        let ctrl = UserController;
        // 调用初始化钩子（对齐 PHP initialize()）
        ctrl.initialize();

        // 读取 POST 数据（合并 body + query，对齐 PHP postData()）
        let data = match ctrl.post_data(req).await {
            Ok(d) => d,
            Err(e) => return ctrl.render_error("参数解析失败", json!({"error": e}), 0),
        };

        let name = data["name"].as_str().unwrap_or("").to_string();
        let age = data["age"].as_i64().unwrap_or(0) as i32;

        // 手动校验（框架 validate() 为占位实现，Phase 5 完整支持 30+ 规则）
        if name.is_empty() {
            return ctrl.render_error("用户名不能为空", json!({"field": "name"}), 0);
        }
        if age <= 0 || age > 200 {
            return ctrl.render_error("年龄必须在 1-200 之间", json!({"field": "age"}), 0);
        }

        let mut store = store.lock().expect("锁被毒化");
        let user = User {
            id: store.next_id,
            name,
            age,
        };
        store.next_id += 1;
        store.users.push(user.clone());
        drop(store); // 提前释放锁

        let data = serde_json::to_value(&user).expect("序列化用户数据失败");
        ctrl.render_success("创建成功", data)
    }

    // 更新：对齐 PHP public function update()

    async fn update(store: &SharedStore, id: i64, req: Request<Body>) -> Response {
        let ctrl = UserController;
        ctrl.initialize();

        let data = match ctrl.post_data(req).await {
            Ok(d) => d,
            Err(e) => return ctrl.render_error("参数解析失败", json!({"error": e}), 0),
        };

        let mut store = store.lock().expect("锁被毒化");
        match store.users.iter_mut().find(|u| u.id == id) {
            Some(user) => {
                if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                    user.name = name.to_string();
                }
                if let Some(age) = data.get("age").and_then(|v| v.as_i64()) {
                    user.age = age as i32;
                }
                let user = user.clone();
                drop(store); // 提前释放锁

                let data = serde_json::to_value(&user).expect("序列化用户数据失败");
                ctrl.render_success("更新成功", data)
            }
            None => ctrl.render_error("用户不存在", json!({"id": id}), 0),
        }
    }

    // 删除：对齐 PHP public function delete()

    async fn delete(store: &SharedStore, id: i64) -> Response {
        let ctrl = UserController;
        let mut store = store.lock().expect("锁被毒化");
        match store.users.iter().position(|u| u.id == id) {
            Some(idx) => {
                let user = store.users.remove(idx);
                drop(store); // 提前释放锁

                let data = serde_json::to_value(&user).expect("序列化用户数据失败");
                ctrl.render_success("删除成功", data)
            }
            None => ctrl.render_error("用户不存在", json!({"id": id}), 0),
        }
    }
}

// ============================================================================
// axum Handler 函数 — 桥接 axum 路由提取器与控制器方法
// ============================================================================

/// 列表查询参数（对齐 PHP getData('page') / getData('size')）
#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_size")]
    size: i64,
}

fn default_page() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

async fn list_handler(
    State(store): State<SharedStore>,
    Query(query): Query<ListQuery>,
) -> Response {
    UserController::list(&store, query.page, query.size).await
}

async fn detail_handler(State(store): State<SharedStore>, Path(id): Path<i64>) -> Response {
    UserController::detail(&store, id).await
}

async fn create_handler(State(store): State<SharedStore>, req: Request<Body>) -> Response {
    UserController::create(&store, req).await
}

async fn update_handler(
    State(store): State<SharedStore>,
    Path(id): Path<i64>,
    req: Request<Body>,
) -> Response {
    UserController::update(&store, id, req).await
}

async fn delete_handler(State(store): State<SharedStore>, Path(id): Path<i64>) -> Response {
    UserController::delete(&store, id).await
}

// ============================================================================
// 路由构建 + 服务启动
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 初始化内存存储，预置 3 条种子数据
    let store: SharedStore = Arc::new(Mutex::new(Store {
        users: vec![
            User {
                id: 1,
                name: "张三".to_string(),
                age: 28,
            },
            User {
                id: 2,
                name: "李四".to_string(),
                age: 35,
            },
            User {
                id: 3,
                name: "王五".to_string(),
                age: 42,
            },
        ],
        next_id: 4,
    }));

    // 构建路由（路径对齐 PHP app/controller/action 约定）
    let router = Router::new()
        .route("/user/list", get(list_handler))
        .route("/user/detail/{id}", get(detail_handler))
        .route("/user/create", post(create_handler))
        .route("/user/update/{id}", post(update_handler))
        .route("/user/delete/{id}", post(delete_handler))
        .with_state(store);

    let addr = "127.0.0.1:9528";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("CRUD 示例服务监听 http://{}/", addr);
    tracing::info!("端点：");
    tracing::info!("  GET  /user/list?page=1&size=10");
    tracing::info!("  GET  /user/detail/{{id}}");
    tracing::info!("  POST /user/create         body: {{\"name\":\"...\",\"age\":...}}");
    tracing::info!("  POST /user/update/{{id}}  body: {{\"name\":\"...\",\"age\":...}}");
    tracing::info!("  POST /user/delete/{{id}}");

    axum::serve(listener, router).await?;

    Ok(())
}
