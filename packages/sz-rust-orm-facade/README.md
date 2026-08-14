> **中文** | [English](README.en.md)

# sz-rust-orm-facade

SZ-Rust ORM 全家桶统一入口。下游业务包通过此 crate 访问所有 `sz-orm-*` 子包，而非直接依赖各子包。

## 功能

- **ORM 核心**：`Model`、`ModelExt`、`Repository`、`Pool`、`Value`
- **SQL 校验**：`validate_sql`、`sql_string!` 宏（编译期 SQL 注入检测）
- **认证**：`JwtClaims`、`JwtEncoder`、`JwtAuthenticator`
- **云存储**：`StorageBuilder`、`AliyunOssStorage`、`TencentCosStorage`、`S3Storage` 等
- **GraphQL / gRPC**：可选 feature 启用

## 用法

```rust
use sz_rust_orm_facade::{Model, Repository, Pool, Value};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};

// 编译期 SQL 校验
let sql = sz_rust_orm_facade::sql_string!("SELECT id, name FROM users WHERE id = ?"; params: 1);
```

## Feature Flags

| Feature | 说明 |
|---------|------|
| `graphql` | 启用 GraphQL facade（依赖 `sz-orm-graphql`） |
| `grpc` | 启用 gRPC facade（依赖 `sz-orm-grpc`） |

## 版本策略

与 `sz-rust-core` 保持同步，版本号遵循 `0.x.0` 语义。
