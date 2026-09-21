# ADR-014: GraphQL 集成 — sz-orm-graphql facade 透传

- **状态**: Accepted
- **日期**: 2026-08-02
- **相关代码**: `packages/sz-rust-core/src/orm/graphql.rs (L1-L129)`、`packages/sz-rust-core/src/orm.rs (L15-L20)`、`packages/sz-rust-core/Cargo.toml (L78-L79, L101-L102)`
- **修复编号**: P2 能力评估遗留项

## 背景

SZ-Rust 作为 Web 框架，需要支持 GraphQL API 场景（前端按需查询、减少过度获取）。sz-orm 生态已有 `sz-orm-graphql` crate 提供 GraphQL 核心能力，sz-rust-core 需要提供统一的访问入口。

不集成的后果：框架只能提供 REST API，无法支持 GraphQL 客户端，限制了前端技术选型。

## 决策

### 方案选择：Feature-gated Facade 透传

sz-rust-core 不自行实现 GraphQL，而是通过 Cargo feature `graphql` 条件依赖 `sz-orm-graphql`，在 `sz_rust_core::orm::graphql` 模块中重新导出其公共类型。

```rust
// packages/sz-rust-core/src/orm/graphql.rs (L1-L30)
//! GraphQL 集成（feature-gated）
//!
//! 启用方式：在 Cargo.toml 中添加 `sz-rust-core = { features = ["graphql"] }`

#[cfg(feature = "graphql")]
pub use sz_orm_graphql::{
    GraphQLField, GraphQLSchema, GraphQLSchemaGenerator, GraphQLServer, GraphQLType,
    resolver,
};

#[cfg(not(feature = "graphql"))]
compile_error!("GraphQL 功能未启用。请在 Cargo.toml 中添加 features = [\"graphql\"] 或 features = [\"p2-addons\"]");
```

### orm.rs 条件模块声明

```rust
// packages/sz-rust-core/src/orm.rs (L15-L20)
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "grpc")]
pub mod grpc;
```

### 依赖管理

```toml
# packages/sz-rust-core/Cargo.toml (L78-L79, L101-L102)
sz-orm-graphql = { workspace = true, optional = true }

[features]
graphql = ["dep:sz-orm-graphql"]
```

### 测试策略

graphql 模块的测试标注 `#[cfg(feature = "graphql")]`，仅在 `--all-features` 或 `--features graphql` 时编译运行。默认构建（无 feature）不编译 graphql 测试，避免引入不必要的依赖。

## 后果

### 正面后果
- 用户通过统一路径 `sz_rust_core::orm::graphql::*` 访问，无需直接依赖 `sz-orm-graphql`。
- Feature flag 隔离：不启用 GraphQL 的项目零依赖开销。
- 与 gRPC、hot-reload 等其他 P2 功能通过 `p2-addons` 组合 feature 统一管理。

### 负面后果
- **版本耦合**：sz-rust-core 的 graphql facade 版本与 `sz-orm-graphql` 版本强绑定。sz-orm-graphql 升级时，facade 需同步更新 re-export 列表。
- **类型透传限制**：facade 仅 re-export 公共类型，不暴露 `sz-orm-graphql` 的内部 API。若用户需要高级用法（自定义 scalar、directive），需直接依赖 `sz-orm-graphql`。
- **编译错误引导**：未启用 feature 时使用 `compile_error!` 而非静默空模块，强制用户显式启用，但会增加初次使用者的困惑。

## 注意事项

- **feature 命名**：使用 `graphql`（非 `graphql-support`），与 `grpc`、`hot-reload` 命名风格一致。
- **组合 feature**：`p2-addons = ["graphql", "grpc", "hot-reload"]` 可一次性启用全部 P2 功能。
- **re-export 完整性**：新增 `sz-orm-graphql` 公共类型时，必须同步更新 graphql.rs 的 `pub use` 列表，否则用户通过 facade 无法访问。
- **测试隔离**：graphql 测试仅在 feature 启用时运行，CI 的 `all-features-compile` job 确保 feature 启用时测试通过。

### Bug 定位提示

如果生产出现"GraphQL 类型未找到"错误：
1. 检查 Cargo.toml 是否启用了 `features = ["graphql"]` 或 `features = ["p2-addons"]`。
2. 检查 `sz-orm-graphql` 版本是否与 workspace 声明一致（`Cargo.toml` L136）。
3. 检查导入路径是否为 `sz_rust_core::orm::graphql::*`（而非直接 `sz_orm_graphql::*`，后者在 facade 模式下可能版本不匹配）。

如果编译报错 `compile_error! GraphQL 功能未启用`：
1. 确认这是预期行为（用户忘记启用 feature），而非代码中误用了 graphql 类型。
2. 若项目确实不需要 GraphQL，移除对 `sz_rust_core::orm::graphql` 的引用。
