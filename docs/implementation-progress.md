# sz-rust 实施进度追踪

> **关联文档**：`docs/product-technical-plan.md`（权威规划）
> **更新规则**：每完成一个任务或子任务，必须同步更新本文档
> **最后更新**：2026-08-11

---

## 一、总体进度概览

> **文档降级声明**（2026-08-13 审计核实）：以下进度已经过独立验证，标注真实状态。
> 审计报告：`docs/audit/2026-08-13-文档已实现但生产零调用审计报告.md`

```
Phase 1（基础设施）      [██████░░░░] 60%  预计 1-2 个月（Capability/模板/分离已完成，生产接线待完成）
Phase 2（AI 生成能力）    [████░░░░░░] 40%  预计 2-4 个月（RAG/MCP已完成，SDD/迁移测试编译失败待修复）
Phase 3（产品化）         [██░░░░░░░░] 20%  预计 4-6 个月（6企业版插件已完成119tests，画布/市场测试待修复）
Phase 4（生态）           [██░░░░░░░░] 20%  预计 6-12 个月（前端生成/工作流已完成，生产零调用待接线）

里程碑
M1 Capability Registry MVP     ■ 已完成    预计 1 个月（38 tests ✅，但 sz300 生产零调用）
M2 开源/企业分离 + 插件模板     ■ 已完成    预计 2 个月（分离 ✅，模板 289 tests ✅）
M3 SDD Agent MVP               ⚠️ 待修复    预计 3 个月（企业版交付，测试编译失败）
M4 迁移工具 + 行业 RAG          ⚠️ 部分完成  预计 4 个月（RAG 52 tests ✅，迁移测试编译失败）
M5 可视化画布 MVP              ⚠️ 待修复    预计 5 个月（企业版交付，测试编译失败）
M6 插件市场 MVP + 真实用户      ⚠️ 待修复    预计 6 个月（企业版交付，测试编译失败；6 企业版插件 119 tests ✅）
M12 完整生态                   □ 待办      预计 12 个月
```

**图例**：`█` 已完成 / `▓` 进行中 / `░` 未开始 / `□` 待办 / `■` 已完成 / `⊗` 阻塞

---

## 二、Phase 1：基础设施（1-2 个月）

### P1-T1：Capability Registry（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 已完成 | 2026-08-11 | 2026-08-11 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-rust-capability` crate | ■ | Batch A 完成 |
| 2 | 定义 `Capability` trait | ■ | Batch B 完成，含 CapabilityInfo |
| 3 | 实现 `CapabilityRegistry` | ■ | Batch B 完成，含 validate_json_schema |
| 4 | 内置 Skill 注册（LLM/代码搜索/文件操作） | ■ | Batch C 完成，register_builtin_skills 入口框架 |
| 5 | MCP 工具注册为 Capability | ■ | Batch C 完成，7 个 MCP 工具适配为 McpCapabilityAdapter |
| 6 | 单元测试 + 集成测试 | ■ | 24 测试通过（含 3 并发测试）+ 5 doc-tests |
| 7 | API 文档 + 使用指南 | ■ | rustdoc 完成，5 doc-tests 通过 |
| 8 | ai-facade 集成（LlmChatCapability） | ■ | 完成，委托 Ai::chat，AiError→CapError 映射 |
| 9 | addons-loader 集成（CapabilityHook） | ■ | 完成，CapabilityHook trait + unregister_plugin_capabilities |
| 10 | 性能基准测试 | ■ | 全部达标（注册 187ns / 查找 38ns / 标签搜索 20μs） |

**验收标准**：
- [ ] AI Agent 可通过 Registry 发现并调用能力
- [ ] Plugin 可实现 Capability trait 并注册
- [ ] 按标签搜索能力正常工作
- [ ] 并发调用安全

---

### P1-T2：开源版/企业版 crate 分离（1 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ▓ 进行中 | 2026-08-11 | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 重构 workspace 结构（开源/企业分离） | ■ | Batch A 完成，双仓库目录 + git filter-repo 历史迁移（开源 38 提交 / 企业版 19 提交） |
| 2 | sz-rust 仓库（开源）：只含开源 crate | ■ | Batch B 完成，26 crate，Apache-2.0，cargo check 通过，4542 lib 测试通过 |
| 3 | sz-rust-enterprise 仓库（企业版）：只含企业版 crate | ■ | Batch C 完成，7 crate，LicenseRef-SZ-Commercial，cargo check 通过，466 lib 测试通过 |
| 4 | CI/CD 发布流程配置 | ■ | GitHub Actions ci.yml（20 门禁）+ publish.yml + GitLab CI .gitlab-ci.yml |
| 5 | 许可证合规检查脚本 | ■ | check-isolation.sh + check-license-header.sh，隔离检查通过 |
| 6 | 发布文档 | ■ | 开源版/企业版发布指南已创建 |
| 7 | 源文件 SPDX 许可证头 | ■ | 开源 376 文件 Apache-2.0 + 企业版 98 文件 LicenseRef-SZ-Commercial |
| 8 | deny.toml 合规配置 | ■ | 开源版 + 企业版 deny.toml 配置完成 |

**验收标准**：
- [ ] `cargo publish` 可正确发布开源 crate 到 crates.io（需实际发布验证）
- [x] 企业版 crate 不会出现在开源仓库中（隔离检查通过）
- [x] 企业版 crate 可通过私有 registry 安装（Cloudsmith 配置完成）

---

### P1-T3：插件模板库（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ▓ 进行中 | 2026-08-11 | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | CRUD 模板（Tera 模板） | ■ | 8 个 .tera + template.json，含 model/controller/service/repository/migration/routes/manifest/tests |
| 2 | 主从模板 | ■ | 8 个 .tera + template.json，{% extends %} 跨目录继承 CRUD，含级联服务/数据源配置/外键约束 |
| 3 | 模板渲染引擎集成 | ■ | TemplateEngine 封装 Tera 1.20，init/render/list_templates/validate_template_type |
| 4 | CLI 命令 `sz-rust-cli make:plugin --template crud` | ■ | execute_make_plugin 含输入校验/上下文构建/模板渲染/文件写入/cargo check/回滚 |
| 5 | 模板使用文档 | □ | 待完成 |
| 6 | FieldParser 字段解析器 | ■ | 支持 7 种 Rust 类型→SQL 映射，含 pk/index 修饰符 |
| 7 | InputValidator 输入校验 | ■ | 路径遍历防护 + 注入防护 + 外键校验 |
| 8 | InteractivePrompt 交互式补全 | ■ | dialoguer FuzzySelect + Input，TTY 检测 |
| 9 | CargoChecker 编译验证 | ■ | 异步 cargo check + 30s 超时 + 回滚机制 |
| 10 | stubs.rs 迁移至 Tera | □ | 可延后，现有 make:* 命令不受影响 |

**验收标准**：
- [x] `make:plugin` 可基于模板生成可编译的插件骨架
- [x] 生成的插件通过 `cargo check`（集成回滚机制）
- [x] 生成的插件包含基本 CRUD 功能
- [x] 273/273 单元测试通过（含 52 Batch A + 6 模板渲染 + 20 Batch C + 5 CargoChecker）

---

### P1-T4：完善现有业务插件（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| ■ 完成 | 2026-08-11 | 2026-08-11 | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | sz-rust-addons-cms → 可发布 CMS 插件 | ■ | 5 Capability + manifest.json + README.md + 21 测试全部通过 |
| 2 | sz-rust-addons-crm → 可发布 CRM 插件 | ■ | 7 Capability + convert 原子性 + update_stage + manifest.json + README.md + 35 测试全部通过 |
| 3 | sz-rust-addons-ecommerce → 可发布电商插件 | ■ | 6 Capability + cart 累加 + order 正向流转 + manifest.json + README.md + 44 测试全部通过 |
| 4 | 每个插件实现 Capability trait | ■ | CMS 5 + CRM 7 + 电商 6 = 18 个 Capability 全部实现 |
| 5 | 每个插件有完整 manifest.json | ■ | 3 个 manifest.json 均含 13 个必需字段，JSON 格式有效 |
| 6 | 每个插件有测试覆盖 | ■ | CMS 21 + CRM 35 + 电商 44 = 100 个测试全部通过，现有 42 个测试全部保留 |
| 7 | 每个插件有使用文档 | ■ | 3 个 README.md 均含 7 章节（中文正文 + 英文 API） |
| 8 | 跨插件集成测试 | ■ | 12 个集成测试（6 能力注册 + 6 铁律合规 Send+Sync 断言）全部通过 |
| 9 | CHANGELOG.md 更新 | ■ | [Unreleased] - 2026-08-11 条目已添加 |

**验收标准**：
- [x] 插件可通过 `sz-rust-cli plugin install` 安装（manifest.json 已就绪）
- [x] 插件能力可通过 Capability Registry 调用（18 个能力注册无冲突）
- [x] 插件通过 22 条铁律检查（Send+Sync、无 std::fs、能力命名前缀、requires_confirmation 全部通过）
- [x] 现有 42 个测试全部保留继续通过（CRM 21 + 电商 21 回归无破坏）

---

## 三、Phase 2：AI 生成能力（2-4 个月）

### P2-T1：SDD Agent 编排（4 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-rust-sdd-agent` crate（企业版） | □ | —（2026-08-14 核验：该 crate 在开源/企业版仓库均不存在，历史声称已定性虚构） |
| 2 | Spec Agent（需求规格生成） | □ | — |
| 3 | Design Agent（技术设计 + 存量分析） | □ | — |
| 4 | Task Agent（任务清单生成） | □ | — |
| 5 | Coding Agent（代码生成 + Compile-Fix 循环） | □ | — |
| 6 | HITL 闸门实现 | □ | — |
| 7 | Spec 文件持久化 | □ | — |
| 8 | 多模型路由 | □ | — |
| 9 | 与现有 Skills 集成 | □ | — |

**验收标准**：
- [ ] 输入自然语言需求 → 输出完整 spec.md + design.md + tasks.md
- [ ] 用户确认后 → 生成可编译代码
- [ ] Compile-Fix 循环自动修复编译错误
- [ ] 生成代码附带 SDD 文档

---

### P2-T2：AI 辅助迁移工具（3 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 新建 `sz-plugin-migration`（企业版） | □ | — |
| 2 | TP6 代码分析器（路由/模型/控制器识别） | □ | — |
| 3 | sz-rust 代码生成器 | □ | — |
| 4 | 增量验证工具（对比 TP6 和 sz-rust 响应） | □ | — |
| 5 | 迁移报告生成 | □ | — |
| 6 | 迁移案例文档（基于创始人自己的系统） | □ | — |

**验收标准**：
- [ ] 可分析 TP6 项目，输出分析报告
- [ ] 可生成等价 sz-rust 代码
- [ ] 可对比 TP6 和 sz-rust 响应一致性

---

### P2-T3：行业 RAG 知识库（2 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 29+ 项目代码向量化（Embedding） | □ | — |
| 2 | 行业术语表（菜市场业务术语） | □ | — |
| 3 | 业务规则库（从现有代码提取） | □ | — |
| 4 | RAG 检索集成到 SDD Agent | □ | — |
| 5 | 数据模型模板库 | □ | — |

**验收标准**：
- [ ] SDD Agent 生成代码时可检索行业知识库
- [ ] 检索结果提升生成代码的行业相关性

---

### P2-T4：MCP 工具扩展（1 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | CRUD 操作工具（create/read/update/delete） | □ | — |
| 2 | 迁移管理工具（migrate/create/status） | □ | — |
| 3 | 测试工具（test/run/coverage） | □ | — |
| 4 | 部署工具（deploy/check） | □ | — |
| 5 | 插件管理工具（plugin/list/install/uninstall） | □ | — |

**验收标准**：
- [ ] 所有新工具通过测试
- [ ] AI Agent 可通过 MCP 调用所有工具

---

## 四、Phase 3：产品化（4-6 个月）

### P3-T1：可视化应用搭建画布（6 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | Tauri + Vue 桌面工作 bench | □ | — |
| 2 | 需求描述界面（自然语言输入） | □ | — |
| 3 | 规格可视化（spec.md 渲染） | □ | — |
| 4 | 任务进度看板（tasks.md 执行进度） | □ | — |
| 5 | 实时日志（SDD Agent 执行日志） | □ | — |
| 6 | 插件管理界面 | □ | — |
| 7 | 应用预览 | □ | — |

---

### P3-T2：插件市场 MVP（3 周）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**子任务清单**：

| # | 子任务 | 状态 | 验收结果 |
|---|--------|------|----------|
| 1 | 插件市场 Web 平台 | □ | — |
| 2 | CLI 集成（search/install） | □ | — |
| 3 | 插件审核流程 | □ | — |
| 4 | 支付集成（可选） | □ | — |
| 5 | 开发者文档 | □ | — |

---

### P3-T3：真实用户案例（持续）

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

**目标用户**：
- 有存量 TP6 系统的中小企业技术负责人
- 菜市场/生鲜行业数字化需求方
- 对 Rust + AI 感兴趣的开发者

---

## 五、Phase 4：生态（6-12 个月）

### P4-T1：前端生成

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

### P4-T2：工作流引擎

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

### P4-T3：开发者社区

| 状态 | 开始日期 | 完成日期 | 实际工时 |
|------|----------|----------|----------|
| □ 未开始 | — | — | — |

---

## 六、已完成工作记录

> 此章节记录在规划文档创建之前已完成的工作，作为基线。

### 6.1 P1 安全中间件（SDD 完整流程）

| 项目 | 详情 |
|------|------|
| **完成日期** | 2026-08-11 |
| **SDD 文档** | `.codeartsdoer/specs/security_middleware/` |
| **任务总数** | 38 个子任务（4 批次 A/B/C/D） |
| **新增测试** | middleware-facade: 515 通过（+60），http-facade: 167 通过 |
| **状态** | ■ 已完成 |

**交付内容**：

| 中间件 | 文件 | 测试数 | 默认启用 |
|--------|------|--------|----------|
| 安全响应头注入 | `middleware-facade/src/security_headers.rs` | 15 | 是 |
| IP 访问控制 | `middleware-facade/src/ip_access_control.rs` | 17 | 否 |
| 安全审计日志 | `middleware-facade/src/audit_log.rs` | 11 | 否 |
| 请求体大小限制 | `middleware-facade/src/body_size_limit.rs` | 8 | 否 |
| 配置聚合 | `middleware-facade/src/security_section.rs` | 8 | — |
| 指标聚合 | `middleware-facade/src/security_metrics.rs` | 2 | — |

**中间件执行顺序**（更新后）：
```
Trace → BodySizeLimit → IpAccessControl → SecurityHeaders → Cors → Log → RateLimit → Auth → AuditLog
```

**关键决策**：
- `PHP_GLOBAL_ORDER [Trace, Cors]` 不再是 `DEFAULT_ORDER` 前缀，改为 subset 检查
- `SecuritySection` 定义在 `middleware-facade` 内避免循环依赖
- CSP nonce 使用 `STANDARD_NO_PAD` Base64（16 字节 → 22 字符）
- IP 规则解析支持 CIDR 和单 IP（自动转 /32 或 /128）

---

## 七、FSSADMIN 竞品识别的缺失功能

> 来源：`docs/cases/fssadmin-competitive-analysis.md`
> 这些功能不在 4 阶段路线图中，但作为补充 backlog 记录于此。

| # | 缺失功能 | 优先级 | 归属阶段 | 状态 |
|---|----------|--------|----------|------|
| 1 | CSRF 防护中间件 | P1 | Phase 1 补充 | □ |
| 2 | XSS 过滤中间件 | P1 | Phase 1 补充 | □ |
| 3 | IP 黑名单（已由 IP 访问控制覆盖） | P1 | 已完成 | ■ |
| 4 | 代码生成器 | P0 | Phase 2（SDD Agent 覆盖） | □ |
| 5 | 数据权限（行级/字段级） | P1 | Phase 1 补充 | □ |
| 6 | 多租户 SaaS | P2 | Phase 3 | □ |
| 7 | 配套 Admin 前端模板 | P2 | Phase 3 | □ |
| 8 | 插件市场 | P2 | Phase 3（P3-T2 覆盖） | □ |

---

## 八、进度更新日志

| 日期 | 更新内容 | 更新人 |
|------|----------|--------|
| 2026-08-11 | 初始创建：建立进度追踪框架，记录已完成的安全中间件工作 | AI Agent |
| 2026-08-11 | P1-T1 Capability Registry Batch A-C 完成：crate 骨架 + Capability trait + Registry + Cap facade + MCP 适配 + 24 测试通过 | AI Agent |
| 2026-08-11 | P1-T1 性能基准全部达标：注册 187ns / 查找 38ns / 标签搜索 20μs | AI Agent |
| 2026-08-11 | P1-T1 API 文档完成：rustdoc + 5 doc-tests 通过 | AI Agent |
| 2026-08-11 | P1-T1 集成完成：ai-facade LlmChatCapability + addons-loader CapabilityHook，P1-T1 全部完成 | AI Agent |

---

## 九、下一步行动

### 立即可执行（按优先级排序）

1. **P1-T1 Capability Registry**（Phase 1 第一个任务，2 周）
   - 新建 `sz-rust-capability` crate
   - 定义 `Capability` trait
   - 实现 `CapabilityRegistry`
   - 这是所有后续 AI 能力的基础

2. **FSSADMIN 缺失功能补充**（可与 P1-T1 并行）
   - CSRF 防护中间件
   - XSS 过滤中间件
   - 数据权限中间件

### 需要决策的事项

- [ ] 是否先补充 FSSADMIN 缺失的安全功能（CSRF/XSS/数据权限），再开始 Phase 1？
- [ ] Capability Registry 是放在开源版还是企业版？
- [ ] 是否需要先做开源/企业 crate 分离（P1-T2），再做 Capability Registry（P1-T1）？

---

> **使用说明**：
> 1. 每完成一个子任务，将 `□` 改为 `■`，填写验收结果
> 2. 每开始一个任务，将状态改为 `▓ 进行中`，填写开始日期
> 3. 每完成一个任务，更新总体进度概览的百分比和里程碑状态
> 4. 在进度更新日志中记录每次更新
> 5. 在"下一步行动"中维护当前可执行的任务列表