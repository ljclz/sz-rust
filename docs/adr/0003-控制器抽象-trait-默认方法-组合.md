# ADR-003：控制器抽象（SzController trait + 默认方法 + 组合）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-001（路由策略）、ADR-008（错误处理）
> **相关代码**：`packages/sz-rust-core/src/controller.rs`、`packages/sz-rust-core/src/response.rs`、`packages/sz-rust-core/src/request.rs`

## 背景

PHP 端控制器继承链为 `BaseController → SzController → AddonsBaseController → 业务控制器`，通过 `$this->request` 在控制器内部访问当前请求。PHP 控制器提供以下核心方法：

| PHP 方法 | 签名 | 用途 |
|---------|------|------|
| `renderJson($code=1, $msg='', $data=[])` | 返回 `compact('code','msg','data')` 数组 | 封装 API 响应 |
| `renderSuccess($msg='success', $data=[])` | `json(renderJson(1, $msg, $data))` | 成功响应 |
| `renderError($msg='error', $data=[], $code=0)` | `json(renderJson($code, $msg, $data))` | 失败响应 |
| `postData($key=null)` | `$this->request->param(...)` | 获取 POST 数据 |
| `getData($key=null)` | `$this->request->get(...)` | 获取 GET 数据 |

Rust 端需要解决以下问题：

1. **PHP 有 `$this->request`，Rust 控制器无状态**：Rust 控制器是 `Send + Sync` 的无状态结构体，请求作为参数传入
2. **PHP 参数顺序与 Rust 习惯相反**：PHP `renderSuccess($msg, $data)` 中 msg 在前，Rust `ApiResponse::success(data, msg)` 中 data 在前
3. **PHP `compact('code', 'msg', 'data')` 的字段顺序**：必须严格保持 `code → msg → data` 顺序
4. **async fn in trait**：Rust 1.75+ 支持原生 `async fn in trait`，但暂不支持 `dyn Trait`

## 决策

采用 **`SzController` trait + 默认方法 + 组合** 模式：

### trait 定义

```rust
pub trait SzController: Send + Sync {
    fn render_json(&self, code: i32, msg: impl Into<String>, data: Value) -> Value { ... }
    fn render_success(&self, msg: impl Into<String>, data: Value) -> Response { ... }
    fn render_error(&self, msg: impl Into<String>, data: Value, code: i32) -> Response { ... }
    async fn post_data(&self, req: Request<Body>) -> Result<Value, ...> { ... }
    async fn post_data_by_key(&self, req: Request<Body>, key: &str) -> Result<Value, ...> { ... }
    async fn get_data(&self, req: Request<Body>) -> Result<Value, ...> { ... }
    async fn get_data_by_key(&self, req: Request<Body>, key: &str) -> Result<Value, ...> { ... }
}
```

### 关键设计

1. **默认方法实现**：所有方法都有默认实现，业务控制器只需 `impl SzController for UserController {}` 即可获得全部方法
2. **PHP 参数顺序**：严格遵循 PHP 顺序（`msg` 在前），内部调用 `ApiResponse` 时调换参数
3. **`render_json` 返回 `Value::Object`**：而非 `Response`，便于调用方进一步处理
4. **`render_success` / `render_error` 返回 `Response`**：直接返回 axum Response，HTTP 200
5. **请求作为参数传入**：`async fn post_data(&self, req: Request<Body>)` 替代 PHP 的 `$this->request`
6. **字段顺序严格一致**：`render_json` 使用 `Map::new()` + `insert("code")` + `insert("msg")` + `insert("data")`，保证 `code → msg → data` 顺序

### PHP 行为对齐

```php
// PHP
protected function renderJson($code = 1, $msg = '', $data = []) {
    return compact('code', 'msg', 'data');  // 字段顺序: code → msg → data
}
```

```rust
// Rust
fn render_json(&self, code: i32, msg: impl Into<String>, data: Value) -> Value {
    let mut map = Map::new();
    map.insert("code".to_string(), Value::Number(code.into()));
    map.insert("msg".to_string(), Value::String(msg.into()));
    map.insert("data".to_string(), data);
    Value::Object(map)
}
```

### 业务失败 HTTP 仍 200

对齐 PHP 行为：`renderError` 返回 HTTP 200，业务错误通过 `code` 字段区分（`code=0` 表示失败）。

## 决策替代方案

### 方案 A：继承式 BaseController 结构体（拒绝）

```rust
// 业务控制器继承 BaseController 结构体
struct UserController {
    base: BaseController,
}
```

**拒绝原因**：
- Rust 没有类继承，结构体组合比继承更灵活
- 继承链 `$this->request` 在 Rust 中无直接等价物
- trait 默认方法已经能提供"继承"的代码复用，无需结构体嵌套

### 方案 B：每个控制器手动实现所有方法（拒绝）

每个业务控制器手写 `render_json` / `render_success` / `post_data` 等方法。

**拒绝原因**：
- 大量重复代码，每个控制器都要写 7-8 个相同方法
- 方法签名不一致风险高，不同控制器可能参数顺序不同
- 后续修改响应格式需要修改所有控制器

### 方案 C：宏生成控制器方法（拒绝）

```rust
// 通过宏自动生成控制器方法
#[derive_controller_methods]
struct UserController;
```

**拒绝原因**：
- 宏的调试体验差，展开后的代码难以阅读
- trait 默认方法已经足够简洁，宏是过度设计
- 宏生成的方法难以自定义（业务需要覆盖某个方法时需要特殊处理）

**最终选择**：`SzController` trait + 默认方法。业务控制器只需空 `impl SzController for UserController {}` 即可获得全部方法，需要自定义时覆盖特定方法即可。

## 后果

### 正面后果

- **PHP 迁移零摩擦**：方法名、参数顺序、返回格式完全对齐 PHP
- **零样板代码**：业务控制器只需空 `impl` 即可获得全部方法
- **字段顺序保证**：使用 `serde_json::Map`（`preserve_order` feature）保证 `code → msg → data` 顺序
- **类型安全**：所有方法都有明确的返回类型（`Value` / `Response`）
- **可测试**：`render_json` 返回 `Value`，便于断言

### 负面后果

- **不支持 `dyn SzController`**：Rust 1.75+ 的 `async fn in trait` 暂不支持 trait object。若未来需要动态分发，需改用 `#[async_trait]` 宏
- **请求作为参数**：每个需要请求数据的方法都必须传入 `req: Request<Body>`，比 PHP 的 `$this->request` 繁琐
- **`render_error` 的 `code` 参数在后**：PHP `renderError($msg, $data, $code)` 的 `code` 在最后，Rust 保持一致，但与 Rust 习惯（错误码在前）相反

## 注意事项

- **`serde_json` 的 `preserve_order` feature**：必须在 `Cargo.toml` 中启用 `serde_json = { version = "1", features = ["preserve_order"] }`，否则 `Map` 会使用 BTreeMap，字段顺序变为字母序
- **`ApiResponse::success(data, msg)` 的参数顺序**：内部 API 与 trait 方法的参数顺序相反，这是为了同时满足 PHP 对齐（trait 方法 msg 在前）和 Rust 习惯（内部 API data 在前）
- **`post_data` 返回 `Value`**：而非具体的 `HashMap<String, Value>`，因为 PHP 的 `postData()` 可以返回任意类型
- **HTTP 200 的业务错误**：`render_error` 返回 HTTP 200，这与 RESTful 最佳实践（返回 4xx/5xx）不一致，但对齐 PHP 行为

## Bug 定位提示

如果生产 Bug 表现为"响应 JSON 字段顺序错误"或"控制器方法未找到"：

1. **L1 决策层**：查阅本 ADR，确认控制器是否正确 `impl SzController`，方法签名是否匹配
2. **L2 运行时层**：检查 tracing span `controller.action` 中的 `controller` 和 `action` 字段
3. **L3 指标层**：检查 `controller.error` 指标按 `controller` 标签的分布
4. **L4 代码层**：
   - 字段顺序 Bug → 检查 `serde_json` 的 `preserve_order` feature 是否启用
   - 参数解析 Bug → 检查 `packages/sz-rust-core/src/request.rs` 的 `fetch_post_data()` / `fetch_query_data()`
   - 响应格式 Bug → 检查 `packages/sz-rust-core/src/response.rs` 的 `ApiResponse::success()` / `error_with_code()`
   - 方法未找到 Bug → 检查控制器是否 `impl SzController for XxxController {}`（即使是空 impl 也需要）
   - **字段顺序静默错误** → 若 `serde_json` 未启用 `preserve_order` feature，`Map` 使用 BTreeMap，字段顺序变为字母序（`code → data → msg`），PHP 客户端解析异常
   - **trait 方法被意外覆盖** → 业务控制器若定义了与 `SzController` 同名的方法（如 `render_json`），会覆盖默认实现，需确认覆盖逻辑是否正确
