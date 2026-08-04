# sz-rust-http-facade

SZ-Rust HTTP 基础层。包含 response、error、request 三大模块，是所有 HTTP 处理的底层依赖。

## 功能

- **response**：`ApiResponse` trait、`respond_json` / `respond_html` / `respond_redirect` 等响应构建器
- **error**：`BaseException`、`AppError` 枚举、标准化错误码映射
- **request**：请求数据提取、`fetch_post_data`、Header 解析

## 用法

```rust
use sz_rust_http_facade::response::{ApiResponse, respond_json};
use sz_rust_http_facade::error::BaseException;
use sz_rust_http_facade::request::fetch_post_data;

// 构建 JSON 响应
let resp = respond_json(&data, 200, "success");
```

## 依赖

- `axum` 0.8
- `serde` / `serde_json`
- `thiserror`
- `http-body-util` / `tower`（测试用）

## 版本策略

与 `sz-rust-core` 保持同步。
