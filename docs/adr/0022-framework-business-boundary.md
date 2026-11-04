# ADR-0022：框架层与业务层边界划分

> **状态**：Accepted
> **日期**：2026-09-24
> **决策者**：SZ-Rust Team
> **关联**：v1.4.0 T07，spec 5.6

## 背景

sz300 业务应用源码已迁入主 workspace（T06），workspace 现有 42 个 crate。需要明确框架层（通用能力）与业务层（sz300 特定逻辑）的职责边界和依赖方向，防止框架层 crate 反向依赖业务层 crate，导致耦合污染。

## 决策

### 1. 层级定义

| 层级 | 职责 | crate 示例 |
|------|------|-----------|
| **框架层** | 通用 Web 框架能力（路由、ORM、中间件、缓存、AI facade 等） | sz-rust-core, sz-rust-orm-facade, sz-rust-http-facade, sz-rust-ai-facade 等所有 `sz-rust-*`（非 sz300） |
| **业务层** | sz300 业务特定逻辑（设备/商户/商品/订单管理） | sz-rust-sz300 |
| **工具层** | 测试工具、CLI、示例 | sz-rust-testkit, sz-rust-cli, sz-rust-examples |

### 2. 依赖方向

```
业务层 → 框架层 → 外部依赖（crates.io）
              ↑
         工具层 → 框架层
```

**禁止框架层 crate 引用 sz-rust-sz300**。框架层不得依赖业务层，业务层可依赖框架层。

### 3. 边界规则

1. **框架层 crate** 不得在 `[dependencies]` 中引用 `sz-rust-sz300`
2. **业务层 crate**（sz-rust-sz300）可引用任意框架层 crate
3. **共享能力** 应提取至框架层，业务层通过 facade 调用
4. **类型定义** 跨层共享的类型须定义在框架层，业务层引用

### 4. 共享能力提取指南

当 sz300 与框架层存在重复能力时：
1. 识别重复逻辑（如自定义中间件、配置解析）
2. 将通用部分提取至框架层 crate
3. sz300 通过 facade 或 trait 引用框架层能力
4. sz300 保留业务特定逻辑，不外泄至框架层

### 5. 检查机制

- `scripts/check-boundary.sh` 基于 `cargo metadata` 依赖图分析
- CI 集成边界检查，违规时阻断合并
- 新增框架 crate 时须确认不引用 sz-rust-sz300

## 影响

- 框架层保持通用性，可被其他业务应用复用
- sz300 业务逻辑隔离，不影响框架层稳定性
- 依赖图清晰，编译缓存命中率提升