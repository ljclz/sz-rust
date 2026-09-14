# SZ-Rust v0.3.0 能力评估报告

> 生成日期：2026-08-02 | 版本：v0.3.0 | sz-orm：v1.2.2

---

## 一、项目概览

SZ-Rust 是一个对标 ThinkPHP 8 的 Rust Web 框架，基于 axum 0.8 + sz-orm 1.2.2 构建。API 设计对齐 ThinkPHP 8 习惯用法，降低 PHP 工程师迁移成本。

- **License**: MIT
- **Rust Edition**: 2021 (MSRV 1.81)
- ** crates.io**: `sz-rust-core` 及 8 个子包均已发布
- **GitHub**: https://github.com/ljclz/sz-rust

---

## 二、Workspace 包清单（14 个包）

| 包名 | 类型 | 说明 |
|------|------|------|
| `sz-rust-core` | 库 | Web 框架核心：路由、中间件、DI 容器、缓存、会话、验证、模板、WebSocket、OpenAPI |
| `sz-rust-macros` | 库 | 过程宏：`#[controller]`、`#[model]`、`compact!` |
| `sz-rust-observability` | 库 | Prometheus 指标 + SLO 多窗口燃烧率 + OTLP/Jaeger/Zipkin 导出器 |
| `sz-rust-addons-loader` | 库 | Addon 插件加载器：发现、清单解析、注册、路由解析 |
| `sz-rust-addons-operate` | 库 | CRUD 业务插件：公司/合同/客户/分类/等级/区域/门店 + 支付集成（建行/富友/工行） |
| `sz-rust-addons-crm` | 库 | CRM 模板插件：联系人/线索/商机 |
| `sz-rust-addons-erp` | 库 | ERP 模板插件：商品/供应商/采购单 |
| `sz-rust-addons-ecommerce` | 库 | 电商模板插件：订单/订单项/购物车 |
| `sz-rust-cli` | 二进制 | 命令行工具 `sz-rust`：脚手架、迁移、调度、缓存 |
| `sz-rust-sz300` | 二进制 | 业务示例应用 `sz300-server`：设备/商户/商品/订单管理 |
| `sz-rust-examples` | 二进制 | 快速入门 + CRUD 示例 |
| `sz-rust-tracing` | 库 | W3C TraceContext 追踪扩展 |
| `sz-rust-pdf` | 库 | PDF/Excel 处理（lopdf + rust_xlsxwriter + calamine） |
| `sz-rust-addons-loader` | 库 | 插件加载机制 |

---

## 三、sz-rust-core 核心能力（48 个模块）

### 3.1 框架核心

| 模块 | 能力 |
|------|------|
| `router` / `routing` | 三层路由：属性宏 / 配置式 / 约定式；`RouterBuilder<S>` 泛型状态支持（v0.3.0 新增） |
| `controller` | `SzController` trait + `BaseController` trait，默认方法实现 ThinkPHP 风格响应 |
| `middleware` | 20+ 中间件：auth、cors、csrf、jwt_blacklist、rate_limit、sanctum、trace、request_scope、order 等 |
| `guard` | 认证守卫：admin/auth/permission/role，支持 AND/OR 链式组合 |
| `container` | DI 容器：自动依赖注入，支持循环依赖检测 |
| `error` / `error_handler` | `AppError` 枚举 + `ErrorCode` 映射，统一错误响应格式 |
| `config` | serde_yaml 配置加载 + 环境变量覆盖 |
| `server` | axum HTTP 服务器，支持 HTTP/2 |
| `static_files` | 静态文件服务 |
| `api_version` | API 版本管理 |
| `openapi` | OpenAPI/Swagger 自动生成 |
| `websocket_route` | WebSocket 路由 |
| `multi_app` | 多应用隔离 |
| `debug_page` | 调试页面 |
| `health` | 健康检查端点 |
| `h2` | HTTP/2 服务 |

### 3.2 ORM 集成

| 模块 | 能力 |
|------|------|
| `orm` | sz-orm-core re-export，`Model` + `ModelExt` trait |
| `relation` | 关联关系：has_many / belongs_to / has_one / belongs_to_many / morph |
| `schema_cache` | 数据库 Schema 缓存（加速启动） |
| `migration_history` | 迁移历史记录 |
| `seed` | 数据填充 |
| `hooks` | Model 生命周期钩子（16 个事件） |

### 3.3 运行时

| 模块 | 能力 |
|------|------|
| `mqtt` | MQTT 消息队列（支持 real-broker feature） |
| `queue` | 内置任务队列 |
| `scheduler` | 定时任务调度器 |
| `shutdown` / `signal` / `spawn` / `worker` | 优雅关闭、信号处理、任务派生、工作线程 |
| `websocket` | WebSocket 连接管理 |

### 3.4 业务 Facade

| 模块 | 能力 |
|------|------|
| `cache` / `cache_warmer` | 缓存门面：多驱动（内存/Redis/文件），预热流水线 |
| `session` / `cookie` | 会话管理（支持 redis-session feature） |
| `event` | 事件分发 |
| `env` | 环境变量管理 |
| `i18n` | 国际化 |
| `mail` | 邮件发送 |
| `notify` | 通知（Slack 等） |
| `oauth` | OAuth2 通用接入 |
| `pay` | 支付网关 |
| `upload` | 文件上传（本地/S3/OSS），图片处理，验证 |
| `validate` | 表单验证（规则/场景/消息） |
| `view` | 模板引擎（布局/继承/渲染） |
| `log` | 日志门面 |
| `qr_code` | 二维码生成 |
| `wechat` | 微信 SDK（模板消息/支付） |
| `gateway` | API 网关 |

---

## 四、CLI 命令（24 个）

### 脚手架
| 命令 | 说明 |
|------|------|
| `make:model` | 生成 Model |
| `make:controller` | 生成 Controller（支持 `--api` / `--plain`） |
| `make:migration` | 生成迁移文件 |
| `make:seeder` | 生成填充器 |
| `make:guard` | 生成守卫 |
| `make:validate` | 生成验证器 |
| `make:event` | 生成事件类 |
| `make:listener` | 生成监听器 |
| `make:command` | 生成自定义命令 |
| `make:service` | 生成服务类 |
| `make:middleware` | 生成中间件（v0.3.0 新增） |
| `make:scaffold` | 生成完整脚手架（Model+Controller+Migration） |

### 数据库
| 命令 | 说明 |
|------|------|
| `migrate` | 运行迁移（支持 `--rollback`） |
| `migrate:status` | 迁移状态 |
| `db:seed` | 填充数据 |

### 运维
| 命令 | 说明 |
|------|------|
| `route:list` | 列出路由 |
| `cache:clear` | 清除缓存 |
| `scheduler:list` | 列出定时任务 |
| `scheduler:run` | 执行一次定时任务 |
| `scheduler:start` | 启动调度守护进程 |
| `optimize:route` | 缓存路由 |
| `optimize:config` | 缓存配置 |
| `optimize:schema` | 缓存数据库 Schema |
| `route:clear` | 清除路由缓存 |

---

## 五、宏（3 个）

| 宏 | 类型 | 说明 |
|----|------|------|
| `#[controller]` | 属性宏 | 自动实现 `SzController` trait |
| `#[model(table, pk)]` | 属性宏 | 自动实现 `Model` + `ModelExt` traits |
| `compact!(a, b, c)` | 函数式宏 | 从变量名创建 `serde_json::Map`（对齐 PHP `compact()`） |

---

## 六、可观测性（sz-rust-observability）

| 模块 | 能力 |
|------|------|
| `MetricsRegistry` | Counter / Gauge / Histogram 指标注册 |
| `SLO` | Google SRE 多窗口燃烧率告警（4-window, 2-burn-rate） |
| `OTLP` | OpenTelemetry 导出（feature `otlp`） |
| `Exporters` | Jaeger / Zipkin / 自定义 HTTP 导出器 |

---

## 七、Addon 插件生态

### addons-operate（业务 CRUD）
- **Controllers**: category, common, company, contract, contract_log, customer, customer_pay, level, rentarea, sync
- **Models**: 11 个业务模型
- **Services**: 支付集成（建行 CCB / 富友 Fuiou / 工行 ICBC）、支付成功回调、退款
- **Endpoints**: 30+ REST 接口

### addons-crm（CRM 模板）
- **Models**: Contact / Lead / Deal
- **Endpoints**: 15 REST 接口（`/api/crm`）

### addons-erp（ERP 模板）
- **Models**: Product / Supplier / PurchaseOrder
- **Endpoints**: 16 REST 接口（`/api/erp`）
- **特色**: `PurchaseOrderController::approve()` 审批流程

### addons-ecommerce（电商模板）
- **Models**: Order / OrderItem / Cart
- **Endpoints**: 13 REST 接口（`/api/ecommerce`）
- **特色**: `OrderController::cancel()` 订单取消、`CartController::clear()` 清空购物车

### addons-loader（插件加载器）
- AddonAutoload / AddonLoader / AddonManifest / AddonRegistry / AddonRoute

---

## 八、sz-rust-sz300 业务应用

**二进制**: `sz300-server`

| 层 | 组件 |
|----|------|
| Controllers | auth, common, device, file, file_serve, health, merchant, order, product（9 个） |
| Models | category, device, market, merchant, merchant_user, operate_log, order, order_item, ota_version, product, settlement, system_config（12 个） |
| Services | auth_service, device_service, file_service, health_service, merchant_service, mqtt_listener, mqtt_service, order_service, product_service（9 个） |

---

## 九、自动化 Skills（19 个）

位于 `.trae/skills/`，在 Trae AI 对话中自动/手动触发：

| Skill | 触发场景 | 模式 |
|-------|---------|------|
| `sz-rust-framework-routing` | 修改 router | auto |
| `sz-rust-framework-middleware` | 新增中间件 | auto |
| `sz-rust-framework-di` | 修改 container | auto |
| `sz-rust-framework-config` | 修改 config/static | manual |
| `sz-rust-framework-load` | 性能压测 | auto |
| `sz-rust-framework-feature-matrix` | 功能矩阵参考 | manual |
| `sz-rust-framework-php-alignment` | PHP 对齐指南 | manual |
| `sz-rust-framework-audit-quality` | 框架审计质量 | manual |
| `sz-rust-auth-guard` | 认证守卫验证 | manual |
| `sz-rust-ci-cd` | CI/CD 检查 | manual |
| `sz-rust-deploy` | 部署就绪检查 | manual |
| `sz-rust-doc-check` | rustdoc 完整性 | manual |
| `sz-rust-error-handling` | 错误处理模式 | manual |
| `sz-rust-migration` | 数据库迁移检查 | manual |
| `sz-rust-n-plus-one` | N+1 查询扫描 | manual |
| `sz-rust-orm-query` | ORM 查询模式 | manual |
| `sz-rust-performance-check` | 性能审计 | manual |
| `sz-rust-security-audit` | 安全审计 | manual |
| `sz-rust-test-coverage` | 测试覆盖率门禁 | manual |

---

## 十、测试覆盖

| 包 | 测试数 | 状态 |
|----|--------|------|
| sz-rust-core | 3500+ | ✅ 全部通过 |
| sz-rust-addons-operate | 227 | ✅ 全部通过 |
| sz-rust-addons-loader | 63 | ✅ 全部通过 |
| sz-rust-addons-crm | 4 (3 ignored) | ✅ |
| sz-rust-observability | 2 (3 ignored) | ✅ |
| sz-rust-addons-erp | 0 | ✅ |
| sz-rust-addons-ecommerce | 0 | ✅ |

**总计**: 3800+ 测试用例，0 失败。

---

## 十一、架构决策记录（12 个 ADR）

| ADR | 主题 |
|-----|------|
| 0001 | 三层路由机制（属性宏 / 配置式 / 约定式） |
| 0002 | 中间件模型（Tower Service + 洋葱模型） |
| 0003 | 控制器抽象（SzController trait + 默认方法 + 组合） |
| 0004 | Model 钩子实现（re-export sz-orm-core + 16 事件） |
| 0005 | 事务管理策略（委托 sz-orm-core） |
| 0006 | 认证授权机制（JWT + Middleware + Guard 三层分离） |
| 0007 | addon 插件化机制（编译期注册 + Cargo feature） |
| 0008 | 错误处理策略（AppError 枚举 + ErrorCode 映射） |
| 0009 | 缓存策略（Cache facade + 全局实例 + 多驱动） |
| 0010 | 配置加载方式（serde_yaml + 环境变量覆盖） |
| 0011 | 可观测性模块（MetricsRegistry + SLO 多窗口燃烧率） |
| 0012 | 分布式追踪（W3C TraceContext + OTLP exporter） |

---

## 十二、v0.3.0 新增内容

1. **RouterBuilder 泛型状态支持** — `RouterBuilder<S>` 支持任意应用状态类型，通过闭包捕获模式注入，不再依赖 `axum::extract::State<>`
2. **3 个模板 Addon 包** — CRM / ERP / E-commerce，可直接作为项目起点
3. **10 个新 Skills** — 覆盖测试覆盖、性能、文档、迁移、部署、N+1、认证守卫、错误处理、CI/CD
4. **CLI make:middleware** — 新增中间件脚手架命令
5. **sz-orm 1.2.2** — 15 个 sz-orm 子包全部升级到 1.2.2

---

## 十三、后续优化方向

### P0 — 生产阻断性

| 问题 | 说明 | 建议 |
|------|------|------|
| addons-operate 测试编译依赖未发布的核心包 | `cargo test --workspace` 因循环依赖无法完整运行 | 将 operate 的测试改为 mock 仓库，不依赖真实 sz-rust-core |
| addons-crm/erp/ecommerce 测试覆盖为 0 | 新包缺少测试 | 补充基础 CRUD 单元测试 |
| sz-rust-sz300 / sz-rust-cli 无测试 | 二进制包未覆盖 | 至少补充 CLI 命令的集成测试 |

### P1 — 质量提升

| 方向 | 说明 |
|------|------|
| **addons 测试补全** | CRM/ERP/E-commerce 各补充 10+ 单元测试，达到 70%+ 覆盖率 |
| **sz300 业务测试** | 为 9 个 service 补充单元测试，重点覆盖订单状态机 |
| **CLI 集成测试** | 为 `make:*` 命令补充文件生成验证测试 |
| **Mutation Testing** | 使用 `cargo-mutants` 对核心模块做变异测试，验证测试有效性 |
| **Soak Test** | 24h 浸泡测试，验证连接池/缓存长时间运行稳定性 |

### P2 — 功能增强

| 方向 | 说明 | 状态 |
|------|------|------|
| **Addon 热加载** | 探索运行时动态加载（`libloading`），`sz-rust-core::runtime::hot_reload`（feature `hot-reload`） | ✅ 已实现（探索性实现，编译期注册仍为主路径） |
| **OpenAPI 完善** | 自动扫描路由生成完整 spec，`routes_to_spec` / `spec_from_route_config` 自动识别 path 参数、派生 tag、合并中间件信息 | ✅ 已实现 |
| **GraphQL 集成** | 通过 sz-orm-graphql 提供 GraphQL 端点，`sz-rust-core::orm::graphql` facade（feature `graphql`） | ✅ 已实现 |
| **gRPC 支持** | 通过 sz-orm-grpc 提供 gRPC 服务，`sz-rust-core::orm::grpc` facade（feature `grpc`） | ✅ 已实现 |
| **多租户支持** | Model 层 tenant_id 自动过滤，`sz-rust-core::multi_tenant`（TenantContext / TenantAware / TenantRepository / tenant_middleware） | ✅ 已实现 |
| **API 版本化增强** | 支持 URL 路径版本（`/v1/`, `/v2/`）和 Header 版本 | ✅ 已实现 |

### P3 — 生态建设

| 方向 | 说明 |
|------|------|
| **插件市场** | 实现 `docs/plugin-marketplace-design.md` 方案，支持 addon 发现/安装/更新 |
| **Frontend SDK** | 提供 TypeScript/JavaScript SDK，封装 API 调用 |
| **Admin UI 模板** | 基于 Vue3 + Element Plus 的管理后台模板，对接 sz300 API |
| **Docker Compose 示例** | 完整的生产部署示例（app + MySQL + Redis + Prometheus + Grafana） |
| **Benchmark 对比** | 与 Actix-web / Axum 原生 / Rocket 的性能对比基准 |

---

## 十四、依赖健康度

| 指标 | 状态 |
|------|------|
| `unsafe_code` | `deny`（全 workspace；`hot-reload` FFI 模块按模块级 `allow` 豁免，符合「unsafe 仅用于 FFI」铁律） |
| 关键依赖版本 | axum 0.8, tower 0.5, tokio 1.40, serde 1, chrono 0.4 |
| sz-orm 依赖 | 15 个子包全部 1.2.2，path + version 双指定 |
| TLS | rustls（无 OpenSSL 依赖） |
| Release 优化 | LTO fat, codegen-units 1, strip, panic = abort, overflow-checks = true |

---

## 十五、crates.io 发布状态

| 包 | 版本 | 状态 |
|----|------|------|
| sz-rust-core | 0.3.0 | ✅ 已发布 |
| sz-rust-macros | 0.3.0 | ✅ 已发布 |
| sz-rust-observability | 0.3.0 | ✅ 已发布 |
| sz-rust-addons-loader | 0.3.0 | ✅ 已发布 |
| sz-rust-addons-operate | 0.3.0 | ✅ 已发布 |
| sz-rust-cli | 0.3.0 | ✅ 已发布 |
| sz-rust-sz300 | 0.3.0 | ✅ 已发布 |
| sz-rust-addons-crm | 0.3.0 | ✅ 已发布 |
| sz-rust-addons-erp | 0.3.0 | ✅ 已发布 |
| sz-rust-addons-ecommerce | 0.3.0 | ✅ 已发布 |
