# sz-rust-orm-facade

> **中文** | [English](README.en.md)

SZ-Rust ORM unified entry point. Downstream business packages access all `sz-orm-*` sub-packages through this crate, rather than directly depending on individual sub-packages.

## Features

- **ORM Core**: `Model`, `ModelExt`, `Repository`, `Pool`, `Value`
- **SQL Validation**: `validate_sql`, `sql_string!` macro (compile-time SQL injection detection)
- **Authentication**: `JwtClaims`, `JwtEncoder`, `JwtAuthenticator`
- **Cloud Storage**: `StorageBuilder`, `AliyunOssStorage`, `TencentCosStorage`, `S3Storage`, etc.
- **GraphQL / gRPC**: Optional feature gates

## Usage

```rust
use sz_rust_orm_facade::{Model, Repository, Pool, Value};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};

// Compile-time SQL validation
let sql = sz_rust_orm_facade::sql_string!("SELECT id, name FROM users WHERE id = ?"; params: 1);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `graphql` | Enable GraphQL facade (depends on `sz-orm-graphql`) |
| `grpc` | Enable gRPC facade (depends on `sz-orm-grpc`) |

## Version Policy

Keeps in sync with `sz-rust-core`, version numbers follow `0.x.0` semantics.