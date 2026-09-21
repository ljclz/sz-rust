# ADR 与生产 Bug 定位规范（可复用）

> **来源**：从 SZ-Rust 项目实测提炼（基于 SZ-ORM 的 5 ADR 决策记忆测试 + 4 生产 bug 定位实测 + tracing 可观测性补全经验，迁移至 Web 框架场景）。
> **适用**：AI 驱动开发的大型项目（不限语言/框架），尤其适合"代码量大、AI 会话无状态、生产 bug 难复现"的场景。
> **版本**：1.0.0（2026-07-22）

---

## 1. 核心问题

AI 驱动开发的项目存在三个痛点：

| 痛点 | 表现 | 后果 |
|------|------|------|
| **无状态** | 每次 AI 会话从零开始，不知道"为什么这么写" | 误改设计决策，引入回归 |
| **覆盖率盲区** | ADR 只覆盖少数决策点，代码量巨大 | 生产 bug 落在 ADR 盲区，无法定位 |
| **运行时黑盒** | 核心代码无 tracing/日志，生产出问题无 span 可查 | 只能靠猜，排查时间长 |

**本规范解决**：用 ADR 解决"无状态"问题，用 tracing 解决"运行时黑盒"问题，用四层定位流程解决"覆盖率盲区"问题。

---

## 2. 核心原则

1. **ADR 是决策记忆，不是 bug 定位工具**——ADR 记录"为什么这么写"，不能指望它定位所有 bug。实测命中率：设计限制类 bug 100% 命中，实现层 bug 50% 部分命中，未覆盖模块 bug 0% 命中。
2. **ADR + tracing + metrics + 源码四层组合**——单靠任何一层都不够。ADR 排除"是不是设计如此"，tracing 定位"生产发生了什么"，metrics 判断系统健康度，源码验证根因。
3. **ADR 必须含"Bug 定位提示"段**——这是从纯决策记忆扩展到 bug 定位辅助的关键。每个 ADR 必须回答："如果这个决策相关模块出 bug，现象是什么？该查哪里？"
4. **关键路径必须有可观测性**——ADR 覆盖不到的模块，靠 tracing span 兜底。没有 span 的代码 = 生产黑盒。
5. **ADR 有效性必须实测**——不能假设"写了 ADR 就有效"，必须用零上下文子代理测试命中率和 bug 定位能力。

---

## 3. ADR 写作规范

### 3.1 文件命名

```
docs/sz-rust/adr/
├── README.md                              # 索引 + 使用说明
├── 0001-简短英文标题-kebab-case.md         # 编号 + 短标题
├── 0002-...
└── template.md                            # 模板
```

- 编号严格递增，不复用、不修改已发布 ADR 的编号
- 标题用英文 kebab-case（即使项目是中文文档），便于 URL 和跨项目引用
- 状态变更（如 Accepted → Superseded）不改编号，只改状态字段

### 3.2 四段式结构（必填）

每个 ADR 必须包含以下五段：

```markdown
# ADR-XXXX: 标题

- **状态**: Accepted | Proposed | Superseded | Deprecated
- **日期**: YYYY-MM-DD
- **相关代码**: `path/to/file.rs (L123-L145)`  ← 必须标行号
- **修复编号**: 修复的 bug 编号（若有，如 Critical C-1）

## 背景
为什么需要做这个决策？解决了什么问题？不做的后果是什么？
（必须包含具体的技术细节，不能泛泛而谈）

## 决策
最终选择了什么方案？关键代码片段？
（必须有可对照的代码，不能只有文字描述）

## 后果
**正面**：方案带来的好处
**负面**：方案的代价、限制、已知缺陷  ← 必须诚实记录负面后果

## 注意事项
- 未来如何迁移？（如 RFC 稳定后改回某写法）
- 调用方必须遵守的约束？
- **Bug 定位提示**：← 必填，见 3.3
```

### 3.3 Bug 定位提示段（必填）

每个 ADR 的"注意事项"段必须包含 **"Bug 定位提示"** 子段，回答：

1. **如果这个决策相关模块出 bug，现象可能是什么？**
2. **该查哪里？（具体到函数/文件/配置项）**
3. **哪些情况可以排除本模块的问题？**（帮助缩小范围）

**示例（SZ-Rust 路由分发场景）**：

```markdown
- **Bug 定位提示**：如果路由匹配返回 404 但路由表已注册该路径：
  1. 路由参数提取器（FromRequest）是否返回了 Err（提取失败会转化为 404/400 而非路由未命中）
  2. 中间件链中是否有提前短路返回的层（如鉴权失败被错误映射为 404）
  3. 多应用前缀是否正确合并（oapc/admin/api 前缀冲突会被前缀树错误匹配）
  4. 路由宏展开后的 HTTP method 是否与请求一致（#[get] vs #[post] 宏写错）
```

**示例（SZ-Rust 模型钩子场景）**：

```markdown
- **Bug 定位提示**：如果 before_save 钩子未执行但 Model 已注册 Hook：
  1. HookDispatcher 是否在 App 启动时注入到 Model 的 State（DI 容器缺失钩子）
  2. 钩子事件类型是否匹配（BeforeSave vs BeforeInsert，insert 走 BeforeInsert 不走 BeforeSave）
  3. 钩子优先级是否被高优先级钩子短路返回（priority < 0 会跳过后续钩子）
  4. Model 是否派生了正确的宏（#[derive(Model)] 才会触发钩子注册）
```

### 3.4 相关代码行号 + 修复编号

- **相关代码**：必须标到行号（`L123-L145`），AI 后续维护时可直接跳转
- **修复编号**：如果 ADR 是 bug 修复的产物，必须关联 bug 编号（如 `Critical C-1`），便于回溯

### 3.5 状态流转

```
Proposed → Accepted → (Superseded by ADR-XXXX)
                  ↘ Deprecated
```

- `Accepted`：已采纳，代码已实现
- `Superseded`：被新 ADR 取代，必须标注 `Superseded by ADR-XXXX`
- `Deprecated`：已废弃，代码已移除

**禁止**修改已 Accepted 的 ADR 内容（历史不可变）。新决策写新 ADR。

---

## 4. ADR 覆盖率标准

### 4.1 必须覆盖的决策类型

| 决策类型 | 说明 | 示例 |
|---------|------|------|
| **并发原语选择** | 锁/无锁/CAS 的选择及原因 | parking_lot::Mutex vs std::sync::Mutex |
| **安全防护机制** | 注入防护/认证/授权的关键决策 | 路由白名单 vs 动态校验 / JWT 常量时间比较 |
| **资源限制策略** | 连接池/请求体大小/批量大小的限制 | max_body_size=10MB / chunk_size=1000 |
| **架构分层边界** | 模块职责边界的决策 | Controller 不直接执行 SQL，必须经 Service |
| **API 设计权衡** | trait/接口设计的权衡 | async trait 手动解糖 / FromRequest 提取器设计 |
| **性能优化决策** | 用空间换时间/用复杂度换性能的决策 | 路由前缀树 vs 线性扫描 / 中间件静态分发 |
| **错误处理策略** | 错误恢复/降级/熔断的决策 | 中间件链短路 vs 全链传播 / AppError 统一映射 |

### 4.2 覆盖率度量

- **ADR 密度** = ADR 数量 / 千行非测试代码
- **目标**：≥ 0.15（即每 1000 行至少 0.15 个 ADR，约每 6700 行 1 个 ADR）
- **SZ-Rust 当前**：0 个 ADR / 待统计千行（项目处于初始审计阶段，ADR 待补建）

### 4.3 覆盖率盲区识别

定期执行（每季度或大版本前）：

1. 列出所有核心模块（router / controller / middleware / hook / model / event / cache / queue / auth）
2. 标记每个模块是否有 ADR 覆盖
3. 无 ADR 的模块 = 盲区，必须补 ADR 或加 tracing 兜底

---

## 5. 运行时可观测性规范

### 5.1 关键路径必须加 tracing（或等效）

ADR 覆盖不到的运行时问题，靠 tracing span 兜底。以下路径**必须**有结构化可观测性：

| 路径类型 | 必须标注的函数 | span 字段 |
|---------|--------------|----------|
| **HTTP 请求生命周期** | handle / dispatch / route_match | method, path, status, latency_ms |
| **路由匹配** | match / merge / nest | method, pattern, matched |
| **中间件链** | layer / call / next | layer_name,短路原因 |
| **控制器分发** | invoke / handle_action | controller, action, app |
| **模型钩子** | before_save / after_save / before_delete | hook_event, model, priority |
| **请求体解析** | from_request / parse_body / multipart | content_type, body_size |
| **响应序列化** | into_response / render_json | status, body_size |
| **认证授权** | authenticate / authorize / guard | user_id, permission, granted |
| **事务操作** | begin / commit / rollback | isolation_level, nesting_depth |
| **缓存读写** | cache_get / cache_set / cache_invalidate | key, hit, ttl |

### 5.2 span 字段标准

- `op`：操作类型（route/middleware/controller/hook/auth/cache）
- `method`：HTTP 方法（GET/POST/PUT/DELETE）
- `path`：请求路径（已脱敏，不含 query 中的敏感参数）
- `status`：HTTP 状态码
- `latency_ms`：耗时（毫秒）
- `error`：错误类型（若有）
- **禁止** span 字段包含敏感数据（密码/token/PII/cookie 原文）

### 5.3 日志分级

| 级别 | 使用场景 |
|------|---------|
| `ERROR` | 请求处理失败，5xx 响应，未捕获 panic |
| `WARN` | 4xx 客户端错误，降级/重试/接近限制 |
| `INFO` | 关键业务节点（应用启动/关闭、路由注册、中间件装载） |
| `DEBUG` | 详细诊断信息（路由匹配过程、中间件链顺序、钩子触发） |
| `TRACE` | 极细粒度（每行参数解析、每字段序列化） |

生产环境默认 `INFO`，排查 bug 时临时开 `DEBUG`/`TRACE`。

---

## 6. 生产 Bug 定位流程（四层模型）

```
生产 bug 报告
     │
     ▼
┌─────────────────────────────────────┐
│ 第 1 层：决策层（ADR）              │
│ 问题：是不是设计如此？              │
│ 操作：grep ADR 中的"Bug 定位提示"   │
│ 输出：排除设计限制 / 确认是 bug     │
└─────────────┬───────────────────────┘
              │ 确认是 bug
              ▼
┌─────────────────────────────────────┐
│ 第 2 层：运行时层（tracing/日志）   │
│ 问题：生产发生了什么？              │
│ 操作：查 tracing span 的耗时/返回值 │
│ 输出：定位到哪个函数/哪一步出问题   │
└─────────────┬───────────────────────┘
              │
              ▼
┌─────────────────────────────────────┐
│ 第 3 层：指标层（metrics）          │
│ 问题：系统当时健康吗？              │
│ 操作：查 Prometheus/Grafana         │
│ 输出：QPS/延迟/内存/CPU 是否异常    │
└─────────────┬───────────────────────┘
              │
              ▼
┌─────────────────────────────────────┐
│ 第 4 层：代码层（源码 + 测试）      │
│ 问题：具体哪行代码错？              │
│ 操作：读源码 + 写复现测试           │
│ 输出：根因 + 修复 + 回归测试        │
└─────────────────────────────────────┘
```

### 6.1 每层的输入/输出/退出条件

| 层 | 输入 | 操作 | 输出 | 退出条件 |
|----|------|------|------|---------|
| 决策层 | bug 现象 | grep ADR "Bug 定位提示" | 是设计限制 / 确认 bug | 现象与 ADR 负面后果吻合 → 设计限制，非 bug |
| 运行时层 | 确认是 bug | 查 tracing span | 哪个函数/步骤异常 | span 显示 404/超时/错误 → 进入第 4 层 |
| 指标层 | 运行时异常 | 查 Prometheus | 系统是否健康 | 指标异常（如 QPS 骤降、p99 飙升）→ 先修基础设施 |
| 代码层 | 定位到函数 | 读源码 + 写测试 | 根因 + 修复 | 复现测试通过 → 修复完成 |

### 6.2 每层的失败处理

- **第 1 层失败**（ADR 无覆盖）→ 直接进第 2 层，事后补 ADR
- **第 2 层失败**（无 tracing）→ 直接进第 3 层，事后补 tracing
- **第 3 层失败**（无 metrics）→ 直接进第 4 层，事后补 metrics
- **第 4 层失败**（无法复现）→ 加更多 tracing/日志，等下次复现

---

## 7. ADR 有效性验证流程

### 7.1 零上下文子代理测试（决策记忆有效性）

**频率**：每次新增 ADR 后执行

**方法**：
1. 启动全新子代理（无项目历史上下文）
2. 仅提供 ADR 文件 + 相关代码段
3. 问 3 个"为什么"问题
4. 验证回答是否正确

**通过标准**：3/3 正确

### 7.2 Bug 定位命中率测试

**频率**：每季度或大版本前执行

**方法**：
1. 构造 4 个生产 bug 现象（覆盖设计限制/实现错误/运行时状态/未覆盖模块）
2. 启动全新子代理，仅提供 ADR
3. 评估每个 bug 的定位结果：能 / 部分 / 不能

**通过标准**：
- 设计限制类 bug：100% 能定位
- 实现错误类 bug：≥ 50% 部分定位
- 未覆盖模块 bug：0%（可接受，但事后必须补 ADR 或 tracing）

### 7.3 测试用例构造规范

构造 bug 测试用例时，必须覆盖 4 类：

| 类型 | 现象特征 | 示例 |
|------|---------|------|
| **设计限制** | 现象与 ADR 负面后果吻合 | 路由参数命名冲突被前缀树拒绝 |
| **实现错误** | ADR 说"会做 X"但实际没做 | 中间件链顺序与声明顺序相反 |
| **运行时状态** | 高并发/长时间运行才出现 | 请求 body 解析在高并发下内存暴涨 |
| **未覆盖模块** | ADR 完全没涉及的模块 | 事件分发器丢失订阅者 |

---

## 8. 工程化门禁

### 8.1 PR 检查清单

每个涉及核心模块的 PR 必须确认：

- [ ] 是否引入了新的设计决策？是 → 必须写 ADR
- [ ] ADR 是否包含"Bug 定位提示"段？
- [ ] ADR 是否标注相关代码行号？
- [ ] 新增/修改的关键路径是否加了 tracing？
- [ ] tracing span 字段是否符合 5.2 标准？
- [ ] 是否有零上下文子代理测试？（新增 ADR 时）

### 8.2 CI 门禁

```yaml
# CI 必须包含的检查（伪代码）
jobs:
  adr-coverage:
    steps:
      - name: 检查 ADR 密度
        run: |
          ADR_COUNT=$(ls docs/sz-rust/adr/0*.md 2>/dev/null | wc -l)
          LOC=$(find . -name "*.rs" -not -path "*/target/*" | xargs wc -l | tail -1)
          # 密度 = ADR_COUNT / (LOC / 1000)，应 >= 0.15

  tracing-coverage:
    steps:
      - name: 检查关键路径 tracing 覆盖
        run: |
          # 检查 router/controller/middleware/hook/auth 等关键模块的 pub fn 是否有 #[tracing::instrument]

  adr-format:
    steps:
      - name: 检查 ADR 格式
        run: |
          # 每个 ADR 必须有：状态/日期/相关代码/背景/决策/后果/注意事项/Bug定位提示
```

### 8.3 定期审计

每季度执行：
1. ADR 覆盖率盲区识别（4.3）
2. Bug 定位命中率测试（7.2）
3. tracing 覆盖率检查
4. 失效 ADR 清理（Superseded/Deprecated 状态的 ADR 是否已标注）

---

## 9. 案例参考（SZ-Rust 预设）

### 9.1 预期数据（项目初始阶段）

| 测试项 | 当前状态 | 目标 |
|--------|---------|------|
| ADR 决策记忆测试（3 题） | 待 ADR 建立后执行 | 3/3 正确 |
| ADR bug 定位测试（4 bug） | 待 ADR 建立后执行 | 1 能 / 2 部分 / 1 不能 |
| ADR 密度 | 0 个 / 待统计千行 | ≥ 0.15 |
| tracing 覆盖 | 待统计关键函数 | router/controller/middleware/hook/auth 全覆盖 |

### 9.2 Bug 定位案例（预设场景）

| Bug | 现象 | 第 1 层（ADR） | 第 2 层（tracing） | 根因 |
|-----|------|---------------|-------------------|------|
| Bug 1 | 路由 404 但路由已注册 | ✅ ADR-000X 负面后果吻合 | 不需要 | 设计限制，参数提取器失败映射为 404 |
| Bug 2 | 中间件顺序与声明相反 | ◐ ADR-000X 解释机制 | span 查 layer 执行顺序 | 中间件 Layer 嵌套方向写反 |
| Bug 3 | 高并发下请求体解析 OOM | ◐ ADR-000X 提示 body_size 限制 | span 查 body_size 实际值 | max_body_size 未生效 |
| Bug 4 | 钩子未触发 | ❌ 无 ADR 覆盖 | span 查 hook_event 字段 | 钩子事件类型不匹配 |

### 9.3 补全措施

Bug 4 暴露后，SZ-Rust 应补：
- 对应 ADR（钩子分发器设计，含 Bug 4 定位提示）
- HookDispatcher 关键函数加 `#[tracing::instrument]`（hook_event span 字段）
- 路由匹配函数加 `#[tracing::instrument]`（matched 字段）
- 中间件链执行加 `#[tracing::instrument]`（layer_name 字段）

---

## 附录 A：ADR 模板

```markdown
# ADR-XXXX: 标题

- **状态**: Accepted
- **日期**: YYYY-MM-DD
- **相关代码**: `path/to/file (L123-L145)`
- **修复编号**: （若无则删除此行）

## 背景

[为什么需要做这个决策？解决了什么问题？不做的后果？]

## 决策

[最终选择了什么方案？关键代码片段？]

```rust
// 代码片段
```

## 后果

**正面**：
- [好处 1]
- [好处 2]

**负面**：
- [代价/限制 1]
- [代价/限制 2]

## 注意事项

- [未来迁移路径]
- [调用方约束]
- **Bug 定位提示**：如果 [现象]，检查：
  1. [排查点 1]
  2. [排查点 2]
  3. [可排除的情况]
```

---

## 附录 B：Bug 定位报告模板

```markdown
# Bug 定位报告

## 现象
[生产环境观察到的现象]

## 定位过程

### 第 1 层：决策层（ADR）
- 查阅 ADR：[ADR-XXXX]
- 结论：[设计限制 / 确认是 bug / 无 ADR 覆盖]

### 第 2 层：运行时层（tracing）
- 查阅 span：[span 名称]
- 异常发现：[耗时/返回值/错误]

### 第 3 层：指标层（metrics）
- 查阅指标：[指标名称]
- 异常发现：[值/趋势]

### 第 4 层：代码层（源码 + 测试）
- 根因：[具体哪行代码错]
- 复现测试：[测试代码]

## 修复
- 修复方案：[描述]
- 回归测试：[测试代码]
- ADR 更新：[是否需要新增/修改 ADR]

## 经验沉淀
- 是否需要补 ADR？[是/否，原因]
- 是否需要补 tracing？[是/否，原因]
```

---

## 附录 C：落地清单（新项目接入）

新项目接入本规范时，按以下顺序执行：

1. [ ] 创建 `docs/sz-rust/adr/` 目录
2. [ ] 复制 ADR 模板（附录 A）到 `docs/sz-rust/adr/template.md`
3. [ ] 创建 `docs/sz-rust/adr/README.md` 索引
4. [ ] 为现有核心决策补 ADR（按 4.1 必须覆盖的决策类型）
5. [ ] 为关键路径加 tracing（按 5.1 必须标注的函数）
6. [ ] 执行零上下文子代理测试（7.1）
7. [ ] 执行 bug 定位命中率测试（7.2）
8. [ ] 配置 CI 门禁（8.2）
9. [ ] 记录基线数据（ADR 密度、tracing 覆盖率、命中率）

---

## 附录 D：与其他文档的关系

- 本规范定义 **SZ-Rust 项目的 ADR 与 Bug 定位方法论**，是 [`sz-rust-engineering-practices.md`](sz-rust-engineering-practices.md) 中"五维审查"和"门禁"的可观测性补充；
- [`sz-rust-engineering-practices.md`](sz-rust-engineering-practices.md) 定义 **工程化门禁与测试体系**；
- [`软件项目审计清单.md`](软件项目审计清单.md) 定义 **审计维度与通过标准**；
- [`adr/README.md`](adr/README.md) 是 ADR 索引；
- [`audit/2026-07-22-初始审计.md`](audit/2026-07-22-初始审计.md) 是首次审计报告，记录本规范落地前的基线状态。

> **文档结束**
