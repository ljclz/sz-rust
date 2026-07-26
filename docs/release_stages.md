# SzRSQL 发布阶段定义

> **版本**：v1.0（2026-07-20）
> **适用范围**：SzRSQL 数据库服务从开发到正式发布的全生命周期
> **参考**：[PostgreSQL Release Cycle](https://www.postgresql.org/developer/) + [Rust Edition Release Process](https://github.com/rust-lang/rust/tree/master/src/release)

## 1. 概述

SzRSQL 采用 4 阶段发布模型，从 Alpha 到 GA，每个阶段有明确的目标、准入条件、退出条件和用户承诺。

```
┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐
│  Alpha  │───▶│  Beta   │───▶│   RC    │───▶│   GA    │
└─────────┘    └─────────┘    └─────────┘    └─────────┘
   Phase 6       Phase 7       Phase 8       Jepsen+长稳
   之前           完成          完成          通过
```

## 2. 阶段定义

### 2.1 Alpha（α 阶段）

**目的**：核心功能验证，仅供演示和早期评估。

| 项目 | 内容 |
|------|------|
| **版本号** | `0.x.y` 或 `1.0.0-alpha.N` |
| **触发条件** | Phase 4 完成（pgwire 协议层 + 基础 SQL 执行） |
| **目标用户** | 开发者、内部测试、PoC 评估 |
| **兼容性承诺** | ❌ 无任何承诺，数据格式可能不兼容 |
| **数据安全** | ❌ 不保证，可能丢失数据 |
| **生产使用** | ❌ 严禁生产环境使用 |
| **升级路径** | ❌ 无升级工具，需重新初始化 |
| **支持周期** | 无（推荐升级到 Beta） |

**Alpha 准入条件**：
- [x] Phase 1-4 完成（存储引擎 + 事务 + SQL + pgwire 协议）
- [x] psql 可连接并执行基础 CRUD
- [x] Python/Go/Rust/Node 至少 1 种驱动可连接
- [x] 基础事务（BEGIN/COMMIT/ROLLBACK）工作正常
- [x] `cargo test --workspace` 通过
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 0 警告

**Alpha 退出条件（进入 Beta）**：
- [ ] Phase 5-7 完成（查询优化器 + 持久化 + 备份恢复）
- [ ] pg_dump 备份/恢复工作正常
- [ ] 7×24h 长稳测试通过（RSS/fd/pool 无泄漏）
- [ ] 至少 1 个真实业务场景 PoC 通过

### 2.2 Beta（β 阶段）

**目的**：内部试用，验证完整功能链路，pg_dump 迁移路径打通。

| 项目 | 内容 |
|------|------|
| **版本号** | `1.0.0-beta.N` |
| **触发条件** | Phase 7 完成（持久化 + 备份恢复） |
| **目标用户** | 内部团队、合作伙伴、早期评估客户 |
| **兼容性承诺** | ⚠️ 有限承诺，同 Beta 版本内 PATCH 兼容 |
| **数据安全** | ⚠️ 基本保证（有 WAL + 备份），但仍可能丢失 |
| **生产使用** | ⚠️ 不推荐生产环境使用，仅限非关键场景 |
| **升级路径** | ✅ pg_dump/restore 跨版本迁移 |
| **支持周期** | 6 个月（或 GA 发布后停止） |

**Beta 准入条件**：
- [ ] Phase 5-7 完成
- [ ] pg_dump 备份 + restore 恢复验证通过
- [ ] 跨版本（Alpha → Beta）pg_dump 迁移通过
- [ ] COPY FROM/TO 大文件（>1GB）验证通过
- [ ] SCRAM-SHA-256 认证安全审计通过
- [ ] TLS 1.3 加密通信验证通过
- [ ] 7×24h 长稳测试通过

**Beta 退出条件（进入 RC）**：
- [ ] Phase 8 完成（高可用 + 复制 + 分布式）
- [ ] Jepsen 一致性测试通过（bank/register/set 模型）
- [ ] 主备切换（failover）验证通过
- [ ] 至少 3 个合作伙伴完成 PoC
- [ ] 已知 critical bug 数 = 0

### 2.3 RC（Release Candidate，候选发布）

**目的**：候选发布版本，除非发现重大问题，否则直接转为 GA。

| 项目 | 内容 |
|------|------|
| **版本号** | `1.0.0-rc.N` |
| **触发条件** | Phase 8 完成（高可用 + 复制 + 分布式） |
| **目标用户** | 评估客户、内部生产预演 |
| **兼容性承诺** | ✅ 与 GA 完全兼容（RC → GA 无需迁移） |
| **数据安全** | ✅ 保证（WAL + 备份 + 复制） |
| **生产使用** | ⚠️ 允许预生产环境使用，不推荐正式生产 |
| **升级路径** | ✅ in-place 升级到 GA（无需 pg_dump） |
| **支持周期** | GA 发布后 3 个月 |

**RC 准入条件**：
- [ ] Phase 8 完成
- [ ] Jepsen 测试通过（CAP 一致性验证）
- [ ] 主备切换（graceful + force）验证通过
- [ ] 跨数据中心复制验证通过
- [ ] 分布式事务（2PC）验证通过
- [ ] 性能基准测试达标（TPC-C > 1000 tpmC）
- [ ] 已知 critical bug 数 = 0
- [ ] 已知 major bug 数 < 5

**RC 退出条件（进入 GA）**：
- [ ] 至少 1 个 RC 版本发布且无 critical bug 报告
- [ ] 7×24h × 7天 长稳测试通过
- [ ] 安全审计（third-party pentest）通过
- [ ] 用户文档、运维手册、迁移指南完整
- [ ] 至少 1 个真实生产环境试运行 ≥ 30 天

### 2.4 GA（General Availability，正式发布）

**目的**：正式生产发布，提供完整支持和兼容性承诺。

| 项目 | 内容 |
|------|------|
| **版本号** | `1.0.0`、`1.0.1`、`1.1.0` 等 |
| **触发条件** | RC 退出条件全部满足 |
| **目标用户** | 所有用户（含生产环境） |
| **兼容性承诺** | ✅ 完整承诺（详见 [VERSIONING.md](../VERSIONING.md)） |
| **数据安全** | ✅ 完整保证（WAL + 备份 + 复制 + 时间旅行） |
| **生产使用** | ✅ 推荐生产环境使用 |
| **升级路径** | ✅ in-place 升级（PATCH/MINOR）+ 迁移工具（MAJOR） |
| **支持周期** | 每个 MINOR 版本支持 18 个月（LTS 版本支持 36 个月） |

**GA 准入条件**：
- [ ] RC 退出条件全部满足
- [ ] 安全审计报告公开
- [ ] 性能基准报告公开
- [ ] SLA 定义文档发布（99.9% availability）
- [ ] 升级/降级/回滚流程文档化
- [ ] 监控告警模板（Prometheus + Grafana）发布
- [ ] 运维手册（部署/扩缩容/备份恢复/故障排查）发布

## 3. 阶段转换矩阵

### 3.1 兼容性矩阵

| 从 \ 到 | Alpha | Beta | RC | GA |
|---------|-------|------|----|----|
| **Alpha** | — | ⚠️ pg_dump | ❌ | ❌ |
| **Beta** | ❌ | — | ✅ in-place | ✅ in-place |
| **RC** | ❌ | ❌ | — | ✅ in-place |
| **GA** | ❌ | ❌ | ❌ | — |

**说明**：
- `pg_dump`：通过 `pg_dump` 导出 + `pg_restore` 导入迁移
- `in-place`：直接替换二进制 + 重启，无需数据迁移
- `❌`：不支持降级

### 3.2 测试要求矩阵

| 测试项 | Alpha | Beta | RC | GA |
|--------|-------|------|----|----|
| 单元测试 | ✅ | ✅ | ✅ | ✅ |
| 集成测试 | ✅ | ✅ | ✅ | ✅ |
| psql 兼容性 | ✅ | ✅ | ✅ | ✅ |
| 驱动兼容性 | 部分 | ✅ | ✅ | ✅ |
| 性能基准 | 基线 | ✅ | ✅ | ✅ |
| 7×24h 长稳 | ❌ | 1×24h | 3×24h | 7×24h |
| Jepsen | ❌ | ❌ | ✅ | ✅ |
| 安全审计 | ❌ | ❌ | ✅ | ✅ |
| 模糊测试 | 部分 | ✅ | ✅ | ✅ |
| 故障注入 | ❌ | 部分 | ✅ | ✅ |

## 4. 发布流程

### 4.1 Alpha 发布流程

```
1. Phase 4 完成 → 标记 git tag v0.1.0-alpha.1
2. CI 自动构建（4 平台二进制）
3. 生成 CHANGELOG（ci/changelog.sh）
4. 上传到 GitHub Releases（pre-release 标记）
5. 发布到 Docker Hub（szrsql/szrsql:0.1.0-alpha.1）
6. 通知评估用户
```

### 4.2 Beta 发布流程

```
1. Phase 7 完成 → 创建 release branch (release/1.0.0-beta)
2. 在 release branch 上运行完整测试套件
3. 修复发现的 bug（cherry-pick 到 release branch）
4. 标记 git tag v1.0.0-beta.1
5. CI 自动构建（4 平台二进制 + Docker 镜像）
6. 生成 CHANGELOG
7. 内部 PoC 邀请（合作伙伴 + 早期评估客户）
8. 收集反馈，迭代到 beta.2、beta.3...
```

### 4.3 RC 发布流程

```
1. Phase 8 完成 → 创建 release branch (release/1.0.0-rc)
2. 运行完整测试 + Jepsen + 7×3 长稳
3. 第三方安全审计（pentest）
4. 修复发现的 critical bug
5. 标记 git tag v1.0.0-rc.1
6. CI 自动构建（4 平台二进制 + Docker 镜像 + Helm Chart）
7. 邀请评估客户预生产部署
8. 监控运行情况 30 天
9. 无 critical bug → 进入 GA；有 bug → 修复后 rc.2
```

### 4.4 GA 发布流程

```
1. RC 验收通过 → 创建 release branch (release/1.0.0)
2. 最终测试套件（含 7×24h 长稳）
3. 标记 git tag v1.0.0
4. CI 自动构建（4 平台二进制 + Docker 镜像 + Helm Chart + 校验和）
5. 发布到 GitHub Releases（latest 标记）
6. 发布到 Docker Hub（szrsql/szrsql:1.0.0 + szrsql/szrsql:latest）
7. 发布到 Helm Chart 仓库
8. 发布到 PyPI（Python 驱动 szrsql-python）
9. 发布到 crates.io（Rust 驱动 szrsql-client）
10. 发布到 npm（Node.js 驱动 szrsql-node）
11. 发布到 Maven Central（JDBC 驱动 szrsql-jdbc）
12. 发布运维手册、迁移指南、API 文档
13. 举办发布会议（webinar）
```

## 5. 发布清单

### 5.1 二进制发布包

每个发布包含以下二进制：

| 平台 | 文件名 | 示例 |
|------|--------|------|
| Linux amd64 | `szrsql-{version}-linux-amd64.tar.gz` | `szrsql-1.0.0-linux-amd64.tar.gz` |
| Linux arm64 | `szrsql-{version}-linux-arm64.tar.gz` | `szrsql-1.0.0-linux-arm64.tar.gz` |
| Windows amd64 | `szrsql-{version}-windows-amd64.zip` | `szrsql-1.0.0-windows-amd64.zip` |
| macOS amd64 | `szrsql-{version}-darwin-amd64.tar.gz` | `szrsql-1.0.0-darwin-amd64.tar.gz` |

每个发布包包含：
- `szrsql` 二进制（服务端）
- `szrsql_cli` 二进制（命令行工具，Phase 7 后）
- `szrsql_dump` 二进制（备份工具，Phase 7 后）
- `szrsql_restore` 二进制（恢复工具，Phase 7 后）
- `README.md`、`LICENSE`、`CHANGELOG.md`
- `SHA256SUMS`（校验和文件）
- `SHA256SUMS.asc`（GPG 签名，GA 后）

### 5.2 Docker 镜像

| Tag | 说明 |
|-----|------|
| `szrsql/szrsql:{version}` | 指定版本 |
| `szrsql/szrsql:{version}-slim` | 精简版（基于 distroless） |
| `szrsql/szrsql:latest` | 最新稳定版（GA 后） |
| `szrsql/szrsql:{version}-alpha.N` | Alpha 版本 |
| `szrsql/szrsql:{version}-beta.N` | Beta 版本 |
| `szrsql/szrsql:{version}-rc.N` | RC 版本 |

### 5.3 Helm Chart

```bash
helm install szrsql szrsql/szrsql \
  --version 1.0.0 \
  --set persistence.enabled=true \
  --set persistence.size=100Gi \
  --set auth.password="your-password"
```

## 6. 支持策略

### 6.1 版本支持周期

| 版本类型 | 支持周期 | 安全补丁 | Bug 修复 |
|---------|---------|---------|---------|
| Alpha | 无 | ❌ | ❌ |
| Beta | 6 个月（或 GA 后停止） | ✅ | ✅ |
| RC | 3 个月（或 GA 后停止） | ✅ | ✅ |
| GA MINOR | 18 个月 | ✅ | ✅ |
| GA LTS（长期支持） | 36 个月 | ✅ | ✅ |
| GA PATCH | 随 MINOR 版本 | ✅ | ✅ |

### 6.2 升级策略

- **PATCH**：建议立即升级（安全补丁）
- **MINOR**：建议 1 个月内升级（新功能 + bug 修复）
- **MAJOR**：建议 6 个月内规划升级（破坏性变更，需迁移工具）
- **跨阶段升级**（如 Beta → GA）：建议在 GA 发布后 1 个月内完成

### 6.3 降级策略

- **PATCH 降级**：支持（in-place，替换二进制）
- **MINOR 降级**：受限支持（未使用新功能时可 in-place；否则需 pg_dump 迁移）
- **MAJOR 降级**：不支持（需 pg_dump 导出 + 重新初始化 + 导入）
- **跨阶段降级**：不支持

## 7. 弃用策略

### 7.1 功能弃用

- 在 MINOR 版本中标注弃用（`#[deprecated]` + 文档标注）
- 至少保留 2 个 MINOR 版本
- 在 MAJOR 版本中删除

### 7.2 平台弃用

- 提前 1 个 MAJOR 版本通知
- 在 MAJOR 版本中停止支持

### 7.3 API 弃用

详见 [VERSIONING.md §4.2 弃用流程](../VERSIONING.md#42-弃用流程deprecation)

## 8. 紧急发布

### 8.1 安全补丁

- **响应时间**：发现后 24 小时内发布
- **范围**：仅修复安全漏洞，不包含其他变更
- **版本号**：PATCH 递增
- **影响范围**：所有受支持版本

### 8.2 数据丢失 bug

- **响应时间**：发现后 48 小时内发布
- **范围**：修复导致数据丢失/损坏的 bug
- **版本号**：PATCH 递增

### 8.3 其他 critical bug

- **响应时间**：发现后 1 周内发布
- **范围**：修复 critical bug + 相关测试
- **版本号**：PATCH 递增

## 9. 发布日历

### 2026 年

| 季度 | 阶段 | 版本 | 说明 |
|------|------|------|------|
| Q3 (Jul-Sep) | Alpha | `0.1.0` | Phase 4 完成 |
| Q4 (Oct-Dec) | Beta | `1.0.0-beta.1` | Phase 5-7 完成 |

### 2027 年

| 季度 | 阶段 | 版本 | 说明 |
|------|------|------|------|
| Q1 (Jan-Mar) | RC | `1.0.0-rc.1` | Phase 8 完成 |
| Q2 (Apr-Jun) | GA | `1.0.0` | Jepsen + 长稳通过 |

> **注**：发布日历为预估值，实际进度可能调整。最新进度详见 `SzRSQL实施进度.md`。

## 10. 参考文档

- [SzRSQL 版本策略](../VERSIONING.md)
- [SzRSQL CHANGELOG](../CHANGELOG.md)
- [SzRSQL 实施进度](../SzRSQL实施进度.md)
- [SzRSQL 技术实现方案](../SzRSQL技术实现方案.md)
- [Semantic Versioning 2.0.0](https://semver.org/)
- [Keep a Changelog](https://keepachangelog.com/)
- [PostgreSQL Release Cycle](https://www.postgresql.org/developer/)
