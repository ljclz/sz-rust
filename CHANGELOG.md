# 变更日志

本项目所有重要变更均会记录在此文件中。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增
- ADR-001 ~ ADR-010 架构决策记录
- criterion 性能基准测试框架（sz-rust-core/benches/core_bench.rs）
- GitHub Actions benchmark workflow
- 初始审计报告（P0 全通过 / P1 需改进 2 项）
- 性能回归基线文档（baseline-v0.1.0.md）

### 变更
- workspace.package 补全 description/repository/homepage/keywords/categories 元数据

### 修复
- 无

## [0.1.0] - 2026-07-22

### 新增
- **框架核心**：sz-rust-core 28 模块就绪（controller/model/relation/middleware/guard/hooks/multi_app/health/h2/routing/addons/cache/event/validate/upload/view 等）
- **路由系统**：三层路由机制（属性宏 / 配置式 / 约定式），对齐 PHP `auto_multi_app` + `config/route.php`
- **中间件**：Tower Service + 洋葱模型，5 个内置中间件（Trace/Cors/Log/RateLimit/Auth）
- **控制器**：SzController trait + BaseController，对齐 PHP `app\SzController`
- **Model 钩子**：16 事件 HookDispatcher（PHP 原生 12 + sz-orm-core 扩展 4）
- **Guard 守卫**：鉴权决策层（AuthGuard/PermissionGuard/GuardChain），借鉴 NestJS
- **响应格式**：`{code, msg, data}` 标准响应，对齐 PHP `renderJson/renderSuccess/renderError`
- **错误体系**：BaseException + 9 个错误码（对齐 PHP + Rust 扩展）
- **缓存系统**：Cache facade + 多驱动（Memory/Redis），对齐 PHP `think\facade\Cache`
- **配置系统**：YAML 加载 + 环境变量覆盖 + 默认值，对齐 PHP `config/*.php`
- **验证器**：规则/场景/消息三件套，对齐 PHP `think\Validate`
- **事件系统**：事件监听器 + 订阅者，对齐 PHP `think\Event`
- **上传**：文件上传 + 图像处理（对齐 PHP `think\Filesystem` + `Grafika`）
- **视图**：模板渲染 + 布局继承，对齐 PHP `think\View`
- **多应用**：`auto_multi_app` 路径解析（oapc/admin/api/farm/oapi/cashier/scene）
- **HTTP/2**：完整 HTTP/2 支持（含 h2c upgrade）
- **插件系统**：addon 插件化机制
- **PDF 处理**：sz-rust-pdf 独立包
- **CLI 工具**：sz-rust-cli 命令行工具
- **业务示例**：sz-rust-addons-operate（375 测试，控制器+服务层迁移完成）
- **示例应用**：sz-rust-examples/crud_demo 完整 CRUD 示例
- **工程化**：10 道门禁（fmt/check/clippy/test/doc/audit/integration + 占位检查/安全扫描/feature 全组合）
- **CI**：GitHub Actions 7 道门禁
- **测试**：2938+ 测试通过（sz-rust-core 2563 + sz-rust-addons-operate 375）
- **文档**：README.md、LICENSE(MIT)、ADR 规范、审计清单、工程化实践规范

### 测试
- 2938+ 测试全部通过
- clippy 0 警告
- fmt 0 差异

## 版本对比链接

[Unreleased]: https://github.com/ljclz/sz-rust/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ljclz/sz-rust/releases/tag/v0.1.0
