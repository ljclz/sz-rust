# SzRSQL 语义化版本策略

> **版本**：v1.0（2026-07-20）
> **适用范围**：SzRSQL 数据库服务（含二进制 `szrsql`、所有 workspace crate、存储格式、协议兼容性）
> **参考**：[Semantic Versioning 2.0.0](https://semver.org/) + PostgreSQL 升级兼容性实践

## 1. 版本号格式

SzRSQL 采用 `MAJOR.MINOR.PATCH` 三段式版本号：

```
MAJOR.MINOR.PATCH
  │    │    │
  │    │    └─ 向后兼容的 bug 修复
  │    └────── 向后兼容的新功能
  └─────────── 不兼容的破坏性变更
```

版本号定义于 `Cargo.toml`（workspace.package.version），所有子 crate 通过 `version.workspace = true` 继承，确保版本号集中管理。

### 1.1 当前版本

- **当前版本**：`0.1.0`（Alpha 阶段，Phase 4 完成前）
- **预期 GA 版本**：`1.0.0`（Phase 8 完成后，Jepsen + 7×24h 长稳测试通过）

### 1.2 预发布标识

发布候选版本使用预发布标识：

- `1.0.0-alpha.1`、`1.0.0-alpha.2`（Alpha 阶段）
- `1.0.0-beta.1`、`1.0.0-beta.2`（Beta 阶段，pg_dump 迁移验证）
- `1.0.0-rc.1`、`1.0.0-rc.2`（Release Candidate，候选发布）
- `1.0.0`（GA 正式发布）

详见 `docs/release_stages.md`。

## 2. MAJOR / MINOR / PATCH 边界

### 2.1 PATCH（补丁版本）

**递增条件**：向后兼容的 bug 修复。

**包含**：
- bug 修复（崩溃、数据错误、协议错误等）
- 性能优化（不改变 API/行为）
- 文档修正
- 测试增强
- 内部重构（不影响公开 API）

**不包含**：
- 任何新功能
- 任何 API 变更
- 任何存储格式变更

**示例**：
- `1.0.0` → `1.0.1`：修复 SCRAM-SHA-256 认证在特定编码下的解析错误
- `1.0.1` → `1.0.2`：修复 COPY FROM 在大文件（>1GB）时的内存溢出

**兼容性**：
- **二进制兼容**：`1.0.x` 的客户端可连接 `1.0.y` 的服务端（x、y 任意）
- **存储兼容**：`1.0.x` 写入的数据文件可被 `1.0.y` 读取
- **协议兼容**：pgwire 协议行为完全一致

### 2.2 MINOR（次版本）

**递增条件**：向后兼容的新功能。

**包含**：
- 新 SQL 语法（不破坏现有语法）
- 新内置函数、新数据类型
- 新系统视图、新管理端点
- 新配置参数（默认值保持原行为）
- 性能优化（可能改变查询计划，但结果一致）
- 存储格式**向前兼容**扩展（详见 §3）

**不包含**：
- 破坏性 API 变更
- 默认行为变更
- 存储格式不兼容变更

**示例**：
- `1.0.0` → `1.1.0`：新增 `JSONB` 数据类型
- `1.1.0` → `1.2.0`：新增 `EXPLAIN ANALYZE` 语法
- `1.2.0` → `1.3.0`：新增并行查询执行器

**兼容性**：
- **二进制兼容**：同 MAJOR 内，高 MINOR 服务端可接受低 MINOR 客户端连接
- **存储兼容**：详见 §3 存储格式兼容性规则
- **协议兼容**：pgwire 协议向后兼容（新增消息类型不影响旧客户端）

### 2.3 MAJOR（主版本）

**递增条件**：不兼容的破坏性变更。

**包含**：
- 删除或重命名公开 API
- 改变默认行为
- 存储格式不兼容变更
- 协议破坏性变更（删除消息类型、改变字段语义）
- 最低支持的 Rust 版本提升
- 最低支持的操作系统版本提升

**示例**：
- `1.x` → `2.0.0`：重设计 WAL 格式，需要 `szrsql_upgrade` 迁移工具
- `2.x` → `3.0.0`：删除已废弃的 v1 协议消息类型

**兼容性**：
- **不保证二进制兼容**：跨 MAJOR 升级需停机 + 数据迁移
- **不保证存储兼容**：需要专门的升级工具
- **提供迁移指南**：每个 MAJOR 版本发布时附带 `MIGRATION-vN-to-v(N+1).md`

## 3. 存储格式兼容性规则

### 3.1 数据文件（.szrsql）

SzRSQL 数据文件采用版本化的文件头（magic + format_version）。

| 升级路径 | 兼容性 | 处理方式 |
|---------|--------|---------|
| `1.0.x` → `1.0.y` (PATCH) | ✅ 完全兼容 | 直接打开，无需迁移 |
| `1.0.x` → `1.1.y` (MINOR) | ✅ 向前兼容 | 新版本可读取旧格式；写入时可选升级到新格式 |
| `1.1.x` → `1.0.y` (MINOR 降级) | ⚠️ 受限兼容 | 旧版本可读取新格式中**未使用新功能**的数据；若使用了新功能（如新数据类型），降级会失败 |
| `1.x` → `2.0.y` (MAJOR) | ❌ 不兼容 | 需要 `szrsql_upgrade` 迁移工具 |

### 3.2 WAL 日志

WAL 记录同样包含 `format_version` 字段。

- **PATCH**：WAL 格式不变，旧 WAL 可被新版本回放
- **MINOR**：WAL 格式可扩展（新增记录类型），旧 WAL 可被新版本回放；新版本写入的新记录类型不被旧版本识别（旧版本遇到未知类型会报错停止）
- **MAJOR**：WAL 格式可能完全重设计，旧 WAL 不可回放

### 3.3 系统目录（system catalog）

系统目录表结构变更遵循：
- **PATCH**：不修改系统目录结构
- **MINOR**：可新增系统表、新增列（默认值填充）、新增索引；不删除现有表/列
- **MAJOR**：可任意重构系统目录

### 3.4 pgwire 协议

- **PATCH**：协议行为完全一致
- **MINOR**：可新增消息类型、新增 ParameterStatus 参数；旧客户端不受影响
- **MAJOR**：可删除/重定义消息类型，需客户端升级

## 4. API 稳定性

### 4.1 公开 API 分层

SzRSQL 的 API 分为三层，稳定性承诺不同：

| 层级 | 包含 | 稳定性 |
|------|------|--------|
| **稳定 API**（Stable） | pgwire 协议、SQL 语法、CLI 参数、配置文件格式 | MINOR 内不破坏 |
| **实验 API**（Experimental） | `szrsql_*` Rust crate 的公开类型、HTTP 管理端点 | PATCH 内可能调整，标注 `#[doc(cfg(feature = "experimental"))]` |
| **内部 API**（Internal） | `*_internal` 模块、私有类型 | 随时变更，无承诺 |

### 4.2 弃用流程（Deprecation）

弃用 API 遵循以下流程：

1. **标注弃用**：在 MINOR 版本中标注 `#[deprecated(since = "1.x.0", note = "...")]`，文档标注 "已弃用"
2. **保留兼容**：至少 2 个 MINOR 版本内保持可用
3. **正式移除**：在下一个 MAJOR 版本中删除

## 5. 发布节奏

### 5.1 PATCH 版本

- **频率**：按需发布（有重要 bug 修复时）
- **窗口**：任何时候都可发布
- **流程**：bug 修复 → 测试 → 标记 tag `v1.0.x` → 自动构建发布

### 5.2 MINOR 版本

- **频率**：每 4-8 周一次
- **窗口**：功能冻结后 1-2 周内
- **流程**：feature branch 合并 → beta 预发布（1 周）→ rc 候选（1 周）→ 正式发布

### 5.3 MAJOR 版本

- **频率**：每年最多 1 次
- **窗口**：Q1 或 Q4（避开年底/年初业务高峰）
- **流程**：架构设计 RFC → 社区评审 → alpha（4 周）→ beta（4 周）→ rc（2 周）→ 正式发布 + 迁移工具

## 6. 版本号管理实践

### 6.1 集中管理

版本号定义于 `Cargo.toml`：

```toml
[workspace.package]
version = "0.1.0"  # 唯一来源
```

所有子 crate 通过 `version.workspace = true` 继承：

```toml
[package]
name = "szrsql-protocol"
version.workspace = true
```

### 6.2 发布前检查清单

发布新版本前需验证：

- [ ] `Cargo.toml` 中 `workspace.package.version` 已更新
- [ ] `cargo build --release --workspace` 通过
- [ ] `cargo test --workspace` 全部通过
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 0 警告
- [ ] `cargo fmt --all --check` 0 差异
- [ ] `CHANGELOG.md` 已更新对应版本条目
- [ ] git tag `vX.Y.Z` 已创建
- [ ] GitHub Release 已发布（含二进制 + 校验和）

### 6.3 版本号递增规则示例

| 当前版本 | 变更内容 | 新版本 |
|---------|---------|--------|
| `1.0.0` | 修复 SCRAM 认证 bug | `1.0.1` |
| `1.0.1` | 新增 JSONB 类型 | `1.1.0` |
| `1.1.0` | 新增 EXPLAIN ANALYZE | `1.2.0` |
| `1.2.0` | 修复 EXPLAIN 输出格式错误 | `1.2.1` |
| `1.2.1` | 重设计 WAL 格式（不兼容） | `2.0.0` |

## 7. 兼容性测试

### 7.1 CI 自动化测试

CI 流水线包含兼容性回归测试：

- **PATCH 回归**：`1.0.x` 数据文件被 `1.0.y` 读取（自动）
- **MINOR 向前兼容**：`1.0.x` 数据文件被 `1.1.y` 读取（自动）
- **MINOR 降级**：`1.1.x` 数据文件（未使用新功能）被 `1.0.y` 读取（手动）
- **MAJOR 迁移**：`1.x` → `2.0` 通过 `szrsql_upgrade` 工具（手动）

### 7.2 长稳测试

每个 MINOR 版本发布前需通过 7×24h 长稳测试（RSS/fd/pool/ops/latency 无退化）。

## 8. 异常版本号

### 8.1 0.x.y 版本（Alpha/Beta 阶段）

`0.x.y` 版本不承诺任何兼容性：
- `0.1.0` → `0.2.0` 可能包含破坏性变更
- 公开 API 可能随时调整
- 存储格式可能不兼容

`0.x.y` 阶段升级时**务必**备份数据，预期需要重新初始化。

### 8.2 元版本（Build Metadata）

构建元数据附加在版本号后，不影响优先级：

- `1.0.0+20260720` — 2026-07-20 构建
- `1.0.0+dev` — 开发版

## 9. 版本号优先级

遵循 SemVer 2.0.0 规则：

```
1.0.0-alpha < 1.0.0-beta < 1.0.0-rc.1 < 1.0.0-rc.2 < 1.0.0
```

预发布版本与正式版本比较时，预发布版本优先级更低。

## 10. 参考文档

- [Semantic Versioning 2.0.0](https://semver.org/)
- [PostgreSQL Versioning Policy](https://www.postgresql.org/support/versioning/)
- [Keep a Changelog](https://keepachangelog.com/)
- [SzRSQL 发布阶段定义](docs/release_stages.md)
- [SzRSQL CHANGELOG](CHANGELOG.md)
