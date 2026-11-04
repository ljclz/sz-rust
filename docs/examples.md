# sz-rust 示例运行指南（docs/examples.md）

> 2026-10-27 UXR P1-4 整改：为 `packages/sz-rust-examples/src/bin/` 下全部 15 个示例补齐「做什么 + 怎么跑 + 是否需要 DB」。
> 所有示例均可直接运行，**无需真实数据库**（数据存储在内存/mock 中，便于快速体验框架 API）。

## 快速开始类

| 示例 | 做什么 | 运行命令 | 访问端口 | 需要 DB？ |
|------|--------|----------|----------|-----------|
| [quick_start](../packages/sz-rust-examples/src/bin/quick_start.rs) | Hello World：最小 axum HTTP 服务，`GET /` 返回 `{"code":1,"msg":"hello","data":{}}` | `cargo run -p sz-rust-examples --bin quick_start` | http://127.0.0.1:9527/ | ❌ 否 |
| [crud_demo](../packages/sz-rust-examples/src/bin/crud_demo.rs) | 完整 User CRUD：`SzController`/`BaseController` 控制器 trait + 约定式路由（`/user/list`、`/user/create` 等） | `cargo run -p sz-rust-examples --bin crud_demo` | http://127.0.0.1:9528/ | ❌ 否（内存 `Vec` 存储） |
| [parse_path_demo](../packages/sz-rust-examples/src/bin/parse_path_demo.rs) | 约定式路由（Layer 3）：`/app/controller/action` URL 解析分发，对齐 PHP `auto_multi_app` | `cargo run -p sz-rust-examples --bin parse_path_demo` | http://127.0.0.1:9533/ | ❌ 否 |
| [admin_demo](../packages/sz-rust-examples/src/bin/admin_demo.rs) | Admin 插件接线：`AdminAddonPlugin` 路由合并 + CapabilityHook 注册（打印后退出，无 HTTP 服务） | `cargo run -p sz-rust-examples --bin admin_demo` | 无（控制台输出） | ❌ 否（mock 连接池） |

## 业务示例

| 示例 | 做什么 | 运行命令 | 访问端口 | 需要 DB？ |
|------|--------|----------|----------|-----------|
| [blog_demo](../packages/sz-rust-examples/src/bin/blog_demo.rs) | 博客：文章 CRUD + 缓存门面（读多写少）+ 事件驱动统计 | `cargo run -p sz-rust-examples --bin blog_demo` | http://127.0.0.1:8081/ | ❌ 否（内存存储） |
| [ecommerce_demo](../packages/sz-rust-examples/src/bin/ecommerce_demo.rs) | 电商：商品/订单/支付 + OpenAPI 自动生成（`spec_from_route_config`） | `cargo run -p sz-rust-examples --bin ecommerce_demo` | http://127.0.0.1:8082/ | ❌ 否（内存/mock 支付） |
| [iot_demo](../packages/sz-rust-examples/src/bin/iot_demo.rs) | IoT：设备上报 + 事件总线 + 限流中间件 + 设备状态缓存 | `cargo run -p sz-rust-examples --bin iot_demo` | http://127.0.0.1:8083/ | ❌ 否（内存/mock） |
| [data_scope_demo](../packages/sz-rust-examples/src/bin/data_scope_demo.rs) | 数据权限：`data_scope_middleware` 生产接线 + 字段级策略注册 + 上下文注入 | `cargo run -p sz-rust-examples --bin data_scope_demo` | http://0.0.0.0:3001/ | ❌ 否 |
| [multi_tenant_demo](../packages/sz-rust-examples/src/bin/multi_tenant_demo.rs) | 多租户 SaaS：租户解析/状态中间件 + 平台管理员跨租户查询 | `cargo run -p sz-rust-examples --bin multi_tenant_demo` | http://0.0.0.0:3000/ | ❌ 否 |

## 性能基准

| 示例 | 做什么 | 运行命令 | 访问端口 | 需要 DB？ |
|------|--------|----------|----------|-----------|
| [bench_sz_rust](../packages/sz-rust-examples/src/bin/bench_sz_rust.rs) | sz-rust 框架开销基准：路由 + 中间件 + JSON 响应 | `cargo run --release -p sz-rust-examples --bin bench_sz_rust -- 127.0.0.1:8080` | 8080（可传参） | ❌ 否 |
| [bench_axum_native](../packages/sz-rust-examples/src/bin/bench_axum_native.rs) | 纯 axum 对照基准：与 bench_sz_rust 相同端点，用于对比框架开销 | `cargo run --release -p sz-rust-examples --bin bench_axum_native -- 127.0.0.1:8081` | 8081（可传参） | ❌ 否 |

## WASM 示例

| 示例 | 做什么 | 运行命令 | 访问端口 | 需要 DB？ |
|------|--------|----------|----------|-----------|
| [wasm_basic](../packages/sz-rust-examples/src/bin/wasm_basic.rs) | WASM 基础：模块加载与执行（`add(3,4)`） | `cargo run -p sz-rust-examples --bin wasm_basic` | 无（控制台输出） | ❌ 否 |
| [wasm_host_function](../packages/sz-rust-examples/src/bin/wasm_host_function.rs) | WASM 宿主函数：注册与调用 | `cargo run -p sz-rust-examples --bin wasm_host_function` | 无（控制台输出） | ❌ 否 |
| [wasm_sandbox](../packages/sz-rust-examples/src/bin/wasm_sandbox.rs) | WASM 沙箱：未授权操作被拒绝（安全验证） | `cargo run -p sz-rust-examples --bin wasm_sandbox` | 无（控制台输出） | ❌ 否 |
| [wasm_edge_deploy](../packages/sz-rust-examples/src/bin/wasm_edge_deploy.rs) | WASM 边缘部署：部署步骤说明 + 示例模块 | `cargo run -p sz-rust-examples --bin wasm_edge_deploy` | 无（控制台输出） | ❌ 否 |

---

## 常见问题

- **端口被占用**：部分示例端口在源码中硬编码（`127.0.0.1:95xx` / `808x`），可自行修改 `main()` 中的 `addr`。
- **首次编译耗时**：workspace 依赖较多，首次 `cargo run` 需要几分钟编译；基准示例请加 `--release`。
- **示例源码**：全部位于 `packages/sz-rust-examples/src/bin/`，每个文件头部均有「做什么 + 怎么跑」文档注释。