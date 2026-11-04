# ADR-018：Facade Crate 独立发布策略

> **状态**：已完成
> **日期**：2026-08-03
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-017（sz-rust-core 拆包策略）
> **相关代码**：`packages/sz-rust-{orm,http,cache,state,infra,auth,pay}-facade/Cargo.toml`、根 `Cargo.toml`（`workspace.dependencies`）

## 背景

ADR-017 拆包后形成 7 个 facade crate，全部使用 `version.workspace = true`（当前 0.3.0）。
审查报告（2026-08-03-P2拆包开发过程审查与优化报告.md）指出：

> 所有 facade crate 使用 `version.workspace = true`，但 facade crate 独立发布到 crates.io 时，
> 版本号管理策略未定义。

未定义策略的风险：
- 业务包只依赖 `sz-rust-http-facade` 时，无法判断该拿哪个版本
- 各 facade 独立演进（bugfix 节奏不同）时，统一版本号无法表达"只修了 cache"这类细粒度变更
- 版本发布节奏缺失，可能出现长期不发布或乱发

## 决策

采用**两阶段版本策略**：

### 阶段一（0.x：框架未稳定前，当前阶段）

- 所有 workspace 成员保持 `version.workspace = true` **统一版本**，随框架大版本同步发布
- 任何 facade 的 breaking change 都视为框架 breaking change，整体升 0.x 小版本
- 发布节奏：随框架版本发布（当前 v0.3.0），不单独发布单个 facade

理由：0.x 阶段 API 快速演进，统一版本可避免"同一次拆包产生 7 个互相不兼容的版本"的
依赖解析地狱；下游业务包锁一个版本号即可。

### 阶段二（1.0 后：框架 API 稳定）

- 各 facade 采用 **semver 独立版本号**，breaking change 仅影响该 crate 的主版本
- 版本同步策略：
  - facade A 依赖 facade B 时，使用 `>=B, <B+1` 的宽松约束（同主版本兼容）
  - 依赖更新需在发布说明中标注"同步更新了哪些 facade"
- 发布节奏：按需发布（修复即发 patch，新特性发 minor，breaking 发 major）

### 统一约束

1. `sz-rust-core` 的 re-export 指向的 facade 版本必须与 core 发布版本兼容（core 升 minor 时，
   所依赖的 facade 不得落后于 core 两个 minor）
2. 发布前必须跑 `cargo semver-checks` 验证 API 兼容性（如判定为 breaking 必须升 major/minor）
3. `publish = false` 的 crate（如 `sz-rust-facade-tests`）不参与发布

## 后果

- 正向：下游业务包可按需只依赖需要的 facade；0.x 阶段心智负担小（一个版本号）
- 负向：1.0 后需要维护 7 个版本的发布说明与依赖矩阵，需要配套工具（cargo-release + semver-checks）

## 注意事项

- 0.x 阶段禁止对单个 facade 单独发版（会破坏统一版本约束）
- 从 0.x 切 1.0 时，需一次性全部升 1.0 并保持 facade 间依赖约束同步
- 发布流程走 `sz-rust-deploy` Skill（release 分支 + 变更摘要）

## Bug 定位提示

- 版本不一致症状：`E0460: found possibly newer version of crate ...` —— 检查 workspace
  `version.workspace` 是否被局部覆盖，或 facade 间 `version = "x.y.z"` 硬编码
- 依赖解析失败：检查阶段二约束是否写成 `>=` 而没有上限
