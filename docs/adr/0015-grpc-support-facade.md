# ADR-015: gRPC 支持 — sz-orm-grpc facade 透传

- **状态**: Accepted
- **日期**: 2026-08-02
- **相关代码**: `packages/sz-rust-core/src/orm/grpc.rs (L1-L239)`、`packages/sz-rust-core/src/orm.rs (L17-L19)`、`packages/sz-rust-core/Cargo.toml (L80-L81, L103-L104)`
- **修复编号**: P2 能力评估遗留项

## 背景

SZ-Rust 作为 Web 框架，需要支持 gRPC 场景（微服务间高效通信、流式数据传输）。sz-orm 生态已有 `sz-orm-grpc` crate 提供 gRPC 核心能力（Channel、Server、Interceptor、RetryPolicy 等），sz-rust-core 需要提供统一的访问入口。

不集成的后果：框架只能提供 HTTP/REST API，无法支持微服务间的 gRPC 通信，限制了分布式架构选型。

## 决策

### 方案选择：Feature-gated Facade 透传（与 GraphQL 一致）

与 ADR-014 GraphQL 集成采用相同策略：通过 Cargo feature `grpc` 条件依赖 `sz-orm-grpc`，在 `sz_rust_core::orm::grpc` 模块中重新导出其公共类型。

```rust
// packages/sz-rust-core/src/orm/grpc.rs (L1-L40)
//! gRPC 支持（feature-gated）
//!
//! 启用方式：在 Cargo.toml 中添加 `sz-rust-core = { features = ["grpc"] }`

#[cfg(feature = "grpc")]
pub use sz_orm_grpc::{
    GrpcChannel, GrpcClient, GrpcServer, GrpcStatusCode,
    Interceptor, RetryPolicy, TimeoutPolicy,
};
```

### 14 个单元测试

grpc.rs 包含 14 个单元测试（L80-L239），覆盖：
- `GrpcServer` 构建与配置（port、max_message_size）
- `GrpcChannel` 连接管理（target、timeout）
- Interceptor 链（请求/响应拦截器）
- RetryPolicy / TimeoutPolicy 配置

### 依赖管理

```toml
# packages/sz-rust-core/Cargo.toml (L80-L81, L103-L104)
sz-orm-grpc = { workspace = true, optional = true }

[features]
grpc = ["dep:sz-orm-grpc"]
```

## 后果

### 正面后果
- 统一访问路径 `sz_rust_core::orm::grpc::*`，与 REST/GraphQL 保持一致的模块层级。
- Feature flag 隔离：不启用 gRPC 的项目零依赖开销（tonic/prost 等重依赖不会进入编译）。
- 14 个单元测试确保 facade 层的 API 稳定性。

### 负面后果
- **重依赖**：`sz-orm-grpc` 依赖 `tonic`、`prost`、`tokio` 等，启用后编译时间显著增加（约 +30s）。
- **版本耦合**：与 GraphQL facade 相同的版本绑定问题（ADR-014 已说明）。
- **端口冲突**：gRPC server 默认端口与 HTTP server 可能冲突，需在配置中显式区分。

## 注意事项

- **feature 命名**：使用 `grpc`（非 `grpc-support`），与 `graphql`、`hot-reload` 命名风格一致。
- **tonic 版本**：`sz-orm-grpc` 基于 `tonic 0.12`，若项目已有 tonic 依赖需注意版本冲突。
- **proto 编译**：gRPC 服务需要 `prost-build` 编译 `.proto` 文件，建议在 `build.rs` 中处理。
- **拦截器顺序**：多个 Interceptor 的执行顺序为声明顺序的逆序（外层先执行），与 Tower Layer 语义一致。

### Bug 定位提示

如果生产出现"gRPC 连接失败"：
1. 检查 `GrpcChannel` 的 target 地址是否正确（`http://host:port` 格式，非 `grpc://`）。
2. 检查 `TimeoutPolicy` 是否设置过短（默认 5s，大 payload 可能超时）。
3. 检查服务端 `GrpcServer` 是否已启动并绑定到正确端口（`server.serve(addr).await` 是否被调用）。

如果生产出现"Interceptor 未生效"：
1. 检查 Interceptor 是否通过 `channel.interceptor(my_interceptor)` 正确注册。
2. 检查 Interceptor 的 `call_mut` 方法是否调用了 `request.next().call_mut(())` 传递请求（遗漏会导致请求被吞掉）。
3. 检查多个 Interceptor 的执行顺序是否符合预期（逆序执行）。

如果编译报错 `compile_error! gRPC 功能未启用`：
1. 确认 Cargo.toml 是否启用了 `features = ["grpc"]` 或 `features = ["p2-addons"]`。
2. 检查 workspace `Cargo.toml` L137 是否声明了 `sz-orm-grpc` 路径依赖。
