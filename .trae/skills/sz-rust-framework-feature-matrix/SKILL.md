---
name: sz-rust-framework-feature-matrix
description: 对比 ThinkPHP 8 / Laravel / NestJS 功能矩阵，列出未实现项。修改功能模块或新增模块时触发。
tools: [grep, glob]
agentMode: auto
---

# 功能矩阵对比审计（sz-rust framework）

## 触发条件

- 新增模块到 `packages/sz-rust-core/src/` 时
- 修改 `lib.rs` 的模块列表时
- 用户询问"还有哪些功能缺失"时
- 季度功能完整性审计时

## 参考框架矩阵

### ThinkPHP 8 核心功能

| 类别 | 功能 | sz-rust 实现状态 |
|------|------|-----------------|
| 路由 | 三层路由机制（属性/配置/约定） | ✅ `routing.rs` |
| 路由 | 路由参数约束 | ✅ `router.rs` |
| 路由 | 路由分组 | ✅ `router.rs` |
| 路由 | API 版本管理 | ✅ `api_version.rs`（P4-2） |
| 容器 | `app()->bind/make/singleton/scoped/instance/alias` | ✅ `container.rs`（P4-1） |
| 容器 | 服务生命周期（Singleton/Transient/Scoped） | ✅ `container.rs`（P4-1） |
| 控制器 | `BaseController` / `SzController` | ✅ `controller.rs` |
| 控制器 | 资源控制器（CRUD 路由） | ⚠️ 部分（需验证） |
| 请求 | `$this->request->post/get` | ✅ `request.rs` |
| 响应 | `renderJson/renderSuccess/renderError` | ✅ `response.rs` |
| 中间件 | CORS/Auth/Log/RateLimit/Trace | ✅ `middleware/` |
| 中间件 | CSRF（双提交 Cookie） | ✅ `middleware/csrf.rs` |
| 中间件 | Guard（NestJS 风格） | ✅ `guard.rs` |
| 模型 | `think\Model`（CRUD） | ✅ `model.rs` |
| 模型 | 关联关系（HasMany/BelongsTo/HasOne/BelongsToMany/Morph） | ✅ `relation.rs` |
| 模型 | 模型钩子（16 事件） | ✅ `hooks.rs` |
| 验证 | `think\Validate` | ✅ `validate.rs` |
| 文件上传 | `think\File` + `UploadedFile` | ✅ `upload.rs` |
| 缓存 | `think\facade\Cache` | ✅ `cache.rs` |
| 缓存 | 缓存预热 | ✅ `cache_warmer.rs`（P4-3） |
| 会话 | `think\facade\Session` | ✅ `session.rs` |
| Cookie | `think\Cookie` | ✅ `cookie.rs` |
| 事件 | `think\Event` | ✅ `event.rs` |
| 环境变量 | `think\facade\Env` | ✅ `env.rs` |
| 国际化 | `think\facade\Lang` | ✅ `i18n.rs` |
| 邮件 | `think\facade\Mail` | ✅ `mail.rs` |
| 迁移 | `think migrate` | ✅ `sz-rust-cli/src/cmd/migrate.rs`（P4-1） |
| 迁移 | 迁移历史表 | ✅ `migration_history.rs`（P4-1） |
| 调试 | Whoops 调试页 | ✅ `debug_page.rs`（P4-4） |
| 错误 | `BaseException` 标准化 JSON | ✅ `error_handler.rs` |
| 日志 | `think-logger` | ✅ `log.rs` |
| 视图 | 模板渲染 | ✅ `view/` |
| 多应用 | `auto_multi_app` | ✅ `multi_app.rs` |
| 插件 | `addons/` | ✅ `addons.rs` |
| 健康检查 | K8s liveness/readiness | ✅ `health.rs` |
| 静态文件 | 静态文件路由 | ✅ `static_files.rs` |
| HTTP/2 | HTTP/2 + TLS | ✅ `h2.rs` |
| 服务器 | `think-swoole` / `think-worker` | ✅ `server.rs` |
| 宏 | `compact()` | ✅ `macros.rs` |
| 运行时 | Runtime context | ✅ `runtime.rs` |

### Laravel 独有功能（参考）

| 功能 | sz-rust 状态 | 备注 |
|------|-------------|------|
| Eloquent ORM 高级查询 | ✅ 通过 sz-orm | 复用 sz-orm-core |
| Queue 队列系统 | ⚠️ 未实现 | sz-orm-scheduler 提供定时任务，无消息队列 |
| Broadcasting 广播 | ❌ 未实现 | WebSocket 推送未实现 |
| Notification 通知 | ⚠️ 部分 | 邮件已实现，Slack/短信等未实现 |
| Horizon 队列监控 | ❌ 未实现 | 依赖队列系统 |
| Telescope 调试工具 | ⚠️ 部分 | debug_page.rs 提供基础调试 |
| Socialite 社交登录 | ❌ 未实现 | OAuth2 客户端未实现 |
| Sanctum API 认证 | ⚠️ 部分 | JWT 已实现，token 管理待补 |

### NestJS 独有功能（参考）

| 功能 | sz-rust 状态 | 备注 |
|------|-------------|------|
| Decorator 装饰器路由 | ✅ 通过属性宏 | sz-rust-macros |
| Guard 守卫 | ✅ 已实现 | guard.rs |
| Interceptor 拦截器 | ⚠️ 部分 | 中间件覆盖大部分场景 |
| Pipe 管道（验证/转换） | ✅ validate.rs | |
| Exception Filter | ✅ error_handler.rs | |
| Dependency Injection | ✅ container.rs | 完整 DI 容器 |
| Module 模块系统 | ✅ multi_app.rs | |
| Microservices 微服务 | ❌ 未实现 | TCP/Redis/NATS transport 未实现 |
| GraphQL | ❌ 未实现 | async-graphql 集成未实现 |
| WebSocket Gateway | ⚠️ 部分 | axum WS 支持，但无 NestJS 风格 API |
| Swagger/OpenAPI | ❌ 未实现 | utoipa 集成未实现 |
| CLI 命令行 | ✅ sz-rust-cli | |

## 执行步骤

1. **扫描 src/lib.rs 的 `pub mod` 列表**，获取当前已实现模块
2. **对比上述矩阵**，列出 ❌ 和 ⚠️ 项
3. **对每个 ⚠️ 项**，打开对应文件验证实际实现深度（而非仅看文件存在）
4. **生成报告**，包含：
   - 已实现功能列表（✅）
   - 部分实现功能列表（⚠️）+ 缺失的具体子功能
   - 未实现功能列表（❌）+ 优先级评估（高/中/低）
5. **建议优先级**：
   - 高：影响生产可用性（如队列、WebSocket）
   - 中：影响开发体验（如 Swagger、Socialite）
   - 低：锦上添花（如 Broadcasting、GraphQL）

## 通过标准

- 所有 ❌ 高优先级项有明确的实现计划或文档说明
- 所有 ⚠️ 项的缺失子功能已记录到 `docs/功能基线清单.md`
- 报告中的状态有代码验证支持，而非主观判断

## 输出文件

- `docs/audit/YYYY-MM-DD-功能矩阵审计.md`
- 更新 `docs/功能基线清单.md`（如不存在则创建）
