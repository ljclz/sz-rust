---
name: sz-rust-framework-php-alignment
description: 检查 PHP 端关键工具/扩展在 Rust 端的对等实现，识别未对齐项。修改 sz-rust-core 模块或对比 PHP 项目时触发。
tools: [grep, glob]
agentMode: auto
---

# PHP 对齐度审计（sz-rust framework）

## 触发条件

- 修改 `packages/sz-rust-core/src/` 任何模块时
- 修改 `packages/sz-rust-cli/src/cmd/` 命令时
- 修改 `packages/sz-rust-addons-loader/` 或 `sz-rust-addons-operate/` 时
- PHP 端（`e:\vue\test\鲜视达\server\`）`composer.json` 新增依赖时
- PHP 端 `addons/` 新增插件时
- 用户询问"PHP 端的 X 功能在 Rust 端实现了吗"时
- 季度 PHP 对齐度审计时

## 审计范围

参考 PHP 项目：`e:\vue\test\鲜视达\server\`（ThinkPHP 8 + 多插件）

## PHP 关键工具对齐矩阵

### ThinkPHP 核心 Console 命令（`php think`）

| PHP 命令 | 用途 | sz-rust 对齐实现 | 状态 |
|---------|------|-----------------|------|
| `think migrate:run` | 执行数据库迁移 | `sz-rust-cli migrate run` | ✅ |
| `think migrate:rollback` | 回滚迁移 | `sz-rust-cli migrate rollback` | ✅ |
| `think migrate:status` | 迁移状态 | `sz-rust-cli migrate status` | ✅ |
| `think migrate:create` | 创建迁移文件 | `sz-rust-cli migrate create` | ✅ |
| `think make:model` | 生成模型 | `sz-rust-cli make model` | ✅ |
| `think make:controller` | 生成控制器 | `sz-rust-cli make controller` | ✅ |
| `think make:middleware` | 生成中间件 | `sz-rust-cli make middleware` | ✅ |
| `think make:validate` | 生成验证器 | `sz-rust-cli make validate` | ⚠️ 需验证 |
| `think make:event` | 生成事件类 | — | ❌ 未实现 |
| `think make:listener` | 生成监听器 | — | ❌ 未实现 |
| `think make:command` | 生成命令 | — | ❌ 未实现 |
| `think make:service` | 生成服务类 | — | ❌ 未实现 |
| `think run` | 启动开发服务器 | `sz-rust-cli serve` | ⚠️ 需验证 |
| `think route:list` | 列出所有路由 | `sz-rust-cli route list` | ✅ |
| `think route:clear` | 清空路由缓存 | — | ⚠️ 待实现 |
| `think cache:clear` | 清空缓存 | `sz-rust-cli cache clear` | ✅ |
| `think cache:warmup` | 缓存预热 | `sz-rust-cli cache warmup`（P4-3） | ✅ |
| `think db:seed` | 数据填充 | — | ❌ 未实现 |
| `think worker` | 启动 WorkerMan | `sz-rust-cli scheduler` + `runtime::worker` | ⚠️ 部分 |
| `think swoole` | 启动 Swoole | `sz-rust-core::server::run` | ⚠️ 部分 |
| `think version` | 显示版本 | `sz-rust-cli --version` | ✅ |
| `think help` | 显示帮助 | `sz-rust-cli --help` | ✅ |
| `think optimize:schema` | 数据表字段缓存 | — | ❌ 未实现 |
| `think optimize:route` | 路由缓存 | — | ❌ 未实现 |
| `think optimize:config` | 配置缓存 | — | ❌ 未实现 |

### ThinkPHP 核心扩展包（`topthink/*`）

| PHP 扩展包 | 功能 | sz-rust 对齐实现 | 状态 |
|-----------|------|-----------------|------|
| `topthink/framework` | ThinkPHP 8 框架核心 | `sz-rust-core` | ✅ |
| `topthink/think-orm` | ORM | `sz-orm-core` | ✅ |
| `topthink/think-multi-app` | 多应用支持 | `sz-rust-core::multi_app` | ✅ |
| `topthink/think-swoole` | Swoole 运行时 | `sz-rust-core::server` + `runtime` | ⚠️ 部分 |
| `topthink/think-worker` | WorkerMan 运行时 | `sz-rust-core::runtime::worker` | ⚠️ 部分 |
| `topthink/think-addons` | 插件系统 | `sz-rust-addons-loader` | ✅ |
| `topthink/think-filesystem` | 文件系统抽象（本地/七牛/阿里云/腾讯云） | `sz-orm-storage`（7 驱动） | ✅ |
| `topthink/think-trace` | 调试追踪 | `sz-rust-core::debug_page` | ✅ |
| `topthink/think-logger` | 日志（Monolog） | `sz-orm-logger` + `sz-rust-core::log` | ✅ |

### Whoops 调试页（`filp/whoops` 风格）

| Whoops 功能 | sz-rust 对齐实现 | 状态 |
|------------|-----------------|------|
| 异常堆栈页面渲染 | `debug_page::render_debug_page` | ✅ |
| 源代码片段高亮 | `debug_page` | ✅ |
| 请求信息展示 | `debug_page` | ✅ |
| 环境变量展示 | `debug_page` | ✅ |
| 数据库查询展示 | — | ⚠️ 待实现 |
| 编辑器跳转链接 | — | ❌ 未实现（PHP `xdebug.file_link_format`） |
| Pretty page handler | ✅ | ✅ |
| Plain text handler | ✅ | ✅ |
| JSON handler | ✅ | ✅ |

### 第三方 SDK 对齐

| PHP SDK | 功能 | sz-rust 状态 | 备注 |
|--------|------|-------------|------|
| `overtrue/wechat` / `w7corp/easywechat` | 微信公众号/小程序/支付 | ❌ 未实现 | 业务核心依赖 |
| `qiniu/php-sdk` | 七牛云存储 | ✅ `sz-orm-storage::qiniu` | |
| `aliyuncs/oss-sdk-php` | 阿里云 OSS | ✅ `sz-orm-storage::oss` | |
| `qcloud/cos-sdk-v5` | 腾讯云 COS | ✅ `sz-orm-storage::cos` | |
| `kosinix/grafika` | 图像处理（GD） | ✅ `sz-rust-core::upload::image` | |
| `endroid/qr-code` | 二维码生成 | ❌ 未实现 | 业务核心依赖 |
| `phpoffice/phpspreadsheet` | Excel 导入导出 | ✅ `sz-rust-pdf::excel_export` + `excel_import` | |
| `tecnickcom/tcpdf` | PDF 生成 | ✅ `sz-rust-pdf::pdf_form` | |
| `mikehaertl/php-pdftk` | PDF 表单填充 | ✅ `sz-rust-pdf::pdf_form` | |
| `yansongda/pay` | 支付聚合（支付宝/微信） | ❌ 未实现 | 业务核心依赖 |
| `alipaysdk/easysdk` | 支付宝 SDK | ❌ 未实现 | |
| `wechatpay/wechatpay` | 微信支付 SDK | ❌ 未实现 | |
| `tencentcloud/tencentcloud-sdk-php` | 腾讯云 SDK（短信/COS 等） | ❌ 未实现 | |
| `zoujingli/wechat-developer` | 微信开发者工具 | ❌ 未实现 | |
| `yuntian001/multi-process-queue` | 多进程队列 | ⚠️ `sz-orm-queue` 部分 | |
| `workerman/mqtt` | MQTT 客户端 | ✅ `sz-orm-mqtt` | |
| `php-mqtt/client` | MQTT 客户端（备用） | ✅ `sz-orm-mqtt` | |
| `phpseclib/phpseclib` | 加密（RSA/AES） | ✅ `sz-orm-auth` | |
| `fabpot/goutte` | 爬虫 | ❌ 未实现 | |
| `myclabs/php-enum` | 枚举类型 | ✅ Rust 原生 `enum` | |
| `lvht/geohash` | GeoHash | ❌ 未实现 | |
| `lcobucci/jwt` | JWT 签发/校验 | ✅ `sz-orm-auth::jwt` | |
| `firebase/php-jwt` | JWT 备用 | ✅ `sz-orm-auth::jwt` | |
| `php-di/php-di` | DI 容器 | ✅ `sz-rust-core::container` | |

### PHP 端核心基类对齐

| PHP 类 | 功能 | sz-rust 对齐实现 | 状态 |
|--------|------|-----------------|------|
| `think\App` | 应用容器 | `sz-rust-core::container::Container` | ✅ |
| `think\BaseController` | 控制器基类 | `sz-rust-core::controller::BaseController` | ✅ |
| `app\SzController` | 业务控制器基类 | `sz-rust-core::controller::SzController` | ✅ |
| `addons\BaseController` | 插件控制器基类 | `sz-rust-addons-loader::BaseController` | ⚠️ 需验证 |
| `addons\Common` | 插件公共方法 | — | ⚠️ 待实现 |
| `addons\WebController` | 插件 Web 控制器基类 | — | ⚠️ 待实现 |
| `think\Request` | 请求对象 | `sz-rust-core::request::Request` | ✅ |
| `think\Response` | 响应对象 | `sz-rust-core::response::Response` | ✅ |
| `think\Db` | 数据库查询 | `sz-orm-core::query` | ✅ |
| `think\Model` | 模型基类 | `sz-rust-core::model::Model` | ✅ |
| `think\Validate` | 验证器 | `sz-rust-core::validate::Validator` | ✅ |
| `think\File` | 文件上传 | `sz-rust-core::upload::UploadedFile` | ✅ |
| `think\facade\Cache` | 缓存门面 | `sz-rust-core::cache` | ✅ |
| `think\facade\Session` | 会话门面 | `sz-rust-core::session` | ✅ |
| `think\facade\Cookie` | Cookie 门面 | `sz-rust-core::cookie` | ✅ |
| `think\facade\Event` | 事件门面 | `sz-rust-core::event` | ✅ |
| `think\facade\Env` | 环境变量门面 | `sz-rust-core::env` | ✅ |
| `think\facade\Lang` | 多语言门面 | `sz-rust-core::i18n` | ✅ |
| `think\facade\Mail` | 邮件门面 | `sz-rust-core::mail` | ✅ |
| `think\facade\Config` | 配置门面 | `sz-rust-core::config` | ✅ |
| `think\facade\Log` | 日志门面 | `sz-rust-core::log` | ✅ |
| `think\facade\Route` | 路由门面 | `sz-rust-core::router` | ✅ |
| `think\facade\Middleware` | 中间件门面 | `sz-rust-core::middleware::builder` | ✅ |
| `think\Exception` | 异常基类 | `sz-rust-core::error::BaseException` | ✅ |
| `think\console\Command` | 命令基类 | `sz-rust-cli::console::Command` | ✅ |
| `think\console\Input` | 命令输入 | `sz-rust-cli::console::Input` | ✅ |
| `think\console\Output` | 命令输出 | `sz-rust-cli::console::Output` | ✅ |

### PHP 端业务插件对齐（`addons/`）

每个插件对应一个 Rust 包，参考 `sz-rust-addons-operate` 的实现模式。

| PHP 插件 | 功能 | sz-rust 实现状态 | 优先级 |
|---------|------|----------------|--------|
| `addons/operate` | 运营管理（合同/客户/支付） | ✅ `sz-rust-addons-operate` | — |
| `addons/assetm` | 资产管理 | ❌ 未实现 | 中 |
| `addons/basic` | 基础配置 | ❌ 未实现 | 高 |
| `addons/cashier` | 收银台 | ❌ 未实现 | 高 |
| `addons/charge` | 充值 | ❌ 未实现 | 中 |
| `addons/erp` | ERP（盘点/出入库） | ❌ 未实现 | 中 |
| `addons/examine` | 审批 | ❌ 未实现 | 低 |
| `addons/finance` | 财务 | ❌ 未实现 | 中 |
| `addons/food` | 餐饮 | ❌ 未实现 | 中 |
| `addons/hr` | 人事 | ❌ 未实现 | 低 |
| `addons/huiyi` | 会议 | ❌ 未实现 | 低 |
| `addons/material` | 物料 | ❌ 未实现 | 中 |
| `addons/monitor` | 监控 | ❌ 未实现 | 低 |
| `addons/news` | 资讯 | ❌ 未实现 | 低 |
| `addons/opinion` | 投诉建议 | ❌ 未实现 | 低 |
| `addons/patrol` | 巡检 | ❌ 未实现 | 低 |
| `addons/poster` | 海报 | ❌ 未实现 | 低 |
| `addons/price` | 价格 | ❌ 未实现 | 中 |
| `addons/repair` | 报修 | ❌ 未实现 | 低 |
| `addons/sale` | 销售 | ❌ 未实现 | 中 |
| `addons/scene` | 场景 | ❌ 未实现 | 中 |
| `addons/score` | 积分 | ❌ 未实现 | 中 |
| `addons/sdp` | 外卖（饿了么） | ❌ 未实现 | 中 |
| `addons/sharing` | 共享 | ❌ 未实现 | 低 |
| `addons/special` | 专题 | ❌ 未实现 | 低 |
| `addons/spread` | 推广 | ❌ 未实现 | 低 |
| `addons/task` | 任务 | ❌ 未实现 | 中 |
| `addons/tools` | 工具 | ❌ 未实现 | 低 |
| `addons/video` | 视频 | ❌ 未实现 | 中 |
| `addons/wmall` | 外卖 | ❌ 未实现 | 中 |
| `addons/worklogs` | 工作日志 | ❌ 未实现 | 低 |
| `addons/zhdp` | 智慧大屏 | ❌ 未实现 | 中 |

### PHP 端配置文件对齐

| PHP 配置 | 用途 | sz-rust 对齐实现 | 状态 |
|---------|------|-----------------|------|
| `config/app.php` | 应用配置 | `config/app.yml` | ✅ |
| `config/database.php` | 数据库配置 | `config/database.yml` | ✅ |
| `config/cache.php` | 缓存配置 | `config/app.yml` | ✅ |
| `config/session.php` | 会话配置 | `config/app.yml` | ✅ |
| `config/cookie.php` | Cookie 配置 | `config/app.yml` | ✅ |
| `config/log.php` | 日志配置 | `config/log.yml` | ✅ |
| `config/route.php` | 路由配置 | `config/app.yml` | ✅ |
| `config/view.php` | 视图配置 | — | ⚠️ 待实现 |
| `config/lang.php` | 多语言配置 | `config/app.yml` | ✅ |
| `config/console.php` | 命令行配置 | — | ⚠️ 待实现 |
| `config/middleware.php` | 中间件配置 | 代码定义 | ✅ |
| `config/swoole.php` | Swoole 配置 | `config/app.yml` | ✅ |
| `config/worker.php` | WorkerMan 配置 | `config/app.yml` | ✅ |
| `config/mp-queue.php` | 多进程队列配置 | — | ⚠️ 待实现 |
| `config/gateway_worker.php` | GatewayWorker 配置 | — | ❌ 未实现 |
| `config/worker_server.php` | WorkerServer 配置 | — | ❌ 未实现 |
| `config/filesystem.php` | 文件系统配置 | `config/app.yml` | ✅ |
| `config/captcha.php` | 验证码配置 | — | ❌ 未实现 |
| `config/addons.php` | 插件配置 | `config/app.yml` | ✅ |
| `config/annotation.php` | 注解配置 | — | ✅ 通过宏 |
| `config/trace.php` | 调试追踪配置 | `config/app.yml` | ✅ |
| `.env` | 环境变量 | `.env` | ✅ |

## 执行步骤

1. **扫描 PHP 项目**：
   - `e:\vue\test\鲜视达\server\composer.json` — 列出所有依赖
   - `e:\vue\test\鲜视达\server\addons\` — 列出所有插件
   - `e:\vue\test\鲜视达\server\config\` — 列出所有配置
   - `e:\vue\test\鲜视达\server\app\` — 列出所有应用模块

2. **扫描 sz-rust 项目**：
   - `e:\vue\test\鲜视达\rust\sz-rust\packages\` — 列出所有 Rust 包
   - `packages\sz-rust-core\src\lib.rs` — 列出已实现模块
   - `packages\sz-rust-cli\src\cmd\` — 列出已实现命令

3. **逐项对比上述矩阵**：
   - 对每个 ✅ 项：用 `grep` 验证实际代码存在（而非仅文件存在）
   - 对每个 ⚠️ 项：打开对应文件，验证实现深度（如是否覆盖 PHP 端的所有方法）
   - 对每个 ❌ 项：确认确实未实现，并评估业务影响

4. **生成报告**，包含：
   - PHP 项目依赖清单（来自 `composer.json`）
   - PHP 插件清单（来自 `addons/`）
   - 已对齐功能列表（✅）+ 代码位置
   - 部分对齐功能列表（⚠️）+ 缺失的具体方法/字段
   - 未对齐功能列表（❌）+ 业务影响评估
   - 迁移优先级建议（高/中/低）

5. **优先级评估原则**：
   - **高**：影响生产可用性或核心业务（如支付、微信、收银台）
   - **中**：影响业务功能完整性（如 ERP、销售、积分）
   - **低**：辅助功能（如海报、资讯、工作日志）

## 通过标准

- 所有 ❌ 高优先级项有明确的实现计划或文档说明（"暂不迁移"也需说明）
- 所有 ⚠️ 项的缺失方法/字段已记录到 `docs/功能基线清单.md`
- 报告中的状态有代码验证支持，而非主观判断
- PHP `composer.json` 中每个 `require` 依赖都有对应的 Rust 实现或迁移说明
- PHP `addons/` 中每个插件都有对应的 Rust 实现或迁移说明

## 输出文件

- `docs/audit/YYYY-MM-DD-PHP对齐度审计.md`
- 更新 `docs/功能基线清单.md`（如不存在则创建）

## 与其他审计 Skill 的区别

| Skill | 关注点 |
|-------|--------|
| `sz-rust-framework-feature-matrix` | 对比 ThinkPHP 8/Laravel/NestJS **功能清单**，识别未实现项 |
| `sz-rust-framework-php-alignment` | 对比 **PHP 项目实际使用的工具/SDK/插件**，识别未对齐项 |
| `sz-rust-framework-routing` | 路由变异测试 |
| `sz-rust-framework-middleware` | 中间件混沌测试 |
| `sz-rust-framework-config` | 配置一致性测试 |
| `sz-rust-framework-di` | DI 容器变异测试 |
| `sz-rust-framework-load` | 负载基线测试 |

**关键区别**：`feature-matrix` 是"框架层面应该有什么"，`php-alignment` 是"PHP 项目实际用了什么、Rust 端是否对齐"。
