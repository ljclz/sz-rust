# serde_yaml 迁移评估

> 评估日期：2026-07-24
> 评估目标：serde_yaml 0.9.34+deprecated 的替代方案

## 现状

- serde_yaml 0.9.34 已标记为 deprecated（作者 dtolnay 建议迁移）
- sz-rust 中使用 serde_yaml 的位置：
  - sz-rust-core/src/config.rs — 配置文件解析（YAML → 结构体）
  - sz-rust-cli — 之前使用，已在本次审计中移除未使用依赖

## 替代方案对比

### 方案 1: serde_yml（fork）

| 项目 | 详情 |
|------|------|
| crate 名 | serde_yml |
| 版本 | 0.0.12+ |
| 维护 | 活跃（社区 fork） |
| API 兼容 | 与 serde_yaml 基本兼容 |
| 迁移成本 | 低（替换 crate 名 + 调整 use 路径） |

### 方案 2: yaml-rust2

| 项目 | 详情 |
|------|------|
| crate 名 | yaml-rust2 |
| 版本 | 0.10+ |
| 维护 | 活跃 |
| API 兼容 | 不兼容 serde（需手动序列化） |
| 迁移成本 | 高（需重写序列化逻辑） |

### 方案 3: 保持 serde_yaml

| 项目 | 详情 |
|------|------|
| 理由 | serde_yaml 虽标记 deprecated 但功能稳定，无安全漏洞 |
| 风险 | 低。deprecated 仅表示不再新增功能，现有功能仍可用 |
| 迁移成本 | 无 |

## 评估结论

**短期决策**：保持 serde_yaml 0.9.34 不变。

**理由**：
1. serde_yaml 功能稳定，deprecated 标记仅表示不再开发新功能
2. 当前仅 sz-rust-core/config.rs 使用，范围可控
3. serde_yml 作为 fork，API 尚未完全稳定，贸然迁移可能引入新问题
4. 可在下一次大版本（0.3.0）时统一评估迁移

**长期计划**：
- 持续关注 serde_yml 的稳定性
- 当 serde_yml 发布 1.0 稳定版时进行迁移
- 迁移时仅需：Cargo.toml 替换 `serde_yaml` → `serde_yml`，代码中 `serde_yaml::` → `serde_yml::`
