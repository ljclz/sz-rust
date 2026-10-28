# ADR-008：错误处理策略（AppError 枚举 + ErrorCode 映射 + BaseException 对齐）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-003（控制器抽象）、ADR-006（认证授权）
> **相关代码**：`packages/sz-rust-core/src/error.rs`、`packages/sz-rust-core/src/error_handler.rs`

## 背景

PHP 端错误处理基于 `app\common\exception\BaseException`：

```php
class BaseException extends \think\Exception {
    protected $code = 0;
    protected $msg = 'error';
    protected $data = [];
}
```

PHP 错误码（从 PHP 后端代码提取）：

| code | 含义 | PHP 使用场景 |
|------|------|-------------|
| `1` | 成功 | `renderSuccess` 默认 |
| `0` | 失败 | `renderError` 默认 / `BaseException` 默认 |
| `-1` | 未登录/参数错误 | `not_login` / `缺少必要的参数` / `密钥不准确` |
| `-2` | 用户不存在/未绑定 | `没有找到用户信息` / `请先绑定,员工信息` |
| `-3` | 用户已禁用/已离职 | `员工信息待审核` / `您已离职` |

PHP 的 JSON 响应格式：
```json
{ "code": <code>, "msg": "<msg>", "data": {} }
```

sz-rust 需要决定如何对齐 PHP 的错误处理，同时利用 Rust 的类型系统优势。

## 决策

采用 **`AppError` 枚举 + `ErrorCode` 映射 + `BaseException` 对齐** 策略：

### 1. ErrorCode 枚举（对齐 PHP 错误码）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(i32)]
pub enum ErrorCode {
    Success = 1,          // PHP renderSuccess 默认
    Failed = 0,           // PHP renderError 默认 / BaseException 默认
    NotLogin = -1,        // PHP not_login / 缺少必要的参数 / 密钥不准确
    UserNotFound = -2,    // PHP 没有找到用户信息 / 请先绑定,员工信息
    UserDisabled = -3,    // PHP 员工信息待审核 / 您已离职
    Forbidden = 403,      // Rust 扩展，HTTP 403
    NotFound = 404,       // Rust 扩展，HTTP 404
    ValidateFailed = 422, // Rust 扩展，HTTP 422
    DbError = 500,        // Rust 扩展，HTTP 500
}
```

### 2. HTTP 状态码映射

```rust
impl ErrorCode {
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Success => 200,
            ErrorCode::Failed => 200,         // 业务失败 HTTP 仍 200（对齐 PHP）
            ErrorCode::NotLogin => 401,
            ErrorCode::UserNotFound => 401,
            ErrorCode::UserDisabled => 403,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::ValidateFailed => 422,
            ErrorCode::DbError => 500,
        }
    }
}
```

### 3. BaseException 对齐

```rust
// sz-rust 的 BaseException 对齐 PHP 的 BaseException
// 包含 code (ErrorCode)、msg (String)、data (Value) 三个字段
// 实现 IntoResponse，自动转换为 {code, msg, data} JSON 响应
```

### 4. 错误传播链

```
业务代码 → BaseException → error_handler 中间件 → JSON 响应
                                  ↓
                            tracing 日志记录
                                  ↓
                            metrics 错误计数
```

### 5. 业务失败 HTTP 仍 200

对齐 PHP 行为：
- `ErrorCode::Failed`（code=0）→ HTTP 200
- `ErrorCode::Success`（code=1）→ HTTP 200
- 业务错误通过 `code` 字段区分，而非 HTTP 状态码

这与 RESTful 最佳实践不一致，但对齐 PHP 行为。

## 决策替代方案

### 方案 A：直接使用 `anyhow::Error`（拒绝）

```rust
// 使用 anyhow 处理所有错误
fn do_something() -> anyhow::Result<()> { ... }
```

**拒绝原因**：
- `anyhow::Error` 是黑盒错误，无法提取错误码（`code` / `msg` / `data`）
- 无法对齐 PHP 的 `BaseException` 结构（`code` + `msg` + `data` 三字段）
- 无法实现 `IntoResponse` 自动转换为 `{code, msg, data}` JSON 响应
- 生产 Bug 定位时无法通过错误码分类统计

### 方案 B：每个业务模块定义自己的错误枚举（拒绝）

每个业务模块（user / order / product）定义独立的错误枚举。

**拒绝原因**：
- 错误码分散在多个枚举中，无法统一管理
- 不同模块可能对同一错误使用不同的 HTTP 状态码
- 无法实现统一的 `error_handler` 中间件（需要处理所有错误类型）
- PHP 端的 `BaseException` 是统一的，Rust 端也应保持统一

### 方案 C：仅使用 HTTP 状态码（RESTful 风格）（拒绝）

```rust
// 完全 RESTful：成功=200，业务失败=400/422，未登录=401，无权限=403
// 不返回业务错误码（code 字段）
```

**拒绝原因**：
- PHP 端的业务错误返回 HTTP 200 + `{code: 0, msg: "error"}`，完全 RESTful 会破坏 PHP 客户端兼容
- 前端代码依赖 `code` 字段判断业务成功/失败，而非 HTTP 状态码
- 迁移期间 PHP 和 Rust 并存，错误处理风格必须一致

**最终选择**：`AppError` 枚举 + `ErrorCode` 映射 + `BaseException` 对齐。统一错误类型，PHP 错误码与 Rust 扩展错误码共存，通过 `error_handler` 中间件统一转换为 JSON 响应。

## 后果

### 正面后果

- **PHP 完全对齐**：错误码、响应格式、HTTP 状态码映射完全对齐 PHP
- **类型安全**：`ErrorCode` 枚举避免魔法数字，编译期检查
- **错误传播清晰**：`BaseException` 实现 `IntoResponse`，错误自动转换为 JSON
- **可观测**：错误通过 `error_handler` 中间件统一记录 tracing 和 metrics
- **扩展性**：Rust 扩展的错误码（403/404/422/500）与 PHP 错误码共存

### 负面后果

- **HTTP 200 的业务错误**：`ErrorCode::Failed` 返回 HTTP 200，不符合 RESTful 实践，HTTP 客户端难以通过状态码判断成功/失败
- **错误码混用**：PHP 错误码（1/0/-1/-2/-3）与 Rust 扩展错误码（403/404/422/500）混用，需要开发者记忆两套体系
- **`From<i32>` 转换**：`ErrorCode::from(i32)` 对未知错误码返回 `Failed`，可能掩盖真实错误

## 注意事项

- **`#[repr(i32)]`**：`ErrorCode` 使用 `#[repr(i32)]` 确保 `as i32` 转换正确
- **`serde::Serialize`**：`ErrorCode` 实现 `Serialize`，序列化为整数（`"code": 1`）
- **错误处理中间件**：`error_handler.rs` 负责捕获 `BaseException` 并转换为 JSON 响应
- **tracing 记录**：所有错误都会通过 tracing 记录，包括错误码、消息、堆栈
- **metrics 计数**：`error.count` 指标按 `code` 标签分类计数
- **`data` 字段**：`BaseException` 的 `data` 字段类型为 `Value`，可以携带任意 JSON 数据

## Bug 定位提示

如果生产 Bug 表现为"错误响应格式错误"或"错误码不匹配"：

1. **L1 决策层**：查阅本 ADR，确认错误是否通过 `BaseException` 抛出，错误码是否使用 `ErrorCode` 枚举
2. **L2 运行时层**：检查 tracing span `error.handle` 中的 `code` 和 `msg` 字段
3. **L3 指标层**：检查 `error.count` 指标按 `code` 标签的分布
4. **L4 代码层**：
   - 响应格式 Bug → 检查 `error_handler.rs` 的 `IntoResponse` 实现，确认字段顺序为 `code → msg → data`
   - 错误码不匹配 Bug → 检查 `ErrorCode` 枚举的 `#[repr(i32)]` 值
   - HTTP 状态码 Bug → 检查 `ErrorCode::http_status()` 的映射
   - 错误吞没 Bug → 检查业务代码是否用 `?` 传播错误，还是用 `unwrap()`/`expect()` 直接 panic
   - **未知错误码静默降级** → `ErrorCode::from(999)` 返回 `Failed`（code=0），真实错误码被掩盖；检查 `From<i32> for ErrorCode` 的 fallback 逻辑
   - **`data` 字段类型不匹配** → `BaseException::data` 为 `Value`，若业务代码传入非 JSON 可序列化类型，`IntoResponse` 转换时 panic
