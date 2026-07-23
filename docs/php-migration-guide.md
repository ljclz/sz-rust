# PHP 迁移指南 — 从 ThinkPHP 8 到 SZ-Rust

本指南帮助 PHP 开发者将 ThinkPHP 8 项目迁移到 SZ-Rust 框架。所有 API 均可在 `packages/sz-rust-core/src/` 实际源码中验证。

---

## 1. 概述

### 1.1 SZ-Rust 是什么

SZ-Rust 是对标 ThinkPHP 8 的 Rust Web 框架，基于 axum 0.8 + SZ-ORM 构建。API 设计与 ThinkPHP 8 一一对齐，便于 PHP 工程师迁移。

### 1.2 为什么迁移

| 维度 | ThinkPHP 8 (PHP) | SZ-Rust (Rust) |
|------|------------------|----------------|
| 性能 | 解释执行 + JIT | AOT 编译，零成本抽象 |
| 类型安全 | 弱类型，运行时错误 | 强类型，编译期错误 |
| 内存安全 | GC 管理，存在泄漏风险 | 所有权机制，编译期保证 |
| 并发模型 | think-swoole 协程 | tokio 异步运行时 |
| SQL 安全 | 运行时转义 | 编译时 SQL 校验（`sql_string!` 宏） |

### 1.3 迁移原则：1:1 复刻（含 bug）

SZ-Rust 遵循 **R5 硬约束**（见 `CONTRIBUTING.md` 第 128-136 行 "PHP 迁移规范"）：

1. **1:1 复刻 PHP 行为**：包括 PHP 源码 bug（必须有注释说明）
2. **控制器方法无参数**：主键从 `$data` 获取（如 `$data['good_id']`）
3. **不使用 GET 请求分支**：禁止 `if ($this->request->isGet())`
4. **标准响应格式**：`{code, msg, data}`，字段顺序严格一致
5. **错误码对齐**：1/0/-1/-2/-3 + 403/404/422/500

> **bug 复刻示例**：PHP `getRouteinfo()` 中 `str_replace` 已将 `.` 替换为 `/`，但 `strstr` 仍以 `.` 为分隔符，导致 `$group` 始终等于 `$controller`。SZ-Rust 在 `controller.rs` 第 374 行严格复刻此 bug（`let group = controller.clone();`）。

---

## 2. 环境准备

### 2.1 Rust 工具链安装

```bash
# 安装 rustup（Rust 官方工具链管理器）
# Windows: 访问 https://rustup.rs 下载 rustup-init.exe
# Linux/macOS: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 安装 stable 工具链（≥ 1.75，需要 async fn in trait 支持）
rustup default stable
rustup update stable
```

### 2.2 项目依赖

SZ-Rust 使用 Cargo workspace 组织，核心依赖通过 path 引用 SZ-ORM 全家桶。在业务项目的 `Cargo.toml` 中添加：

```toml
[dependencies]
sz-rust-core = { path = "../sz-rust/packages/sz-rust-core" }
sz-orm-core  = { path = "../sz-orm/packages/sz-orm-core" }

# 异步运行时
tokio = { version = "1", features = ["full"] }
axum  = "0.8"

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
serde_yaml = "0.9"
```

### 2.3 构建命令

```bash
# 编译检查
cargo check --workspace --all-targets

# 运行测试
cargo test --workspace

# clippy 代码审查（必须 0 警告）
cargo clippy --workspace --all-targets -- -D warnings

# 格式检查（必须 0 差异）
cargo fmt --all -- --check

# 运行 Hello World 示例
cargo run -p sz-rust-examples --bin quick_start
```

详见 `CONTRIBUTING.md` 第 13-29 行 "快速开始"。

---

## 3. 核心概念映射表

| PHP (ThinkPHP 8) | Rust (SZ-Rust) | 说明 |
|------------------|----------------|------|
| `app\SzController` | `sz_rust_core::controller::SzController` trait | 控制器基类 trait |
| `app\BaseController` | `sz_rust_core::controller::BaseController` trait | 基础控制器 trait |
| `addons\BaseController` | `sz_rust_core::controller::AddonsBaseController` trait | 插件控制器 trait |
| `think\Model` | `sz_rust_core::model::BaseModel` trait | Model 抽象（Data Mapper 模式） |
| `$this->request->post()` | `sz_rust_core::request::fetch_post_data()` | 请求参数获取 |
| `$this->request->param()` | `SzController::post_data()` | 合并参数（body + query） |
| `renderJson/renderSuccess/renderError` | `sz_rust_core::response::ApiResponse` | JSON 响应 |
| `BaseException` | `sz_rust_core::error::BaseException` | 异常处理 |
| `app/middleware.php` | `sz_rust_core::middleware::DEFAULT_ORDER` | 中间件顺序 |
| `config/app.php` | `config/app.yml` (serde YAML) | 配置文件 |
| `think\facade\Cache` | `sz_rust_core::cache::Cache` facade | 缓存 |
| `think\Event` | `sz_rust_core::event::EventDispatcher` | 事件系统 |
| `think\Validate` | `sz_rust_core::validate::Validate` | 数据验证 |
| `addons/` | `sz-rust-addons-loader` (Cargo feature) | 插件机制 |
| `think-logger` | `sz_rust_core::log::LogFacade` | 日志 |
| think-orm Model 钩子 | `sz_rust_core::hooks::HookDispatcher` | 16 事件钩子 |
| `compact()` | `sz_rust_core::macros::compact!` | 宏（Phase 2） |
| `app()` 容器 | `sz_rust_core::container::App` | 应用容器 |

完整对标表见 `README.md` 第 89-116 行 "与 ThinkPHP 8 对标表"。

---

## 4. 控制器迁移

### 4.1 控制器继承链

PHP 继承链与 Rust trait 继承链对齐（见 `controller.rs` 模块文档）：

```
PHP:    BaseController → SzController → AddonsBaseController → 业务控制器
Rust:   BaseController: SzController  →  AddonsBaseController: BaseController  →  业务控制器
```

### 4.2 PHP 控制器示例 → Rust 控制器示例

**PHP 端**（ThinkPHP 8）：

```php
namespace app\oapc\controller;

use app\SzController;
use think\facade\Db;

class Customer extends SzController
{
    public function info()
    {
        $data = $this->postData();
        $customer_id = $data['customer_id'] ?? 0;
        $customer = Db::table('customer')->where('customer_id', $customer_id)->find();
        if (empty($customer)) {
            return $this->renderError('未找到客户信息');
        }
        return $this->renderSuccess('success', $customer);
    }
}
```

**Rust 端**（SZ-Rust）：

```rust
use sz_rust_core::controller::SzController;
use sz_rust_core::response::ApiResponse;
use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;

struct CustomerController;

impl SzController for CustomerController {}

impl CustomerController {
    pub async fn info(&self, req: Request<Body>) -> Response {
        // postData() 合并 body + query，对齐 PHP $this->request->param()
        let data = self.post_data(req).await.unwrap();
        let customer_id = data
            .get("customer_id")
            .cloned()
            .unwrap_or(json!(0));

        // 业务逻辑：查询客户（持久化由 Repository 提供，见第 5 节）
        let customer = json!({"customer_id": customer_id, "name": "示例客户"});

        // renderSuccess 参数顺序：msg, data（与 PHP 一致）
        self.render_success("success", customer)
    }
}
```

### 4.3 关键差异

| 差异点 | PHP | Rust | 说明 |
|--------|-----|------|------|
| 控制器方法参数 | 无参数，从 `$this->request` 获取 | 无参数，从 `req: Request<Body>` 获取 | 主键从 `$data` 获取（R5 约束 2） |
| GET 请求分支 | 允许 `if ($this->request->isGet())` | 禁止使用 | R5 约束 3 |
| Request 传递 | `$this->request` 内部状态 | 方法参数传入 | Rust 控制器无状态（见 `controller.rs` 第 22-24 行） |
| 返回格式 | `{code, msg, data}` | `{code, msg, data}` | 字段顺序严格一致（R5 约束 4） |
| `renderSuccess` 参数顺序 | `($msg, $data)` | `(msg, data)` | 与 PHP 一致；注意 `ApiResponse::success(data, msg)` 顺序相反（见 `controller.rs` 第 18-19 行） |

### 4.4 SzController trait 方法清单

以下方法均来自 `controller.rs` 第 69-169 行（`SzController` trait 定义）：

| Rust 方法 | PHP 等价 | 签名 |
|-----------|---------|------|
| `render_json(&self, code: i32, msg: impl Into<String>, data: Value) -> Value` | `renderJson($code, $msg, $data)` | 返回 `Value::Object`，字段顺序 code→msg→data |
| `render_success(&self, msg: impl Into<String>, data: Value) -> Response` | `renderSuccess($msg, $data)` | code=1，HTTP 200 |
| `render_error(&self, msg: impl Into<String>, data: Value, code: i32) -> Response` | `renderError($msg, $data, $code)` | code 默认 0，HTTP 200 |
| `post_data(&self, req: Request<Body>) -> impl Future<Output = Result<Value, String>>` | `postData()` | 合并 body + query |
| `post_data_by_key(&self, req: Request<Body>, key: &str) -> impl Future<Output = Result<Option<Value>, String>>` | `postData($key)` | 获取指定 key |
| `get_data(&self, req: &Request<Body>) -> Value` | `getData()` | query 参数 |
| `get_data_by_key(&self, req: &Request<Body>, key: &str) -> Option<Value>` | `getData($key)` | 获取指定 query key |

### 4.5 BaseController trait 扩展方法

`BaseController: SzController`（见 `controller.rs` 第 192-261 行）：

| Rust 方法 | PHP 等价 | 默认实现 |
|-----------|---------|---------|
| `batch_validate(&self) -> bool` | `protected bool $batchValidate = false` | 返回 `false` |
| `middlewares(&self) -> Vec<String>` | `protected array $middleware = []` | 返回空 Vec |
| `initialize(&self)` | `protected function initialize() {}` | 空实现 |
| `validate(&self, data, rules, messages) -> Result<(), String>` | `protected function validate(...)` | 占位实现（Phase 5 完整实现） |

---

## 5. Model 迁移

### 5.1 架构差异：Active Record vs Data Mapper

这是 PHP → Rust 迁移中**最重要的架构差异**（见 `model.rs` 第 22-31 行 "架构决策"）：

| 模式 | PHP `think\Model` | Rust `BaseModel` + Repository |
|------|-------------------|------------------------------|
| 范式 | Active Record | Data Mapper |
| 数据载体 | Model 既是数据也是行为 | `BaseModel` trait 只描述元数据 |
| 持久化 | `save()` / `delete()` 在 Model 内 | 由 `sz_orm_core::repository::Repository` 提供 |
| 事务 | `startTrans()` / `commit()` / `rollback()` 在 Model 内 | 由 Repository 提供 |

### 5.2 PHP Model 示例 → Rust Model 示例

**PHP 端**：

```php
namespace app\common\model\szoa;

use think\Model;

class Customer extends Model
{
    protected $name = 'customer';
    protected $pk = 'customer_id';
    protected $append = ['status_text'];

    public function getStatusTextAttr($value, $data)
    {
        $status = $data['status'] ?? 0;
        return [0 => '禁用', 1 => '启用'][$status] ?? '未知';
    }
}
```

**Rust 端**：

```rust
use sz_rust_core::model::BaseModel;
use serde_json::{json, Value};

struct Customer {
    customer_id: i64,
    status: i32,
}

impl BaseModel for Customer {
    // 对齐 PHP $append = ['status_text']
    fn append() -> Vec<&'static str> {
        vec!["status_text"]
    }

    // 对齐 PHP getStatusTextAttr($value, $data)
    fn get_appended_value(&self, field: &str) -> Option<Value> {
        match field {
            "status_text" => Some(json!(match self.status {
                0 => "禁用",
                1 => "启用",
                _ => "未知",
            })),
            _ => None,
        }
    }
}
```

### 5.3 BaseModel 元数据属性映射

`BaseModel` 通过组合 SZ-ORM trait 获得元数据能力（见 `model.rs` 第 76-91 行）：

| PHP 属性 | Rust 等价 | 来源 trait |
|----------|----------|-----------|
| `$name` | `Model::table_name()` | SZ-ORM `Model` |
| `$pk` | `Model::pk_name()` + `Model::pk()` | SZ-ORM `Model` |
| `$field` / `$fillable` | `ModelExt::fillable()` | SZ-ORM `ModelExt` |
| `$disuse` / `$guarded` | `ModelExt::guarded()` | SZ-ORM `ModelExt` |
| `$hidden` | `ModelExt::hidden()` | SZ-ORM `ModelExt` |
| `$visible` | `ModelExt::visible()` | SZ-ORM `ModelExt` |
| `$type` | `ModelExt::casts()` | SZ-ORM `ModelExt` |
| `$append` | `BaseModel::append()` | SZ-Rust 扩展 |

### 5.4 关联关系映射

PHP `think\Model` 4 种基础关联 + 2 种多态关联，对齐 SZ-ORM `Relation` 枚举（见 `relation/mod.rs` 第 19-30 行）：

| PHP 方法 | Rust 等价 | 说明 |
|---------|----------|------|
| `hasMany($model, $foreignKey, $localKey)` | `Relation::HasMany(HasMany)` | 一对多 |
| `belongsTo($model, $foreignKey, $localKey)` | `Relation::BelongsTo(BelongsTo)` | 多对一 |
| `hasOne($model, $foreignKey, $localKey)` | `Relation::HasOne(HasOne)` | 一对一 |
| `belongsToMany($model, $table, $foreignKey, $localKey)` | `Relation::BelongsToMany(BelongsToMany)` | 多对多 |
| `morphMany($model, $name, $type)` | `Relation::MorphMany(MorphMany)` | 多态一对多 |
| `morphTo($name, $type, $id)` | `Relation::MorphTo(MorphTo)` | 多态反向 |

**PHP 命名约定**（SZ-Rust 提供辅助函数对齐）：
- `hasMany` 默认外键：`Str::snake(class_name) . '_id'`（如 `User` → `user_id`）
- `belongsTo` 默认外键：`Str::snake(relation_name) . '_id'`（关联名而非类名）
- `belongsToMany` 默认中间表：字母序 `snake_case(a) + '_' + snake_case(b)`

### 5.5 访问器 / 修改器

对齐 PHP `getAttr` / `setAttr` / `getXxxAttr` / `setXxxAttr`（见 `model.rs` 第 157-290 行）：

| PHP 方法 | Rust 等价 | trait |
|----------|----------|-------|
| `getAttr($name)` | `Accessor::get_attr(name)` | `Accessor` |
| `getData($name)` | `Accessor::get_data(name)` | `Accessor`（取原始值，不触发访问器） |
| `getXxxAttr($value, $data)` | `Accessor::accessor_for(field, value)` | `Accessor` |
| `setAttr($name, $value, $data)` | `Mutator::set_attr(name, value)` | `Mutator` |
| `setXxxAttr($value, $data)` | `Mutator::mutator_for(field, value)` | `Mutator` |

**缓存机制**（PHP bug 复刻）：
- 首次 `get_attr(field)` 触发访问器，结果缓存
- 同名字段被 `set_attr` 时失效对应缓存
- **PHP bug 复刻**：不同名字段修改不失效派生字段缓存（如 `set_attr("status", ...)` 不失效 `status_text` 缓存）

### 5.6 Model 钩子（16 事件）

对齐 PHP think-orm 2.0.x 钩子机制，扩展 4 个事件（见 `hooks.rs` 第 1-34 行）：

| 操作 | PHP 触发顺序 | Rust 触发顺序 |
|------|-------------|--------------|
| INSERT | `before_write` → `before_insert` → (INSERT) → `after_insert` → `after_write` | `before_write` → `before_save` → `before_validate` → `validate` → `after_validate` → `before_insert` → (INSERT) → `after_insert` → `after_save` → `after_write` |
| UPDATE | `before_write` → `before_update` → (UPDATE) → `after_update` → `after_write` | 同上，`before_update` / `after_update` 替换 insert |
| DELETE | `before_delete` → (DELETE) → `after_delete` | 同 PHP |
| RESTORE | `before_restore` → (UPDATE deleted_at=NULL) → `after_restore` | 同 PHP |
| FIND | `before_find` → (SELECT) → `after_find` | 同 PHP |

**SZ-ORM 扩展 4 事件**（借鉴 Rails/ActiveRecord 命名）：
- `BeforeSave` / `AfterSave`：与 write 等价
- `BeforeValidate` / `AfterValidate`：数据验证前后触发

---

## 6. 中间件迁移

### 6.1 PHP `app/middleware.php` → Rust `DEFAULT_ORDER`

PHP 端全局中间件（见 `middleware/order.rs` 第 1-10 行）：

```php
// app/middleware.php
return [
    \think\middleware\SessionInit::class,
    \think\middleware\AllowCrossDomain::class,
];
```

Rust 端默认中间件顺序（见 `middleware/order.rs` 第 108-114 行）：

```rust
pub const DEFAULT_ORDER: &[MiddlewareKind] = &[
    MiddlewareKind::Trace,      // 1. 追踪（对齐 SessionInit）
    MiddlewareKind::Cors,       // 2. CORS（对齐 AllowCrossDomain）
    MiddlewareKind::Log,        // 3. 日志（sz-rust 自研）
    MiddlewareKind::RateLimit,  // 4. 限流（sz-rust 自研）
    MiddlewareKind::Auth,       // 5. 鉴权（对齐 app\<app>\middleware\Auth）
];
```

### 6.2 5 个中间件顺序说明

| 顺序 | Rust 中间件 | PHP 对应 | 职责 | 设计理由 |
|------|------------|---------|------|---------|
| 1 | `Trace` | `SessionInit` | 生成 request_id | 最先执行，确保 request_id 在所有日志可用 |
| 2 | `Cors` | `AllowCrossDomain` | 跨域预处理 | OPTIONS 预检直接返回，不消耗后续资源 |
| 3 | `Log` | （PHP 端无全局） | 请求/响应日志 | 记录所有请求（含被拒绝的），用于审计 |
| 4 | `RateLimit` | （PHP 端无全局） | 限流 | 鉴权之前限流，避免无效请求消耗鉴权开销 |
| 5 | `Auth` | `app\<app>\middleware\Auth` | JWT 鉴权 | 通过限流后鉴权，未登录返回 NotLogin(-1) |

### 6.3 PHP 全局中间件前缀

PHP 全局中间件顺序是 `DEFAULT_ORDER` 的前缀（见 `middleware/order.rs` 第 124 行）：

```rust
pub const PHP_GLOBAL_ORDER: &[MiddlewareKind] = &[MiddlewareKind::Trace, MiddlewareKind::Cors];
```

业务层中间件（Log/RateLimit/Auth）在全局之后追加。

---

## 7. 错误码映射

### 7.1 ErrorCode 枚举

对齐 PHP `BaseException` 的 code 字段（见 `error.rs` 第 28-50 行）：

| PHP code | 含义 | Rust `ErrorCode` | HTTP 状态码 | PHP 使用场景 |
|----------|------|------------------|------------|-------------|
| `1` | 成功 | `Success` | 200 | `renderSuccess` 默认 |
| `0` | 失败 | `Failed` | 200 | `renderError` 默认 / `BaseException` 默认 |
| `-1` | 未登录/参数错误 | `NotLogin` | 401 | `not_login` / `缺少必要的参数` / `密钥不准确` |
| `-2` | 用户不存在/未绑定 | `UserNotFound` | 401 | `没有找到用户信息` / `请先绑定,员工信息` |
| `-3` | 用户已禁用/已离职 | `UserDisabled` | 403 | `员工信息待审核` / `您已离职` |
| `403` | 无权限 | `Forbidden` | 403 | （Rust 扩展） |
| `404` | 资源不存在 | `NotFound` | 404 | （Rust 扩展） |
| `422` | 验证失败 | `ValidateFailed` | 422 | （Rust 扩展） |
| `500` | 数据库错误 | `DbError` | 500 | （Rust 扩展） |

### 7.2 BaseException 使用

**PHP 端**：

```php
throw new BaseException(['code' => -1, 'msg' => 'not_login']);
throw new BaseException(['msg' => '没有找到用户信息', 'code' => -2]);
```

**Rust 端**（见 `error.rs` 第 113-160 行）：

```rust
use sz_rust_core::error::{BaseException, ErrorCode};

// 通用构造
let e = BaseException::new(ErrorCode::NotLogin, "not_login");

// 快捷构造（对齐 PHP 常用场景）
let e = BaseException::not_login("not_login");           // code = -1
let e = BaseException::user_not_found("没有找到用户信息"); // code = -2
let e = BaseException::user_disabled("您已离职");         // code = -3
let e = BaseException::failed("操作失败");                // code = 0
let e = BaseException::forbidden("无权限");               // code = 403
let e = BaseException::not_found("资源不存在");           // code = 404
let e = BaseException::validate_failed("参数错误");       // code = 422
let e = BaseException::db_error("数据库错误");            // code = 500

// 转为 JSON 响应
let json = e.to_json();
// {"code": -1, "msg": "not_login", "data": {}}
```

### 7.3 JSON 响应格式

所有错误响应统一格式（见 `error.rs` 第 20-23 行）：

```json
{ "code": <code>, "msg": "<msg>", "data": {} }
```

---

## 8. 配置迁移

### 8.1 PHP `config/app.php` → Rust `config/app.yml`

PHP 配置使用 PHP 数组，Rust 使用 YAML + serde 反序列化（见 `config.rs` 第 1-15 行）。

**PHP 端**（`config/app.php`）：

```php
return [
    'app_host'        => '',
    'app_namespace'   => '',
    'with_route'      => true,
    'with_event'      => true,
    'default_app'     => 'index',
    'default_timezone' => 'Asia/Shanghai',
    'auto_multi_app'  => true,
    'app_map'         => ['oapc' => 'oapc', 'admin' => 'admin'],
    'deny_app_list'   => ['common'],
];
```

**Rust 端**（`config/app.yml`，见实际文件）：

```yaml
app_host: ""
app_namespace: ""
with_route: true
with_event: true
default_app: "index"
default_timezone: "Asia/Shanghai"
auto_multi_app: true
app_map:
  oapc: oapc
  admin: admin
  api: api
deny_app_list:
  - common
```

### 8.2 配置结构

`AppConfig` 含 5 个 section（见 `config.rs` 第 47-64 行）：

| 配置段 | 对齐 PHP | 说明 |
|--------|---------|------|
| `app` | `config/app.php` | 应用配置 |
| `database` | `config/database.php` | 数据库配置 |
| `cache` | `config/cache.php` | 缓存配置 |
| `addons` | `config/addons.php` | 插件配置 |
| `log` | `config/log.php` | 日志配置 |

所有配置项都有默认值（通过 serde `#[serde(default)]`），即使 YAML 文件缺失也能正常加载。

### 8.3 环境变量覆盖

支持两种环境变量覆盖格式（见 `config.rs` 第 6-12 行）：

| 格式 | 示例 | 说明 |
|------|------|------|
| `SZ_{SECTION}__{KEY}` | `SZ_APP__DEFAULT_APP=api` | 标准格式，双下划线分隔层级 |
| `SZ_DB_{CONN}_PASSWORD` | `SZ_DB_MYSQL_PASSWORD=xxx` | 数据库密码简写格式 |

### 8.4 加载配置

```rust
use sz_rust_core::config::AppConfig;

let config_dir = std::path::PathBuf::from("config");
let config = AppConfig::load_from_dir(&config_dir)
    .unwrap_or_else(|_| AppConfig::default());
```

---

## 9. 路由迁移

### 9.1 三层路由机制

对齐 PHP `think-route` + `config/route.php` + `auto_multi_app`（见 `routing.rs` 第 1-18 行）：

| 层级 | 机制 | PHP 对齐 | 启用方式 | 适用场景 |
|------|------|---------|---------|---------|
| Layer 1 | 属性宏路由 | `#[Route]` 注解 | `#[controller]` + `#[get]` | 控制器内嵌路由声明 |
| Layer 2 | 配置式路由 | `config/route.php` | YAML/JSON 配置文件 | 路由与代码解耦（推荐生产） |
| Layer 3 | 约定式路由 | `auto_multi_app` + `app/controller/action` | `parse_path` 自动映射 | 快速原型 / 内部 API |

**设计原则**：
1. 三层独立，可单独或组合使用
2. 优先级递减：Layer 1 > Layer 2 > Layer 3（前层覆盖后层）
3. 类型安全：Layer 1 编译期检查；Layer 2/3 加载期检查
4. 渐进迁移：从 Layer 3 起步，逐步迁移到 Layer 2/1

### 9.2 Layer 2 — 配置式路由（推荐生产使用）

**PHP 端**（`config/route.php`）：

```php
use think\facade\Route;

Route::get('users', 'User/list');
Route::post('users', 'User/create');
```

**Rust 端**（YAML 配置，见 `routing.rs` 第 22-39 行）：

```yaml
# config/route.yml
routes:
  - method: GET
    path: /users
    handler: User@list
  - method: POST
    path: /users
    handler: User@create
```

```rust
use sz_rust_core::routing::load_routes_from_yaml_str;

let yaml = std::fs::read_to_string("config/route.yml")?;
let config = load_routes_from_yaml_str(&yaml)?;
// config.routes 包含解析后的路由规则
```

### 9.3 Layer 3 — 约定式路由

对齐 PHP `auto_multi_app`，按 URI 前缀自动映射（见 `routing.rs` 第 41-50 行）：

```rust
use sz_rust_core::router::parse_path;

let p = parse_path("/oapc/customer/index");
assert_eq!(p.app, "oapc");
assert_eq!(p.controller, "Customer");
assert_eq!(p.action, "index");
```

### 9.4 HttpMethod 枚举

对齐 PHP `think\Route::$method`，支持 RESTful 5 大方法 + OPTIONS（见 `routing.rs` 第 65-80 行）：

```rust
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    OPTIONS,
}
```

---

## 10. 常见陷阱

### 10.1 PHP 动态类型 vs Rust 静态类型

| 陷阱 | PHP | Rust 解决方案 |
|------|-----|--------------|
| 变量类型随时变化 | `$x = 1; $x = "a";` 合法 | 编译期错误，需显式声明类型 |
| 函数返回类型不确定 | 可返回 int 或 string | 使用 `Result<T, E>` 或枚举 |
| 数组元素异构 | `[1, "a", true]` 合法 | 使用 `Vec<Value>` 或 enum |

**迁移建议**：使用 `serde_json::Value` 处理异构数据（对齐 PHP 关联数组），业务逻辑使用强类型 struct。

### 10.2 PHP 关联数组 vs Rust struct / serde_json::Value

| 场景 | PHP | Rust |
|------|-----|------|
| 已知结构的数据 | 关联数组 `$data['name']` | `struct` + `serde` 反序列化 |
| 未知结构的数据 | 关联数组 | `serde_json::Value`（`json!` 宏） |
| 字段顺序 | PHP 数组保持插入顺序 | `serde_json` 启用 `preserve_order` feature |

```rust
use serde_json::json;

// 对齐 PHP compact('code', 'msg', 'data')
let resp = json!({
    "code": 1,
    "msg": "success",
    "data": {"id": 1}
});
```

### 10.3 PHP 异常处理 vs Rust Result / `?`

| 陷阱 | PHP | Rust |
|------|-----|------|
| 异常控制流 | `try/catch/throw` | `Result<T, E>` + `?` 操作符 |
| 异常可忽略 | catch 后可忽略 | `Result` 必须处理（编译期强制） |
| 异常类型 | 所有异常继承 `Exception` | 自定义 error 类型 + `thiserror` |

**PHP 端**：

```php
try {
    $user = User::find($id);
    if (!$user) {
        throw new BaseException(['msg' => '用户不存在', 'code' => -2]);
    }
} catch (BaseException $e) {
    return json(['code' => $e->code, 'msg' => $e->message, 'data' => []]);
}
```

**Rust 端**：

```rust
use sz_rust_core::error::{BaseException, ErrorCode};

fn find_user(id: i64) -> Result<User, BaseException> {
    let user = repository.find(id)
        .map_err(|_| BaseException::user_not_found("用户不存在"))?;
    Ok(user)
}

// 调用处使用 ? 传播错误
let user = find_user(id)?; // 错误自动向上传播
```

### 10.4 PHP `null` vs Rust `Option<T>`

| 陷阱 | PHP | Rust |
|------|-----|------|
| null 检查 | `is_null($x)` / `$x === null` | `Option::is_none()` / `match` |
| null 传播 | `$user?->name`（PHP 8） | `user.and_then(\|u\| u.name)` |
| 默认值 | `$x ?? 'default'` | `option.unwrap_or("default")` |
| null 算术 | `$x + null` = `$x` | 编译期错误，需显式处理 |

**迁移建议**：PHP 的 `null` 在 Rust 中映射为 `Option<T>`。数据库字段可为 null 时，使用 `Option<i64>` / `Option<String>`。

```rust
// PHP: $data['customer_id'] ?? 0
let customer_id = data.get("customer_id")
    .cloned()
    .unwrap_or(json!(0));
```

### 10.5 PHP `$this->request` 状态 vs Rust 无状态控制器

PHP 控制器通过 `$this->request` 在内部访问当前请求；Rust 控制器无状态，`Request` 作为方法参数传入（见 `controller.rs` 第 22-24 行）。

**影响**：PHP `initialize()` 在构造函数中自动调用；Rust handler 需显式调用初始化方法。

### 10.6 append 字段绕过 hidden 过滤（PHP bug 复刻）

PHP `appendAttrToArray` 直接赋值，不检查 `$hidden`（见 `model.rs` 第 136-137 行）。SZ-Rust 严格复刻：

```rust
fn to_json_with_append(&self) -> Value {
    let mut json = self.to_json();
    if let Value::Object(ref mut map) = json {
        for field in Self::append() {
            // PHP 行为：append 字段始终输出（None → null）
            let value = self.get_appended_value(field).unwrap_or(Value::Null);
            map.insert(field.to_string(), value); // 绕过 hidden 过滤
        }
    }
    json
}
```

---

## 11. 缓存系统迁移（Phase 6）

### 11.1 PHP `think\facade\Cache` → Rust `sz_rust_core::cache::Cache`

PHP 缓存 facade 对齐 Rust Cache facade（见 `cache.rs` 模块文档）：

| PHP 方法 | Rust 方法 | 说明 |
|---------|----------|------|
| `Cache::set($name, $value, $ttl)` | `Cache::set(key, value, ttl)` | 写入缓存（可选 TTL） |
| `Cache::get($name)` | `Cache::get::<T>(key)` | 读取缓存（泛型反序列化） |
| `Cache::delete($name)` | `Cache::delete(key)` | 删除缓存 |
| `Cache::has($name)` | `Cache::has(key)` | 判断缓存是否存在 |
| `Cache::inc($name, $step)` | `Cache::inc(key, step)` | 自增（不经序列化层） |
| `Cache::dec($name, $step)` | `Cache::dec(key, step)` | 自减（不经序列化层） |
| `Cache::pull($name)` | `Cache::pull::<T>(key)` | 读取并删除 |
| `Cache::push($name, $value)` | `Cache::push(key, value)` | 追加到数组（上限 1000 + 去重） |
| `Cache::remember($name, $ttl, $callback)` | `Cache::remember(key, ttl, callback)` | 缓存击穿防护（锁机制） |
| `Cache::clear()` | `Cache::clear()` | 清空所有缓存 |
| `Cache::store('redis')` | `Cache::with_store('redis')` | 切换存储驱动 |

### 11.2 缓存序列化策略（PHP bug 复刻）

对齐 PHP `think\cache\Driver::serialize` / `unserialize` 的弱类型行为（见 `cache.rs` 第 612-630 行）：

| PHP 行为 | Rust 实现 | 说明 |
|---------|----------|------|
| `is_numeric($data)` 短路返回 `(string) $data` | `php_serialize` 对数字直接返回 String | PHP bug：serialize 对数字不经 serialize() |
| `unserialize` 对 numeric 返回 string | `php_unserialize` 对 Number 返回 String | PHP bug：unserialize 返回类型与存储类型不一致 |
| `remember` 锁 key 无 TTL | `Cache::set(lock_key, 1, None)` | PHP bug：锁无过期时间，进程崩溃导致死锁 |
| `remember` `has()` + `get()` 双查 TOCTOU | `has` + `get_weak` 双查 | PHP bug：has 和 get 之间缓存可能过期 |

### 11.3 缓存使用示例

**PHP 端**：
```php
use think\facade\Cache;

// 写入缓存 60 秒
Cache::set('user_123', ['name' => '张三'], 60);

// 读取缓存
$user = Cache::get('user_123');

// remember 模式（缓存击穿防护）
$data = Cache::remember('expensive_query', 300, function () {
    return Db::table('orders')->count();
});
```

**Rust 端**：
```rust
use sz_rust_core::cache::Cache;
use serde_json::json;

let cache = Cache::new();

// 写入缓存 60 秒
cache.set("user_123", json!({"name": "张三"}), Some(60))?;

// 读取缓存
let user: serde_json::Value = cache.get("user_123")?;

// remember 模式（缓存击穿防护）
let data: i64 = cache.remember("expensive_query", 300, || {
    Box::pin(async { Ok(42) })
}).await?;
```

---

## 12. 文件上传迁移（Phase 5）

### 12.1 PHP `think\File` → Rust `sz_rust_core::upload`

PHP 文件上传对齐 Rust upload 模块（见 `upload.rs` 模块文档）：

| PHP 类 | Rust 类型 | 说明 |
|--------|----------|------|
| `think\File` | `sz_rust_core::upload::File` | 文件抽象 |
| `think\file\UploadedFile` | `sz_rust_core::upload::UploadedFile` | 上传文件 |
| `think\Filesystem` | `sz_rust_core::upload::storage::*` | 存储引擎 |

### 12.2 5 种存储引擎

| 引擎 | PHP 对应 | Rust 实现 |
|------|---------|----------|
| Local | `think\filesystem\driver\Local` | `storage::Local` |
| 阿里云 OSS | `think\filesystem\driver\Oss` | `storage::Oss` |
| 腾讯云 COS | `think\filesystem\driver\Cos` | `storage::Cos` |
| 七牛 Kodo | `think\filesystem\driver\Qiniu` | `storage::Qiniu` |
| S3 兼容 | （PHP 无原生） | `storage::S3` |

### 12.3 文件名生成规则（PHP 对齐）

对齐 PHP `think\Filesystem::buildSaveName`（见 `upload.rs` 文档注释）：

| PHP 方法 | Rust 方法 | 规则 |
|---------|----------|------|
| `buildSaveName` | `build_save_name` | uniqid + ext |
| `hashName('md5')` | `hash_name_md5` | md5(content) + ext |
| `hashName('sha1')` | `hash_name_sha1` | sha1(content) + ext |
| `hashName()` | `hash_name` | md5(uniqid) 前 2 位做子目录 |

### 12.4 图像处理（对齐 PHP Grafika）

| PHP Grafika | Rust 实现 | 说明 |
|-------------|----------|------|
| `Grafika\Gd\Editor::resize` | `image::resize` | 缩放 |
| `Grafika\Gd\Editor::crop` | `image::crop` | 裁剪 |
| `Grafika\Gd\Editor::text` | `image::text` | 文字水印 |
| `Grafika\Gd\Editor::blend` | `image::blend` | 图片水印 |

---

## 13. 视图模板迁移（Phase 7）

### 13.1 PHP `think\View` → Rust `sz_rust_core::view`

PHP 模板引擎对齐 Rust view 模块（见 `view.rs` 模块文档）：

| PHP 方法 | Rust 方法 | 说明 |
|---------|----------|------|
| `View::fetch($template, $data)` | `View::fetch(template, data)` | 渲染模板 |
| `View::display($content, $data)` | `View::display(content, data)` | 渲染字符串 |
| `View::assign($name, $value)` | `View::assign(name, value)` | 模板变量赋值 |
| `View::config('layout_on', true)` | `ViewConfig::layout_on = true` | 开启布局 |

### 13.2 模板标签（PHP 对齐）

| PHP 标签 | Rust 支持 | 说明 |
|---------|----------|------|
| `{$var}` | ✅ | 变量输出 |
| `{include file="header"}` | ✅ | 包含文件 |
| `{layout name="layout" /}` | ✅ | 布局继承 |
| `{block name="content"}{/block}` | ✅ | 块定义 |
| `{extend name="base" /}` | ✅ | 模板继承 |
| `{volist name="list" id="v"}{/volist}` | ✅ | 循环 |
| `{if condition="..."}{else /}{/if}` | ✅ | 条件 |

### 13.3 布局继承（PHP 双阶段）

对齐 PHP `compiler()` + `parseLayout()` 双阶段布局（见 `view/layout.rs` 文档）：

1. **配置模式布局**：`layout_on=true` + `layout_name="layout"` → 自动应用布局
2. **标签模式布局**：`{layout name="custom" /}` → 在模板中声明布局

**PHP bug 复刻**：`{layout name="custom" replace="{__BODY__}"}` 中 `}` 提前终止标签匹配，导致 `replace` 属性无法解析，回退到 `layout_item`（`{__CONTENT__}`）。

---

## 14. 可观测性迁移（v0.2.0 新增）

### 14.1 PHP 无原生对应 → Rust `sz_rust_observability`

PHP 端无原生 metrics/SLO 能力（依赖 Prometheus client_php 第三方库）。SZ-Rust 提供 `sz-rust-observability` 包：

| 能力 | Rust API | 说明 |
|------|---------|------|
| 指标注册 | `MetricsRegistry::new()` | 创建指标注册中心 |
| Counter | `registry.register_counter(name, help)` | 单调递增计数器 |
| Gauge | `registry.register_gauge(name, help)` | 可增可减瞬时值 |
| Histogram | `registry.register_histogram(name, help, buckets)` | 分桶统计 |
| Prometheus 输出 | `registry.render()` | 文本格式导出 |
| SLO 燃烧率 | `slo::SloMonitor` | 4 窗口多燃烧率告警 |

### 14.2 SLO 多窗口燃烧率（Google SRE 对齐）

对齐 Google SRE Workbook 第 5 章：

| 窗口对 | 长窗口 | 短窗口 | 燃烧率阈值 | 告警级别 |
|--------|--------|--------|-----------|---------|
| 第 1 对 | 1h | 5m | 14.4 | Page |
| 第 2 对 | 6h | 30m | 6.0 | Ticket |

### 14.3 使用示例

```rust
use sz_rust_observability::{MetricsRegistry, MetricType};

let registry = MetricsRegistry::new();
let counter = registry.register_counter("sz_rust_requests_total", "Total requests");
counter.inc();

// Prometheus /metrics 端点输出
let output = registry.render();
```

---

## 15. 分布式追踪迁移（v0.2.0 新增）

### 15.1 PHP 无原生对应 → Rust `sz_rust_tracing`

PHP 端无原生分布式追踪（依赖 OpenTelemetry PHP 扩展）。SZ-Rust 提供 `sz-rust-tracing` 包：

| 能力 | Rust API | 说明 |
|------|---------|------|
| Span 创建 | `Tracer::start_span(name)` | 创建追踪片段 |
| Span 结束 | `Tracer::end_span(span)` | 结束追踪片段 |
| 上下文注入 | `Tracer::inject_context(headers)` | W3C traceparent |
| 上下文提取 | `Tracer::extract_context(headers)` | 解析 traceparent |
| OTLP 导出 | `OtlpExporter`（feature="otlp"） | 对接 OTel Collector |

### 15.2 W3C TraceContext 格式

```
traceparent: 00-<trace_id>-<span_id>-<flags>
```

| 字段 | 长度 | 说明 |
|------|------|------|
| version | 2 字符 | 固定 `00` |
| trace_id | 32 字符 | 16 字节 trace ID |
| span_id | 16 字符 | 8 字节 span ID |
| flags | 2 字符 | `01` 表示 sampled |

### 15.3 向后兼容 legacy header

对齐旧版客户端（如 PHP 端历史调用方）：

- **入站**：优先解析 `traceparent`，若缺失则解析 `trace-id` / `span-id`
- **出站**：始终发送 W3C `traceparent`，同时发送 `trace-id` / `span-id`

### 15.4 使用示例

```rust
use sz_rust_tracing::{SzTracer, Tracer};

let tracer = SzTracer::new("my-service");
let mut span = tracer.start_span("GET /api/users");
// ... 业务逻辑 ...
tracer.end_span(&mut span);
```

---

## 附录：迁移检查清单

迁移一个 PHP 控制器时，逐项检查：

- [ ] 控制器实现 `SzController` trait
- [ ] 控制器方法无参数，从 `req: Request<Body>` 获取请求
- [ ] 主键从 `post_data(req).await` 获取（不从 URL 参数获取）
- [ ] 不使用 GET 请求分支（`if is_get()`）
- [ ] 返回 `render_success(msg, data)` 或 `render_error(msg, data, code)`
- [ ] 响应字段顺序：`code → msg → data`
- [ ] 错误码使用 `ErrorCode` 枚举或对应整数值
- [ ] Model 实现 `BaseModel` trait
- [ ] 访问器实现 `Accessor` trait（如有 `getXxxAttr`）
- [ ] 修改器实现 `Mutator` trait（如有 `setXxxAttr`）
- [ ] PHP bug 复刻有注释说明

---

## 参考

- [README.md](../README.md) — 项目介绍和 ThinkPHP 8 对标表
- [CONTRIBUTING.md](../CONTRIBUTING.md) — PHP 迁移规范 5 条 + 工程化门禁
- [架构总览](sz-rust/architecture.md) — 模块划分、Phase 路线图
- [ADR 与生产 Bug 定位规范](ADR与生产Bug定位规范.md) — 决策记录与 bug 复刻规范
- 源码模块文档：`cargo doc -p sz-rust-core --open`
