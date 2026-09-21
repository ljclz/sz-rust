# sz-rust-http-facade

> **中文** | [English](README.en.md)

SZ-Rust HTTP foundation layer. Contains three major modules: response, error, request. It is the base dependency for all HTTP handling.

## Features

- **response**: `ApiResponse` trait, `respond_json` / `respond_html` / `respond_redirect` response builders
- **error**: `BaseException`, `AppError` enum, standardized error code mapping
- **request**: Request data extraction, `fetch_post_data`, Header parsing

## Usage

```rust
use sz_rust_http_facade::response::{ApiResponse, respond_json};
use sz_rust_http_facade::error::BaseException;
use sz_rust_http_facade::request::fetch_post_data;

// Build JSON response
let resp = respond_json(&data, 200, "success");
```

## Dependencies

- `axum` 0.8
- `serde` / `serde_json`
- `thiserror`
- `http-body-util` / `tower` (for testing)

## Version Policy

Keeps in sync with `sz-rust-core`.