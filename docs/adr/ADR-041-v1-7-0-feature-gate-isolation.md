# ADR-041: v1.7.0 Feature Gate 隔离与 all-v1-7 聚合

**状态**：Accepted
**日期**：2026-10-03
**决策者**：SZ-Rust Team

## 背景

v1.7.0 新增 27 个功能模块，跨 15 个新增 crate 和 10 个扩展 crate。需确保：
1. v1.6.0 用户零修改升级（向后兼容）
2. 新功能按需启用，不影响默认编译
3. 全量功能可一键启用用于集成测试

候选方案：

1. **Cargo feature gate + 聚合 feature**：每个功能模块用独立 feature gate 控制，定义 `all-v1-7` 聚合 feature 启用全部。
2. **默认启用所有新功能**：新功能默认编译，用户无需配置。
3. **独立 crate 无 feature gate**：新 crate 独立编译，不通过 feature 控制。

## 决策

选择 **Cargo feature gate + 聚合 feature**（方案 1）。

## 理由

1. **向后兼容**：v1.6.0 用户 `cargo build` 无 feature 时，编译行为与 v1.6.0 完全一致，零修改升级。方案 2 会导致所有用户被迫编译新依赖。

2. **按需启用**：用户只需启用所需功能（如 `--features api-graphql`），避免引入不必要的依赖。方案 2 违反最小化原则。

3. **全量验证**：`all-v1-7` 聚合 feature 一键启用全部 27 个功能，用于 CI 全量编译/测试/clippy 验证。方案 3 无法统一控制。

4. **不破坏 v1.6.0 pub API**：所有新增 pub API 在 feature gate 后，默认不导出。v1.6.0 的 pub API 保持不变。

5. **依赖传递**：聚合 feature（如 `d5-api-protocol`）自动启用子 feature（`api-graphql` + `api-websocket` + `api-sse` + `api-upload`），用户只需记住高层聚合名。

## 实施

- 根 `Cargo.toml` 定义 27 个 feature + 6 个方向聚合 + 1 个 `all-v1-7` 全量聚合
- 每个 feature 通过 `dep:` 前缀启用对应 optional 依赖
- 新 crate 的 `[features]` 定义内部 feature（如 `sz-rust-auth-facade` 的 `rbac`）
- 默认 feature 为空（`default = []`）

## 后果

- 用户需显式启用 `--features` 才能使用 v1.7.0 新功能
- `cargo doc --features all-v1-7` 生成完整文档
- CI 使用 `--features all-v1-7` 进行全量验证