# sz-rust API 快速参考（v2.0.0）

> 2026-10-27 UXR P2-2 整改：本文档替代已归档的 [api-guide.md](api-guide.md)（基于 v0.2.1，已过期）。
> 本文档仅列出 **v2.0.0 实际存在**的核心 API 入口；完整签名以 `cargo doc -p sz-rust-core --open` 生成的文档为准。

## 1. 应用与配置

| API | 位置 | 说明 |
|-----|------|------|
| `AppConfig::load_from_dir(&Path)` | `sz_rust_core::config` | 从目录加载 YAML 配置（`config/` 下 app/database/cache/log/addons） |
| `App::init(config)` | `sz_rust_core::container` | 初始化应用容器（DI） |
| `LogFacade::init(&config.log)` | `sz_rust_core::log` | 初始化日志门面（tracing 集成） |

最小可运行示例见 [quick_start.rs](../packages/sz-rust-examples/src/bin/quick_start.rs)。

## 2. 控制器

| API | 位置 | 说明 |
|-----|------|------|
| trait `SzController` | `sz_rust_core::controller` | 顶层控制器 trait：`renderJson` / `renderSuccess` / `renderError` / `postData` / `getData` |
| trait `BaseController` | `sz_rust_core::controller` | 业务控制器：`batchValidate` / `middlewares()` / `initialize()` / `validate()` |
| trait `AddonsBaseController` | `sz_rust_core::controller` | 插件控制器：`allowAllAction` / `getRouteinfo()` / `getToken()` / `checkLogin()` |

## 3. 路由

sz-rust 基于 **axum 0.8** `Router`，并提供三层路由机制：

| 机制 | 入口 | 说明 |
|------|------|------|
| 属性宏 | `sz_rust_core::router` 宏 | 在 handler 上声明路径 |
| 配置式 | `sz_rust_core::router` 配置表 | 从配置生成路由 |
| 约定式 | `sz_rust_core::router::parse_path` | 解析 `/app/controller/action` URL（见 [parse_path_demo.rs](../packages/sz-rust-examples/src/bin/parse_path_demo.rs)） |

使用 axum 原生路由：

```rust
let router = Router::new()
    .route("/", axum::routing::get(handler))
    .merge(admin_router);
```

## 4. ORM（Data Mapper + Repository）

| API | 位置 | 说明 |
|-----|------|------|
| `Db::init(pool)` / `Db::pool_ref()` | `sz_rust_core::orm` 门面 | 连接池管理（**无** Active Record 流畅查询） |
| trait `Repository` | `sz_rust_orm_facade::repository` | `find_by_id` / `find_all` / `find_by` / `find_one_by` / `save` / `save_many` / `delete` |
| `WhereCondition` / `WhereOp` | `sz_rust_orm_facade::repository` | 条件构造（`Eq` / `Ne` / `Gt` / `Ge` / `Lt` / `Le` / `Like` / `In` / `Between` 等） |
| `Value` | `sz_rust_orm_facade` | 参数化值类型（`I64` / `String` / `Bool` 等） |

```rust
use sz_rust_orm_facade::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_orm_facade::Value;

let active = repo.find_by(&[WhereCondition::new("status", WhereOp::Eq, Value::I64(1))])?;
```

## 5. 验证器

| API | 位置 | 说明 |
|-----|------|------|
| `Validate::new()` | `sz_rust_core::validate` | 创建验证器 |
| `rule(name, rules)` | 同上 | 添加规则（`"require|max:100"`；name 支持 `field|title` 格式） |
| `message(IndexMap)` | 同上 | 自定义消息（key 支持 `field.rule`） |
| `field(name, title)` | 同上 | 设置字段描述 |
| `check(&data)` | 同上 | 执行校验，返回 `Result<(), ValidateError>` |

默认错误消息为**中文**（内置默认中文语言包，2026-10-27 UXR P1-5 起）。

## 6. 缓存 / 事件 / 中间件

| API | 位置 | 说明 |
|-----|------|------|
| `sz_rust_core::cache` | 缓存门面 | Memory / Redis / Memcached / 多级驱动 |
| `sz_rust_core::event` | 事件门面 | Listener / Subscriber / Observer |
| `sz_rust_core::middleware` | 中间件 | CORS / Auth(JWT) / Log / RateLimit / Trace + `MiddlewareChain` |
| `sz_rust_core::error::ErrorCode` | 错误码 | 标准业务错误码 |

## 7. CLI

```bash
cargo run -p sz-rust-cli -- --help          # 全部命令
cargo run -p sz-rust-cli -- make model User # 生成模型
cargo run -p sz-rust-cli -- route:list      # 路由列表
```

> `serve:reload` / `serve:log-level` 为 [WIP]（2026-10-27 UXR P1-3 标注），请使用 `--watch-config` 或重启服务。

## 8. 相关文档

- [开发者使用指南](developer-guide.md)
- [示例运行指南](examples.md)
- [ThinkPHP API 对照表](thinkphp-api-mapping.md)
- 完整 API：`cargo doc -p sz-rust-core --open`