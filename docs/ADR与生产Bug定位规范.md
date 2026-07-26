# ADR 与生产 Bug 定位规范（SzRSQL 分布式数据库专用）

> **来源**：从 SzRSQL 项目代码审计提炼（289K 行 Rust 代码 / 16 个 crate / 分布式 SQL 数据库）。
> **适用**：AI 驱动开发的大型分布式数据库项目，尤其适合"代码量大、模块多、AI 会话无状态、生产 bug 难复现"的场景。
> **版本**：1.1.0（2026-07-23）

---

## 1. 核心问题

SzRSQL 作为 289K 行、16 个 crate 的分布式 SQL 数据库，AI 驱动开发存在三个痛点：

| 痛点 | 表现 | 后果 |
|------|------|------|
| **无状态** | 每次 AI 会话从零开始，不知道"为什么选 MVCC 而非 2PL""为什么 Page 是 16KB" | 误改核心设计决策，引入数据损坏/一致性回归 |
| **覆盖率盲区** | ADR 只覆盖少数决策点，16 个 crate 中 14 个无 tracing | 生产 bug 落在 ADR 盲区，无法定位是设计如此还是实现错误 |
| **运行时黑盒** | SQL 解析/优化/执行、WAL、Raft、分布式事务等核心路径无 tracing span | 生产出问题只能靠猜，排查时间长，分布式场景下几乎无法复现 |

**本规范解决**：用 ADR 解决"无状态"问题，用 tracing 解决"运行时黑盒"问题，用四层定位流程解决"覆盖率盲区"问题。

---

## 2. 核心原则

1. **ADR 是决策记忆，不是 bug 定位工具**——ADR 记录"为什么这么写"，不能指望它定位所有 bug。数据库项目中设计限制类 bug（如隔离级别导致的幻读）应 100% 命中，实现层 bug ≥50% 部分命中，未覆盖模块 bug 0% 命中（可接受，但事后必须补 ADR 或 tracing）。
2. **ADR + tracing + metrics + 源码四层组合**——单靠任何一层都不够。ADR 排除"是不是设计如此"，tracing 定位"生产发生了什么"，metrics 判断系统健康度，源码验证根因。分布式数据库中四层缺一不可。
3. **ADR 必须含"Bug 定位提示"段**——每个 ADR 必须回答："如果这个决策相关模块出 bug，现象是什么？该查哪里？"对数据库尤其重要：持久性/一致性/隔离性 bug 现象往往相似，必须靠 ADR 区分。
4. **关键路径必须有可观测性**——ADR 覆盖不到的模块，靠 tracing span 兜底。没有 span 的代码 = 生产黑盒。SzRSQL 当前 14/16 crate 无 tracing，是首要整改项。
5. **ADR 有效性必须实测**——不能假设"写了 ADR 就有效"，必须用零上下文子代理测试命中率和 bug 定位能力。

---

## 3. ADR 写作规范

### 3.1 文件命名

```
docs/adr/
├── README.md                              # 索引 + 使用说明
├── 0001-kebab-case-title.md               # 编号 + 短标题（英文）
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
- **决策类型**: 并发原语 | 存储引擎 | 事务隔离 | 分布式共识 | 分布式事务 | 时钟方案 | 分片策略 | SQL注入防护 | 资源限制 | 安全防护
- **相关代码**: `path/to/file.rs (L123-L145)`  ← 必须标行号
- **修复编号**: 修复的 bug 编号（若有，如 Critical C-1）

## 背景
为什么需要做这个决策？解决了什么问题？不做的后果是什么？
（必须包含具体的技术细节，如"不选 OCC 会导致死锁频率升高"，不能泛泛而谈）

## 决策
最终选择了什么方案？关键代码片段？
（必须有可对照的代码，不能只有文字描述）

## 后果
**正面**：方案带来的好处
**负面**：方案的代价、限制、已知缺陷  ← 必须诚实记录负面后果

## 注意事项
- 未来如何迁移？（如 Raft 稳定后改用 Multi-Raft）
- 调用方必须遵守的约束？
- **Bug 定位提示**：← 必填，见 3.3
```

### 3.3 Bug 定位提示段（必填）

每个 ADR 的"注意事项"段必须包含 **"Bug 定位提示"** 子段，回答：

1. **如果这个决策相关模块出 bug，现象可能是什么？**
2. **该查哪里？（具体到函数/文件/配置项/span 字段）**
3. **哪些情况可以排除本模块的问题？**（帮助缩小范围）

**示例（持久性模型 ADR）**：

```markdown
- **Bug 定位提示**：如果 commit 后宕机导致数据丢失：
  1. 检查 commit-then-log vs log-then-commit：WAL fsync 是否在 commit 返回前完成
  2. 查 `wal.rs` 的 append + fsync 调用顺序，确认 lsn 是否在 commit 前持久化
  3. 查 tracing span `wal.fsync` 的返回值，确认 fsync 是否成功
  4. 若 fsync 在 commit 之后 → 持久性模型缺陷（设计如此，需迁移到 log-then-commit）
  5. 可排除：Raft 复制层（单节点宕机不涉及共识）
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

### 4.1 必须覆盖的决策类型（数据库专用）

| 决策类型 | 说明 | 示例 |
|---------|------|------|
| **并发原语选择** | MVCC / OCC / 2PL 的选择及原因 | MVCC vs OCC：高读写冲突场景选 MVCC |
| **存储引擎决策** | Page 大小 / B-Tree / LSM / WAL 的选择 | Page 16KB / B+Tree / WAL append-only |
| **事务隔离级别选择** | RC / RR / SER + SSI 的选择 | 默认 RR，SSI 作为可选串行化 |
| **分布式共识选择** | Raft / Paxos / Multi-Paxos 的选择 | Raft（领导者选举清晰，工程实现简单） |
| **分布式事务模型** | Percolator / 2PC / 3PC / Saga 的选择 | Percolator（TiDB 验证，单点协调者） |
| **时钟方案** | HLC / TrueTime / Vector Clock 的选择 | HLC（无需专用硬件，因果一致性） |
| **分片策略** | 哈希 / 范围 / 一致性哈希的选择 | Range Sharding（范围查询友好） |
| **SQL注入防护** | 白名单 / 参数化 / 标识符转义的选择 | 标识符必须白名单 + 转义，参数必须绑定 |
| **资源限制策略** | 连接池 / buffer pool / WAL 大小的限制 | buffer_pool=1GB / WAL segment=64MB |
| **安全防护机制** | TDE / 审计 / 防火墙的选择 | TDE 静态加密 + 审计日志 |

### 4.2 覆盖率度量

- **ADR 密度** = ADR 数量 / 千行非测试代码
- **目标**：≥ 0.15（即每 1000 行至少 0.15 个 ADR，约每 6700 行 1 个 ADR）
- **SzRSQL 当前**：289 千行代码，按目标需 ≥ 43 个 ADR（当前远低于此，需补全）

### 4.3 覆盖率盲区识别

定期执行（每季度或大版本前）：

1. 列出 16 个 crate 的所有核心模块
2. 标记每个模块是否有 ADR 覆盖 + 是否有 tracing 覆盖
3. 无 ADR 且无 tracing 的模块 = 盲区，必须补 ADR 或加 tracing 兜底

---

## 5. 运行时可观测性规范

### 5.1 关键路径必须加 tracing

ADR 覆盖不到的运行时问题，靠 tracing span 兜底。以下路径**必须**有结构化可观测性：

| 路径类型 | 必须标注的函数 | span 字段 |
|---------|--------------|----------|
| **SQL 解析/优化/执行** | parse / optimize / execute / physical_plan | op, table, plan_cost |
| **事务操作** | begin / commit / rollback | tx_id, isolation_level |
| **WAL 操作** | append / fsync / checkpoint | lsn, tx_id |
| **Page 操作** | read / write / evict | page_id, table, lsn |
| **Buffer Pool** | acquire / evict / flush | page_id, pool_size, hit_rate |
| **Raft 操作** | propose / append / commit | term, leader_id, shard_id |
| **分布式事务** | prepare / commit / abort | tx_id, shard_id, participant_count |
| **分片迁移** | migrate / split / merge | shard_id, src_node, dst_node |
| **连接管理** | accept / close / auth | conn_id, user |

### 5.2 span 字段标准

- `op`：操作类型（select/insert/update/ddl/wal_append/raft_propose）
- `table`：表名
- `tx_id`：事务 ID
- `lsn`：WAL 日志序列号
- `isolation_level`：隔离级别（rc/rr/ser/ssi）
- `shard_id`：分片 ID
- `term`：Raft 任期
- `leader_id`：Raft 领导者 ID
- `error`：错误类型（若有）
- **禁止** span 字段包含敏感数据（密码/token/PII/明文 SQL 参数值）

### 5.3 日志分级

| 级别 | 使用场景 |
|------|---------|
| `ERROR` | 操作失败，影响数据一致性（WAL fsync 失败、Raft commit 失败） |
| `WARN` | 降级/重试/接近限制（buffer pool eviction 频繁、连接数接近上限） |
| `INFO` | 关键业务节点（事务提交、checkpoint 完成、leader 切换） |
| `DEBUG` | 详细诊断信息（SQL 计划、Page 命中率、Raft 日志条目） |
| `TRACE` | 极细粒度（每行 lock acquire、每页 read/write） |

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
│ 输出：buffer pool/连接数/CPU/IO 异常│
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
| 运行时层 | 确认是 bug | 查 tracing span | 哪个函数/步骤异常 | span 显示空返回/超时/错误 → 进入第 4 层 |
| 指标层 | 运行时异常 | 查 Prometheus | 系统是否健康 | 指标异常（如 buffer pool 命中率骤降）→ 先修基础设施 |
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
3. 问 3 个"为什么"问题（如"为什么选 MVCC 而非 2PL？""为什么 Page 是 16KB？"）
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

| 类型 | 现象特征 | 数据库示例 |
|------|---------|------|
| **设计限制** | 现象与 ADR 负面后果吻合 | RR 隔离级别下出现幻读（设计如此） |
| **实现错误** | ADR 说"会做 X"但实际没做 | WAL fsync 顺序写反（commit-then-log） |
| **运行时状态** | 高并发/长时间运行才出现 | buffer pool eviction 风暴导致性能抖动 |
| **未覆盖模块** | ADR 完全没涉及的模块 | 分片迁移中途失败导致数据不一致 |

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
          ADR_COUNT=$(ls docs/adr/0*.md | wc -l)
          LOC=$(find . -name "*.rs" -not -path "*/target/*" | xargs wc -l | tail -1)
          # 密度 = ADR_COUNT / (LOC / 1000)，应 >= 0.15

  tracing-coverage:
    steps:
      - name: 检查关键路径 tracing 覆盖
        run: |
          # 检查 sql/wal/raft/tx/buffer_pool 等关键模块的 pub fn 是否有 #[tracing::instrument]

  adr-format:
    steps:
      - name: 检查 ADR 格式
        run: |
          # 每个 ADR 必须有：状态/日期/决策类型/相关代码/背景/决策/后果/注意事项/Bug定位提示
```

### 8.3 定期审计

每季度执行：
1. ADR 覆盖率盲区识别（4.3）
2. Bug 定位命中率测试（7.2）
3. tracing 覆盖率检查（重点：14/16 crate 无 tracing 的整改进度）
4. 失效 ADR 清理（Superseded/Deprecated 状态的 ADR 是否已标注）

---

## 9. 案例参考（SzRSQL 审计发现）

### 9.1 审计发现的问题

| 问题 | 严重度 | 根因 | 整改方向 |
|------|--------|------|---------|
| **持久性模型风险** | Critical | commit-then-log（commit 返回前未 fsync WAL） | 迁移到 log-then-commit，写 ADR |
| **标识符转义缺失** | High | 表名/列名未白名单+转义，SQL 注入风险 | 写 ADR + 加白名单校验 |
| **可观测性缺失** | High | 14/16 crate 无 tracing，核心路径黑盒 | 按 5.1 补 tracing |
| **锁毒化风险** | Medium | 大量 `.lock().unwrap()`，panic 会导致锁毒化 | 改用 `lock().unwrap_or_else` + 写 ADR |

### 9.2 Bug 定位案例（基于审计）

| Bug | 现象 | 第 1 层（ADR） | 第 2 层（tracing） | 根因 |
|-----|------|---------------|-------------------|------|
| Bug 1 | commit 后宕机数据丢失 | ❌ 无 ADR 覆盖（需补持久性模型 ADR） | ❌ 无 wal.fsync span（需补） | commit-then-log，fsync 在 commit 后 |
| Bug 2 | 表名含特殊字符导致 SQL 异常 | ❌ 无 ADR 覆盖（需补标识符转义 ADR） | ❌ 无 parse span | 标识符未转义，注入风险 |
| Bug 3 | 高并发下 panic 后服务不可用 | ❌ 无 ADR 覆盖（需补锁毒化 ADR） | ❌ 无 lock span | `.lock().unwrap()` 导致锁毒化 |
| Bug 4 | 分片迁移中途失败数据不一致 | ❌ 无 ADR 覆盖 | ❌ 无 migrate span | 迁移流程无原子性保证 |

### 9.3 补全措施

针对上述审计发现，SzRSQL 必须补：

- **ADR-0001**：持久性模型（log-then-commit），含 Bug 1 定位提示
- **ADR-0002**：标识符转义（白名单+转义），含 Bug 2 定位提示
- **ADR-0003**：锁毒化防护（避免 `.lock().unwrap()`），含 Bug 3 定位提示
- **ADR-0004**：分片迁移原子性，含 Bug 4 定位提示
- **tracing 补全**：14 个无 tracing 的 crate 按 5.1 关键路径逐步补全

---

## 附录 A：ADR 模板（数据库专用）

```markdown
# ADR-XXXX: 标题

- **状态**: Accepted
- **日期**: YYYY-MM-DD
- **决策类型**: 并发原语 | 存储引擎 | 事务隔离 | 分布式共识 | 分布式事务 | 时钟方案 | 分片策略 | SQL注入防护 | 资源限制 | 安全防护
- **相关代码**: `path/to/file.rs (L123-L145)`
- **修复编号**: （若无则删除此行）

## 背景

[为什么需要做这个决策？解决了什么问题？不做的后果？]
[必须包含具体技术细节，如"不选 log-then-commit 会导致宕机丢数据"]

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
  1. [排查点 1，含具体函数/文件/span 字段]
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
- 查阅 span：[span 名称，如 wal.fsync / raft.commit]
- 异常发现：[耗时/返回值/错误，含 lsn/tx_id/term 等字段值]

### 第 3 层：指标层（metrics）
- 查阅指标：[指标名称，如 buffer_pool_hit_rate / connection_count]
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

## 附录 C：落地清单（SzRSQL 接入）

SzRSQL 接入本规范时，按以下顺序执行：

1. [ ] 创建 `docs/adr/` 目录
2. [ ] 复制 ADR 模板（附录 A）到 `docs/adr/template.md`
3. [ ] 创建 `docs/adr/README.md` 索引
4. [ ] 为审计发现的 4 个 Critical/High 问题补 ADR（ADR-0001~0004）
5. [ ] 按 4.1 必须覆盖的 10 类决策补全 ADR（并发/存储/隔离/共识/事务/时钟/分片/注入/资源/安全）
6. [ ] 为 14 个无 tracing 的 crate 按 5.1 关键路径补 tracing
7. [ ] 执行零上下文子代理测试（7.1）
8. [ ] 执行 bug 定位命中率测试（7.2）
9. [ ] 配置 CI 门禁（8.2）
10. [ ] 记录基线数据（ADR 密度、tracing 覆盖率、命中率）
