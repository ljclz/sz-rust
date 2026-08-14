# sz-rust 路线图与实施文档

> ⚠️ 已归档，原因：部分过时，任务状态需核实，归档日期：2026-08-09

> **制定日期**：2026-08-02  
> **版本**：v1.0  
> **状态**：Level 4 定量管理级 → 冲刺 Level 5 优化级  
> **综合评分**：91.2 / 100

---

## 勘误表（v3 审计报告修正）

| 项目 | v3 报告内容 | 实际情况 | 修正 |
|------|------------|---------|------|
| sz-orm 版本 | v1.0.0 | **v1.2.1**（Cargo.lock 已解析，workspace Cargo.toml 仍 pin 1.0.0） | 更新 workspace 依赖版本声明 |
| Rate limiting | P2 待实施 | **已实现**（`sz-rust-core/src/middleware/rate_limit.rs`，复用 sz-orm-limit，支持 SlidingWindow + TokenBucket） | 标记为已完成，补充配置文档 |
| 生产案例 | 无 | **sz-pay 已在生产使用** | 补充生产案例章节 |
| 第三方安全审计 | 仅外部团队 | **AI 辅助审计可行**（静态分析 + 模式匹配 + 模糊测试） | 新增 AI 安全审计方案 |

---

## 一、任务总览与优先级

| # | 任务 | 优先级 | 状态 | 预计工时 | 负责人 |
|---|------|--------|------|---------|--------|
| T1 | sz-orm 版本声明更新（1.0.0 → 1.2.1） | P1 | ⏳ 待执行 | 0.5h | 维护者 |
| T2 | rustdoc 文档完善 | P2 | ⏳ 待执行 | 16h | 维护者 |
| T3 | Rate limiting 配置文档完善 | P2 | ✅ 已完成 | — | — |
| T4 | AI 辅助安全审计方案 | P2 | ⏳ 待执行 | 8h | 维护者 + AI |
| T5 | sz-pay 生产案例整理 | P2 | ⏳ 待执行 | 4h | 维护者 |
| T6 | 插件市场方案设计 | P3 | ⏳ 待执行 | 24h | 维护者 |
| T7 | CLI 增强（脚手架 + 代码生成） | P3 | ⏳ 待执行 | 20h | 维护者 |
| T8 | addons 生态（CRM/ERP/电商模板） | P3 | ⏳ 待执行 | 40h | 维护者 |
| T9 | .trae/skills/ 扩展 | P3 | ⏳ 待执行 | 12h | 维护者 |

---

## 二、T1：sz-orm 版本声明更新

### 2.1 现状

workspace `Cargo.toml` 中 sz-orm 依赖仍 pin `version = "1.0.0"`，但 `Cargo.lock` 已解析到 `1.2.1`。

```toml
# 当前（需更新）
sz-orm-core = { path = "../sz-orm/packages/sz-orm-core", version = "1.0.0" }
```

### 2.2 执行步骤

1. 确认 sz-orm 仓库最新版本号
2. 批量更新 workspace `Cargo.toml` 中所有 sz-orm 依赖版本
3. 运行 `cargo check` 验证兼容性
4. 提交并推送

### 2.3 命令

```bash
cd sz-rust
# 批量更新版本（示例：1.0.0 → 1.2.1）
python -c "
import re
with open('Cargo.toml', 'r') as f:
    content = f.read()
content = re.sub(r'(sz-orm-\w+ = \{ path = [^,]+, version = )\"1\.0\.0\"', r'\g<1>\"1.2.1\"', content)
with open('Cargo.toml', 'w') as f:
    f.write(content)
print('Updated sz-orm versions to 1.2.1')
"

cargo check --workspace
git add Cargo.toml Cargo.lock
git commit -m "chore(deps): bump sz-orm to 1.2.1"
```

---

## 三、T2：rustdoc 文档完善

### 3.1 目标

为核心公共 API 添加 rustdoc 文档，生成 docs.rs 风格的在线文档，降低新用户上手成本 50%。

### 3.2 覆盖范围

| 模块 | 优先级 | 关键 trait/struct | 文档要求 |
|------|--------|-------------------|---------|
| `controller` | P0 | `SzController` | trait 方法说明 + 使用示例 |
| `middleware` | P0 | `SzMiddleware`, `RateLimitConfig` | 中间件链说明 + 配置示例 |
| `router` | P0 | `Router`, `Route` | 路由定义示例 |
| `request` | P1 | `RequestContext` | 请求上下文说明 |
| `response` | P1 | `ApiResponse` | 响应格式化说明 |
| `cache` | P1 | `Cache`, `MemoryCache`, `RedisCache` | 多级缓存使用示例 |
| `orm` | P1 | `Model`, `Relation`, `WithRelation` | ORM 使用示例 |
| `di` | P2 | `Container` | DI 容器使用说明 |

### 3.3 文档模板

```rust
/// 控制器 trait — 所有业务控制器的基类
///
/// # 快速开始
///
/// ```rust,no_run
/// use sz_rust_core::controller::SzController;
/// use sz_rust_core::response::ApiResponse;
/// use serde_json::json;
///
/// struct MyController;
/// impl SzController for MyController {}
///
/// let ctrl = MyController;
/// let resp = ctrl.render_success("操作成功", json!({"id": 1}));
/// ```
///
/// # 响应格式
///
/// 所有控制器响应遵循统一格式：
///
/// ```json
/// {
///   "code": 1,
///   "msg": "操作成功",
///   "data": { "id": 1 },
///   "total": 0
/// }
/// ```
pub trait SzController: Sized {
    // ...
}
```

### 3.4 执行步骤

1. **Phase 1（4h）**：核心 trait 文档（controller, middleware, router）
2. **Phase 2（4h）**：request/response 文档
3. **Phase 3（4h）**：cache/orm 文档
4. **Phase 4（4h）**：示例代码验证 + `cargo doc` 生成验证

### 3.5 验收标准

- [ ] 所有 `pub trait` 和 `pub struct` 有文档注释
- [ ] 每个文档至少一个 `/// # Examples` 代码块
- [ ] `cargo doc --no-deps` 无 `missing_docs` 警告
- [ ] 示例代码可通过 `cargo test --doc` 验证

---

## 四、T3：Rate limiting 配置文档（已完成）

### 4.1 现状

Rate limiting 中间件已实现（`sz-rust-core/src/middleware/rate_limit.rs`）：

- **算法**：SlidingWindow + TokenBucket（复用 sz-orm-limit）
- **Key 提取**：Ip / UserId / IpPlusRoute
- **位置**：中间件链第 4 位（鉴权之前）
- **Fail-open**：limiter 错误时放行

### 4.2 待补充

- [ ] 配置示例（`config/rate_limit.yaml`）
- [ ] 不同场景的限流策略推荐
- [ ] 监控指标（`X-RateLimit-Remaining` 等）

---

## 五、T4：AI 辅助安全审计方案

### 5.1 可行性分析

传统第三方安全审计成本高（5-20 万/次），AI 辅助审计可作为补充手段：

| 审计类型 | AI 可行性 | 说明 |
|---------|----------|------|
| 静态代码分析 | ✅ 高 | 模式匹配 + 规则引擎 |
| 依赖漏洞扫描 | ✅ 高 | cargo-audit + AI 解读 |
| 模糊测试 | ✅ 中 | cargo-fuzz + AI 生成用例 |
| 渗透测试 | ⚠️ 中 | 需结合真实环境 |
| 业务逻辑审计 | ⚠️ 中 | 需领域知识 |

### 5.2 AI 审计工具链

```
┌─────────────────────────────────────────────────────────────┐
│                    AI 安全审计流水线                          │
├─────────────────────────────────────────────────────────────┤
│  Stage 1: 静态分析                                           │
│  ├── cargo-audit（依赖漏洞扫描）                              │
│  ├── cargo-geiger（unsafe code 检测）                        │
│  └── AI 规则引擎（SQL 注入/XSS/CSRF 模式匹配）                │
├─────────────────────────────────────────────────────────────┤
│  Stage 2: 模糊测试                                           │
│  ├── cargo-fuzz（基于 AFL 的模糊测试）                        │
│  └── AI 生成边界用例（空值/超长/特殊字符）                     │
├─────────────────────────────────────────────────────────────┤
│  Stage 3: 渗透测试                                           │
│  ├── OWASP ZAP（Web 漏洞扫描）                                │
│  └── AI 分析扫描结果 + 生成修复建议                            │
├─────────────────────────────────────────────────────────────┤
│  Stage 4: 报告生成                                           │
│  ├── 风险等级分类（Critical/High/Medium/Low）                 │
│  └── 修复建议 + 代码定位                                      │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 实施步骤

#### Step 1：配置 cargo-audit CI 集成

```yaml
# .github/workflows/security.yml
name: Security Audit
on:
  schedule:
    - cron: '0 0 * * 0'  # 每周日
  push:
    paths:
      - '**/Cargo.toml'
      - '**/Cargo.lock'

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install cargo-audit
        run: cargo install cargo-audit
      - name: Run audit
        run: cargo audit
      - name: AI Analysis (optional)
        # 调用 AI API 分析审计结果
        run: python scripts/ai_audit_analysis.py
```

#### Step 2：配置 cargo-geiger

```yaml
# .github/workflows/safety.yml
name: Safety Check
on: [push, pull_request]

jobs:
  geiger:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install cargo-geiger
        run: cargo install cargo-geiger
      - name: Run geiger
        run: cargo geiger --include-tests
```

#### Step 3：AI 规则引擎（自定义 Skill）

创建 `.trae/skills/sz-rust-security-audit/SKILL.md`：

```markdown
---
name: sz-rust-security-audit
description: AI 辅助安全审计，检测 SQL 注入、XSS、CSRF、敏感信息泄露等漏洞
agentMode: auto
---

# sz-rust 安全审计 Skill

## 审计范围

1. **SQL 注入**：检查所有 SQL 拼接，确认参数化绑定
2. **XSS**：检查用户输入输出，确认转义处理
3. **CSRF**：检查表单/状态修改操作，确认 CSRF token
4. **敏感信息泄露**：检查日志/响应，确认脱敏
5. **认证授权**：检查鉴权中间件覆盖范围
6. **文件上传**：检查文件类型/大小限制
7. **密码安全**：检查哈希算法/盐值

## 审计命令

```bash
# 全量审计
cargo audit
cargo geiger --include-tests
cargo fuzz run --help

# AI 辅助审计
@sz-rust-security-audit 执行全量安全审计
```

## 通过标准

- 零 Critical/High 级别漏洞
- Medium 级别漏洞有明确的修复计划
- forbid(unsafe_code) 全覆盖
```

#### Step 4：模糊测试配置

```rust
// fuzz/fuzz_targets/request_parser.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use sz_rust_core::request::parse_post_data;

fuzz_target!|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse_post_data(s.as_bytes());
    }
};
```

### 5.4 验收标准

- [ ] CI 集成 cargo-audit（每周自动扫描）
- [ ] cargo-geiger 零 unsafe（已满足）
- [ ] AI 审计 Skill 可触发
- [ ] 模糊测试覆盖核心解析器
- [ ] 生成 AI 安全审计报告

---

## 六、T5：sz-pay 生产案例整理

### 6.1 案例信息收集

| 项目 | 内容 |
|------|------|
| 项目名称 | sz-pay |
| 使用场景 | 支付网关核心服务 |
| 部署规模 | （待填写） |
| 性能指标 | QPS / 延迟 / 内存占用 |
| 稳定性 | 运行时长 / 故障次数 |
| 迁移收益 | 性能提升 / 成本降低 |

### 6.2 案例文档模板

```markdown
# 生产案例：sz-pay

## 项目概述

sz-pay 是基于 sz-rust 框架开发的支付网关核心服务，负责...

## 技术栈

- sz-rust-core: v0.2.1
- sz-orm: v1.2.1
- Rust: 1.81+
- 数据库: MySQL 8.0 / PostgreSQL 15

## 部署架构

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Nginx     │ ──→ │ sz-pay × 3  │ ──→ │   MySQL     │
│  (LB)       │     │ (Docker)    │     │ (主从)      │
└─────────────┘     └─────────────┘     └─────────────┘
```

## 性能指标

| 指标 | 值 | 说明 |
|------|-----|------|
| QPS | — | 峰值 |
| p99 延迟 | — | |
| 内存占用 | — | 单实例 |
| 可用性 | — | SLA |

## 迁移收益

| 维度 | PHP 版本 | Rust 版本 | 提升 |
|------|---------|-----------|------|
| 性能 | — | — | — |
| 内存 | — | — | — |
| 稳定性 | — | — | — |

## 经验总结

...
```

### 6.3 执行步骤

1. 联系 sz-pay 项目负责人收集数据
2. 填写案例文档模板
3. 发布到 `docs/cases/sz-pay.md`
4. 在 README.md 中添加案例链接

---

## 七、T6：插件市场方案设计

### 7.1 目标

建立 sz-rust 插件市场，允许开发者发布和分享插件，形成生态。

### 7.2 架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                    sz-rust 插件市场                          │
├─────────────────────────────────────────────────────────────┤
│  插件分类                                                    │
│  ├── 认证插件（OAuth2 / SAML / LDAP）                        │
│  ├── 存储插件（OSS / S3 / 七牛云）                           │
│  ├── 消息插件（短信 / 邮件 / 推送）                          │
│  ├── 支付插件（微信 / 支付宝 / 银联）                        │
│  ├── 监控插件（Prometheus / SkyWalking）                     │
│  └── 业务插件（CRM / ERP / 电商）                            │
├─────────────────────────────────────────────────────────────┤
│  插件格式                                                    │
│  ├── Cargo.toml（Rust crate）                               │
│  ├── plugin.json（插件元数据）                               │
│  └── README.md（使用说明）                                   │
├─────────────────────────────────────────────────────────────┤
│  分发方式                                                    │
│  ├── crates.io（官方插件）                                   │
│  ├── Git 仓库（社区插件）                                    │
│  └── 本地目录（私有插件）                                    │
└─────────────────────────────────────────────────────────────┘
```

### 7.3 插件元数据格式

```json
{
  "name": "sz-plugin-oauth2",
  "version": "1.0.0",
  "description": "OAuth2 认证插件",
  "author": "your-name",
  "license": "MIT",
  "sz_rust_version": ">=0.2.0",
  "category": "auth",
  "tags": ["oauth2", "sso", "authentication"],
  "entry_point": "Oauth2Plugin",
  "config_schema": {
    "client_id": "string",
    "client_secret": "string",
    "redirect_uri": "string"
  },
  "dependencies": []
}
```

### 7.4 插件接口

```rust
use sz_rust_core::plugin::{Plugin, PluginContext, PluginResult};

pub struct Oauth2Plugin;

#[sz_rust_core::plugin]
impl Plugin for Oauth2Plugin {
    fn name(&self) -> &'static str { "oauth2" }
    
    fn version(&self) -> &'static str { "1.0.0" }
    
    fn on_init(&self, ctx: &PluginContext) -> PluginResult {
        // 注册路由
        ctx.router.get("/auth/oauth2/login", login_handler);
        ctx.router.get("/auth/oauth2/callback", callback_handler);
        
        // 注册中间件
        ctx.middleware.add(Oauth2Middleware);
        
        Ok(())
    }
    
    fn on_shutdown(&self) -> PluginResult {
        Ok(())
    }
}
```

### 7.5 实施步骤

1. **Phase 1（8h）**：定义 Plugin trait 和插件接口
2. **Phase 2（8h）**：实现插件加载器（`sz-rust-addons-loader` 增强）
3. **Phase 3（8h）**：创建 2-3 个示例插件
4. **Phase 4（4h）**：编写插件开发文档

---

## 八、T7：CLI 增强

### 8.1 现状

`sz-rust-cli` 已提供基础脚手架功能。

### 8.2 增强目标

| 功能 | 描述 | 优先级 |
|------|------|--------|
| `sz new` | 创建新项目（交互式向导） | P0 |
| `sz make:controller` | 生成控制器（含 CRUD 模板） | P0 |
| `sz make:model` | 生成模型（含 migration） | P0 |
| `sz make:middleware` | 生成中间件 | P1 |
| `sz make:service` | 生成服务层 | P1 |
| `sz migrate` | 数据库迁移管理 | P1 |
| `sz serve` | 本地开发服务器 | P2 |
| `sz build` | 生产构建 | P2 |

### 8.3 命令设计

```bash
# 创建新项目
sz new my-project --template web|api|microservice
sz new my-project --db mysql|postgres --orm sz-orm

# 代码生成
sz make:controller User --resource          # 资源控制器（CRUD）
sz make:controller Auth --thin              # 轻量控制器
sz make:model User --migration              # 模型 + 迁移文件
sz make:middleware RateLimit                # 中间件
sz make:service UserService                 # 服务层
sz make:job ProcessPayment                  # 异步任务

# 数据库
sz migrate:create create_users_table
sz migrate:run
sz migrate:rollback
sz migrate:status

# 开发
sz serve --port 8080 --reload               # 热重载开发服务器
sz build                                     # 生产构建
sz test                                      # 运行测试
```

### 8.4 代码生成模板

```rust
// templates/controller.rs.j2
//! {{ name }} 控制器

use sz_rust_core::controller::SzController;
use sz_rust_core::request::fetch_post_data;
use axum::{body::Body, extract::State, http::Request, response::Response};
use serde_json::json;

struct {{ name }}Controller;
impl SzController for {{ name }}Controller {}

impl {{ name }}Controller {
    /// 列表查询
    pub async fn index(_state: &AppState, req: Request<Body>) -> Response {
        let ctrl = {{ name }}Controller;
        // TODO: 实现列表查询逻辑
        ctrl.render_success("ok", json!({}))
    }
}

#[tracing::instrument(skip(state, req))]
pub async fn index(State(state): State<AppState>, req: Request<Body>) -> Response {
    {{ name }}Controller::index(&state, req).await
}
```

### 8.5 实施步骤

1. **Phase 1（8h）**：重构 CLI 架构（使用 clap derive）
2. **Phase 2（8h）**：实现代码生成引擎（基于 Tera/Minijinja）
3. **Phase 3（4h）**：创建模板文件
4. **Phase 4（2h）**：编写 CLI 使用文档

---

## 九、T8：addons 生态（CRM/ERP/电商模板）

### 9.1 目标

提供企业级业务模板，开箱即用。

### 9.2 模板规划

| 模板 | 功能 | 优先级 | 预计工时 |
|------|------|--------|---------|
| **sz-addons-crm** | 客户管理、销售漏斗、跟进记录 | P1 | 16h |
| **sz-addons-erp** | 采购、库存、销售、财务 | P1 | 24h |
| **sz-addons-ecommerce** | 商品、订单、支付、物流 | P1 | 20h |
| **sz-addons-cms** | 内容管理、页面、SEO | P2 | 12h |
| **sz-addons-hrm** | 员工、考勤、薪资 | P2 | 16h |

### 9.3 sz-addons-crm 功能清单

```
sz-addons-crm
├── 客户管理
│   ├── 客户列表（分页、筛选、导出）
│   ├── 客户详情（基本信息、联系人、跟进记录）
│   ├── 客户导入（Excel/CSV）
│   └── 客户公海（分配、领取）
├── 销售漏斗
│   ├── 线索管理
│   ├── 商机管理
│   └── 成交管理
├── 跟进记录
│   ├── 跟进类型（电话/拜访/邮件）
│   ├── 跟进提醒
│   └── 跟进统计
└── 报表分析
    ├── 客户统计
    ├── 销售漏斗分析
    └── 跟进效率分析
```

### 9.4 实施步骤

1. **Phase 1（8h）**：设计数据模型和 API 规范
2. **Phase 2（16h）**：实现 CRUD 服务层
3. **Phase 3（8h）**：实现控制器层
4. **Phase 4（8h）**：编写使用文档和示例

---

## 十、T9：.trae/skills/ 扩展

### 10.1 现有 Skills

| Skill | 触发场景 | 模式 |
|------|---------|------|
| sz-rust-framework-routing | 修改 router | auto |
| sz-rust-framework-middleware | 新增中间件 | auto |
| sz-rust-framework-di | 修改 container | auto |
| sz-rust-framework-config | 修改 config/static | manual |
| sz-rust-framework-load | 性能压测 | auto |
| sz-rust-framework-php-alignment | PHP 对齐检查 | manual |
| sz-rust-framework-feature-matrix | 功能矩阵 | manual |
| sz-rust-framework-audit-quality | 审计质量 | manual |

### 10.2 扩展计划

| 新 Skill | 触发场景 | 模式 | 说明 |
|---------|---------|------|------|
| sz-rust-security-audit | 安全审计 | auto | T4 AI 安全审计 |
| sz-rust-make-controller | 生成控制器 | manual | T7 CLI 增强 |
| sz-rust-make-model | 生成模型 | manual | T7 CLI 增强 |
| sz-rust-make-middleware | 生成中间件 | manual | T7 CLI 增强 |
| sz-rust-test-coverage | 测试覆盖检查 | auto | 检测未覆盖公共函数 |
| sz-rust-performance-check | 性能检查 | auto | N+1 检测、慢查询检测 |
| sz-rust-doc-check | 文档检查 | auto | 检测缺失 rustdoc |
| sz-rust-migration | 数据库迁移 | manual | 生成 migration 文件 |
| sz-rust-deploy | 部署 | manual | Docker/K8s 部署 |
| sz-rust-orm-query | ORM 查询辅助 | manual | 辅助编写 ORM 查询 |

### 10.3 sz-rust-security-audit SKILL.md

```markdown
---
name: sz-rust-security-audit
description: AI 辅助安全审计，检测 SQL 注入、XSS、CSRF、敏感信息泄露等漏洞
agentMode: auto
---

# sz-rust 安全审计 Skill

## 触发条件

- 修改涉及用户输入处理的代码
- 新增认证/授权逻辑
- 修改数据库查询
- 提交前自动触发

## 审计范围

### 1. SQL 注入检测

```rust
// ❌ 错误：字符串拼接 SQL
let sql = format!("SELECT * FROM users WHERE name = '{}'", name);

// ✅ 正确：参数化查询
let sql = "SELECT * FROM users WHERE name = ?";
```

### 2. XSS 检测

```rust
// ❌ 错误：直接输出用户输入
resp.body(user_input);

// ✅ 正确：转义后输出
resp.body(escape_html(user_input));
```

### 3. CSRF 检测

```rust
// ❌ 错误：状态修改操作无 CSRF 保护
POST /api/users/delete

// ✅ 正确：携带 CSRF token
POST /api/users/delete
Headers: X-CSRF-Token: <token>
```

### 4. 敏感信息泄露检测

```rust
// ❌ 错误：日志输出密码
tracing::info!("login attempt: user={}, password={}", user, password);

// ✅ 正确：脱敏处理
tracing::info!("login attempt: user={}, password=***", user);
```

### 5. 认证授权检测

- 检查敏感接口是否有鉴权中间件
- 检查权限粒度是否合理
- 检查 JWT 过期时间是否合理

### 6. 文件上传检测

- 检查文件类型白名单
- 检查文件大小限制
- 检查文件路径遍历防护

### 7. 密码安全检测

- 检查哈希算法（bcrypt/argon2）
- 检查盐值生成
- 检查密码强度验证

## 通过标准

- 零 Critical/High 级别漏洞
- Medium 级别漏洞有修复计划
- forbid(unsafe_code) 全覆盖
```

### 10.4 实施步骤

1. **Phase 1（4h）**：创建 sz-rust-security-audit Skill
2. **Phase 2（4h）**：创建 sz-rust-test-coverage Skill
3. **Phase 3（4h）**：创建 sz-rust-performance-check Skill
4. **Phase 4（2h）**：配置 preCommitCheck 自动触发

---

## 十一、执行计划与里程碑

### 11.1 Sprint 规划

| Sprint | 周期 | 任务 | 交付物 |
|--------|------|------|--------|
| Sprint 1 | Week 1-2 | T1, T2, T3 | 版本更新 + rustdoc + 限流文档 |
| Sprint 2 | Week 3-4 | T4, T5 | AI 审计方案 + 生产案例 |
| Sprint 3 | Week 5-7 | T6, T7 | 插件市场 + CLI 增强 |
| Sprint 4 | Week 8-10 | T8, T9 | addons 生态 + Skills 扩展 |

### 11.2 里程碑

| 里程碑 | 目标日期 | 验收标准 |
|--------|---------|---------|
| M1: 文档完善 | 2026-08-16 | rustdoc 覆盖率 > 80% |
| M2: 安全加固 | 2026-08-30 | AI 审计报告 + 生产案例 |
| M3: 生态建设 | 2026-09-14 | 插件市场 + CLI 增强 |
| M4: Level 5 | 2026-09-28 | 综合评分 > 95 |

---

## 附录 A：快速检查清单

### 提交前检查

```bash
# 1. 代码检查
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check

# 2. 测试检查
RUST_MIN_STACK=8388608 cargo test --workspace -- --test-threads=1

# 3. 安全检查
cargo audit
cargo geiger --include-tests

# 4. 文档检查
cargo doc --no-deps

# 5. AI 门禁检查
@sz-rust-framework-audit-quality 执行全量安全门禁
```

### 发布前检查

```bash
# 1. 版本号更新
# 修改 Cargo.toml workspace.package.version

# 2. CHANGELOG 更新
# 修改 CHANGELOG.md

# 3. 发布到 crates.io
cargo publish --workspace

# 4. Git tag
git tag -a v0.2.2 -m "release: v0.2.2"
git push origin v0.2.2
```

---

*本文档为 sz-rust 项目路线图与实施指南，后续优化请参考执行。*
