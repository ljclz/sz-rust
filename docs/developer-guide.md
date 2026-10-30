# sz-rust 开发者使用指南

> 2026-10-27 UXR P2-1 整改：本文档面向**人类开发者**，提供从零开始使用 sz-rust 的导航。
> AI 协作 / Skill 触发说明已独立到 [sz-rust-skills使用指南.md](sz-rust-skills使用指南.md)（供 AI Agent 使用）。

## 1. 快速开始

1. 克隆仓库并运行最小示例：

   ```bash
   git clone <your-fork>/sz-rust.git
   cd sz-rust
   cargo run -p sz-rust-examples --bin quick_start
   ```

2. 访问 http://127.0.0.1:9527/，应返回 `{"code":1,"msg":"hello","data":{}}`。

3. 完整 CRUD 示例：`cargo run -p sz-rust-examples --bin crud_demo`（端口 9528）。

> 全部 15 个示例的「做什么 + 怎么跑 + 是否需要 DB」见 [docs/examples.md](examples.md)。

## 2. 核心概念

| 概念 | 入口 | 说明 |
|------|------|------|
| 应用容器 | `sz_rust_core::container::App` | 配置加载、依赖装配 |
| 配置 | `sz_rust_core::config::AppConfig` | YAML 配置加载（`config/` 目录） |
| 控制器 | `sz_rust_core::controller::{SzController, BaseController, AddonsBaseController}` | 三层 trait 继承链，对齐 PHP `think\Controller` |
| 路由 | axum `Router` + `sz_rust_core::router` | 三层路由机制（属性宏 / 配置式 / 约定式） |
| ORM | `sz_rust_orm_facade::repository::{Repository, WhereCondition, WhereOp}` | **Data Mapper + Repository 模式**（非 Active Record） |
| 验证器 | `sz_rust_core::validate::Validate` | 30+ 规则、场景、批量验证、自定义消息（默认中文消息） |
| 缓存 | `sz_rust_core::cache` | Memory / Redis / Memcached / 多级驱动 |
| 事件 | `sz_rust_core::event` | Listener / Subscriber / Observer |
| 中间件 | `sz_rust_core::middleware` | CORS / Auth(JWT) / Log / RateLimit / Trace + 链构建器 |
| 错误码 | `sz_rust_core::error::ErrorCode` | 标准错误码 |
| CLI | `sz-rust-cli` | make / migrate / route / cache / scheduler / plugin / admin |

## 3. 写一个「用户 CRUD」

1. 参考 [crud_demo.rs](../packages/sz-rust-examples/src/bin/crud_demo.rs) 的控制器写法（`SzController` trait + axum `Router`）。
2. 查询数据使用 Repository 模式：

   ```rust
   use sz_rust_orm_facade::repository::{Repository, WhereCondition, WhereOp};
   use sz_rust_orm_facade::Value;

   // 等价于 ThinkPHP：Db::name('user')->where('status', 1)->select()
   let active_users = repo.find_by(&[WhereCondition::new("status", WhereOp::Eq, Value::I64(1))])?;
   ```

   > 注意：ThinkPHP 的 `Db::name()->where()->select()` 流畅 API 在 sz-rust 中**不存在**，
   > ORM 是 Data Mapper 模式。详见 [thinkphp-api-mapping.md](thinkphp-api-mapping.md) 的 Db 门面说明。

3. 参数校验使用验证器（默认输出中文错误消息）：

   ```rust
   use sz_rust_core::validate::Validate;

   let mut v = Validate::new()
       .rule("name|用户名称", "require|max:50")
       .message(indexmap! { "name.require".into() => "用户名称不能为空" });
   if let Err(e) = v.check(&data) {
       return ctrl.render_error(&e.to_string(), json!({}), 0);
   }
   ```

   > 生产接入示范见 [sz300 merchant 创建接口](../packages/sz-rust-sz300/src/controllers/merchant.rs)。

## 4. 部署

### 4.1 Docker Compose 开发环境

```bash
# 一键启动（自动加载 .env.example，无需手动 export 环境变量）
docker compose --env-file .env.example up -d --build
```

### 4.2 生产环境

```bash
export SZ300_JWT_SECRET="$(openssl rand -hex 32)"
export SZ300_DB_PASSWORD="$(openssl rand -hex 16)"
export SZ300_METRICS_TOKEN="$(openssl rand -hex 32)"
docker compose -f docker-compose.prod.yml up -d --build
```

> 安全基线（默认启用 `v18-security-headers` / `v18-data-mask` / `v18-audit-chain` /
> `v18-key-rotation` / `v19-jwt-audience`）详见 [README](../README.md)「生产部署安全基线」。

## 5. 更多文档

- [README](../README.md) — 框架总览与特性清单
- [ThinkPHP API 对照表](thinkphp-api-mapping.md) — 门面与 ThinkPHP API 逐项对照
- [API 快速参考](api-reference.md) — v2.0.0 核心 API 入口（替代已归档的 api-guide.md）
- [示例运行指南](examples.md) — 15 个示例的用途与运行方式
- [工程化实践规范](sz-rust-engineering-practices.md) — 门禁 / CI / CD / 代码风格
- 完整 API 文档：`cargo doc -p sz-rust-core --open`