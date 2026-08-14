# sz-rust 产品技术方案

> **版本**：v1.0  
> **编写日期**：2026-08-11  
> **性质**：sz-rust 产品化开发的权威技术指南，后续所有开发围绕本文档执行  
> **前置文档**：`docs/cases/` 目录下全部战略讨论文档  
> **关联文档**：`docs/adr/`（架构决策记录）、`.trae/rules/project_rules.md`（22 条铁律）

---

## 目录

1. [产品定位与愿景](#一产品定位与愿景)
2. [商业模式：开源版与企业版](#二商业模式开源版与企业版)
3. [目标用户与核心场景](#三目标用户与核心场景)
4. [当前能力盘点](#四当前能力盘点)
5. [能力缺口分析](#五能力缺口分析)
6. [整体技术架构](#六整体技术架构)
7. [Capability Registry 设计](#七capability-registry-设计)
8. [SDD Agent 编排设计](#八sdd-agent-编排设计)
9. [插件系统：数据互通与模板机制](#九插件系统数据互通与模板机制)
10. [AI 生成流水线](#十ai-生成流水线)
11. [开源版/企业版分离设计](#十一开源版企业版分离设计)
12. [分阶段实施路线图](#十二分阶段实施路线图)
13. [风险与应对](#十三风险与应对)

---

# 一、产品定位与愿景

## 1.1 一句话定位

> **sz-rust 是一个 AI-native 的通用 Rust Web 框架——用低代码的易用性，生成高代码的能力，交付完整的源代码和数据主权。**

三个关键词缺一不可：
- **AI-native**：AI 是核心生产力，不是附加功能。框架内置 LLM/Agent/RAG/MCP，每个 sz-rust 应用天然具备 AI 能力。
- **通用框架**：框架本身不包含任何行业逻辑。行业能力通过插件提供。框架对标 ThinkPHP 8 / Laravel，服务任意行业的 Web 应用开发。
- **高代码**：输出标准 Rust 源代码 + 编译产物，不是平台专属配置。应用可脱离 AI 独立运行、独立维护。

## 1.2 与竞品的本质区别

| 维度 | 低代码（秒搭/简道云） | 通用 AI IDE（Cursor/Windsurf） | **sz-rust** |
|------|---------------------|---------------------------|-------------|
| 输出产物 | 平台配置（JSON/XML） | 代码文件 | **代码文件 + 框架运行时** |
| 能力上限 | 平台能力边界 | 无上限 | **无上限（Rust 能做的都能生成）** |
| 数据主权 | SaaS 托管，数据在平台 | 本地，数据在自己手里 | **自部署，数据完全自控** |
| AI 角色 | 辅助搭建界面 | 辅助写代码 | **核心生产力（生成插件 + 迁移存量）** |
| 框架感知 | 无 | 浅（通用） | **深（22 条铁律内置合规检查）** |
| 目标用户 | 非技术人员 | 专业开发者 | **中小企业技术负责人** |

## 1.3 三层价值模型

```
第三层（生态层）：插件市场 + AI 生成
  → 非开发者描述需求 → AI 生成插件 → 热加载运行
  → 第三方开发者发布行业插件 → 市场交易

第二层（AI 层）：内置 AI 能力
  → 每个 sz-rust 应用天然有 LLM/RAG/Agent
  → MCP 工具桥接框架能力给 AI 调用

第一层（框架层）：高性能 Rust Web 框架
  → 对标 ThinkPHP 8 的 Rust 方案
  → axum 0.8 + sz-orm 3.5 + 22 条铁律
```

## 1.4 与"微擎模式"的关系

sz-rust 是 **AI 时代的微擎**，但不是简单重复：

```
微擎（2013-2018）的五大死因          sz-rust 的对应解法
──────────────────────────────────────────────────────────────
1. 平台风险（寄生微信生态）     →   独立框架，不寄生任何平台
2. 技术代际（PHP 过时）         →   Rust 是下一代系统编程语言
3. SaaS 竞争（微盟/有赞）       →   自部署 + 热加载降低运维成本
4. 模块质量参差                 →   22 条铁律 + AI 生成合规检查
5. 开发者变现难                 →   AI 降低开发门槛 + 插件市场

新增优势：
6. AI 解决供给瓶颈              →   不需要等开发者手动写插件
7. 数据主权趋势                 →   SaaS 疲劳创造市场时机
```

## 1.5 不做的事情（明确边界）

```
❌ 不做低代码平台
   不生成平台专属配置，不限制能力边界

❌ 不做 SaaS 托管平台
   不自建应用托管（与数据主权定位矛盾）
   但可提供托管作为可选项（与自部署并存）

❌ 不做通用 AI IDE
   不直接与 Cursor/Windsurf 竞争
   sz-rust 是框架 + 运行时，不是编辑器

❌ 不将行业逻辑写入框架核心
   菜市场/餐饮/零售逻辑全部在插件层
   框架核心保持通用性
```

---

# 二、商业模式：开源版与企业版

## 2.1 核心原则：从第一天设计分离

开源版和企业版的分离**不是发布时才考虑**，而是从 crate 结构设计、许可证配置、发布流程的**第一天**就确定。

```
错误做法（会导致重构）：
  先做一个版本 → 以后再分离 → 发现代码耦合无法分离 → 需要重构

正确做法（本文档要求）：
  设计 crate 结构时明确边界 → 开发时严格遵守 → 发布时自然分离
```

## 2.2 开源版（Community Edition）

### 发布渠道
- **GitHub**：`https://github.com/ljclz/sz-rust`（完整源码）
- **crates.io**：各核心 crate 独立发布

### 包含内容

```
开源核心 crate（全部 Apache-2.0 或 MIT 许可）：

├── sz-rust-core              框架核心（路由、中间件、DI、多租户、多应用）
├── sz-rust-ai-facade         AI 能力抽象（LLM、Agent、RAG、MCP 桥接）
├── sz-rust-orm-facade        ORM 统一入口（sz-orm 全家桶）
├── sz-rust-orm-ext-facade    ORM 扩展（模型、钩子、关系）
├── sz-rust-router-facade     路由层（三层路由 + OpenAPI）
├── sz-rust-middleware-facade 中间件层（14 个 Tower 中间件）
├── sz-rust-mvc-facade        MVC 层（控制器、守卫、视图）
├── sz-rust-http-facade       HTTP 原语（响应、错误、请求）
├── sz-rust-cache-facade      缓存抽象（Memory/Redis/Memcached/多级）
├── sz-rust-state-facade      应用状态（Session/Cookie/I18n/Mail/Event）
├── sz-rust-infra-facade      基础设施（配置、校验、上传、静态文件）
├── sz-rust-auth-facade       认证（微信、OAuth2、SSO、网关）
├── sz-rust-pay-facade        支付（支付宝、微信支付）
├── sz-rust-macros            过程宏（路由注册、DI、SQL 校验）
├── sz-rust-addons-loader     插件加载机制（发现、注册、热加载）
├── sz-rust-mcp               MCP 工具服务器（7 个框架工具）
├── sz-rust-cli               命令行工具（make/migrate/route/cache）
├── sz-rust-observability     可观测性（Prometheus、SLO）
├── sz-rust-tracing           分布式追踪（W3C TraceContext）
├── sz-rust-pdf               PDF/Excel 处理
├── sz-rust-examples          开源示例项目
└── sz-rust-facade-tests      集成测试套件
```

### 许可证
- 核心框架：**Apache-2.0**（允许商业使用、修改、分发，需保留许可证声明）
- 部分工具 crate：**MIT**（更宽松）

### 不包含的内容（这些在企业版）
- 行业插件包（菜市场、餐饮、零售等）
- 企业级功能插件（SSO 企业集成、审计日志、高可用集群）
- AI 辅助迁移工具（TP6→Rust）
- SDD Agent 企业版（多 Agent 协作、可视化画布）
- 行业 RAG 知识库

## 2.3 企业版（Enterprise Edition）

### 发布渠道
- **私有 crate registry**（Cloudsmith / Artifactory / 自建）
- **独立下载**（`.crate` 文件 + `sz-rust-cli plugin install`）
- **SaaS 托管**（预装企业版插件的托管环境）

### 包含内容

```
企业版插件包（商业许可，禁止转售/再分发）：

行业插件包：
├── sz-addons-market          菜市场行业插件包
│   ├── stall                 摊位管理（租赁、合同、费用）
│   ├── cashier               收银系统（称重、支付、对账）
│   ├── merchant              商户管理（入驻、资质、评分）
│   ├── delivery              配送管理（外卖接单、调度）
│   ├── supply-chain          供应链（采购、库存、损耗）
│   ├── dashboard             数据大屏（交易实时展示）
│   └── food-safety           食安监管（溯源、抽检、公示）
│
├── sz-addons-restaurant      餐饮行业插件包（待开发）
├── sz-addons-retail          零售行业插件包（待开发）
└── ...（其他行业插件包）

企业级功能插件：
├── sz-plugin-sso-enterprise  企业 SSO 集成（LDAP/AD/SAML/OIDC）
├── sz-plugin-audit           审计日志 + 合规报表
├── sz-plugin-ha              高可用集群管理
├── sz-plugin-migration       AI 辅助迁移工具（TP6→Rust）
└── sz-plugin-workflow        工作流引擎（审批流、状态机）

SDD Agent 企业版：
├── sz-sdd-agent              四阶段 SDD Agent 编排
├── sz-sdd-canvas             可视化应用搭建画布（Tauri + Vue）
└── sz-sdd-knowledge          行业 RAG 知识库
```

### 许可证
- **商业许可**（每插件单独授权）
- 授权模式：按插件购买 / 年度订阅 / 按租户数

### 收费模式

```
收费维度：

1. 插件授权费
   ├── 行业插件包：¥X,XXX - ¥XX,XXX / 包（一次性）
   ├── 企业功能插件：¥X,XXX - ¥X,XXX / 插件（一次性）
   └── SDD Agent 企业版：¥XX,XXX / 年（订阅）

2. 订阅服务费
   ├── 技术支持：¥X,XXX / 月
   ├── 版本更新：包含在订阅中
   └── 安全补丁：包含在订阅中

3. AI 调用分成（可选）
   └── 内置 LLM 调用的收入分成

4. 托管服务费（可选）
   └── 自部署之外的托管选项
```

## 2.4 收费插件 vs 收费应用的区别

```
收费插件（Plugin）：
  ├── 扩展框架能力的模块
  ├── 安装到 sz-rust 框架中运行
  ├── 目标用户：开发者 / 技术负责人
  ├── 收费模式：一次性购买 / 订阅
  └── 示例：菜市场收银插件、餐饮配送插件

收费应用（Application）：
  ├── 基于 sz-rust + 插件构建的完整业务系统
  ├── 可以独立部署，也可以作为 SaaS
  ├── 目标用户：最终业务用户（菜市场管理者、餐饮店主）
  ├── 收费模式：SaaS 订阅 / 一次性授权 / 按租户
  └── 示例：菜市场数字化管理系统（含收银+商户+配送+大屏）

关键区别：
  插件 = 开发者购买，扩展框架能力
  应用 = 业务用户使用，解决业务问题
```

## 2.5 开源版与企业版的关系

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│   开源版（免费）                    企业版（收费）            │
│   ─────────────                   ────────────              │
│   框架核心                        行业插件包                  │
│   基础 AI 能力                    企业级功能插件              │
│   插件加载机制                    SDD Agent 企业版            │
│   7 个 MCP 工具                   AI 辅助迁移工具             │
│   CLI 基础命令                    行业 RAG 知识库             │
│   基础示例                        可视化画布                  │
│                                                             │
│   ┌─────────────────────────────────────────────────────┐   │
│   │              升级路径                                │   │
│   │                                                     │   │
│   │   开发者用开源版开发        →    购买企业版插件部署    │   │
│   │   评估框架能力              →    生产环境使用企业版    │   │
│   │   社区支持                  →    企业技术支持          │   │
│   │                                                     │   │
│   │   开源版是企业版的"试用"和"基础"                    │   │
│   │   企业版是开源版的"增值"和"变现"                    │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                             │
│   关键原则：                                                 │
│   - 开源版功能完整，可以独立运行生产应用                     │
│   - 企业版提供行业深度和企业级能力                           │
│   - 企业版插件依赖开源版核心，不修改核心                     │
│   - 开源版用户可以随时升级到企业版                           │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

# 三、目标用户与核心场景

## 3.1 目标用户：中小企业技术负责人

```
┌─────────────────────────────────────────────────────────────┐
│  画像：中小企业 / 特定行业的技术负责人                        │
│                                                             │
│  特征：                                                       │
│  ├── 有业务系统（TP6/Laravel/旧框架）在运行                   │
│  ├── 有行业 Know-How（懂业务，不只是懂技术）                  │
│  ├── 对性能/安全有诉求（现有系统有瓶颈）                      │
│  ├── 尝试过 AI 编程工具（Cursor、Windsurf 等）                │
│  ├── 觉得 SDD 方法更安心                                     │
│  └── 一个人维护多个系统（人力有限）                           │
│                                                             │
│  痛点：                                                       │
│  ├── 现有系统想升级但成本高                                   │
│  ├── 想用 Rust 但学习曲线太陡                                 │
│  ├── 想引入 AI 但不知道怎么做                                 │
│  └── 业务需求多，开发排期排不过来                             │
│                                                             │
│  不是目标用户：                                               │
│  ❌ 完全不懂技术的非技术人员（用秒搭/简道云）                  │
│  ❌ 专业 Rust 开发者（直接用 axum + 手写）                     │
│  ❌ 大型互联网公司的专业开发团队（用 Cursor/Windsurf）         │
└─────────────────────────────────────────────────────────────┘
```

## 3.2 核心使用场景

### 场景一：AI 辅助新建应用

```
用户：某生鲜配送公司技术负责人
需求：搭建一个"配送管理系统"

流程：
1. 用户在 sz-rust CLI 或可视化界面描述需求
2. SDD Agent 生成规格（spec.md）
3. 用户确认规格
4. SDD Agent 生成代码（符合 22 条铁律）
5. 自动编译 + 测试 + 热加载
6. 应用运行，附带 SDD 文档（spec/design/tasks）

价值：
- 无需手写 Rust 代码
- 生成的代码符合框架规范
- 有规格文档可追溯，可审查
```

### 场景三：AI 辅助迁移存量系统

```
用户：使用 ThinkPHP 6 的中小企业技术负责人
需求：将现有 TP6 系统迁移到 Rust

流程：
1. 用户提供 TP6 代码库路径
2. 迁移工具分析 TP6 代码（路由、模型、控制器）
3. 生成 sz-rust 等价代码（路由 → axum，模型 → sz-orm）
4. 增量验证（对比 TP6 和 sz-rust 的响应一致性）
5. 逐个模块切换，每个模块有回滚方案
6. 最终完成迁移

价值：
- 不需要重写，是迁移
- 有参照物，验证更容易
- 渐进式，风险可控
```

### 场景四：安装行业插件快速搭建

```
用户：菜市场管理者（有技术负责人）
需求：快速搭建菜市场管理系统

流程：
1. 购买"菜市场行业插件包"（企业版）
2. `sz-rust-cli plugin install sz-addons-market`
3. 配置数据库 + 商户数据导入
4. 系统上线（摊位管理 + 收银 + 商户 + 配送 + 大屏）

价值：
- 无需从零开发
- 行业最佳实践内置
- 开箱即用
```

### 场景五：跨插件数据互通

```
用户：已安装 CRM 插件和电商插件的企业
需求：电商订单自动同步到 CRM 客户记录

流程：
1. CRM 插件和电商插件共享统一用户系统
2. 电商订单创建时触发事件
3. CRM 插件监听事件，自动更新客户记录
4. 跨插件查询：查询某客户的订单 + 跟进记录

价值：
- 不是孤立应用，是集成系统
- 统一用户、统一权限、统一数据
- 这是 sz-rust 区别于秒搭等工具的核心差异化
```

---

# 四、当前能力盘点

## 4.1 已完成能力（可直接使用）

### 框架核心层

| Crate | 功能 | 成熟度 | 说明 |
|-------|------|--------|------|
| `sz-rust-core` | 框架核心（DI、多租户、多应用） | ✅ 完成 | 1200+ 行容器测试，800+ 行多租户测试 |
| `sz-rust-macros` | 过程宏（路由注册、DI、SQL 校验） | ✅ 完成 | `sql_string!`、`query!` 宏 |
| `sz-rust-router-facade` | 三层路由 + OpenAPI | ✅ 完成 | 属性宏/配置/约定三层路由 |
| `sz-rust-middleware-facade` | 14 个 Tower 中间件 | ✅ 完成 | Auth、ScopeId、日志等 |
| `sz-rust-mvc-facade` | MVC 层（控制器、守卫、视图） | ✅ 完成 | 对齐 ThinkPHP 8 MVC |
| `sz-rust-http-facade` | HTTP 原语（响应、错误、请求） | ✅ 完成 | `ApiResponse` 等 |
| `sz-rust-cache-facade` | 缓存抽象（4 种驱动） | ✅ 完成 | Memory/Redis/Memcached/多级 |
| `sz-rust-state-facade` | 应用状态（7 个模块） | ✅ 完成 | Session/Cookie/I18n/Mail/Event/Env/Notify |
| `sz-rust-infra-facade` | 基础设施 | ✅ 完成 | 配置、校验、上传、静态文件、调试页 |
| `sz-rust-orm-facade` | ORM 统一入口 | ✅ 完成 | sz-orm 3.5 全家桶统一入口 |
| `sz-rust-orm-ext-facade` | ORM 扩展 | ✅ 完成 | 模型、钩子、关系抽象 |
| `sz-rust-auth-facade` | 认证（微信/OAuth2/SSO） | ✅ 完成 | 多 feature flag 生产就绪 |
| `sz-rust-pay-facade` | 支付（支付宝/微信） | ✅ 完成 | 统一支付抽象 |
| `sz-rust-addons-loader` | 插件加载 + 热加载 | ✅ 完成 | M2 安全热加载（进程重启+状态迁移） |
| `sz-rust-mcp` | MCP 工具服务器 | ✅ 完成 | 7 个工具，JSON-RPC 2.0 |
| `sz-rust-cli` | 命令行工具 | ✅ 完成 | make/migrate/route/cache/scheduler |
| `sz-rust-observability` | 可观测性 | ✅ 完成 | Prometheus、SLO、Grafana 模板 |
| `sz-rust-tracing` | 分布式追踪 | ✅ 完成 | W3C TraceContext 传播 |
| `sz-rust-pdf` | PDF/Excel 处理 | ✅ 完成 | 对齐 phpspreadsheet + php-pdftk |
| `sz-rust-operator` | K8s Operator | ✅ 完成 | CRD 派生 + Controller |
| `sz-rust-examples` | 示例项目 | ✅ 完成 | 含热加载示例 |
| `sz-rust-sz300` | 业务示例应用 | ✅ 完成 | 设备/商户/商品/订单管理 |
| `sz-rust-facade-tests` | 集成测试套件 | ✅ 完成 | 跨 facade 集成测试 |

### AI 能力层

| Crate | 功能 | 成熟度 | 说明 |
|-------|------|--------|------|
| `sz-rust-ai-facade` | AI 统一抽象 | ✅ 完成 | LLM（OpenAI/Claude/Gemini）、Agent、RAG、MCP 桥接 |

`sz-rust-ai-facade` 详细能力：
- **LLM**：多提供商（OpenAI/Claude/Gemini）、模型路由 + 故障转移、Token 计数 + 上下文截断、流式输出
- **Embedding**：OpenAI 兼容 + 本地 Embedding、批量分块
- **RAG**：检索管道、引用溯源、警告码
- **Agent**：工具选择循环、短期/长期记忆、终止策略
- **MCP Bridge**：将 `sz-rust-mcp` 的 7 个工具暴露为 Agent 工具

### 业务插件层（已有但需完善）

| Crate | 功能 | 成熟度 | 说明 |
|-------|------|--------|------|
| `sz-rust-addons-cms` | CMS 插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-crm` | CRM 插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-ecommerce` | 电商插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-erp` | ERP 插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-forum` | 论坛插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-im` | IM 插件 | ⚠️ 部分完成 | 需完善 |
| `sz-rust-addons-operate` | 运营插件 | ⚠️ 部分完成 | 需完善 |

## 4.2 已有能力总结

```
已完成（可直接用于生产）：
├── 框架核心：完整（对标 ThinkPHP 8 的 Rust 实现）
├── AI 基础设施：完整（LLM/Agent/RAG/MCP）
├── 插件机制：完整（加载 + 热加载）
├── CLI 工具：完整
├── 可观测性：完整
└── 22 条铁律 + 19 个 Skills：完整

需完善：
├── 业务插件：部分完成，需作为企业版首发内容完善
└── AI 生成能力：基础设施有，编排层缺失

缺失（需新建）：
├── Capability Registry（统一能力注册表）
├── SDD Agent 编排（四阶段 Agent）
├── 插件数据互通机制
├── AI 辅助迁移工具
├── 可视化应用搭建画布
├── 行业 RAG 知识库
├── 开源版/企业版分离的 crate 结构
└── 插件市场基础设施
```

---

# 五、能力缺口分析

## 5.1 缺口总览

基于产品定位（AI-native 通用框架 + 开源版/企业版分离），当前缺失的核心能力分为 **P0（必须有）**、**P1（应该有）**、**P2（可以有）** 三个优先级。

## 5.2 P0 缺口（产品化的前提）

### P0-1：Capability Registry（统一能力注册表）

**问题**：当前 Skills（AI 视角的能力）和 Plugins（用户视角的能力）是两套独立系统，没有统一抽象。AI 无法发现和调用业务插件，业务插件也无法暴露给 AI。

**影响**：没有 Capability Registry，"AI 生成插件"和"AI 调用业务插件"都无法实现。

**解决方案**：新建 `sz-rust-capability` crate，定义统一的 `Capability` trait，Skills 和 Plugins 都实现这个 trait。

### P0-2：SDD Agent 编排

**问题**：当前 `sz-rust-ai-facade` 有基础的 Agent 引擎（工具选择循环），但没有四阶段 SDD 编排（Spec/Design/Task/Coding），没有 HITL 闸门，没有存量分析。

**影响**：无法实现"规格先行"的 AI 生成流程，生成的代码缺乏可追溯性。

**解决方案**：新建 `sz-rust-sdd-agent` crate（企业版），实现四阶段 Agent 编排。

### P0-3：开源版/企业版 crate 分离

**问题**：当前 workspace 中所有 crate 混在一起，没有明确的开源/企业边界。发布到 GitHub/crates.io 时可能混入企业版代码。

**影响**：无法安全地开源核心框架，也无法保护企业版代码。

**解决方案**：重构 workspace 结构，将企业版 crate 移到独立的 workspace 或子目录，配置独立的发布流程。

### P0-4：插件数据互通机制

**问题**：当前插件是孤立的，没有跨插件的数据共享机制（统一用户系统、跨插件查询、事件通知、权限继承）。

**影响**：生成的应用是孤立的应用，不是集成系统。这失去了 sz-rust 最核心的差异化优势。

**解决方案**：在框架核心层设计"共享 Schema"机制，定义跨插件数据互通的标准接口。

## 5.3 P1 缺口（差异化竞争力）

### P1-1：AI 辅助迁移工具

**问题**：没有从 TP6/Laravel 迁移到 sz-rust 的自动化工具。

**影响**：无法利用"迁移"作为获客渠道。

**解决方案**：新建 `sz-plugin-migration`（企业版），分析存量代码 → 生成 sz-rust 等价代码 → 增量验证。

### P1-2：行业 RAG 知识库

**问题**：AI 生成代码时没有行业上下文，生成的是通用 CRUD，没有行业特色。

**影响**：生成的应用缺乏行业深度。

**解决方案**：构建行业 RAG 知识库（基于创始人 29+ 项目代码），让 AI 生成时能检索行业最佳实践。

### P1-3：插件模板库

**问题**：AI 从零生成代码，质量不稳定（Rust 借位检查器导致 50-60% 成功率）。

**影响**：生成质量低，用户信任度低。

**解决方案**：预置经过验证的插件模板（Few-shot 示例），AI 基于模板生成而非从零生成。

### P1-4：MCP 工具扩展

**问题**：当前只有 7 个 MCP 工具，不足以支持复杂的 AI 生成任务。

**影响**：AI 可调用的框架能力有限。

**解决方案**：从 7 个扩展到 15+ 个 MCP 工具（覆盖 CRUD 操作、迁移、测试、部署等）。

## 5.4 P2 缺口（生态增强）

### P2-1：可视化应用搭建画布

**问题**：没有拖拽式的可视化界面。

**影响**：非技术用户无法使用。

**解决方案**：Tauri + Vue 构建桌面工作bench（远期）。

### P2-2：插件市场基础设施

**问题**：没有插件发布、安装、更新、交易的基础设施。

**影响**：第三方开发者无法发布插件。

**解决方案**：构建插件市场（Web 平台 + CLI 集成）。

### P2-3：前端生成

**问题**：当前只生成后端 Rust 代码，前端需要手写。

**影响**：不是完整的"应用生成"。

**解决方案**：根据数据模型自动生成前端页面（Vue/React）。

### P2-4：工作流引擎

**问题**：没有审批流、状态机等业务逻辑编排能力。

**影响**：复杂业务流程无法表达。

**解决方案**：内置工作流引擎（远期）。

---

# 六、整体技术架构

## 6.1 架构全景图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              用户交互层                                      │
│                                                                             │
│   ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐            │
│   │   CLI 界面       │  │  可视化画布      │  │  Web 管理台     │            │
│   │  (sz-rust-cli)  │  │  (Tauri+Vue)    │  │  (axum+Vue)     │            │
│   └────────┬────────┘  └────────┬────────┘  └────────┬────────┘            │
│            │                    │                    │                      │
│            └────────────────────┼────────────────────┘                      │
│                                 ▼                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                              AI 生成层（企业版）                              │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                      SDD Agent 编排引擎                              │   │
│   │                                                                     │   │
│   │   ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐     │   │
│   │   │ Spec     │ →  │ Design   │ →  │ Task     │ →  │ Coding   │     │   │
│   │   │ Agent    │    │ Agent    │    │ Agent    │    │ Agent    │     │   │
│   │   │ 需求规格  │    │ 技术设计  │    │ 任务规划  │    │ 代码生成  │     │   │
│   │   └──────────┘    └──────────┘    └──────────┘    └──────────┘     │   │
│   │        │              │              │              │               │   │
│   │        └──────────────┴──────┬───────┴──────────────┘               │   │
│   │                               ▼                                      │   │
│   │                    ┌────────────────────┐                           │   │
│   │                    │   HITL 闸门        │  ← 用户确认 tasks.md      │   │
│   │                    │  (Phase 3→4)       │                           │   │
│   │                    └────────────────────┘                           │   │
│   │                                                                     │   │
│   │   辅助组件：                                                         │   │
│   │   ├── 多模型路由（推理模型 → 代码模型）                               │   │
│   │   ├── 行业 RAG 知识库（检索行业最佳实践）                             │   │
│   │   ├── Compile-Fix Agent（编译错误自动修复循环）                       │   │
│   │   └── Spec 持久化（specs/{feature}/）                               │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                 │                                           │
│                                 ▼                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                           Capability Registry 层                             │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                     Capability Registry                              │   │
│   │                                                                     │   │
│   │   ┌──────────────┐    ┌──────────────┐    ┌──────────────┐          │   │
│   │   │    Skill     │    │   Plugin     │    │   Service    │          │   │
│   │   │  (AI内置能力) │    │  (业务插件)   │    │  (框架服务)   │          │   │
│   │   │              │    │              │    │              │          │   │
│   │   │ - LLM 调用   │    │ - 摊位管理   │    │ - 文件 IO    │          │   │
│   │   │ - 代码搜索   │    │ - 收银系统   │    │ - HTTP 请求  │          │   │
│   │   │ - 文件操作   │    │ - 商户管理   │    │ - 数据库     │          │   │
│   │   │ - ...        │    │ - ...        │    │ - ...        │          │   │
│   │   └──────┬───────┘    └──────┬───────┘    └──────┬───────┘          │   │
│   │          │                   │                   │                  │   │
│   │          └───────────────────┼───────────────────┘                  │   │
│   │                              ▼                                      │   │
│   │              ┌─────────────────────────────────┐                    │   │
│   │              │      Capability Trait           │                    │   │
│   │              │  - name() -> &str               │                    │   │
│   │              │  - description() -> &str        │                    │   │
│   │              │  - schema() -> Value            │                    │   │
│   │              │  - tags() -> &[&str]            │                    │   │
│   │              │  - call(args) -> Result<Value>  │                    │   │
│   │              │  - source() -> CapabilitySource │                    │   │
│   │              └─────────────────────────────────┘                    │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                 │                                           │
│                                 ▼                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                              框架运行时层                                     │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────────────────┐   │
│   │                        sz-rust-core                                  │   │
│   │                                                                     │   │
│   │   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐           │   │
│   │   │ 路由层   │  │ 中间件   │  │ DI 容器  │  │ 插件     │           │   │
│   │   │ (axum)   │  │ 层       │  │          │  │ 运行时   │           │   │
│   │   └──────────┘  └──────────┘  └──────────┘  └──────────┘           │   │
│   │   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐           │   │
│   │   │ 多租户   │  │ 多应用   │  │ ORM      │  │ 热加载   │           │   │
│   │   │ 隔离     │  │ 分发     │  │ Facade   │  │ 机制     │           │   │
│   │   └──────────┘  └──────────┘  └──────────┘  └──────────┘           │   │
│   │                                                                     │   │
│   │   铁律合规检查（22 条）：                                            │   │
│   │   ├── 内存安全：overflow-checks, no unsafe                          │   │
│   │   ├── 异步安全：tokio::fs, IO timeout, no lock across await         │   │
│   │   ├── 安全脱敏：敏感字段自动 skip_serializing                       │   │
│   │   ├── 性能基线：p99 < 5ms, 启动 < 30MB                              │   │
│   │   ├── 数据库安全：参数化绑定, 列投影, N+1 检测                       │   │
│   │   ├── 并发模型：Send + 'static                                       │   │
│   │   └── 文档同步：pub API 变更必须更新文档                             │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
│                                 │                                           │
│                                 ▼                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                              基础设施层                                       │
│                                                                             │
│   Rust 运行时 / tokio / axum 0.8 / sz-orm 3.5 / Redis / PostgreSQL / MySQL │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## 6.2 关键架构决策

### 决策一：框架核心保持通用，行业逻辑在插件层

```
正确：
  sz-rust-core（通用）
  └── sz-addons-market（菜市场插件，企业版）
      └── stall（摊位管理）
      └── cashier（收银系统）

错误（禁止）：
  sz-rust-core
  └── market（菜市场逻辑直接写在核心里）
```

### 决策二：开源版和企业版从 crate 结构上分离

```
两个独立的 workspace：

sz-rust/（开源，发布到 GitHub/crates.io）
├── packages/sz-rust-core/
├── packages/sz-rust-ai-facade/
├── ...（所有开源 crate）
└── Cargo.toml（workspace members 只含开源 crate）

sz-rust-enterprise/（企业版，私有发布）
├── packages/sz-addons-market/
├── packages/sz-plugin-migration/
├── packages/sz-sdd-agent/
└── Cargo.toml（workspace members 只含企业版 crate）
    └── 依赖开源版的 sz-rust（通过 crates.io 或私有 registry）
```

### 决策三：插件数据互通通过"共享 Schema"机制实现

```
插件 A（CRM）                插件 B（电商）
├── 用户表（共享）            ├── 用户表（共享，同一张表）
├── 客户表（A 专属）          ├── 订单表（B 专属）
└── 跟进记录表（A 专属）      └── 订单事件（发布）

互通机制：
├── 共享 Schema：框架定义统一的用户表、权限表、事件表
├── 数据扩展点：插件可以扩展共享表（额外字段）
├── 事件总线：插件可以发布/监听跨插件事件
└── 跨插件查询：通过 Capability Registry 调用其他插件的查询能力
```

### 决策四：SDD 文档作为 AI 生成的交付物

```
AI 生成插件的产出：
├── 源代码（Rust 代码）
├── spec.md（需求规格）
├── design.md（技术设计）
├── tasks.md（任务清单）
├── 测试（cargo test 通过）
└── 编译产物（可热加载的插件包）

价值：
├── 用户可以审查规格，而不只是审查代码
├── 修改时先改规格，再由 AI 重新生成
└── 规格文档是知识资产（培训、交接、审计）
```

---

# 七、Capability Registry 设计

## 7.1 为什么需要 Capability Registry

当前 sz-rust 有两套"能力"系统：

```
Skills（AI 视角）：
  ├── 定义位置：.trae/skills/（Markdown 文件）
  ├── 触发方式：AI 对话中自动触发
  ├── 作用：指导 AI 如何操作框架
  └── 问题：不是运行时能力，AI 无法在运行时调用

Plugins（用户视角）：
  ├── 定义位置：packages/sz-rust-addons-*/
  ├── 触发方式：HTTP 路由分发
  ├── 作用：提供业务功能
  └── 问题：AI 无法发现和调用业务插件

这两套系统互不相通。Capability Registry 的目标是统一它们。
```

## 7.2 Capability Trait 设计

```rust
// 文件：packages/sz-rust-capability/src/lib.rs

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use serde_json::Value;

/// 统一能力 Trait —— 同时服务 AI 和人类用户
/// 
/// 这是 sz-rust 能力系统的核心抽象。
/// Skill（AI 内置能力）、Plugin（业务插件）、Service（框架服务）
/// 都实现这个 trait，从而可以被统一发现、调用和管理。
pub trait Capability: Send + Sync + 'static {
    /// 能力唯一标识（如 "crm.create_customer"、"llm.chat"）
    fn name(&self) -> &'static str;
    
    /// 人类可读描述（AI 用于发现和理解）
    /// 应该清晰描述这个能力做什么、何时使用
    fn description(&self) -> &'static str;
    
    /// JSON Schema 描述输入输出契约
    /// AI 用于构造调用参数，框架用于参数校验
    fn schema(&self) -> Value;
    
    /// 标签（AI 用于搜索和分类）
    /// 如 ["crud", "customer", "write"]
    fn tags(&self) -> &[&'static str];
    
    /// 执行调用
    /// 
    /// # Arguments
    /// * `args` - 符合 schema 的参数（由框架校验后传入）
    /// 
    /// # Returns
    /// * `Ok(Value)` - 符合 schema 输出定义的返回值
    /// * `Err(CapError)` - 错误信息（将返回给 AI 用于推理修复）
    fn call(
        &self,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = CapResult<Value>> + Send + '_>>;
    
    /// 能力来源（用于区分 Skill / Plugin / Service）
    fn source(&self) -> CapabilitySource;
    
    /// 版本信息（用于兼容性检查）
    fn version(&self) -> &'static str {
        "1.0.0"
    }
    
    /// 是否需要人工确认才能执行（默认 false）
    /// 对于敏感操作（删除、支付），可以返回 true
    fn requires_confirmation(&self) -> bool {
        false
    }
}

/// 能力来源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySource {
    /// AI 内置能力（如 LLM 调用、代码搜索、文件操作）
    Skill,
    /// 业务插件（如 CRM 客户管理、电商订单管理）
    Plugin,
    /// 框架内置服务（如文件 IO、HTTP 请求、数据库查询）
    Service,
}

/// 调用结果
pub type CapResult<T> = Result<T, CapError>;

/// 能力调用错误
#[derive(Debug, thiserror::Error)]
pub enum CapError {
    #[error("能力未找到: {0}")]
    NotFound(String),
    
    #[error("参数校验失败: {0}")]
    ValidationError(String),
    
    #[error("执行失败: {0}")]
    ExecutionError(String),
    
    #[error("权限不足: {0}")]
    PermissionDenied(String),
    
    #[error("需要人工确认")]
    ConfirmationRequired,
}
```

## 7.3 Capability Registry 设计

```rust
// 文件：packages/sz-rust-capability/src/registry.rs

use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;
use crate::{Capability, CapabilitySource, CapResult, CapError};

/// 统一能力注册表
/// 
/// 所有 Capability（Skills / Plugins / Services）都注册到这里。
/// AI Agent 通过 Registry 发现和调用能力。
/// 
/// # 线程安全
/// 使用 RwLock 保护内部 HashMap，支持高并发读取。
pub struct CapabilityRegistry {
    capabilities: RwLock<HashMap<String, Arc<dyn Capability>>>,
}

impl CapabilityRegistry {
    /// 创建新的空 Registry
    pub fn new() -> Self {
        Self {
            capabilities: RwLock::new(HashMap::new()),
        }
    }
    
    /// 注册一个能力
    /// 
    /// 如果同名能力已存在，将被覆盖（返回旧能力）。
    /// 插件热加载时，旧版本插件的能力会被新版本覆盖。
    pub fn register(&self, cap: Arc<dyn Capability>) -> Option<Arc<dyn Capability>> {
        let name = cap.name().to_string();
        self.capabilities.write().insert(name, cap)
    }
    
    /// 根据名称查找能力
    pub fn get(&self, name: &str) -> Option<Arc<dyn Capability>> {
        self.capabilities.read().get(name).cloned()
    }
    
    /// 根据标签搜索能力（AI 发现能力的主要方式）
    /// 
    /// # Arguments
    /// * `tags` - 需要匹配的标签（AND 逻辑，必须包含所有标签）
    /// * `source` - 可选的来源过滤
    pub fn find_by_tags(
        &self,
        tags: &[&str],
        source: Option<CapabilitySource>,
    ) -> Vec<Arc<dyn Capability>> {
        self.capabilities.read()
            .values()
            .filter(|cap| {
                // 来源过滤
                if let Some(s) = source {
                    if cap.source() != s {
                        return false;
                    }
                }
                // 标签匹配（AND 逻辑）
                tags.iter().all(|tag| cap.tags().contains(tag))
            })
            .cloned()
            .collect()
    }
    
    /// 根据关键词搜索能力（模糊匹配名称和描述）
    pub fn search(&self, query: &str) -> Vec<Arc<dyn Capability>> {
        let query = query.to_lowercase();
        self.capabilities.read()
            .values()
            .filter(|cap| {
                cap.name().to_lowercase().contains(&query)
                    || cap.description().to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }
    
    /// 调用一个能力
    /// 
    /// 这是 AI Agent 调用能力的主要入口。
    pub async fn call(&self, name: &str, args: Value) -> CapResult<Value> {
        let cap = self.get(name)
            .ok_or_else(|| CapError::NotFound(name.to_string()))?;
        
        // 检查是否需要人工确认
        if cap.requires_confirmation() {
            return Err(CapError::ConfirmationRequired);
        }
        
        // TODO: 参数校验（根据 schema）
        
        cap.call(args).await
    }
    
    /// 注销一个能力
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn Capability>> {
        self.capabilities.write().remove(name)
    }
    
    /// 列出所有能力
    pub fn list_all(&self) -> Vec<Arc<dyn Capability>> {
        self.capabilities.read().values().cloned().collect()
    }
    
    /// 按来源分组列出
    pub fn list_by_source(&self, source: CapabilitySource) -> Vec<Arc<dyn Capability>> {
        self.capabilities.read()
            .values()
            .filter(|cap| cap.source() == source)
            .cloned()
            .collect()
    }
    
    /// 当前注册的能力数量
    pub fn len(&self) -> usize {
        self.capabilities.read().len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.capabilities.read().is_empty()
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

## 7.4 能力发现与调用流程

```
AI Agent 调用能力的完整流程：

1. AI 描述任务
   "查询 CRM 中名为'张三'的客户，并创建一条跟进记录"

2. Capability Registry 发现匹配的能力
   Agent → Registry.search("CRM 客户")
   → 返回 [crm.search_customer, crm.create_followup, ...]

3. AI 读取能力 schema
   Agent → Registry.get("crm.search_customer").schema()
   → 返回 JSON Schema（输入：keyword, 输出：Customer[]）

4. AI 构造参数
   Agent → {"keyword": "张三"}

5. AI 调用能力
   Agent → Registry.call("crm.search_customer", {"keyword": "张三"})
   → Plugin 执行查询 → 返回结果

6. AI 观察结果，决定下一步
   结果 → AI 推理 → 调用 crm.create_followup
```

## 7.5 Plugin 如何注册为 Capability

```rust
// 文件：packages/sz-rust-addons-market/src/stall/capability.rs

use sz_rust_capability::{Capability, CapabilitySource, CapResult, CapabilityRegistry};
use serde_json::{json, Value};
use std::sync::Arc;

/// 摊位搜索能力（作为 Capability 暴露给 AI）
pub struct SearchStallCapability {
    registry: Arc<CapabilityRegistry>, // 用于获取 DB 连接等
}

impl SearchStallCapability {
    pub fn new(registry: Arc<CapabilityRegistry>) -> Self {
        Self { registry }
    }
}

impl Capability for SearchStallCapability {
    fn name(&self) -> &'static str {
        "market.search_stall"
    }
    
    fn description(&self) -> &'static str {
        "搜索菜市场摊位。根据摊位号、区域、状态等条件查询摊位信息。
         当用户需要查找摊位、查看摊位详情、检查摊位占用情况时使用。"
    }
    
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "stall_no": {"type": "string", "description": "摊位号"},
                "area": {"type": "string", "description": "区域（如 A区、B区）"},
                "status": {"type": "string", "enum": ["空闲", "已租", "维护中"], "description": "状态"},
                "limit": {"type": "integer", "description": "返回数量上限", "default": 20}
            }
        })
    }
    
    fn tags(&self) -> &[&'static str] {
        &["market", "stall", "search", "read"]
    }
    
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = CapResult<Value>> + Send + '_>> {
        Box::pin(async move {
            // 解析参数
            let stall_no = args.get("stall_no").and_then(|v| v.as_str());
            let area = args.get("area").and_then(|v| v.as_str());
            let status = args.get("status").and_then(|v| v.as_str());
            let limit = args.get("limit").and_then(|v| v.as_u64).unwrap_or(20) as i64;
            
            // 查询数据库（通过 ORM facade）
            // let stalls = Stall::query()
            //     .filter_if(stall_no, |q, v| q.where_eq("stall_no", v))
            //     .filter_if(area, |q, v| q.where_eq("area", v))
            //     .filter_if(status, |q, v| q.where_eq("status", v))
            //     .limit(limit)
            //     .fetch()
            //     .await?;
            
            // 返回结果
            Ok(json!({
                "stalls": [/* 摊位列表 */],
                "total": 42
            }))
        })
    }
    
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
}

// 插件激活时注册能力
pub fn register_capabilities(registry: Arc<CapabilityRegistry>) {
    registry.register(Arc::new(SearchStallCapability::new(registry.clone())));
    // 注册其他能力...
}
```

## 7.6 Skill 如何注册为 Capability

```rust
// 文件：packages/sz-rust-capability/src/builtin_skills.rs
// 将 AI 内置能力也注册为 Capability

pub struct LlmChatCapability {
    llm: Arc<dyn LlmProvider>,
}

impl Capability for LlmChatCapability {
    fn name(&self) -> &'static str {
        "ai.llm_chat"
    }
    
    fn description(&self) -> &'static str {
        "调用 LLM 进行对话。当需要 AI 推理、文本生成、代码生成、
         需求分析时使用。支持 OpenAI / Claude / Gemini 等提供商。"
    }
    
    fn schema(&self) -> Value { /* ... */ }
    
    fn tags(&self) -> &[&'static str] {
        &["ai", "llm", "chat", "reasoning"]
    }
    
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = CapResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let messages = args.get("messages")
                .ok_or(CapError::ValidationError("missing messages".into()))?;
            let request = ChatRequest::from_value(messages.clone())?;
            let response = self.llm.chat_completion(request).await?;
            Ok(response.to_value())
        })
    }
    
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Skill
    }
}
```

## 7.7 Capability Registry 的核心价值

```
统一 Capability Registry 的价值：

1. AI 可以发现并调用业务插件
   "查询 CRM 里的客户张三" → AI 发现 crm.search_customer → 调用

2. 业务插件可以暴露给 AI
   插件开发者只需实现 Capability trait
   插件自动成为 AI 可调用的能力

3. 用户安装插件 = AI 获得新能力
   插件市场安装 → Registry 注册 → AI 立即可用
   无需额外配置

4. 应用和 AI 的边界消失
   AI 能力（Skill）和业务功能（Plugin）在 Registry 中是平等的
   AI 可以调用业务插件，业务插件可以调用 AI 能力
   真正的 AI-Native 应用架构
```

---

# 八、SDD Agent 编排设计

## 8.1 四阶段架构

```
SDD Agent 编排引擎（企业版 crate: sz-rust-sdd-agent）

自然语言需求
      │
      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 1: Spec Agent（需求规格设计）                              │
│  ───────────────────────────────────────────────────────────    │
│  输入：自然语言需求描述                                           │
│  模型：推理模型（DeepSeek-R1 / o3）                               │
│  职责：                                                           │
│  ├── 理解需求意图                                                 │
│  ├── 识别角色与边界                                               │
│  ├── 使用 EARS 格式编写业务规则                                    │
│  ├── 定义验收条件                                                 │
│  └── 输出：specs/{feature}/spec.md                               │
│                                                                   │
│  关键：规格必须结构化、可机器解析、可人工审查                       │
└─────────────────────────────────────────────────────────────────┘
      │  spec.md
      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 2: Design Agent（实现方案创建）                            │
│  ───────────────────────────────────────────────────────────    │
│  输入：spec.md + 项目上下文                                       │
│  模型：推理模型（DeepSeek-R1 / o3）                               │
│  职责：                                                           │
│  ├── 存量功能关联分析（已实现 / 需扩展 / 需新增）                  │
│  ├── 架构设计（模块划分、接口设计、数据模型）                      │
│  ├── 铁律合规检查（22 条铁律适用性分析）                           │
│  ├── Skills 触发分析（路由/中间件/DI/config 变更需触发对应 Skill） │
│  └── 输出：specs/{feature}/design.md                             │
│                                                                   │
│  关键：存量分析避免重复造轮子；铁律检查确保合规                     │
└─────────────────────────────────────────────────────────────────┘
      │  design.md
      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 3: Task Agent（编码任务规划）                              │
│  ───────────────────────────────────────────────────────────    │
│  输入：design.md + spec.md + 项目上下文                           │
│  模型：推理模型（DeepSeek-R1 / o3）                               │
│  职责：                                                           │
│  ├── 拆解为可执行任务（按依赖排序）                                │
│  ├── 每项任务附验收标准（可执行、可量化、可溯源）                  │
│  ├── 分析 Skills 触发需求                                          │
│  └── 输出：specs/{feature}/tasks.md                              │
│                                                                   │
│  关键：任务粒度适中，验收标准明确                                   │
└─────────────────────────────────────────────────────────────────┘
      │  tasks.md
      ▼
┌─────────────────────────────────────────────────────────────────┐
│  HITL 闸门：人工审阅                                              │
│  ───────────────────────────────────────────────────────────    │
│  用户审查：spec.md + design.md + tasks.md                        │
│  用户操作：确认 / 修改 / 补充                                      │
│  用户回复："进入下一阶段"                                          │
│                                                                   │
│  关键：这是唯一的用户介入点，决策成本低                             │
└─────────────────────────────────────────────────────────────────┘
      │  确认后
      ▼
┌─────────────────────────────────────────────────────────────────┐
│  Phase 4: Coding Agent（任务执行）                                │
│  ───────────────────────────────────────────────────────────    │
│  输入：tasks.md（逐任务执行）                                     │
│  模型：代码模型（CodeLlama / StarCoder / DeepSeek-Coder）         │
│  职责：                                                           │
│  ├── 生成可编译 Rust 代码                                          │
│  ├── Compile-Fix 循环（编译错误自动修复）                          │
│  ├── 依据验收条件初步验证                                          │
│  ├── 触发 Skills（test-coverage / performance-check / doc-check） │
│  └── 输出：可运行的代码 + 测试 + SDD 文档                          │
│                                                                   │
│  关键：质量内建，执行阶段即完成初步验证                             │
└─────────────────────────────────────────────────────────────────┘
```

## 8.2 多模型路由策略

```
不同阶段使用不同模型，平衡质量和成本：

Phase 1-3（Spec/Design/Task）：
  需要深度推理、需求理解、架构设计
  → 使用推理模型：DeepSeek-R1 / o3 / Claude-3.5-Sonnet
  → 成本高，但阶段只执行一次

Phase 4（Coding）：
  需要代码生成、编译修复
  → 使用代码模型：DeepSeek-Coder / CodeLlama / StarCoder
  → 成本较低，但可能多次调用（Compile-Fix 循环）

模型路由配置：
  {
    "spec_agent": {"provider": "deepseek", "model": "deepseek-reasoner"},
    "design_agent": {"provider": "deepseek", "model": "deepseek-reasoner"},
    "task_agent": {"provider": "deepseek", "model": "deepseek-reasoner"},
    "coding_agent": {"provider": "deepseek", "model": "deepseek-coder"},
    "compile_fix_agent": {"provider": "deepseek", "model": "deepseek-coder"},
    "fallback": {"provider": "openai", "model": "gpt-4o"}
  }
```

## 8.3 Compile-Fix 循环

```
Phase 4 的核心机制：生成 → 编译 → 修复 → 验证

┌─────────────────────────────────────────────────────────────────┐
│                    Compile-Fix 循环                              │
│                                                                   │
│  1. Coding Agent 生成代码                                         │
│       │                                                           │
│       ▼                                                           │
│  2. cargo build（或 cargo check）                                 │
│       │                                                           │
│       ├── 编译通过 → 进入步骤 4                                    │
│       │                                                           │
│       └── 编译失败 → 进入步骤 3                                    │
│       │                                                           │
│  3. Compile-Fix Agent 分析错误                                    │
│       ├── 读取编译错误信息                                         │
│       ├── 分析错误原因（借位检查器、生命周期、类型不匹配等）         │
│       ├── 生成修复方案                                             │
│       ├── 应用修复                                                 │
│       └── 返回步骤 2（最多重试 N 次）                              │
│       │                                                           │
│  4. cargo test（运行测试）                                        │
│       │                                                           │
│       ├── 测试通过 → 完成                                          │
│       │                                                           │
│       └── 测试失败 → 返回步骤 3                                    │
│                                                                   │
│  最大重试次数：5 次（超过则报告人工介入）                           │
│  超时时间：每次编译 60 秒（Rust 编译可能较慢）                      │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

## 8.4 Spec 文件结构

```
specs/{feature}/
├── spec.md          # Phase 1 产出：需求规格
├── design.md        # Phase 2 产出：技术设计
├── tasks.md         # Phase 3 产出：任务清单
├── session.jsonl    # 完整对话历史（用于审计和复盘）
└── artifacts/       # 执行过程中的产物证据
    ├── cargo_build_before.txt
    ├── cargo_build_after.txt
    ├── cargo_test_output.txt
    ├── bench_before.txt
    ├── bench_after.txt
    └── screenshots/
```

## 8.5 SDD Agent 与现有 Skills 的集成

```
SDD Agent 在执行过程中会触发项目配置的 Skills：

Phase 2（Design Agent）：
├── 设计含路由变更 → 触发 sz-rust-framework-routing
├── 设计含中间件变更 → 触发 sz-rust-framework-middleware
├── 设计含 DI 变更 → 触发 sz-rust-framework-di
└── 设计含 config/static 变更 → 触发 sz-rust-framework-config

Phase 4（Coding Agent）：
├── 编码完成 → 触发 sz-rust-test-coverage
├── 编码含热路径变更 → 触发 sz-rust-performance-check
├── 编码含 pub API 变更 → 触发 sz-rust-doc-check
├── 编码含路由变更 → 触发 sz-rust-auth-guard
└── 编码含错误处理变更 → 触发 sz-rust-error-handling

集成方式：
SDD Agent 通过 Capability Registry 调用 Skills
Skills 本身也是 Capability（CapabilitySource::Skill）
```

## 8.6 SDD Agent 的配置

```toml
# 配置文件：config/sdd.toml

[agent]
max_steps = 25           # 每个 Agent 最大推理步数
max_retries = 5          # Compile-Fix 最大重试次数
compile_timeout_secs = 60  # 编译超时（秒）
test_timeout_secs = 120   # 测试超时（秒）

[model]
spec_agent_provider = "deepseek"
spec_agent_model = "deepseek-reasoner"
design_agent_provider = "deepseek"
design_agent_model = "deepseek-reasoner"
task_agent_provider = "deepseek"
task_agent_model = "deepseek-reasoner"
coding_agent_provider = "deepseek"
coding_agent_model = "deepseek-coder"
fallback_provider = "openai"
fallback_model = "gpt-4o"

[hitl]
enabled = true           # 是否启用人工审阅闸门
review_required_phases = ["spec", "design", "tasks"]

[spec]
output_dir = "specs"     # Spec 文件输出目录
persist_session = true   # 是否持久化对话历史
```

---

# 九、插件系统：数据互通与模板机制

## 9.1 插件数据互通：核心差异化

这是 sz-rust 区别于秒搭等工具的最核心差异化：

```
秒搭等工具的问题：
  生成的是孤立应用
  ├── 应用 A 有自己的用户表
  ├── 应用 B 有自己的用户表
  └── A 和 B 的用户数据不互通，无法跨应用查询

sz-rust 的方案：
  生成的是集成系统
  ├── 所有插件共享统一用户系统
  ├── 插件可以监听其他插件的事件
  ├── 插件可以通过 Capability Registry 互相调用
  └── 跨插件查询：查询某用户的订单 + CRM 跟进记录 + 权限
```

## 9.2 共享 Schema 机制

```rust
// 文件：packages/sz-rust-core/src/plugin/schema.rs

/// 共享 Schema 定义
/// 
/// 所有插件共享的基础数据表。
/// 框架提供这些表的迁移和模型，插件无需重复定义。

/// 统一用户表
#[derive(Model, Clone, Debug)]
#[table(name = "sys_users")]
pub struct SysUser {
    #[primary_key]
    pub id: i64,
    pub tenant_id: i64,      // 多租户隔离
    pub username: String,
    pub email: String,
    pub phone: String,
    pub status: UserStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    // 插件可以扩展字段（通过 JSON 列）
    pub extra: serde_json::Value,
}

/// 统一权限表
#[derive(Model, Clone, Debug)]
#[table(name = "sys_permissions")]
pub struct SysPermission {
    #[primary_key]
    pub id: i64,
    pub role_id: i64,
    pub resource: String,    // 如 "market.stall"
    pub action: String,      // 如 "read", "write"
    pub tenant_id: i64,
}

/// 统一事件表
#[derive(Model, Clone, Debug)]
#[table(name = "sys_events")]
pub struct SysEvent {
    #[primary_key]
    pub id: i64,
    pub event_type: String,  // 如 "order.created"
    pub source_plugin: String, // 如 "ecommerce"
    pub payload: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

## 9.3 插件事件总线

```rust
// 文件：packages/sz-rust-core/src/plugin/event_bus.rs

/// 插件事件总线
/// 
/// 插件可以发布事件，其他插件可以订阅。
/// 这是跨插件通信的主要机制。

pub trait EventBus: Send + Sync + 'static {
    /// 发布一个事件
    async fn publish(&self, event: PluginEvent) -> CapResult<()>;
    
    /// 订阅一个事件类型
    fn subscribe(
        &self,
        event_type: &str,
        handler: Box<dyn EventHandler>,
    ) -> CapResult<SubscriptionId>;
    
    /// 取消订阅
    fn unsubscribe(&self, subscription_id: SubscriptionId) -> CapResult<()>;
}

/// 插件事件
pub struct PluginEvent {
    pub event_type: String,      // 如 "order.created"
    pub source_plugin: String,   // 发布事件的插件
    pub payload: serde_json::Value,
    pub tenant_id: i64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 事件处理器
pub trait EventHandler: Send + Sync + 'static {
    fn handle(&self, event: &PluginEvent) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

// 使用示例：
// 电商插件发布订单创建事件
// event_bus.publish(PluginEvent {
//     event_type: "order.created".into(),
//     source_plugin: "ecommerce".into(),
//     payload: json!({"order_id": 123, "user_id": 456}),
//     ...
// }).await?;

// CRM 插件订阅订单创建事件
// event_bus.subscribe("order.created", Box::new(|event| {
//     // 自动在 CRM 中创建客户跟进记录
//     let order_id = event.payload["order_id"].as_u64().unwrap();
//     let user_id = event.payload["user_id"].as_u64().unwrap();
//     // create_followup_record(user_id, order_id)...
//     Box::pin(async {})
// }));
```

## 9.4 插件模板机制

```
插件模板的作用：

问题：AI 从零生成 Rust 代码，成功率只有 50-60%（借位检查器、生命周期等）
解决：预置经过验证的插件模板，AI 基于模板生成而非从零生成

模板类型：
├── CRUD 模板（最常用）
│   └── 输入：数据模型定义 → 输出：完整的 CRUD 插件（模型 + 控制器 + 路由 + 测试）
│
├── 主从模板
│   └── 输入：主表 + 从表定义 → 输出：主从关系管理插件
│
├── 工作流模板
│   └── 输入：状态定义 + 流转规则 → 输出：状态机驱动的插件
│
└── 报表模板
    └── 输入：数据源 + 聚合规则 → 输出：数据报表插件

模板格式：
  Tera 模板引擎（Rust 生态的 Jinja2 兼容模板）
  模板文件：templates/plugin-crud/
  ├── model.rs.tera      # 模型代码模板
  ├── controller.rs.tera # 控制器代码模板
  ├── routes.rs.tera     # 路由代码模板
  ├── migration.sql.tera # 数据库迁移模板
  ├── manifest.json.tera # 插件清单模板
  └── tests.rs.tera      # 测试代码模板
```

## 9.5 插件清单格式

```json
{
  "name": "sz-addons-market",
  "version": "1.0.0",
  "title": "菜市场行业插件包",
  "description": "菜市场行业完整解决方案：摊位管理、收银、商户、配送、大屏",
  "author": "SZ-Rust Team",
  "license": "commercial",
  "sz_rust_version": ">=1.1.0",
  
  "capabilities": [
    {
      "name": "market.search_stall",
      "description": "搜索摊位",
      "tags": ["market", "stall", "search"]
    },
    {
      "name": "market.create_order",
      "description": "创建订单",
      "tags": ["market", "order", "write"],
      "requires_confirmation": true
    }
  ],
  
  "dependencies": {
    "plugins": ["sz-rust-addons-crm"],
    "shared_schemas": ["sys_users", "sys_permissions"]
  },
  
  "routes": [
    {"method": "GET", "path": "/market/stalls", "handler": "StallController@index"},
    {"method": "POST", "path": "/market/stalls", "handler": "StallController@create"}
  ],
  
  "migrations": [
    "migrations/20260811000000_create_stalls_table.sql",
    "migrations/20260811000001_create_merchants_table.sql"
  ],
  
  "events_published": ["stall.rented", "stall.vacated"],
  "events_subscribed": ["merchant.license_expired"]
}
```

---

# 十、AI 生成流水线

## 10.1 完整生成流程

```
用户输入：自然语言需求描述
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 1: 需求理解与分类                                          │
│  ───────────────────────────────────────────────────────────    │
│  ├── 意图识别：CRUD / 工作流 / 报表 / 集成                       │
│  ├── 行业识别：菜市场 / 餐饮 / 零售 / 通用                       │
│  ├── 复杂度评估：简单 / 中等 / 复杂                              │
│  └── 输出：需求分类标签                                          │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 2: 模板匹配（优先）/ 从零生成（备选）                       │
│  ───────────────────────────────────────────────────────────    │
│  ├── 根据需求分类匹配最合适的模板                                 │
│  │   ├── CRUD 需求 → CRUD 模板                                  │
│  │   ├── 主从关系 → 主从模板                                    │
│  │   └── 无匹配模板 → 从零生成                                  │
│  ├── 从行业 RAG 知识库检索相似案例                               │
│  └── 输出：模板 + Few-shot 示例                                  │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 3: SDD 四阶段生成                                          │
│  ───────────────────────────────────────────────────────────    │
│  ├── Phase 1: Spec Agent → spec.md                              │
│  ├── Phase 2: Design Agent → design.md                          │
│  ├── Phase 3: Task Agent → tasks.md                             │
│  └── HITL 闸门：用户确认                                         │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 4: 代码生成与编译                                          │
│  ───────────────────────────────────────────────────────────    │
│  ├── Phase 4: Coding Agent 逐任务生成代码                        │
│  ├── cargo check 编译验证                                        │
│  ├── Compile-Fix 循环（最多 5 次）                               │
│  └── 输出：可编译的 Rust 代码                                    │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 5: 测试与验证                                              │
│  ───────────────────────────────────────────────────────────    │
│  ├── cargo test 运行测试                                         │
│  ├── 触发 Skills：test-coverage / performance-check              │
│  ├── 铁律合规检查（22 条）                                       │
│  └── 输出：测试报告 + 合规报告                                   │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 6: 热加载与部署                                            │
│  ───────────────────────────────────────────────────────────    │
│  ├── 编译插件为 .so/.dll（或 WASM）                              │
│  ├── AddonLoader 热加载（M2：进程重启 + 状态迁移）               │
│  ├── Capability Registry 注册新能力                              │
│  └── 输出：运行中的应用                                          │
└─────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─────────────────────────────────────────────────────────────────┐
│  Step 7: 交付物打包                                              │
│  ───────────────────────────────────────────────────────────    │
│  ├── 源代码（Rust 代码）                                         │
│  ├── SDD 文档（spec.md / design.md / tasks.md）                  │
│  ├── 测试报告                                                    │
│  ├── 合规报告                                                    │
│  └── 插件清单（manifest.json）                                   │
└─────────────────────────────────────────────────────────────────┘
```

## 10.2 生成质量保障

```
多层质量保障：

第一层：模板优先
  ├── 使用经过验证的模板，而非从零生成
  └── 模板本身已通过编译和测试

第二层：Compile-Fix 循环
  ├── 编译错误自动修复
  └── 最多 5 次重试，超过则报告人工介入

第三层：Skills 触发
  ├── test-coverage：测试覆盖率检查
  ├── performance-check：性能基线检查
  ├── auth-guard：权限检查
  ├── doc-check：文档完整性检查
  └── n-plus-one：N+1 查询检测

第四层：铁律合规检查
  ├── 22 条铁律自动检查
  └── 违规即失败，必须修复

第五层：人工审阅（HITL）
  └── Phase 3→4 闸门，用户确认后才执行
```

---

# 十一、开源版/企业版分离设计

## 11.1 Workspace 结构设计

```
两个独立的 Git 仓库：

┌─────────────────────────────────────────────────────────────────┐
│  仓库一：sz-rust（开源）                                         │
│  GitHub: https://github.com/ljclz/sz-rust                       │
│  crates.io: sz-rust-core, sz-rust-ai-facade, ...                │
│  许可证: Apache-2.0                                             │
├─────────────────────────────────────────────────────────────────┤
│  仓库二：sz-rust-enterprise（企业版）                            │
│  私有仓库（GitLab / 自建 Git）                                   │
│  发布: 私有 crate registry                                      │
│  许可证: 商业许可                                                │
└─────────────────────────────────────────────────────────────────┘

sz-rust/（开源仓库）
├── Cargo.toml              # workspace，只含开源 crate
├── packages/
│   ├── sz-rust-core/
│   ├── sz-rust-ai-facade/
│   ├── sz-rust-orm-facade/
│   ├── sz-rust-addons-loader/
│   ├── sz-rust-mcp/
│   ├── sz-rust-cli/
│   ├── ...（所有开源 crate）
│   └── sz-rust-examples/   # 开源示例
├── docs/
│   ├── cases/              # 战略讨论文档
│   ├── adr/                # 架构决策记录
│   └── ...
├── .trae/                  # Skills + 铁律
├── deploy/                 # 部署配置
└── README.md

sz-rust-enterprise/（企业版仓库）
├── Cargo.toml              # workspace，只含企业版 crate
├── packages/
│   ├── sz-addons-market/   # 菜市场行业插件包
│   ├── sz-plugin-migration/ # AI 辅助迁移工具
│   ├── sz-sdd-agent/       # SDD Agent 编排
│   ├── sz-sdd-canvas/      # 可视化画布
│   ├── sz-plugin-sso-enterprise/ # 企业 SSO
│   └── ...（其他企业版 crate）
├── config/
│   └── registry.toml       # 私有 registry 配置
└── README.md

企业版 crate 如何依赖开源版：
  [dependencies]
  sz-rust-core = "1.1"       # 从 crates.io 拉取
  sz-rust-ai-facade = "1.1"  # 从 crates.io 拉取
  sz-rust-capability = "1.1" # 从 crates.io 拉取（新增）
```

## 11.2 发布流程设计

```
开源版发布流程：

1. 开发者在 sz-rust 仓库提交代码
2. CI 运行测试 + 铁律检查
3. 手动触发发布 workflow
4. cargo publish 到 crates.io
5. GitHub Release 创建

企业版发布流程：

1. 开发者在 sz-rust-enterprise 仓库提交代码
2. CI 运行测试
3. 手动触发发布 workflow
4. cargo publish 到私有 registry（如 Cloudsmith）
5. 企业客户通过配置 Cargo.toml 的 source 拉取

Cargo.toml source 配置（企业客户）：
  [registries]
  sz-enterprise = { index = "sparse+https://registry.example.com/" }
  
  [dependencies]
  sz-addons-market = { version = "1.0", registry = "sz-enterprise" }
```

## 11.3 许可证设计

```
开源版许可证（Apache-2.0）：

  Copyright 2026 SZ-Rust Team
  
  Licensed under the Apache License, Version 2.0 (the "License");
  you may not use this file except in compliance with the License.
  
  → 允许商业使用、修改、分发
  → 需保留许可证声明
  → 修改需说明
  → 不授予商标使用权

企业版许可证（商业许可，摘要）：

  → 按授权书约定的范围使用
  → 禁止转售、再分发、反向工程
  → 按约定的租户数/实例数部署
  → 订阅期内获得版本更新和技术支持
  → 订阅到期后停止使用

关键：开源版用户升级到企业版时，不需要重写代码
  企业版插件依赖开源版核心，无缝集成
```

## 11.4 代码隔离检查

```
CI 中增加许可证合规检查：

检查项：
├── 开源仓库中不得包含企业版代码
│   └── 检查：packages/ 下不得有 sz-addons-market 等企业版 crate
│
├── 企业版 crate 不得修改开源核心
│   └── 检查：企业版 crate 只能通过公开 API 使用开源核心
│
├── 许可证头检查
│   └── 检查：每个源文件顶部有正确的许可证声明
│
└── 依赖检查
    └── 检查：企业版 crate 的依赖不包含未授权的第三方代码

工具：cargo-deny + 自定义脚本
```

---

# 十二、分阶段实施路线图

## 12.1 总体时间线

```
Phase 1（1-2 个月）：基础设施
  └── Capability Registry + 开源/企业分离 + 插件模板

Phase 2（2-4 个月）：AI 生成能力
  └── SDD Agent + 迁移工具 + 行业 RAG

Phase 3（4-6 个月）：产品化
  └── 可视化画布 + 插件市场 + 真实用户案例

Phase 4（6-12 个月）：生态
  └── 前端生成 + 工作流引擎 + 开发者社区
```

## 12.2 Phase 1：基础设施（1-2 个月）

### P1-T1：Capability Registry（2 周）

```
任务：新建 sz-rust-capability crate

交付物：
├── Capability trait 定义
├── CapabilityRegistry 实现
├── 内置 Skill 注册（LLM 调用、代码搜索、文件操作）
├── MCP 工具注册为 Capability
├── 单元测试 + 集成测试
└── 文档（API 文档 + 使用指南）

验收标准：
├── AI Agent 可以通过 Registry 发现并调用能力
├── Plugin 可以实现 Capability trait 并注册
├── 按标签搜索能力正常工作
└── 并发调用安全
```

### P1-T2：开源版/企业版 crate 分离（1 周）

```
任务：重构 workspace 结构

交付物：
├── sz-rust 仓库（开源）：只含开源 crate
├── sz-rust-enterprise 仓库（企业版）：只含企业版 crate
├── CI/CD 发布流程配置
├── 许可证合规检查脚本
└── 发布文档

验收标准：
├── cargo publish 可以正确发布开源 crate 到 crates.io
├── 企业版 crate 不会出现在开源仓库中
├── 企业版 crate 可以通过私有 registry 安装
```

### P1-T3：插件模板库（2 周）

```
任务：预置 CRUD / 主从 / 工作流模板

交付物：
├── CRUD 模板（Tera 模板）
├── 主从模板
├── 模板渲染引擎集成
├── CLI 命令：`sz-rust-cli make:plugin --template crud`
└── 模板使用文档

验收标准：
├── `make:plugin` 命令可以基于模板生成可编译的插件骨架
├── 生成的插件通过 cargo check
├── 生成的插件包含基本的 CRUD 功能
```

### P1-T4：完善现有业务插件（2 周）

```
任务：将现有 addons 完善为企业版首发内容

目标插件：
├── sz-rust-addons-cms → 完善为可发布的 CMS 插件
├── sz-rust-addons-crm → 完善为可发布的 CRM 插件
├── sz-rust-addons-ecommerce → 完善为可发布的电商插件

交付物：
├── 每个插件实现 Capability trait
├── 每个插件有完整的 manifest.json
├── 每个插件有测试覆盖
└── 每个插件有使用文档

验收标准：
├── 插件可以通过 `sz-rust-cli plugin install` 安装
├── 插件的能力可以通过 Capability Registry 调用
├── 插件通过 22 条铁律检查
```

## 12.3 Phase 2：AI 生成能力（2-4 个月）

### P2-T1：SDD Agent 编排（4 周）

```
任务：新建 sz-rust-sdd-agent crate（企业版）

交付物：
├── Spec Agent（需求规格生成）
├── Design Agent（技术设计生成 + 存量分析）
├── Task Agent（任务清单生成）
├── Coding Agent（代码生成 + Compile-Fix 循环）
├── HITL 闸门实现
├── Spec 文件持久化
├── 多模型路由
└── 与现有 Skills 的集成

验收标准：
├── 输入自然语言需求 → 输出完整的 spec.md + design.md + tasks.md
├── 用户确认后 → 生成可编译的代码
├── Compile-Fix 循环自动修复编译错误
├── 生成的代码附带 SDD 文档
```

### P2-T2：AI 辅助迁移工具（3 周）

```
任务：新建 sz-plugin-migration（企业版）

交付物：
├── TP6 代码分析器（路由、模型、控制器识别）
├── sz-rust 代码生成器
├── 增量验证工具（对比 TP6 和 sz-rust 响应）
├── 迁移报告生成
└── 迁移案例文档（基于创始人自己的系统）

验收标准：
├── 可以分析一个 TP6 项目，输出分析报告
├── 可以生成等价的 sz-rust 代码
├── 可以对比 TP6 和 sz-rust 的响应一致性
```

### P2-T3：行业 RAG 知识库（2 周）

```
任务：构建行业 RAG 知识库

交付物：
├── 29+ 项目代码的向量化（Embedding）
├── 行业术语表（菜市场业务术语）
├── 业务规则库（从现有代码中提取）
├── RAG 检索集成到 SDD Agent
└── 数据模型模板库

验收标准：
├── SDD Agent 生成代码时可以检索行业知识库
├── 检索结果能提升生成代码的行业相关性
```

### P2-T4：MCP 工具扩展（1 周）

```
任务：从 7 个扩展到 15+ 个 MCP 工具

新增工具：
├── CRUD 操作工具（create/read/update/delete）
├── 迁移管理工具（migrate/create/status）
├── 测试工具（test/run/coverage）
├── 部署工具（deploy/check）
└── 插件管理工具（plugin/list/install/uninstall）

验收标准：
├── 所有新工具通过测试
├── AI Agent 可以通过 MCP 调用所有工具
```

## 12.4 Phase 3：产品化（4-6 个月）

### P3-T1：可视化应用搭建画布（6 周）

```
任务：Tauri + Vue 构建桌面工作bench

交付物：
├── 需求描述界面（自然语言输入）
├── 规格可视化（spec.md 渲染）
├── 任务进度看板（tasks.md 执行进度）
├── 实时日志（SDD Agent 执行日志）
├── 插件管理界面
└── 应用预览

验收标准：
├── 用户可以在画布中描述需求
├── 实时看到 SDD Agent 的执行进度
├── 可以审查和确认规格
```

### P3-T2：插件市场 MVP（3 周）

```
任务：插件发布、安装、交易的基础设施

交付物：
├── 插件市场 Web 平台
├── CLI 集成（`sz-rust-cli plugin search/install`）
├── 插件审核流程
├── 支付集成（可选）
└── 开发者文档

验收标准：
├── 开发者可以发布插件
├── 用户可以搜索和安装插件
├── 插件安装后自动注册到 Capability Registry
```

### P3-T3：真实用户案例（持续）

```
任务：找到 3-5 个真实用户，验证产品

目标用户：
├── 有存量 TP6 系统的中小企业技术负责人
├── 菜市场/生鲜行业的数字化需求方
└── 对 Rust + AI 感兴趣的开发者

交付物：
├── 用户案例文档
├── 用户反馈收集
├── 产品迭代
```

## 12.5 Phase 4：生态（6-12 个月）

### P4-T1：前端生成 ✅

```
任务：根据数据模型自动生成前端页面
状态：已完成（2026-08-12，71 测试通过）

技术方案：
├── 根据 ORM 模型生成 Vue/React 组件
├── 根据路由定义生成前端路由
├── 根据权限定义生成前端权限控制
└── 支持自定义前端模板
```

### P4-T2：工作流引擎 ✅

```
任务：内置审批流、状态机等业务逻辑编排
状态：已完成（2026-08-12，131 测试通过）

技术方案：
├── 状态机定义（YAML/JSON）
├── 审批流设计器
└── 与插件系统集成
```

### P4-T3：开发者社区 ⏭️ 跳过

```
任务：建立开发者社区
状态：跳过（非编码任务，2026-08-12）

交付物：
├── 开发者文档门户
├── 社区论坛
├── 插件开发教程
├── 示例项目库
└── 年度开发者大会（远期）
!
任务：根据数据模型自动生成前端页面

技术方案：
├── 根据 ORM 模型生成 Vue/React 组件
├── 根据路由定义生成前端路由
├── 根据权限定义生成前端权限控制
└── 支持自定义前端模板

### P4-T2：工作流引擎

```
任务：内置审批流、状态机等业务逻辑编排

技术方案：
├── 状态机定义（YAML/JSON）
├── 审批流设计器
├── 与插件系统集成
```

### P4-T3：开发者社区

```
任务：建立开发者社区

交付物：
├── 开发者文档门户
├── 社区论坛
├── 插件开发教程
├── 示例项目库
└── 年度开发者大会（远期）
```

## 12.6 里程碑总览

```
时间线           里程碑                              验收标准
──────────────────────────────────────────────────────────────────
M1（1 个月）     Capability Registry MVP             AI 可发现+调用能力
M2（2 个月）     开源/企业分离 + 插件模板             可发布开源版 + 生成插件骨架
M3（3 个月）     SDD Agent MVP                       自然语言→可编译代码
M4（4 个月）     迁移工具 + 行业 RAG                 可迁移 TP6 项目
M5（5 个月）     可视化画布 MVP                       桌面工作bench 可用
M6（6 个月）     插件市场 MVP + 3 个真实用户案例      有真实用户在用
M12（12 个月）   前端生成 + 工作流引擎 + 开发者社区    完整生态
```

---

# 十三、风险与应对

## 13.1 技术风险

### R1：Rust 编译速度慢，影响热加载体验

**风险等级**：P0

**描述**：Rust 编译时间（10-60 秒）会破坏"即时应用"的体验。

**应对策略**：
```
├── 预编译插件模板（模板本身已编译，只需增量编译用户定制部分）
├── 使用 sccache 缓存编译结果
├── 增量编译（只编译变更的 crate）
├── WASM 热加载（部分逻辑用 WASM，无需编译 Rust）
├── 管理用户预期（明确告知编译时间）
└── 后台编译 + 前台通知（用户不阻塞等待）
```

### R2：AI 生成的 Rust 代码质量不稳定

**风险等级**：P1

**描述**：借位检查器、生命周期、Send 约束等导致 AI 生成代码成功率只有 50-60%。

**应对策略**：
```
├── 模板优先：基于预验证模板生成，而非从零生成
├── Compile-Fix Agent：专门的编译错误修复循环
├── 限制生成模式：AI 只生成特定模式的代码（减少复杂性）
├── 减少 Rust 暴露：用户看到的接口简化，底层 Rust 复杂性对 AI 也简化
└── 持续优化：收集编译错误模式，优化模板和提示词
```

### R3：插件数据互通实现复杂

**风险等级**：P0

**描述**：跨插件查询、数据扩展、事件通知、权限继承是分布式系统级别的挑战。

**应对策略**：
```
├── 先实现最简单的场景：共享用户表 + 事件总线
├── 渐进式：Phase 1 只实现共享 Schema，Phase 2 实现事件总线，Phase 3 实现跨插件查询
├── 限制范围：初期只支持同一租户内的跨插件查询
├── 文档化：明确数据互通的边界和限制
└── 参考成熟方案：借鉴 WordPress plugin API 的 hook 系统
```

## 13.2 产品风险

### R4：目标用户定位不清

**风险等级**：P0

**描述**："任何人"太宽泛，非技术人员无法部署 Rust，专业开发者不需要 AI 生成。

**应对策略**：
```
├── 明确定位：中小企业技术负责人（已在综合结论中确认）
├── 所有营销材料围绕这个画像
├── 不说"让任何人构建应用"
├── 说"让技术负责人用 AI 高效构建和迁移应用"
└── 持续验证：找到真实用户，收集反馈
```

### R5：生态系统冷启动

**风险等级**：P1

**描述**：没有插件 → 没有用户 → 没有开发者 → 没有插件（鸡生蛋问题）。

**应对策略**：
```
├── 官方先提供核心插件（CMS/CRM/电商等）
├── AI 生成替代：用户可以用 AI 自己生成插件
├── 迁移工具获客：用迁移工具吸引有存量系统的用户
├── 创始人自己的 29+ 项目作为首发插件
└── 不追求大市场，先服务小众垂直行业
```

### R6：商业模式不可持续

**风险等级**：P1

**描述**：AI 生成可能蚕食插件市场；AI 调用利润薄；企业支持不具可扩展性。

**应对策略**：
```
├── 多元化收入：插件授权 + AI 调用分成 + 托管服务 + 企业支持
├── AI 生成的是"骨架"，高质量插件仍需人工（AI 和插件市场互补）
├── 垂直行业深度：行业插件的 Know-How 是 AI 无法替代的
├── 订阅模式：技术支持 + 版本更新打包为订阅
└── 持续验证：早期就尝试收费，验证支付意愿
```

## 13.3 执行风险

### R7："做大做全"的冲动导致无法完成

**风险等级**：P0

**描述**：创始人的历史模式是"不停往里塞东西"，导致项目无法上线。

**应对策略**：
```
├── 每个 Phase 有明确的"完成"标准
├── 完成比完美重要：先做出可用的 MVP，再迭代
├── 一次只做一件事：Phase 1 完成后再开始 Phase 2
├── 外部约束：公开承诺里程碑，增加违约成本
└── 定期复盘：每两周检查进度，调整计划
```

### R8：方向漂移

**风险等级**：P1

**描述**：在开发过程中可能偏离本文档确定的方向。

**应对策略**：
```
├── 本文档作为权威参考，重大决策需对照本文档
├── 方向变更需写 ADR（架构决策记录）
├── 定期回顾本文档（每月一次）
└── 新想法记录到"待讨论"列表，不立即实施
```

## 13.4 风险总览

```
风险                  等级    应对优先级    负责人
──────────────────────────────────────────────────
R1 Rust 编译速度      P0      高            技术团队
R3 插件数据互通       P0      高            技术团队
R4 目标用户定位       P0      高            产品
R7 "做大做全"冲动     P0      高            创始人
R2 AI Rust 代码质量   P1      中            技术团队
R5 生态冷启动         P1      中            产品
R6 商业模式           P1      中            产品
R8 方向漂移           P1      中            创始人
```

---

# 附录 A：文档索引

本文档是 sz-rust 产品化的权威技术指南。相关文档：

| 文档 | 性质 | 位置 |
|------|------|------|
| 本文档 | 产品技术方案（权威） | `docs/product-technical-plan.md` |
| 产品定位与愿景 | 战略讨论 | `docs/cases/ai-native-platform-vision.md` |
| 平台对比分析 | 竞品分析 | `docs/cases/platform-comparison-analysis.md` |
| 批判性分析 | 风险识别 | `docs/cases/critical-analysis.md` |
| 创始人洞察 | 方向探索 | `docs/cases/founder-insights-product-direction.md` |
| 创始人经历 | 用户画像参考 | `docs/cases/founder-journey-insights.md` |
| 低代码对比 | 定位澄清 | `docs/cases/low-code-vs-ai-native.md` |
| SDD+Loopra 架构 | 技术参考 | `docs/cases/sdd-loopra-architecture-reference.md` |
| SDD 实践指南 | 开发方法 | `docs/cases/sdd-practice-guide.md` |
| 综合结论 | 战略收敛 | `docs/cases/comprehensive-conclusion.md` |
| 架构决策记录 | 决策追踪 | `docs/adr/` |
| 22 条铁律 | 约束 | `.trae/rules/project_rules.md` |

# 附录 B：术语表

| 术语 | 定义 |
|------|------|
| AI-native | AI 是核心生产力，不是附加功能 |
| 高代码 | 输出完整源代码，不是平台配置 |
| Capability Registry | 统一能力注册表，Skills + Plugins 的统一抽象 |
| SDD | Specification-Driven Development，规范驱动开发 |
| HITL | Human-In-The-Loop，人工介入点 |
| M2 热加载 | 进程重启 + 状态迁移的安全热加载方式 |
| 共享 Schema | 跨插件共享的基础数据表（用户、权限、事件） |
| 开源版 | Community Edition，免费，Apache-2.0 许可 |
| 企业版 | Enterprise Edition，收费，商业许可 |
| 插件模板 | 预验证的代码模板，AI 基于模板生成 |

# 附录 C：版本历史

| 版本 | 日期 | 变更 |
|------|------|------|
| v1.0 | 2026-08-11 | 初始版本，整合所有战略讨论文档 |
